use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use tupi_app::app::App;
use tupi_app::sessions::{
    InMemorySessionArchive, SessionArchive, SessionLock, SessionMeta, SessionRecord,
};
use tupi_domain::{Event, Role};
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream,
};

struct Partial;

struct Scripted;

#[async_trait]
impl Provider for Partial {
    fn id(&self) -> &str {
        "partial"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let first = stream::iter(vec![Ok(ProviderEvent::Text("parcial".to_string()))]);
        Ok(Box::pin(first.chain(stream::pending())))
    }
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(stream::iter(vec![Ok(ProviderEvent::Text(
            "ok".to_string(),
        ))])))
    }
}

struct FailingArchive(InMemorySessionArchive);

impl SessionArchive for FailingArchive {
    fn repo_key(&self) -> &str {
        self.0.repo_key()
    }

    fn create(&self, _meta: &SessionMeta) -> Result<SessionLock, String> {
        Err("disco cheio".to_string())
    }

    fn append(&self, id: &str, record: &SessionRecord) -> Result<(), String> {
        self.0.append(id, record)
    }

    fn load(&self, id: &str) -> Result<Option<tupi_app::sessions::StoredSession>, String> {
        self.0.load(id)
    }

    fn list(&self) -> Result<Vec<tupi_app::sessions::SessionSummary>, String> {
        self.0.list()
    }

    fn update_meta(&self, meta: &SessionMeta) -> Result<(), String> {
        self.0.update_meta(meta)
    }

    fn delete(&self, id: &str) -> Result<bool, String> {
        self.0.delete(id)
    }

    fn lock(&self, id: &str) -> Result<SessionLock, String> {
        self.0.lock(id)
    }
}

fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
    let registry = ProviderRegistry::new();
    registry.register(provider);
    registry
}

fn app(provider: Arc<dyn Provider>, bus: EventBus, archive: Arc<dyn SessionArchive>) -> Arc<App> {
    let provider_id = provider.id().to_string();
    App::with_tools(
        registry(provider),
        provider_id,
        "modelo".into(),
        bus,
        None,
        tupi_kernel::ToolRegistry::new(),
        String::new(),
    )
    .with_session_archive(archive)
}

#[tokio::test]
async fn cancel_records_the_partial_answer() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let archive = Arc::new(InMemorySessionArchive::new("repo"));
    let app = app(Arc::new(Partial), bus, archive.clone());

    let outcome = app
        .handle(Command::SendMessage { text: "vai".into() })
        .await
        .unwrap();
    let CommandOutcome::Started { request_id } = outcome else {
        panic!("esperava Started: {outcome:?}");
    };

    loop {
        match rx.recv().await.unwrap() {
            Event::Token { .. } => break,
            Event::StreamFailed { .. } => panic!("stream falhou"),
            _ => {}
        }
    }

    app.handle(Command::CancelStream { request_id })
        .await
        .unwrap();

    let sessions = archive.list().unwrap();
    assert_eq!(sessions.len(), 1);
    let stored = archive.load(&sessions[0].id).unwrap().unwrap();
    assert!(
        stored
            .messages
            .iter()
            .any(|message| message.role == Role::Assistant && message.content == "parcial"),
        "{:?}",
        stored.messages
    );
}

#[tokio::test]
async fn persistence_failure_does_not_fail_the_turn() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let archive = Arc::new(FailingArchive(InMemorySessionArchive::new("repo")));
    let app = app(Arc::new(Scripted), bus, archive);

    app.handle(Command::SendMessage { text: "oi".into() })
        .await
        .unwrap();

    let mut noticed = false;
    loop {
        match rx.recv().await.unwrap() {
            Event::Notice { message } if message.starts_with("sessão não gravada") => {
                noticed = true;
            }
            Event::StreamFinished { .. } | Event::StreamFailed { .. } => break,
            _ => {}
        }
    }
    assert!(noticed, "faltou o aviso de gravação");
}
