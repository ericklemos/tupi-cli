use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::StreamExt;
use tupi_domain::{Event, RequestId, Role, Session};
use tupi_kernel::{
    ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, ProviderEvent,
    ProviderRegistry, Query, QueryError, QueryHandler, QueryResult,
};
use tupi_store::{MODELS_TTL_SECS, Store};

use crate::store::SessionStore;

const SESSION_ID: u64 = 1;

struct StreamHandle {
    request_id: RequestId,
    abort: tokio::task::AbortHandle,
    partial: Arc<Mutex<String>>,
}

pub struct App {
    session: Arc<SessionStore>,
    registry: ProviderRegistry,
    provider_id: String,
    model: Mutex<String>,
    events: EventBus,
    store: Option<Arc<Store>>,
    next_request: AtomicU64,
    active: Arc<AtomicU64>,
    current: Arc<Mutex<Option<StreamHandle>>>,
}

impl App {
    pub fn new(
        registry: ProviderRegistry,
        provider_id: impl Into<String>,
        model: String,
        events: EventBus,
    ) -> Arc<Self> {
        Self::with_store(registry, provider_id, model, events, None)
    }

    pub fn with_store(
        registry: ProviderRegistry,
        provider_id: impl Into<String>,
        model: String,
        events: EventBus,
        store: Option<Arc<Store>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session: Arc::new(SessionStore::new(SESSION_ID)),
            registry,
            provider_id: provider_id.into(),
            model: Mutex::new(model),
            events,
            next_request: AtomicU64::new(1),
            active: Arc::new(AtomicU64::new(0)),
            current: Arc::new(Mutex::new(None)),
            store,
        })
    }

    pub fn session(&self) -> Session {
        self.session.snapshot()
    }

    fn provider(&self) -> Result<std::sync::Arc<dyn tupi_kernel::Provider>, CommandError> {
        self.registry.get(&self.provider_id).ok_or_else(|| {
            CommandError::Internal(format!("provedor '{}' não registrado", self.provider_id))
        })
    }

    fn set_model(&self, model: String) -> Result<CommandOutcome, CommandError> {
        if self.active.load(Ordering::SeqCst) != 0 {
            return Err(CommandError::Busy);
        }

        *self.model.lock().unwrap() = model.clone();
        Ok(CommandOutcome::ModelChanged { model })
    }

    fn send_message(&self, text: String) -> Result<CommandOutcome, CommandError> {
        if self
            .active
            .compare_exchange(0, u64::MAX, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(CommandError::Busy);
        }

        let provider = match self.provider() {
            Ok(provider) => provider,
            Err(error) => {
                self.active.store(0, Ordering::SeqCst);
                return Err(error);
            }
        };

        let request_id = self.next_request.fetch_add(1, Ordering::SeqCst);
        self.active.store(request_id, Ordering::SeqCst);
        self.session.push(Role::User, text);

        let request = ChatRequest {
            model: self.model.lock().unwrap().clone(),
            messages: self.session.messages(),
        };

        let partial = Arc::new(Mutex::new(String::new()));
        let events = self.events.clone();
        let session = Arc::clone(&self.session);
        let active = Arc::clone(&self.active);
        let current = Arc::clone(&self.current);
        let task_partial = Arc::clone(&partial);

        let task = tokio::spawn(async move {
            let mut failure = None;

            match provider.stream_chat(request).await {
                Err(error) => failure = Some(error.to_string()),
                Ok(mut stream) => {
                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(ProviderEvent::Text(text)) => {
                                task_partial.lock().unwrap().push_str(&text);
                                events.publish(Event::Token { request_id, text });
                            }
                            Err(error) => {
                                failure = Some(error.to_string());
                                break;
                            }
                        }
                    }
                }
            }

            let mut slot = current.lock().unwrap();
            if slot.as_ref().map(|handle| handle.request_id) == Some(request_id) {
                *slot = None;
            }
            drop(slot);

            let content = task_partial.lock().unwrap().clone();
            if !content.is_empty() {
                session.push(Role::Assistant, content);
            }

            match failure {
                Some(message) => {
                    events.publish(Event::StreamFailed {
                        request_id,
                        message,
                    });
                }
                None => {
                    events.publish(Event::StreamFinished { request_id });
                }
            }

            active.store(0, Ordering::SeqCst);
        });

        *self.current.lock().unwrap() = Some(StreamHandle {
            request_id,
            abort: task.abort_handle(),
            partial,
        });

        Ok(CommandOutcome::Started { request_id })
    }

    fn cancel(&self, request_id: RequestId) -> CommandOutcome {
        if self
            .active
            .compare_exchange(request_id, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return CommandOutcome::Noop;
        }

        let handle = self.current.lock().unwrap().take();
        let (abort, partial) = match handle {
            Some(handle) if handle.request_id == request_id => {
                (Some(handle.abort), Some(handle.partial))
            }
            _ => (None, None),
        };

        let partial_text = partial
            .map(|partial| partial.lock().unwrap().clone())
            .unwrap_or_default();

        if let Some(abort) = abort {
            abort.abort();
        }

        if !partial_text.is_empty() {
            self.session.push_assistant_once(&partial_text);
        }

        self.events.publish(Event::StreamFinished { request_id });
        CommandOutcome::Cancelled
    }
}

