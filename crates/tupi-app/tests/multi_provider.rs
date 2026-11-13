use std::sync::Arc;
use tupi_app::App;
use tupi_domain::Event;
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream, ToolRegistry,
};

struct DynamicMockProvider {
    name: String,
    models: Vec<String>,
}

#[async_trait::async_trait]
impl Provider for DynamicMockProvider {
    fn id(&self) -> &str {
        &self.name
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self.models.clone())
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::empty::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

#[tokio::test]
async fn set_model_with_provider_prefix_switches_provider() {
    let reg = ProviderRegistry::new();
    reg.register(Arc::new(DynamicMockProvider {
        name: "prov_a".into(),
        models: vec!["model_1".into()],
    }));
    reg.register(Arc::new(DynamicMockProvider {
        name: "prov_b".into(),
        models: vec!["model_2".into()],
    }));

    let bus = EventBus::new(8);
    let app = App::with_tools(
        reg,
        "prov_a",
        "model_1".into(),
        bus,
        None,
        ToolRegistry::new(),
        String::new(),
    );

    let outcome = app
        .handle(Command::SetModel {
            model: "prov_b/model_2".into(),
        })
        .await
        .unwrap();

    assert_eq!(
        outcome,
        CommandOutcome::ModelChanged {
            model: "prov_b/model_2".into()
        }
    );
    assert_eq!(app.provider().unwrap().id(), "prov_b");
}

#[tokio::test]
async fn set_model_with_unknown_provider_returns_error() {
    let reg = ProviderRegistry::new();
    reg.register(Arc::new(DynamicMockProvider {
        name: "prov_a".into(),
        models: vec!["model_1".into()],
    }));

    let bus = EventBus::new(8);
    let app = App::with_tools(
        reg,
        "prov_a",
        "model_1".into(),
        bus,
        None,
        ToolRegistry::new(),
        String::new(),
    );

    let res = app
        .handle(Command::SetModel {
            model: "inexistente/model_x".into(),
        })
        .await;

    assert!(res.is_err());
}

#[tokio::test]
async fn refresh_models_aggregates_multiple_providers() {
    let reg = ProviderRegistry::new();
    reg.register(Arc::new(DynamicMockProvider {
        name: "prov_a".into(),
        models: vec!["mod_a1".into()],
    }));
    reg.register(Arc::new(DynamicMockProvider {
        name: "prov_b".into(),
        models: vec!["mod_b1".into()],
    }));

    let bus = EventBus::new(8);
    let mut rx = bus.subscribe();
    let app = App::with_tools(
        reg,
        "prov_a",
        "mod_a1".into(),
        bus,
        None,
        ToolRegistry::new(),
        String::new(),
    );

    app.handle(Command::RefreshModels).await.unwrap();

    loop {
        if let Event::ModelsLoaded { models } = rx.recv().await.unwrap() {
            assert!(models.contains(&"prov_a/mod_a1".to_string()));
            assert!(models.contains(&"prov_b/mod_b1".to_string()));
            break;
        }
    }
}
