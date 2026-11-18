use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use tupi_app::app::App;
use tupi_domain::{Event, ToolCall};
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream, Tool, ToolError, ToolRegistry, ToolSpec,
};

struct Capture {
    seen: Mutex<Vec<ChatRequest>>,
}

#[async_trait]
impl Provider for Capture {
    fn id(&self) -> &str {
        "fake"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["modelo".to_string()])
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        self.seen.lock().unwrap().push(request);
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ProviderEvent::Text("ok".to_string())),
            Ok(ProviderEvent::Usage {
                prompt_tokens: Some(1),
                cached_tokens: Some(0),
                completion_tokens: Some(1),
            }),
        ])))
    }
}

#[tokio::test]
async fn the_configured_max_tokens_reaches_the_provider() {
    let capture = Arc::new(Capture {
        seen: Mutex::new(Vec::new()),
    });
    let providers = ProviderRegistry::new();
    providers.register(capture.clone());

    let events = EventBus::new(16);
    let mut receiver = events.subscribe();
    let app = App::with_tools(
        providers,
        "fake",
        "modelo".to_string(),
        events,
        None,
        ToolRegistry::new(),
        String::new(),
    )
    .with_max_tokens(1234);

    let CommandOutcome::Started { .. } = app
        .handle(Command::SendMessage {
            text: "oi".to_string(),
        })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    loop {
        match receiver.recv().await.unwrap() {
            Event::StreamFinished { .. } => break,
            Event::StreamFailed { message, .. } => panic!("stream falhou: {message}"),
            _ => {}
        }
    }

    let seen = capture.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].max_tokens, Some(1234));
}

/// Provedor com roteiro fixo: cada passo do turno consome o próximo script.
struct Scripted {
    steps: Mutex<Vec<Vec<Result<ProviderEvent, ProviderError>>>>,
}

impl Scripted {
    fn new(steps: Vec<Vec<Result<ProviderEvent, ProviderError>>>) -> Self {
        Self {
            steps: Mutex::new(steps),
        }
    }
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str {
        "fake"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["modelo".to_string()])
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let next = self.steps.lock().unwrap().remove(0);
        Ok(Box::pin(futures_util::stream::iter(next)))
    }
}

struct CountingTool {
    calls: Arc<AtomicUsize>,
}

impl Tool for CountingTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".to_string(),
            description: "conta chamadas".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }
    }

    fn effect(&self, _: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::ReadOnly
    }

    fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok("ok".to_string())
    }
}

fn truncated_step() -> Vec<Result<ProviderEvent, ProviderError>> {
    vec![
        Ok(ProviderEvent::Text("vou editar".to_string())),
        Ok(ProviderEvent::ToolCalls(vec![ToolCall {
            id: "c1".to_string(),
            name: "echo".to_string(),
            arguments: "{\"path\"".to_string(),
        }])),
        Ok(ProviderEvent::Finished("length".to_string())),
    ]
}

fn answered_step() -> Vec<Result<ProviderEvent, ProviderError>> {
    vec![
        Ok(ProviderEvent::Text("pronto".to_string())),
        Ok(ProviderEvent::Finished("stop".to_string())),
    ]
}

/// Roda um turno e devolve a falha, se houver: `None` quando o turno termina
/// sem erro, `Some(mensagem)` quando publica `StreamFailed`.
async fn run_turn_outcome(
    app: &Arc<App>,
    receiver: &mut tokio::sync::broadcast::Receiver<Event>,
) -> Option<String> {
    let CommandOutcome::Started { .. } = app
        .handle(Command::SendMessage {
            text: "oi".to_string(),
        })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    loop {
        match receiver.recv().await.unwrap() {
            Event::StreamFinished { .. } => return None,
            Event::StreamFailed { message, .. } => return Some(message),
            _ => {}
        }
    }
}

async fn run_turn(app: &Arc<App>, receiver: &mut tokio::sync::broadcast::Receiver<Event>) {
    if let Some(message) = run_turn_outcome(app, receiver).await {
        panic!("stream falhou: {message}");
    }
}

#[tokio::test]
async fn a_truncated_tool_call_is_not_executed_and_the_turn_retries() {
    let calls = Arc::new(AtomicUsize::new(0));
    let providers = ProviderRegistry::new();
    providers.register(Arc::new(Scripted::new(vec![
        truncated_step(),
        answered_step(),
    ])));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CountingTool {
        calls: Arc::clone(&calls),
    }));

    let events = EventBus::new(64);
    let mut receiver = events.subscribe();
    let app = App::with_tools(
        providers,
        "fake",
        "modelo".to_string(),
        events,
        None,
        tools,
        String::new(),
    )
    .with_max_tokens(64);

    run_turn(&app, &mut receiver).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a tool de uma resposta truncada não pode rodar"
    );
    let session = app.session();
    assert!(
        session
            .messages
            .iter()
            .any(|message| message.content.contains("cortada")),
        "o turno deveria registrar o corte: {:?}",
        session.messages
    );
    assert!(
        session
            .messages
            .iter()
            .any(|message| message.content == "pronto"),
        "o retry deveria entregar a resposta final: {:?}",
        session.messages
    );
}

#[tokio::test]
async fn an_empty_truncated_answer_is_retried_instead_of_ending_the_turn_mute() {
    let calls = Arc::new(AtomicUsize::new(0));
    let providers = ProviderRegistry::new();
    providers.register(Arc::new(Scripted::new(vec![
        vec![
            Ok(ProviderEvent::Usage {
                prompt_tokens: Some(10),
                cached_tokens: Some(0),
                completion_tokens: Some(64),
            }),
            Ok(ProviderEvent::Finished("length".to_string())),
        ],
        answered_step(),
    ])));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CountingTool { calls }));

    let events = EventBus::new(64);
    let mut receiver = events.subscribe();
    let app = App::with_tools(
        providers,
        "fake",
        "modelo".to_string(),
        events,
        None,
        tools,
        String::new(),
    )
    .with_max_tokens(64);

    run_turn(&app, &mut receiver).await;

    let session = app.session();
    assert!(
        session
            .messages
            .iter()
            .any(|message| message.content == "pronto"),
        "o turno vazio deveria ser refeito, não encerrado: {:?}",
        session.messages
    );
}

/// Três cortes seguidos esgotam os retries. O turno não pode terminar como se
/// tivesse respondido: sem texto, sem erro, o usuário só vê o aviso e o
/// processo para como se nada tivesse acontecido.
#[tokio::test]
async fn exhausted_truncation_retries_fail_the_turn_with_the_real_cause() {
    let calls = Arc::new(AtomicUsize::new(0));
    let providers = ProviderRegistry::new();
    providers.register(Arc::new(Scripted::new(vec![
        truncated_step(),
        truncated_step(),
        truncated_step(),
    ])));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(CountingTool {
        calls: Arc::clone(&calls),
    }));

    let events = EventBus::new(64);
    let mut receiver = events.subscribe();
    let app = App::with_tools(
        providers,
        "fake",
        "modelo".to_string(),
        events,
        None,
        tools,
        String::new(),
    )
    .with_max_tokens(64);

    let failure = run_turn_outcome(&app, &mut receiver).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "tool de resposta truncada nunca roda"
    );
    let message = failure.expect("retries esgotados precisam falhar, não encerrar mudo");
    assert!(
        message.contains("64"),
        "a falha precisa nomear o teto: {message}"
    );
    assert!(
        message.contains("cortada"),
        "a falha precisa nomear a causa: {message}"
    );
}
