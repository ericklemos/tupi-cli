use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tupi_app::app::App;
use tupi_domain::Event;
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream, ToolRegistry,
};
use tupi_store::Store;

struct Scripted;

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::iter(vec![Ok(
            ProviderEvent::Text("ok".to_string()),
        )])))
    }
}

fn app(provider: Arc<dyn Provider>, store: Option<Arc<Store>>, bus: EventBus) -> Arc<App> {
    let mut registry = ProviderRegistry::new();
    registry.register(provider);
    App::with_tools(
        registry,
        "scripted",
        "modelo".into(),
        bus,
        store,
        ToolRegistry::new(),
        String::new(),
    )
}

fn temp_store() -> (tempfile::TempDir, PathBuf, Arc<Store>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tupi.redb");
    let store = Arc::new(Store::open(&path).unwrap());
    (dir, path, store)
}

fn expect_prompt(event: Event) -> (u64, String) {
    match event {
        Event::PromptMessages { request_id, body } => (request_id, body),
        other => panic!("esperava PromptMessages, veio {other:?}"),
    }
}

async fn drain_until_finished(rx: &mut tokio::sync::broadcast::Receiver<Event>) {
    while let Ok(event) = rx.recv().await {
        if matches!(event, Event::StreamFinished { .. }) {
            return;
        }
    }
}

#[tokio::test]
async fn enabling_prompts_publishes_the_body_before_the_answer() {
    let bus = EventBus::new(32);
    let mut rx = bus.subscribe();
    let app = app(Arc::new(Scripted), None, bus);

    let outcome = app.handle(Command::ToggleShowPrompts).await.unwrap();
    assert_eq!(outcome, CommandOutcome::ShowPrompts { enabled: true });

    let CommandOutcome::Started { request_id } = app
        .handle(Command::SendMessage { text: "oi".into() })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    let (id, body) = expect_prompt(rx.recv().await.unwrap());
    assert_eq!(id, request_id);
    assert!(body.contains("modelo: modelo"), "{body}");
    assert!(body.contains("[user]"), "{body}");
    assert!(body.contains("oi"), "{body}");
    assert!(body.contains("tools: 0"), "{body}");

    assert!(matches!(
        rx.recv().await.unwrap(),
        Event::Token { text, .. } if text == "ok"
    ));
}

#[tokio::test]
async fn prompts_are_silent_by_default_and_toggle_back_off() {
    let bus = EventBus::new(32);
    let mut rx = bus.subscribe();
    let app = app(Arc::new(Scripted), None, bus);

    app.handle(Command::SendMessage { text: "um".into() })
        .await
        .unwrap();
    assert!(matches!(
        rx.recv().await.unwrap(),
        Event::Token { text, .. } if text == "ok"
    ));
    drain_until_finished(&mut rx).await;

    app.handle(Command::ToggleShowPrompts).await.unwrap();
    let outcome = app.handle(Command::ToggleShowPrompts).await.unwrap();
    assert_eq!(outcome, CommandOutcome::ShowPrompts { enabled: false });

    app.handle(Command::SendMessage {
        text: "dois".into(),
    })
    .await
    .unwrap();
    assert!(matches!(
        rx.recv().await.unwrap(),
        Event::Token { text, .. } if text == "ok"
    ));
}

#[tokio::test]
async fn the_setting_survives_a_restart() {
    let (_dir, path, store) = temp_store();

    {
        let bus = EventBus::new(32);
        let app = app(Arc::new(Scripted), Some(store), bus);
        let outcome = app.handle(Command::ToggleShowPrompts).await.unwrap();
        assert_eq!(outcome, CommandOutcome::ShowPrompts { enabled: true });
    }

    let reopened = Arc::new(Store::open(path).unwrap());
    let bus = EventBus::new(32);
    let mut rx = bus.subscribe();
    let app = app(Arc::new(Scripted), Some(reopened), bus);

    app.handle(Command::SendMessage {
        text: "persistiu".into(),
    })
    .await
    .unwrap();

    let (_, body) = expect_prompt(rx.recv().await.unwrap());
    assert!(body.contains("persistiu"), "{body}");
}