#[async_trait]
impl CommandHandler for App {
    async fn handle(&self, command: Command) -> Result<CommandOutcome, CommandError> {
        match command {
            Command::SendMessage { text } => self.send_message(text),
            Command::CancelStream { request_id } => Ok(self.cancel(request_id)),
            Command::SetModel { model } => self.set_model(model),
            Command::RefreshModels => {
                // A TUI só reage a eventos: banco e provedor rodam fora do loop.
                let provider = self.provider()?;
                let events = self.events.clone();
                let store = self.store.clone();
                let provider_id = self.provider_id.clone();

                tokio::spawn(async move {
                    let now_unix = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|elapsed| elapsed.as_secs())
                        .unwrap_or(0);

                    let cached = match store.as_ref() {
                        Some(store) => {
                            let store = Arc::clone(store);
                            let id = provider_id.clone();
                            tokio::task::spawn_blocking(move || store.models(&id))
                                .await
                                .ok()
                                .and_then(Result::ok)
                                .flatten()
                        }
                        None => None,
                    };

                    if let Some(entry) = cached.as_ref() {
                        if entry.is_fresh(now_unix, MODELS_TTL_SECS) {
                            events.publish(Event::ModelsLoaded {
                                models: entry.models.clone(),
                            });
                            return;
                        }
                    }

                    match provider.list_models().await {
                        Ok(models) => {
                            if let Some(store) = store.as_ref() {
                                let store = Arc::clone(store);
                                let id = provider_id.clone();
                                let to_save = models.clone();

                                match tokio::task::spawn_blocking(move || {
                                    store.save_models(&id, &to_save, now_unix)
                                })
                                .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(error)) => {
                                        tracing::warn!(%error, "falha ao gravar catálogo local");
                                    }
                                    Err(error) => {
                                        tracing::warn!(%error, "falha ao agendar gravação");
                                    }
                                }
                            }

                            events.publish(Event::ModelsLoaded { models });
                        }
                        Err(error) => {
                            if let Some(entry) = cached {
                                tracing::warn!(%error, "usando catálogo salvo");
                                events.publish(Event::ModelsLoaded {
                                    models: entry.models,
                                });
                            } else {
                                events.publish(Event::ModelsFailed {
                                    message: error.to_string(),
                                });
                            }
                        }
                    }
                });

                Ok(CommandOutcome::Noop)
            }
        }
    }
}

