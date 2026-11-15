//! Retomada de um plano SDD por `Command::ResumeSdd`.

use std::sync::Arc;

use async_trait::async_trait;
use tupi_app::App;
use tupi_domain::AgentMode;
use tupi_kernel::{
    ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, Provider,
    ProviderError, ProviderEvent, ProviderRegistry, ProviderStream, Query, QueryHandler,
    QueryResult,
};
use tupi_tools::sdd::{
    InMemorySddStore, SddPlan, SddPlanStatus, SddStore, SddSubtask, SddSubtaskStatus, SddTask,
    SddTaskStatus,
};

/// Provedor que a retomada nunca chama.
struct UnusedProvider;

#[async_trait]
impl Provider for UnusedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::empty::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

/// Provedor cujo stream nunca entrega nada: mantém o turno ativo.
struct HangingProvider;

#[async_trait]
impl Provider for HangingProvider {
    fn id(&self) -> &str {
        "hanging"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::pending::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

fn app(store: Arc<dyn SddStore>) -> Arc<App> {
    let registry = ProviderRegistry::new();
    registry.register(Arc::new(UnusedProvider));
    App::new(registry, "scripted", "modelo".into(), EventBus::new(8))
        .with_repo_key("repo".into())
        .with_sdd_store(store)
}

fn app_with_events(
    store: Arc<dyn SddStore>,
) -> (
    Arc<App>,
    tokio::sync::broadcast::Receiver<tupi_domain::Event>,
) {
    let events = EventBus::new(16);
    let receiver = events.subscribe();
    let registry = ProviderRegistry::new();
    registry.register(Arc::new(UnusedProvider));
    let app = App::new(registry, "scripted", "modelo".into(), events)
        .with_repo_key("repo".into())
        .with_sdd_store(store);
    (app, receiver)
}

fn task(id: &str, status: SddTaskStatus, subtasks: Vec<SddSubtask>) -> SddTask {
    SddTask {
        id: id.to_string(),
        title: format!("Task {id}"),
        details: format!("Detalhes de {id}"),
        nuances: Vec::new(),
        parallel: false,
        status,
        subtasks,
    }
}

fn subtask(id: &str, status: SddSubtaskStatus) -> SddSubtask {
    SddSubtask {
        id: id.to_string(),
        title: format!("Subtask {id}"),
        acceptance: format!("Aceite de {id}"),
        touches: Vec::new(),
        acceptance_test: None,
        status,
    }
}

fn plan(slug: &str, status: SddPlanStatus) -> SddPlan {
    SddPlan {
        slug: slug.to_string(),
        repo_key: "repo".to_string(),
        title: "Autenticação".to_string(),
        status,
        goal: "Permitir login com token".to_string(),
        constraints: vec!["Somente TUI".to_string()],
        nuances: vec!["O store já resolve o slug ativo".to_string()],
        tasks: vec![
            task(
                "T1",
                SddTaskStatus::Done,
                vec![subtask("T1.1", SddSubtaskStatus::Done)],
            ),
            task(
                "T2",
                SddTaskStatus::InProgress,
                vec![
                    subtask("T2.1", SddSubtaskStatus::Done),
                    subtask("T2.2", SddSubtaskStatus::Open),
                ],
            ),
        ],
        created_at: 1,
        updated_at: 1,
        ..Default::default()
    }
}

async fn history(app: &Arc<App>) -> Vec<tupi_domain::Message> {
    match app.query(Query::History).await.unwrap() {
        QueryResult::History(messages) => messages,
        other => panic!("esperava History, veio {other:?}"),
    }
}

#[tokio::test]
async fn resume_activates_the_slug_and_locks_the_sdd_mode() {
    let store = Arc::new(InMemorySddStore::default());
    store.save(&plan("auth", SddPlanStatus::Draft)).unwrap();
    let app = app(store.clone());

    let outcome = app
        .handle(Command::ResumeSdd {
            slug: "auth".into(),
        })
        .await
        .unwrap();

    match outcome {
        CommandOutcome::SddResumed {
            slug,
            title,
            mode,
            open_tasks,
        } => {
            assert_eq!(slug, "auth");
            assert_eq!(title, "Autenticação");
            assert_eq!(mode, AgentMode::Sdd);
            assert_eq!(open_tasks, 1);
        }
        other => panic!("esperava SddResumed, veio {other:?}"),
    }
    assert_eq!(app.mode(), AgentMode::Sdd);
    assert_eq!(store.active_slug("repo").unwrap(), Some("auth".to_string()));

    // `sdd_flow_started` está ligado: sair do modo SDD vira Noop.
    assert_eq!(
        app.handle(Command::SetMode {
            mode: AgentMode::Execution
        })
        .await
        .unwrap(),
        CommandOutcome::Noop
    );
    assert_eq!(app.mode(), AgentMode::Sdd);
}

#[tokio::test]
async fn resume_injects_exactly_one_brief_that_must_not_be_answered() {
    let store = Arc::new(InMemorySddStore::default());
    store.save(&plan("auth", SddPlanStatus::Draft)).unwrap();
    let app = app(store);

    app.handle(Command::ResumeSdd {
        slug: "auth".into(),
    })
    .await
    .unwrap();

    let messages = history(&app).await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(messages[0].role, tupi_domain::Role::User);
    let brief = &messages[0].content;
    for needle in [
        "auth",
        "Autenticação",
        "Permitir login com token",
        "Somente TUI",
        "O store já resolve o slug ativo",
        "T2",
        "T2.2",
        "Não responda",
        "continue",
    ] {
        assert!(brief.contains(needle), "brief sem `{needle}`:\n{brief}");
    }
    // O brief de retomada não carrega o papel de executor: seria contraditório
    // com a instrução de não responder e de não sair executando.
    for forbidden in [
        "agente de implementação",
        "Implemente apenas esta subtarefa",
    ] {
        assert!(
            !brief.contains(forbidden),
            "brief de retomada contém `{forbidden}`:\n{brief}"
        );
    }
}

#[tokio::test]
async fn resume_refuses_completed_and_unknown_plans_without_changing_the_mode() {
    let store = Arc::new(InMemorySddStore::default());
    store
        .save(&plan("velho", SddPlanStatus::Completed))
        .unwrap();

    for slug in ["velho", "inexistente"] {
        let app = app(store.clone());
        let error = app
            .handle(Command::ResumeSdd {
                slug: slug.to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(error, CommandError::Internal(_)), "{error:?}");
        assert_eq!(app.mode(), AgentMode::Execution, "{slug}");
        assert_eq!(store.active_slug("repo").unwrap(), None, "{slug}");
        assert!(history(&app).await.is_empty(), "{slug}");
    }
}

#[tokio::test]
async fn closed_plans_resume_into_execution_with_the_handoff() {
    let store = Arc::new(InMemorySddStore::default());
    store.save(&plan("fechado", SddPlanStatus::Closed)).unwrap();
    let app = app(store);

    let outcome = app
        .handle(Command::ResumeSdd {
            slug: "fechado".into(),
        })
        .await
        .unwrap();

    assert!(
        matches!(
            outcome,
            CommandOutcome::SddResumed {
                mode: AgentMode::Execution,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(app.mode(), AgentMode::Execution);

    let messages = history(&app).await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    let brief = &messages[0].content;
    assert!(brief.contains("SDD fechado"), "{brief}");
    assert!(brief.contains("Permitir login com token"), "{brief}");
    assert!(!brief.contains("Não responda"), "{brief}");

    // A execução não trava a saída do modo: Shift+Tab (SetMode) continua vivo.
    assert_eq!(
        app.handle(Command::SetMode {
            mode: AgentMode::Sdd
        })
        .await
        .unwrap(),
        CommandOutcome::ModeChanged {
            mode: AgentMode::Sdd
        }
    );
    assert_eq!(app.mode(), AgentMode::Sdd);
}

#[tokio::test]
async fn failed_plans_resume_into_execution_with_the_handoff() {
    let store = Arc::new(InMemorySddStore::default());
    store
        .save(&plan("quebrado", SddPlanStatus::Failed))
        .unwrap();
    let app = app(store.clone());

    let outcome = app
        .handle(Command::ResumeSdd {
            slug: "quebrado".into(),
        })
        .await
        .unwrap();

    assert!(
        matches!(
            outcome,
            CommandOutcome::SddResumed {
                mode: AgentMode::Execution,
                ..
            }
        ),
        "{outcome:?}"
    );
    assert_eq!(app.mode(), AgentMode::Execution);
    assert_eq!(
        store.active_slug("repo").unwrap(),
        Some("quebrado".to_string())
    );

    let messages = history(&app).await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    let brief = &messages[0].content;
    assert!(brief.contains("Inicie a execução"), "{brief}");
    assert!(brief.contains("Permitir login com token"), "{brief}");
}

#[tokio::test]
async fn resume_publishes_the_confirmation_after_the_mode_change() {
    let store = Arc::new(InMemorySddStore::default());
    store.save(&plan("auth", SddPlanStatus::Draft)).unwrap();
    let (app, mut events) = app_with_events(store);

    app.handle(Command::ResumeSdd {
        slug: "auth".into(),
    })
    .await
    .unwrap();

    let mut kinds = Vec::new();
    let mut notice = None;
    while let Ok(event) = events.try_recv() {
        match event {
            tupi_domain::Event::ModeChanged { mode } => kinds.push(format!("mode:{mode:?}")),
            tupi_domain::Event::Notice { message } => {
                kinds.push("notice".to_string());
                notice = Some(message);
            }
            _ => {}
        }
    }

    let notice = notice.expect("retomada publica um Notice");
    assert!(notice.contains("SDD retomado"), "{notice}");
    assert!(notice.contains("continue"), "{notice}");
    let mode_at = kinds
        .iter()
        .position(|kind| kind.starts_with("mode:"))
        .expect("ModeChanged publicado");
    let notice_at = kinds.iter().position(|kind| kind == "notice").unwrap();
    assert!(mode_at < notice_at, "{kinds:?}");
}

#[tokio::test]
async fn resume_without_store_degrades_with_a_clear_error() {
    let registry = ProviderRegistry::new();
    registry.register(Arc::new(UnusedProvider));
    let app = App::new(registry, "scripted", "modelo".into(), EventBus::new(8));

    let error = app
        .handle(Command::ResumeSdd {
            slug: "auth".into(),
        })
        .await
        .unwrap_err();

    match error {
        CommandError::Internal(message) => assert!(message.contains("SddStore"), "{message}"),
        other => panic!("esperava Internal, veio {other:?}"),
    }
}

#[tokio::test]
async fn resume_is_busy_while_a_turn_runs_and_leaves_the_store_untouched() {
    let store = Arc::new(InMemorySddStore::default());
    store.save(&plan("auth", SddPlanStatus::Draft)).unwrap();
    let registry = ProviderRegistry::new();
    registry.register(Arc::new(HangingProvider));
    let app = App::new(registry, "hanging", "modelo".into(), EventBus::new(8))
        .with_repo_key("repo".into())
        .with_sdd_store(store.clone());

    let outcome = app
        .handle(Command::SendMessage { text: "oi".into() })
        .await
        .unwrap();
    let CommandOutcome::Started { request_id } = outcome else {
        panic!("esperava Started, veio {outcome:?}");
    };

    let error = app
        .handle(Command::ResumeSdd {
            slug: "auth".into(),
        })
        .await
        .unwrap_err();

    assert!(matches!(error, CommandError::Busy), "{error:?}");
    assert_eq!(app.mode(), AgentMode::Execution);
    assert_eq!(store.active_slug("repo").unwrap(), None);

    let _ = app.handle(Command::CancelStream { request_id }).await;
}
