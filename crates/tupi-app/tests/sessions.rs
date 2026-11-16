use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_util::stream;
use tupi_app::app::App;
use tupi_app::sessions::{InMemorySessionArchive, SessionArchive, SessionMeta, SessionRecord};
use tupi_domain::{AgentMode, Event, Message};
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream,
};

struct Scripted {
    scripts: Mutex<Vec<Vec<Result<ProviderEvent, ProviderError>>>>,
}

impl Scripted {
    fn new(scripts: Vec<Vec<Result<ProviderEvent, ProviderError>>>) -> Self {
        Self {
            scripts: Mutex::new(scripts),
        }
    }
}

struct Hanging;

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let items = self.scripts.lock().unwrap().remove(0);
        Ok(Box::pin(stream::iter(items)))
    }
}

#[async_trait]
impl Provider for Hanging {
    fn id(&self) -> &str {
        "hanging"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(stream::pending()))
    }
}

fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
    let registry = ProviderRegistry::new();
    registry.register(provider);
    registry
}

fn app(
    provider: Arc<dyn Provider>,
    bus: EventBus,
    archive: Option<Arc<dyn SessionArchive>>,
) -> Arc<App> {
    let provider_id = provider.id().to_string();
    let app = App::with_tools(
        registry(provider),
        provider_id,
        "modelo".into(),
        bus,
        None,
        tupi_kernel::ToolRegistry::new(),
        String::new(),
    );
    match archive {
        Some(archive) => app.with_session_archive(archive),
        None => app,
    }
}

fn text(value: &str) -> Vec<Result<ProviderEvent, ProviderError>> {
    vec![Ok(ProviderEvent::Text(value.to_string()))]
}

async fn wait_for_end(rx: &mut tokio::sync::broadcast::Receiver<Event>) {
    loop {
        match rx.recv().await {
            Ok(Event::StreamFinished { .. } | Event::StreamFailed { .. }) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

fn meta(id: &str, title: &str, updated_at: u64, mode: AgentMode) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        repo_key: "repo".into(),
        title: title.into(),
        created_at: updated_at,
        updated_at,
        message_count: 0,
        mode,
        provider: "scripted".into(),
        model: "modelo".into(),
    }
}

#[tokio::test]
async fn new_session_keeps_the_previous_file_and_the_next_turn_creates_another() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let archive = Arc::new(InMemorySessionArchive::new("repo"));
    let provider = Arc::new(Scripted::new(vec![text("ok 1"), text("ok 2")]));
    let app = app(provider, bus, Some(archive.clone()));

    app.handle(Command::SendMessage {
        text: "primeira".into(),
    })
    .await
    .unwrap();
    wait_for_end(&mut rx).await;

    app.handle(Command::NewSession).await.unwrap();

    app.handle(Command::SendMessage {
        text: "segunda".into(),
    })
    .await
    .unwrap();
    wait_for_end(&mut rx).await;

    let sessions = archive.list().unwrap();
    assert_eq!(sessions.len(), 2);
    let first = archive.load(&sessions[1].id).unwrap().unwrap();
    assert!(
        first
            .messages
            .iter()
            .any(|message| message.content == "primeira"),
        "{:?}",
        first.messages
    );
}

#[tokio::test]
async fn list_sessions_puts_the_newest_first() {
    let bus = EventBus::new(64);
    let archive = Arc::new(InMemorySessionArchive::new("repo"));
    let _a = archive
        .create(&meta("100-1", "Antiga", 100, AgentMode::Execution))
        .unwrap();
    let _b = archive
        .create(&meta("200-2", "Nova", 200, AgentMode::Execution))
        .unwrap();
    let app = app(Arc::new(Scripted::new(Vec::new())), bus, Some(archive));

    match app.handle(Command::ListSessions).await.unwrap() {
        CommandOutcome::SessionsList { items, ids } => {
            assert_eq!(ids, vec!["200-2", "100-1"]);
            assert!(items[0].contains("Nova"));
            assert!(items[1].contains("Antiga"));
        }
        other => panic!("outcome errado: {other:?}"),
    }
}

#[tokio::test]
async fn resume_restores_messages_mode_and_entries() {
    let bus = EventBus::new(64);
    let archive = Arc::new(InMemorySessionArchive::new("repo"));
    let lock = archive
        .create(&meta("100-1", "Retomável", 100, AgentMode::Sdd))
        .unwrap();
    archive
        .append("100-1", &SessionRecord::user(100, Message::user("oi")))
        .unwrap();
    archive
        .append(
            "100-1",
            &SessionRecord::assistant(101, Message::assistant("olá")),
        )
        .unwrap();
    drop(lock);
    let app = app(Arc::new(Scripted::new(Vec::new())), bus, Some(archive));

    let outcome = app
        .handle(Command::ResumeSession { id: "100-1".into() })
        .await
        .unwrap();

    match outcome {
        CommandOutcome::SessionResumed {
            id,
            title,
            mode,
            entries,
        } => {
            assert_eq!(id, "100-1");
            assert_eq!(title, "Retomável");
            assert_eq!(mode, AgentMode::Sdd);
            assert_eq!(
                entries,
                vec![
                    tupi_kernel::TranscriptEntry::User { text: "oi".into() },
                    tupi_kernel::TranscriptEntry::Assistant {
                        text: "olá".into()
                    }
                ]
            );
        }
        other => panic!("outcome errado: {other:?}"),
    }

    assert_eq!(app.mode(), AgentMode::Sdd);
    assert_eq!(
        app.session()
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["oi", "olá"]
    );
}

#[tokio::test]
async fn resume_refuses_while_a_turn_runs() {
    let bus = EventBus::new(64);
    let archive = Arc::new(InMemorySessionArchive::new("repo"));
    let _ = archive
        .create(&meta("100-1", "A", 100, AgentMode::Execution))
        .unwrap();
    let app = app(Arc::new(Hanging), bus, Some(archive));

    app.handle(Command::SendMessage {
        text: "pendura".into(),
    })
    .await
    .unwrap();

    assert!(matches!(
        app.handle(Command::ResumeSession { id: "100-1".into() })
            .await,
        Err(tupi_kernel::CommandError::Busy)
    ));
}

#[tokio::test]
async fn delete_removes_another_session_but_not_the_current() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let archive = Arc::new(InMemorySessionArchive::new("repo"));
    let app = app(
        Arc::new(Scripted::new(vec![text("ok")])),
        bus,
        Some(archive.clone()),
    );

    app.handle(Command::SendMessage {
        text: "atual".into(),
    })
    .await
    .unwrap();
    wait_for_end(&mut rx).await;
    let current = archive.list().unwrap()[0].id.clone();

    let _ = archive
        .create(&meta("outra", "Outra", 100, AgentMode::Execution))
        .unwrap();
    assert_eq!(
        app.handle(Command::DeleteSession { id: "outra".into() })
            .await
            .unwrap(),
        CommandOutcome::SessionDeleted {
            id: "outra".into(),
            found: true
        }
    );
    assert!(
        app.handle(Command::DeleteSession { id: current })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn without_archive_list_is_empty() {
    let bus = EventBus::new(64);
    let app = app(Arc::new(Scripted::new(Vec::new())), bus, None);

    assert_eq!(
        app.handle(Command::ListSessions).await.unwrap(),
        CommandOutcome::SessionsList {
            items: Vec::new(),
            ids: Vec::new()
        }
    );
}
