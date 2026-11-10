use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use tupi_app::App;
use tupi_domain::Event;
use tupi_kernel::{
    ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, Provider,
    ProviderError, ProviderEvent, ProviderRegistry, ProviderStream,
};

struct EchoModelProvider;

#[async_trait]
impl Provider for EchoModelProvider {
    fn id(&self) -> &str {
        "echo"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let items = vec![Ok(ProviderEvent::Text(request.model))];
        Ok(Box::pin(futures_util::stream::iter(items)))
    }
}

#[async_trait]
impl Provider for SlowProvider {
    fn id(&self) -> &str {
        "slow"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(Vec::new())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let first = futures_util::stream::iter(vec![Ok(ProviderEvent::Text("a".to_string()))]);
        Ok(Box::pin(first.chain(futures_util::stream::pending::<
            Result<ProviderEvent, ProviderError>,
        >())))
    }
}

struct SlowProvider;

fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(provider);
    registry
}

#[tokio::test]
async fn set_model_changes_model_for_next_request() {
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();
    let app = App::new(
        registry(Arc::new(EchoModelProvider)),
        "echo",
        "deepseek-flash".into(),
        bus,
    );

    let outcome = app
        .handle(Command::SetModel {
            model: "deepseek-v4-pro".into(),
        })
        .await
        .unwrap();
    assert_eq!(
        outcome,
        CommandOutcome::ModelChanged {
            model: "deepseek-v4-pro".into()
        }
    );

    let CommandOutcome::Started { request_id } = app
        .handle(Command::SendMessage { text: "oi".into() })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    assert_eq!(
        rx.recv().await.unwrap(),
        Event::Token {
            request_id,
            text: "deepseek-v4-pro".into()
        }
    );
}

#[tokio::test]
async fn set_model_while_streaming_is_busy() {
    let app = App::new(
        registry(Arc::new(SlowProvider)),
        "slow",
        "modelo".into(),
        EventBus::new(16),
    );

    app.handle(Command::SendMessage { text: "oi".into() })
        .await
        .unwrap();

    let outcome = app
        .handle(Command::SetModel {
            model: "outro".into(),
        })
        .await;

    assert!(matches!(outcome, Err(CommandError::Busy)));
}
