use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tupi_app::app::App;
use tupi_domain::{Event, Role, ToolCall};
use tupi_kernel::{
    AgentMode, ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider,
    ProviderError, ProviderEvent, ProviderRegistry, ProviderStream, Tool, ToolError, ToolRegistry,
    ToolSpec,
};
use tupi_store::{
    SddPlan, SddPlanStatus, SddSubtask, SddSubtaskStatus, SddTask, SddTaskStatus, Store,
};

struct ScriptedProvider {
    scripts: Mutex<VecDeque<Vec<Result<ProviderEvent, ProviderError>>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Vec<Result<ProviderEvent, ProviderError>>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["m".into()])
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        self.requests.lock().unwrap().push(request);
        let items = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::pin(futures_util::stream::iter(items)))
    }
}

fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
    let reg = ProviderRegistry::new();
    reg.register(provider);
    reg
}

struct ReadOnlyStub;
impl Tool for ReadOnlyStub {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: "read".into(),
            schema: serde_json::json!({ "type": "object" }),
        }
    }
    fn call(&self, _: Value) -> Result<String, ToolError> {
        Ok("conteudo".into())
    }
    fn sdd_allowed(&self) -> bool {
        true
    }
}

struct MutatorStub(Arc<AtomicBool>);
impl Tool for MutatorStub {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".into(),
            description: "edit".into(),
            schema: serde_json::json!({ "type": "object" }),
        }
    }
    fn call(&self, _: Value) -> Result<String, ToolError> {
        self.0.store(true, Ordering::SeqCst);
        Ok("edited".into())
    }
}

struct CloseSddStub {
    store: Arc<Store>,
    repo_key: String,
    slug: String,
}
impl Tool for CloseSddStub {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sdd".into(),
            description: "sdd".into(),
            schema: serde_json::json!({ "type": "object" }),
        }
    }
    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        if arguments.get("action").and_then(Value::as_str) == Some("close") {
            let mut plan = self
                .store
                .load_sdd_plan(&self.repo_key, &self.slug)
                .unwrap()
                .unwrap();
            plan.status = SddPlanStatus::Closed;
            plan.closed_at = Some(200);
            self.store.save_sdd_plan(&plan).unwrap();
            return Ok("spec fechada".into());
        }
        Ok("ok".into())
    }
    fn mutates(&self) -> bool {
        true
    }
    fn sdd_allowed(&self) -> bool {
        true
    }
}

async fn wait_finished(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
    let mut seen = Vec::new();
    while let Ok(ev) = rx.recv().await {
        let done = matches!(
            ev,
            Event::StreamFinished { .. } | Event::StreamFailed { .. }
        );
        seen.push(ev);
        if done {
            break;
        }
    }
    seen
}

#[tokio::test]
async fn shift_tab_is_blocked_after_first_message_in_sdd() {
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![vec![Ok(ProviderEvent::Text(
        "vamos planejar".into(),
    ))]]));
    let app = App::with_tools(
        registry(provider),
        "scripted",
        "m".into(),
        bus,
        None,
        ToolRegistry::new(),
        "BASE".into(),
    )
    .with_mode_cell(Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8())));

    // Antes de qualquer mensagem, a troca é livre: SDD -> Execution -> SDD.
    assert!(matches!(
        app.handle(Command::ToggleMode).await.unwrap(),
        CommandOutcome::ModeChanged {
            mode: AgentMode::Execution
        }
    ));
    app.handle(Command::ToggleMode).await.unwrap();

    app.handle(Command::SendMessage {
        text: "quero auth".into(),
    })
    .await
    .unwrap();
    wait_finished(&mut rx).await;

    let outcome = app.handle(Command::ToggleMode).await.unwrap();
    assert!(matches!(outcome, CommandOutcome::Noop));
    assert_eq!(app.mode(), AgentMode::Sdd);

    // SetMode também não pode furar a trava.
    assert!(matches!(
        app.handle(Command::SetMode {
            mode: AgentMode::Execution
        })
        .await
        .unwrap(),
        CommandOutcome::Noop
    ));

    let mut locked_notice = false;
    while let Ok(event) = rx.try_recv() {
        if let Event::Notice { message } = event
            && message.contains("SDD em andamento")
        {
            locked_notice = true;
        }
    }
    assert!(locked_notice, "a tentativa de troca precisa avisar");

    // /new destrava.
    app.handle(Command::NewSession).await.unwrap();
    assert!(matches!(
        app.handle(Command::ToggleMode).await.unwrap(),
        CommandOutcome::ModeChanged {
            mode: AgentMode::Execution
        }
    ));
}