#[async_trait]
impl QueryHandler for App {
    async fn query(&self, query: Query) -> Result<QueryResult, QueryError> {
        match query {
            Query::History => Ok(QueryResult::History(self.session.messages())),
            Query::ListProviders => Ok(QueryResult::Providers(self.registry.ids())),
            Query::ListModels => {
                let provider = self.registry.get(&self.provider_id).ok_or_else(|| {
                    QueryError::Internal(format!("provedor '{}' não registrado", self.provider_id))
                })?;
                Ok(QueryResult::Models(provider.list_models().await?))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures_util::StreamExt;
    use tupi_domain::{Event, Role};
    use tupi_kernel::{
        ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, Provider,
        ProviderError, ProviderEvent, ProviderRegistry, ProviderStream, Query, QueryHandler,
        QueryResult,
    };

    struct FakeProvider {
        tokens: Vec<&'static str>,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn id(&self) -> &str {
            "fake"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let items: Vec<Result<ProviderEvent, ProviderError>> = self
                .tokens
                .iter()
                .map(|token| Ok(ProviderEvent::Text((*token).to_string())))
                .collect();
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    struct HangingProvider;

    #[async_trait]
    impl Provider for HangingProvider {
        fn id(&self) -> &str {
            "hanging"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let first = futures_util::stream::iter(vec![Ok(ProviderEvent::Text("a".to_string()))]);
            Ok(Box::pin(first.chain(futures_util::stream::pending::<
                Result<ProviderEvent, ProviderError>,
            >())))
        }
    }

    struct FailingProvider;

    #[async_trait]
    impl Provider for FailingProvider {
        fn id(&self) -> &str {
            "failing"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let items = vec![
                Ok(ProviderEvent::Text("a".to_string())),
                Err(ProviderError::Network("caiu".to_string())),
            ];
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    struct ModelProvider;

    #[async_trait]
    impl Provider for ModelProvider {
        fn id(&self) -> &str {
            "models"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(vec![
                "deepseek-flash".to_string(),
                "deepseek-v4-pro".to_string(),
            ])
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
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

    #[tokio::test]
    async fn list_models_query_returns_provider_models() {
        let app = App::new(
            registry(Arc::new(ModelProvider)),
            "models",
            "modelo".into(),
            EventBus::new(4),
        );

        let QueryResult::Models(models) = app.query(Query::ListModels).await.unwrap() else {
            panic!("esperava Models");
        };

        assert_eq!(
            models,
            vec!["deepseek-flash".to_string(), "deepseek-v4-pro".to_string()]
        );
    }

    #[tokio::test]
    async fn send_message_publishes_tokens_and_appends_history() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let app = App::new(
            registry(Arc::new(FakeProvider {
                tokens: vec!["a", "b"],
            })),
            "fake",
            "modelo".into(),
            bus,
        );

        let outcome = app
            .handle(Command::SendMessage { text: "oi".into() })
            .await
            .unwrap();
        let CommandOutcome::Started { request_id } = outcome else {
            panic!("esperava Started");
        };

        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "a".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "b".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFinished { request_id }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "oi");
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].content, "ab");
    }

    #[tokio::test]
    async fn second_send_while_streaming_is_busy() {
        let app = App::new(
            registry(Arc::new(HangingProvider)),
            "hanging",
            "modelo".into(),
            EventBus::new(16),
        );

        let first = app
            .handle(Command::SendMessage { text: "1".into() })
            .await
            .unwrap();
        assert!(matches!(first, CommandOutcome::Started { .. }));

        let second = app.handle(Command::SendMessage { text: "2".into() }).await;
        assert!(matches!(second, Err(CommandError::Busy)));
    }

    #[tokio::test]
    async fn cancel_appends_partial_and_finishes() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let app = App::new(
            registry(Arc::new(HangingProvider)),
            "hanging",
            "modelo".into(),
            bus,
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
                text: "a".into()
            }
        );

        let outcome = app
            .handle(Command::CancelStream { request_id })
            .await
            .unwrap();
        assert_eq!(outcome, CommandOutcome::Cancelled);
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFinished { request_id }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].content, "a");
    }

    #[tokio::test]
    async fn failure_appends_partial_and_reports_error() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let app = App::new(
            registry(Arc::new(FailingProvider)),
            "failing",
            "modelo".into(),
            bus,
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
                text: "a".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFailed {
                request_id,
                message: "falha de rede: caiu".into()
            }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].content, "a");
    }
}
