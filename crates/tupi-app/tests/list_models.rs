use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tupi_app::App;
use tupi_domain::Event;
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream,
};
use tupi_store::Store;

struct StaticProvider;

#[async_trait]
impl Provider for StaticProvider {
    fn id(&self) -> &str {
        "estatico"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["deepseek-flash".to_string()])
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::empty::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

struct PendingListProvider;

#[async_trait]
impl Provider for PendingListProvider {
    fn id(&self) -> &str {
        "pendente"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        futures_util::future::pending().await
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::empty::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

struct PanicListProvider;

#[async_trait]
impl Provider for PanicListProvider {
    fn id(&self) -> &str {
        "estatico"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        panic!("cache fresco não pode chamar o provedor")
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::empty::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

struct FailingProvider;

#[async_trait]
impl Provider for FailingProvider {
    fn id(&self) -> &str {
        "falha"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Err(ProviderError::Network("caiu".to_string()))
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        Ok(Box::pin(futures_util::stream::empty::<
            Result<ProviderEvent, ProviderError>,
        >()))
    }
}

fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(provider);
    registry
}

fn temp_store(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("tupi-app-{tag}-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("tempo esgotado esperando evento")
        .expect("canal fechado")
}

#[tokio::test]
async fn refresh_models_publishes_the_provider_catalog() {
    let bus = EventBus::new(8);
    let mut rx = bus.subscribe();
    let app = App::new(
        registry(Arc::new(StaticProvider)),
        "estatico",
        "deepseek-flash".into(),
        bus,
    );

    let outcome = app
        .handle(Command::RefreshModels)
        .await
        .expect("o comando não pode falhar");

    assert!(matches!(outcome, CommandOutcome::Noop));
    assert_eq!(
        next_event(&mut rx).await,
        Event::ModelsLoaded {
            models: vec!["deepseek-flash".to_string()]
        }
    );
}

#[tokio::test]
async fn refresh_models_does_not_wait_for_a_slow_provider() {
    let app = App::new(
        registry(Arc::new(PendingListProvider)),
        "pendente",
        "modelo".into(),
        EventBus::new(8),
    );

    let outcome = tokio::time::timeout(
        Duration::from_millis(100),
        app.handle(Command::RefreshModels),
    )
    .await;

    assert!(
        outcome.is_ok(),
        "a TUI não pode ficar presa esperando /models"
    );
}

#[tokio::test]
async fn fresh_cache_skips_the_provider() {
    let store = Arc::new(Store::open(temp_store("fresh")).unwrap());
    store
        .save_models("estatico", &["cache".to_string()], now_unix())
        .unwrap();

    let bus = EventBus::new(8);
    let mut rx = bus.subscribe();
    let app = App::with_store(
        registry(Arc::new(PanicListProvider)),
        "estatico",
        "modelo".into(),
        bus,
        Some(Arc::clone(&store)),
    );

    app.handle(Command::RefreshModels).await.unwrap();

    assert_eq!(
        next_event(&mut rx).await,
        Event::ModelsLoaded {
            models: vec!["cache".to_string()]
        }
    );
}

#[tokio::test]
async fn stale_cache_refreshes_and_saves() {
    let store = Arc::new(Store::open(temp_store("stale")).unwrap());
    store
        .save_models("estatico", &["velho".to_string()], 0)
        .unwrap();

    let bus = EventBus::new(8);
    let mut rx = bus.subscribe();
    let app = App::with_store(
        registry(Arc::new(StaticProvider)),
        "estatico",
        "modelo".into(),
        bus,
        Some(Arc::clone(&store)),
    );

    app.handle(Command::RefreshModels).await.unwrap();

    assert_eq!(
        next_event(&mut rx).await,
        Event::ModelsLoaded {
            models: vec!["deepseek-flash".to_string()]
        }
    );

    let saved = store.models("estatico").unwrap().unwrap();
    assert_eq!(saved.models, vec!["deepseek-flash".to_string()]);
    assert!(saved.fetched_at_unix > 0);
}

#[tokio::test]
async fn provider_failure_falls_back_to_stale_cache() {
    let store = Arc::new(Store::open(temp_store("fallback")).unwrap());
    store
        .save_models("falha", &["salvo".to_string()], 0)
        .unwrap();

    let bus = EventBus::new(8);
    let mut rx = bus.subscribe();
    let app = App::with_store(
        registry(Arc::new(FailingProvider)),
        "falha",
        "modelo".into(),
        bus,
        Some(Arc::clone(&store)),
    );

    app.handle(Command::RefreshModels).await.unwrap();

    assert_eq!(
        next_event(&mut rx).await,
        Event::ModelsLoaded {
            models: vec!["salvo".to_string()]
        }
    );
}

#[tokio::test]
async fn provider_failure_without_cache_reports_models_failed() {
    let store = Arc::new(Store::open(temp_store("nofallback")).unwrap());
    let bus = EventBus::new(8);
    let mut rx = bus.subscribe();
    let app = App::with_store(
        registry(Arc::new(FailingProvider)),
        "falha",
        "modelo".into(),
        bus,
        Some(store),
    );

    app.handle(Command::RefreshModels).await.unwrap();

    assert_eq!(
        next_event(&mut rx).await,
        Event::ModelsFailed {
            message: "falha de rede: caiu".to_string()
        }
    );
}