#[tokio::test]
async fn toggle_mode_emits_event_and_filters_specs_in_sdd_mode() {
    let bus = EventBus::new(32);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![vec![Ok(ProviderEvent::Text(
        "qual o objetivo?".into(),
    ))]]));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ReadOnlyStub));
    tools.register(Arc::new(MutatorStub(Arc::new(AtomicBool::new(false)))));

    let app = App::with_tools(
        registry(provider.clone()),
        "scripted",
        "m".into(),
        bus,
        None,
        tools,
        "BASE".into(),
    );

    let out = app.handle(Command::ToggleMode).await.unwrap();
    assert!(matches!(
        out,
        CommandOutcome::ModeChanged {
            mode: AgentMode::Sdd
        }
    ));
    assert_eq!(app.mode(), AgentMode::Sdd);
    assert!(matches!(
        rx.recv().await.unwrap(),
        Event::ModeChanged {
            mode: AgentMode::Sdd
        }
    ));

    app.handle(Command::SendMessage {
        text: "quero fazer auth".into(),
    })
    .await
    .unwrap();
    wait_finished(&mut rx).await;

    let reqs = provider.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    let tool_names: Vec<&str> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tool_names, vec!["read"]);
    assert!(reqs[0].messages[0].content.contains("SDD"));
}

#[tokio::test]
async fn sdd_mode_blocks_mutating_tool_call_even_if_model_emits_it() {
    let bus = EventBus::new(32);
    let mut rx = bus.subscribe();
    let edited = Arc::new(AtomicBool::new(false));
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![Ok(ProviderEvent::ToolCalls(vec![ToolCall {
            id: "c1".into(),
            name: "edit".into(),
            arguments: "{}".into(),
        }]))],
        vec![Ok(ProviderEvent::Text("bloqueado".into()))],
    ]));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ReadOnlyStub));
    tools.register(Arc::new(MutatorStub(edited.clone())));

    let app = App::with_tools(
        registry(provider),
        "scripted",
        "m".into(),
        bus,
        None,
        tools,
        String::new(),
    );

    app.handle(Command::SetMode {
        mode: AgentMode::Sdd,
    })
    .await
    .unwrap();
    app.handle(Command::SendMessage {
        text: "edita".into(),
    })
    .await
    .unwrap();
    let events = wait_finished(&mut rx).await;

    assert!(!edited.load(Ordering::SeqCst));
    assert!(
        events.iter().any(
            |e| matches!(e, Event::ToolFinished { call_id, ok: false, .. } if call_id == "c1")
        )
    );
}

#[tokio::test]
async fn closing_sdd_mid_turn_trims_session_without_orphaned_tool_results_and_unlocks_execution_in_next_step()
 {
    let db_path = std::env::temp_dir().join(format!("tupi-app-sdd-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&db_path);
    let store = Arc::new(Store::open(&db_path).unwrap());
    let repo_key = "repo-1".to_string();
    let plan = SddPlan {
        slug: "auth".into(),
        repo_key: repo_key.clone(),
        title: "Autenticação".into(),
        status: SddPlanStatus::Draft,
        goal: "Adicionar OAuth".into(),
        constraints: vec!["Sem quebrar CLI".into()],
        nuances: vec!["Token expira em 55m".into()],
        tasks: vec![SddTask {
            id: "T1".into(),
            title: "Refresh".into(),
            details: "Renovar antes de expirar".into(),
            nuances: vec![],
            parallel: false,
            status: SddTaskStatus::Open,
            subtasks: vec![SddSubtask {
                id: "T1.1".into(),
                title: "Mutex".into(),
                acceptance: "Teste passa".into(),
                touches: vec![],
                acceptance_test: None,
                status: SddSubtaskStatus::Open,
            }],
        }],
        created_at: 100,
        updated_at: 100,
        closed_at: None,
    };
    store.save_sdd_plan(&plan).unwrap();
    store.set_active_sdd_slug(&repo_key, Some("auth")).unwrap();

    let mode_cell = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
    let edited = Arc::new(AtomicBool::new(false));
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![Ok(ProviderEvent::ToolCalls(vec![ToolCall {
            id: "c_read".into(),
            name: "read".into(),
            arguments: "{}".into(),
        }]))],
        vec![Ok(ProviderEvent::Text("debate longo sobre oauth".into()))],
        vec![Ok(ProviderEvent::ToolCalls(vec![ToolCall {
            id: "c_close".into(),
            name: "sdd".into(),
            arguments: r#"{"action":"close"}"#.into(),
        }]))],
        vec![Ok(ProviderEvent::ToolCalls(vec![ToolCall {
            id: "c_edit".into(),
            name: "edit".into(),
            arguments: "{}".into(),
        }]))],
        vec![Ok(ProviderEvent::Text("feito".into()))],
    ]));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CloseSddStub {
        store: store.clone(),
        repo_key: repo_key.clone(),
        slug: "auth".into(),
    }));
    tools.register(Arc::new(ReadOnlyStub));
    tools.register(Arc::new(MutatorStub(edited.clone())));

    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let app = App::with_tools(
        registry(provider.clone()),
        "scripted",
        "m".into(),
        bus,
        Some(store.clone()),
        tools,
        "BASE".into(),
    )
    .with_repo_key(repo_key)
    .with_mode_cell(mode_cell);

    // Turno 1: debate SDD com leitura de código/docs
    app.handle(Command::SendMessage {
        text: "quero fazer auth".into(),
    })
    .await
    .unwrap();
    wait_finished(&mut rx).await;

    // Turno 2: usuário autoriza fechar e executar
    app.handle(Command::SendMessage {
        text: "pode fechar e executar".into(),
    })
    .await
    .unwrap();
    let events = wait_finished(&mut rx).await;

    assert!(
        edited.load(Ordering::SeqCst),
        "edit deve estar liberado no step seguinte"
    );
    assert_eq!(app.mode(), AgentMode::Execution);

    // O fechamento destrava a troca de modo.
    assert!(matches!(
        app.handle(Command::ToggleMode).await.unwrap(),
        CommandOutcome::ModeChanged {
            mode: AgentMode::Sdd
        }
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ModeChanged {
            mode: AgentMode::Execution
        }
    )));
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Notice { message } if message.contains("SDD fechado")
        )),
        "o fechamento do SDD precisa ser anunciado na interface"
    );

    let reqs = provider.requests.lock().unwrap();
    assert_eq!(reqs.len(), 5);
    // Step após close (reqs[3]) não carrega debate antigo nem tool_result órfão
    let step2 = &reqs[3];
    assert!(step2.tools.iter().any(|t| t.name == "edit"));
    assert!(
        !step2
            .messages
            .iter()
            .any(|m| m.content.contains("quero fazer auth"))
    );
    assert!(!step2.messages.iter().any(|m| m.role == Role::Tool));
    assert!(
        step2
            .messages
            .iter()
            .any(|m| m.role == Role::User && m.content.contains("Token expira em 55m"))
    );
    assert!(
        !step2.messages[0]
            .content
            .contains(tupi_app::sdd::SDD_POSTURE_PROMPT)
    );

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn toggling_mode_twice_keeps_the_sdd_plan_open_and_does_not_announce_close() {
    let db_path =
        std::env::temp_dir().join(format!("tupi-app-sdd-toggle-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&db_path);
    let store = Arc::new(Store::open(&db_path).unwrap());
    let repo_key = "repo-toggle".to_string();
    let plan = SddPlan {
        slug: "auth".into(),
        repo_key: repo_key.clone(),
        title: "Autenticação".into(),
        status: SddPlanStatus::Draft,
        goal: "Adicionar OAuth".into(),
        constraints: vec![],
        nuances: vec![],
        tasks: vec![],
        created_at: 100,
        updated_at: 100,
        closed_at: None,
    };
    store.save_sdd_plan(&plan).unwrap();
    store.set_active_sdd_slug(&repo_key, Some("auth")).unwrap();

    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();
    let app = App::with_tools(
        registry(Arc::new(ScriptedProvider::new(vec![]))),
        "scripted",
        "m".into(),
        bus,
        Some(store.clone()),
        ToolRegistry::new(),
        "BASE".into(),
    )
    .with_repo_key(repo_key.clone())
    .with_mode_cell(Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8())));

    // shift+tab duas vezes: SDD -> edição rápida -> SDD.
    app.handle(Command::ToggleMode).await.unwrap();
    app.handle(Command::ToggleMode).await.unwrap();

    assert_eq!(app.mode(), AgentMode::Sdd);
    let saved = store.load_sdd_plan(&repo_key, "auth").unwrap().unwrap();
    assert_eq!(
        saved.status,
        SddPlanStatus::Draft,
        "alternar entre edição rápida e SDD não fecha o plano"
    );

    let mut noticed_close = false;
    while let Ok(event) = rx.try_recv() {
        if let Event::Notice { message } = event
            && message.contains("SDD fechado")
        {
            noticed_close = true;
        }
    }
    assert!(
        !noticed_close,
        "o fechamento só pode ser anunciado pela tool sdd(action=\"close\") no planejamento"
    );

    let _ = std::fs::remove_file(&db_path);
}
