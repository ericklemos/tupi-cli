use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::StreamExt;
use tupi_domain::{Event, Message, RequestId, Role, Session, ToolCall};
use tupi_kernel::{
    ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, ProviderEvent,
    ProviderRegistry, Query, QueryError, QueryHandler, QueryResult, ToolRegistry,
};
use tupi_store::{MODELS_TTL_SECS, Store};

use crate::store::SessionStore;

const SESSION_ID: u64 = 1;

pub const MAX_TOOL_STEPS: usize = 8;

async fn run_tool(tools: &ToolRegistry, call: &ToolCall) -> Result<String, String> {
    let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .map_err(|error| format!("argumento inválido: {error}"))?;
    let tool = tools
        .get(&call.name)
        .ok_or_else(|| format!("tool desconhecida: {}", call.name))?;

    tokio::task::spawn_blocking(move || tool.call(arguments))
        .await
        .map_err(|error| format!("falha ao executar tool: {error}"))?
        .map_err(|error| error.to_string())
}

fn describe_call(tools: &ToolRegistry, call: &ToolCall) -> String {
    serde_json::from_str::<serde_json::Value>(&call.arguments)
        .ok()
        .and_then(|arguments| {
            tools
                .get(&call.name)
                .map(|tool| tool.describe_call(&arguments))
        })
        .unwrap_or_else(|| call.arguments.clone())
}

fn close_unanswered(session: &SessionStore, pending: &Mutex<Vec<ToolCall>>, reason: &str) {
    let calls = std::mem::take(&mut *pending.lock().unwrap());
    for call in calls {
        session.push_message(Message::tool(call.id, reason));
    }
}

fn summarize(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    let mut summary: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        summary.push('…');
    }
    summary
}

struct StreamHandle {
    request_id: RequestId,
    task: tokio::task::JoinHandle<()>,
    partial: Arc<Mutex<String>>,
}

pub struct App {
    session: Arc<SessionStore>,
    registry: ProviderRegistry,
    provider_id: String,
    model: Mutex<String>,
    events: EventBus,
    store: Option<Arc<Store>>,
    tools: ToolRegistry,
    pending: Arc<Mutex<Vec<ToolCall>>>,
    next_request: AtomicU64,
    active: Arc<AtomicU64>,
    current: Arc<Mutex<Option<StreamHandle>>>,
    prefix: String,
    show_prompts: AtomicBool,
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
        Self::with_tools(
            registry,
            provider_id,
            model,
            events,
            store,
            ToolRegistry::new(),
            String::new(),
        )
    }

    pub fn with_tools(
        registry: ProviderRegistry,
        provider_id: impl Into<String>,
        model: String,
        events: EventBus,
        store: Option<Arc<Store>>,
        tools: ToolRegistry,
        prefix: String,
    ) -> Arc<Self> {
        let show_prompts = store
            .as_ref()
            .and_then(|store| match store.setting(crate::debug::SHOW_PROMPTS_KEY) {
                Ok(value) => value,
                Err(error) => {
                    tracing::warn!(%error, "falha ao ler a configuração de debug");
                    None
                }
            })
            .is_some_and(|value| crate::debug::enabled(Some(value.as_str())));

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
            tools,
            pending: Arc::new(Mutex::new(Vec::new())),
            prefix,
            show_prompts: AtomicBool::new(show_prompts),
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

        let model = self.model.lock().unwrap().clone();
        let specs = self.tools.specs();
        let tools = self.tools.clone();
        let prefix = self.prefix.clone();
        let show_prompts = self.show_prompts.load(Ordering::SeqCst);

        let partial = Arc::new(Mutex::new(String::new()));
        let events = self.events.clone();
        let session = Arc::clone(&self.session);
        let active = Arc::clone(&self.active);
        let current = Arc::clone(&self.current);
        let pending = Arc::clone(&self.pending);
        let task_partial = Arc::clone(&partial);

        let task = tokio::spawn(async move {
            let mut failure = None;
            let mut step = 0;
            let mut completion_tokens = 0u64;
            let mut has_usage = false;

            while step < MAX_TOOL_STEPS {
                step += 1;
                task_partial.lock().unwrap().clear();

                let mut messages = Vec::new();
                if !prefix.is_empty() {
                    messages.push(Message::new(Role::System, prefix.clone()));
                }
                messages.extend(session.messages());
                let request = ChatRequest::new(model.clone(), messages).with_tools(specs.clone());

                if show_prompts {
                    events.publish(Event::PromptMessages {
                        request_id,
                        body: crate::debug::render_prompt(&request),
                    });
                }

                let mut streamed_text = String::new();
                let mut calls = Vec::new();
                let mut stream_failed = false;

                match provider.stream_chat(request).await {
                    Err(error) => failure = Some(error.to_string()),
                    Ok(mut stream) => {
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(ProviderEvent::Text(text)) => {
                                    streamed_text.push_str(&text);
                                    task_partial.lock().unwrap().push_str(&text);
                                    events.publish(Event::Token { request_id, text });
                                }
                                Ok(ProviderEvent::ToolCalls(tool_calls)) => {
                                    calls = tool_calls;
                                }
                                Ok(ProviderEvent::Usage {
                                    completion_tokens: tokens,
                                }) => {
                                    completion_tokens += tokens;
                                    has_usage = true;
                                }
                                Err(error) => {
                                    failure = Some(error.to_string());
                                    stream_failed = true;
                                    break;
                                }
                            }
                        }
                    }
                }

                if !streamed_text.is_empty() || !calls.is_empty() {
                    session
                        .push_message(Message::assistant_with_calls(streamed_text, calls.clone()));
                    *pending.lock().unwrap() = calls.clone();
                }

                if stream_failed || failure.is_some() {
                    close_unanswered(
                        &session,
                        &pending,
                        "turno interrompido: chamada não executada",
                    );
                    break;
                }
                if calls.is_empty() {
                    break;
                }

                if step >= MAX_TOOL_STEPS {
                    close_unanswered(
                        &session,
                        &pending,
                        "limite de passos atingido: chamada não executada",
                    );
                    failure = Some(format!(
                        "limite de {MAX_TOOL_STEPS} passos de tools atingido"
                    ));
                    break;
                }

                for call in calls {
                    if active.load(Ordering::SeqCst) != request_id {
                        break;
                    }

                    events.publish(Event::ToolStarted {
                        request_id,
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                        summary: describe_call(&tools, &call),
                    });

                    let (content, ok) = match run_tool(&tools, &call).await {
                        Ok(result) => (result, true),
                        Err(error) => (error, false),
                    };
                    let summary = summarize(&content);

                    events.publish(Event::ToolFinished {
                        request_id,
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        ok,
                        summary,
                    });

                    // O resultado e a baixa no pending são atômicos sob o mesmo
                    // lock que o cancel usa: nenhuma call recebe dois resultados.
                    let mut pending_calls = pending.lock().unwrap();
                    session.push_message(Message::tool(call.id.clone(), content));
                    pending_calls.retain(|pending| pending.id != call.id);
                }
                pending.lock().unwrap().clear();
            }

            let mut slot = current.lock().unwrap();
            if slot.as_ref().map(|handle| handle.request_id) == Some(request_id) {
                *slot = None;
            }
            drop(slot);

            match failure {
                Some(message) => {
                    events.publish(Event::StreamFailed {
                        request_id,
                        message,
                    });
                }
                None => {
                    events.publish(Event::StreamFinished {
                        request_id,
                        completion_tokens: has_usage.then_some(completion_tokens),
                    });
                }
            }

            active.store(0, Ordering::SeqCst);
        });

        *self.current.lock().unwrap() = Some(StreamHandle {
            request_id,
            task,
            partial,
        });

        Ok(CommandOutcome::Started { request_id })
    }

    async fn cancel(&self, request_id: RequestId) -> CommandOutcome {
        if self
            .active
            .compare_exchange(request_id, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return CommandOutcome::Noop;
        }

        let handle = self.current.lock().unwrap().take();
        let (task, partial) = match handle {
            Some(handle) if handle.request_id == request_id => {
                (Some(handle.task), Some(handle.partial))
            }
            _ => (None, None),
        };

        // Espera a task parar de verdade: depois disso ela não escreve mais no
        // histórico, e um novo turno pode começar com o histórico consistente.
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }

        let had_pending = !self.pending.lock().unwrap().is_empty();
        close_unanswered(&self.session, &self.pending, "cancelado pelo usuário");

        if !had_pending {
            let partial_text = partial
                .map(|partial| partial.lock().unwrap().clone())
                .unwrap_or_default();
            if !partial_text.is_empty() {
                self.session.push_assistant_once(&partial_text);
            }
        }

        self.events.publish(Event::StreamFinished {
            request_id,
            completion_tokens: None,
        });
        CommandOutcome::Cancelled
    }
}

#[async_trait]
impl CommandHandler for App {
    async fn handle(&self, command: Command) -> Result<CommandOutcome, CommandError> {
        match command {
            Command::SendMessage { text } => self.send_message(text),
            Command::CancelStream { request_id } => Ok(self.cancel(request_id).await),
            Command::SetModel { model } => self.set_model(model),
            Command::ToggleShowPrompts => {
                let enabled = !self.show_prompts.load(Ordering::SeqCst);

                if let Some(store) = self.store.as_ref()
                    && let Err(error) = store.save_setting(
                        crate::debug::SHOW_PROMPTS_KEY,
                        crate::debug::value(enabled),
                    )
                {
                    return Err(CommandError::Internal(format!(
                        "falha ao gravar configuração: {error}"
                    )));
                }

                self.show_prompts.store(enabled, Ordering::SeqCst);
                Ok(CommandOutcome::ShowPrompts { enabled })
            }
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

                    if let Some(entry) = cached.as_ref()
                        && entry.is_fresh(now_unix, MODELS_TTL_SECS)
                    {
                        events.publish(Event::ModelsLoaded {
                            models: entry.models.clone(),
                        });
                        return;
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
mod prefix_tests {
    use super::*;

    use async_trait::async_trait;
    use tupi_kernel::{ChatRequest, Provider, ProviderError, ProviderEvent, ProviderStream};

    struct Scripted {
        scripts: Mutex<Vec<Vec<Result<ProviderEvent, ProviderError>>>>,
        requests: Mutex<Vec<ChatRequest>>,
    }

    #[async_trait]
    impl Provider for Scripted {
        fn id(&self) -> &str {
            "scripted"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request);
            let next = self.scripts.lock().unwrap().remove(0);
            Ok(Box::pin(futures_util::stream::iter(next)))
        }
    }

    fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(provider);
        registry
    }

    fn text(text: &str) -> Vec<Result<ProviderEvent, ProviderError>> {
        vec![Ok(ProviderEvent::Text(text.to_string()))]
    }

    fn app_with_prefix(provider: Arc<dyn Provider>, bus: EventBus, prefix: &str) -> Arc<App> {
        App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            ToolRegistry::new(),
            prefix.to_string(),
        )
    }

    async fn wait_for_finished(rx: &mut tokio::sync::broadcast::Receiver<Event>) {
        while let Ok(event) = rx.recv().await {
            if matches!(event, Event::StreamFinished { .. }) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn system_prefix_is_the_first_message_of_every_request() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(Scripted {
            scripts: Mutex::new(vec![text("um"), text("dois")]),
            requests: Mutex::new(Vec::new()),
        });
        let app = app_with_prefix(provider.clone(), bus, "PREFIXO");

        app.handle(Command::SendMessage { text: "a".into() })
            .await
            .unwrap();
        wait_for_finished(&mut rx).await;
        app.handle(Command::SendMessage { text: "b".into() })
            .await
            .unwrap();
        wait_for_finished(&mut rx).await;

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests[0].messages[0].role, Role::System);
        assert_eq!(requests[0].messages[0].content, "PREFIXO");
        assert_eq!(requests[1].messages[0], requests[0].messages[0]);
    }

    #[tokio::test]
    async fn empty_prefix_sends_no_system_message() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(Scripted {
            scripts: Mutex::new(vec![text("ok")]),
            requests: Mutex::new(Vec::new()),
        });
        let app = app_with_prefix(provider.clone(), bus, "");

        app.handle(Command::SendMessage { text: "a".into() })
            .await
            .unwrap();
        wait_for_finished(&mut rx).await;

        let requests = provider.requests.lock().unwrap();
        assert!(
            requests[0]
                .messages
                .iter()
                .all(|message| message.role != Role::System)
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures_util::StreamExt;
    use tupi_domain::{Event, Role, ToolCall};
    use tupi_kernel::{
        ChatRequest, Command, CommandError, CommandHandler, CommandOutcome, EventBus, Provider,
        ProviderError, ProviderEvent, ProviderRegistry, ProviderStream, Query, QueryHandler,
        QueryResult, Tool, ToolError, ToolRegistry, ToolSpec,
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
            Event::StreamFinished {
                request_id,
                completion_tokens: None,
            }
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
            Event::StreamFinished {
                request_id,
                completion_tokens: None,
            }
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

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

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
            Ok(Vec::new())
        }

        async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
            self.requests.lock().unwrap().push(request);
            let items = self.scripts.lock().unwrap().pop_front().expect("script");
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    struct EchoTool {
        calls: AtomicUsize,
    }

    impl EchoTool {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl Tool for EchoTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "ecoa os argumentos".into(),
                schema: serde_json::json!({"type": "object"}),
            }
        }

        fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(format!("eco: {arguments}"))
        }
    }

    fn app_with_tools(provider: Arc<dyn Provider>, bus: EventBus, tool: Arc<dyn Tool>) -> Arc<App> {
        let mut tools = ToolRegistry::new();
        tools.register(tool);
        App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        )
    }

    fn text_event(text: &str) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::Text(text.to_string()))
    }

    fn calls_event(calls: Vec<ToolCall>) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::ToolCalls(calls))
    }

    async fn wait_for_end(rx: &mut tokio::sync::broadcast::Receiver<Event>) {
        loop {
            match rx.recv().await.unwrap() {
                Event::StreamFinished { .. } | Event::StreamFailed { .. } => return,
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn tool_call_is_executed_and_the_model_is_called_again() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "echo", "{\"n\":1}")])],
            vec![text_event("fim")],
        ]));
        let app = app_with_tools(provider.clone(), bus, Arc::new(EchoTool::new()));

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage {
                text: "leia".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        match rx.recv().await.unwrap() {
            Event::ToolStarted {
                call_id,
                name,
                summary,
                ..
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(name, "echo");
                assert_eq!(summary, "{\"n\":1}");
            }
            other => panic!("esperava ToolStarted, veio {other:?}"),
        }
        assert!(matches!(
            rx.recv().await.unwrap(),
            Event::ToolFinished { call_id, ok: true, .. } if call_id == "c1"
        ));
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::Token {
                request_id,
                text: "fim".into()
            }
        );
        assert_eq!(
            rx.recv().await.unwrap(),
            Event::StreamFinished {
                request_id,
                completion_tokens: None,
            }
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 4);
        assert_eq!(history[1].role, Role::Assistant);
        assert_eq!(history[1].tool_calls.len(), 1);
        assert_eq!(history[2].role, Role::Tool);
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert!(history[2].content.starts_with("eco:"));
        assert_eq!(history[3].content, "fim");

        let requests = provider.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| message.role == Role::Tool)
        );
    }

    #[tokio::test]
    async fn multiple_tool_calls_run_in_order() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![
                tool_call("c1", "echo", "{\"n\":1}"),
                tool_call("c2", "echo", "{\"n\":2}"),
            ])],
            vec![text_event("fim")],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(history[3].tool_call_id.as_deref(), Some("c2"));
    }

    #[tokio::test]
    async fn malformed_arguments_do_not_invoke_the_tool() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "echo", "{")])],
            vec![text_event("fim")],
        ]));
        let tool = Arc::new(EchoTool::new());
        let app = app_with_tools(provider, bus, tool.clone());

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();

        wait_for_end(&mut rx).await;
        assert_eq!(tool.calls.load(Ordering::SeqCst), 0);
        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert!(history[2].content.contains("argumento inválido"));
    }

    #[tokio::test]
    async fn unknown_tool_returns_an_explicit_tool_message() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call("c1", "nope", "{}")])],
            vec![text_event("fim")],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();

        wait_for_end(&mut rx).await;
        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert!(history[2].content.contains("tool desconhecida"));
    }

    #[tokio::test]
    async fn app_runs_the_real_read_tool_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
        )
        .unwrap();

        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call(
                "c1",
                "read",
                "{\"path\":\"auth.rs\",\"symbol\":\"verify_token\"}",
            )])],
            vec![text_event("li")],
        ]));
        let mut tools = ToolRegistry::new();
        tupi_tools::register(dir.path().to_path_buf(), &mut tools);
        let app = App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].role, Role::Tool);
        assert!(
            history[2].content.contains("!raw.is_empty()"),
            "{:?}",
            history[2]
        );
    }

    #[tokio::test]
    async fn app_runs_the_real_search_tool_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
        )
        .unwrap();

        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call(
                "c1",
                "search",
                "{\"query\":\"onde o token é validado\"}",
            )])],
            vec![text_event("li")],
        ]));
        let mut tools = ToolRegistry::new();
        tupi_tools::register(dir.path().to_path_buf(), &mut tools);
        let app = App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        app.handle(Command::SendMessage {
            text: "procure".into(),
        })
        .await
        .unwrap();

        let mut summary = None;
        loop {
            match rx.recv().await.unwrap() {
                Event::ToolStarted { summary: value, .. } => summary = Some(value),
                Event::StreamFinished { .. } => break,
                Event::StreamFailed { message, .. } => panic!("stream falhou: {message}"),
                _ => {}
            }
        }

        assert!(
            summary
                .as_deref()
                .is_some_and(|value| value.contains("query=")),
            "{summary:?}"
        );
        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].role, Role::Tool);
        assert!(
            history[2].content.contains("verify_token"),
            "{:?}",
            history[2]
        );
        assert!(
            history[2].content.contains("!raw.is_empty()"),
            "{:?}",
            history[2]
        );
    }

    #[tokio::test]
    async fn app_runs_edit_then_read_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.rs");
        let content = "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n";
        std::fs::write(&path, content).unwrap();
        let hash = blake3::hash(content.as_bytes()).to_hex()[..12].to_string();

        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![tool_call(
                "c1",
                "edit",
                &serde_json::json!({
                    "path": "auth.rs",
                    "symbol": "verify_token",
                    "expected_hash": hash,
                    "new_body": "{\n    raw.len() > 3\n}"
                })
                .to_string(),
            )])],
            vec![calls_event(vec![tool_call(
                "c2",
                "read",
                "{\"path\":\"auth.rs\",\"symbol\":\"verify_token\"}",
            )])],
            vec![text_event("ok")],
        ]));
        let mut tools = ToolRegistry::new();
        tupi_tools::register(dir.path().to_path_buf(), &mut tools);
        let app = App::with_tools(
            registry(provider),
            "scripted",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        app.handle(Command::SendMessage {
            text: "edite".into(),
        })
        .await
        .unwrap();

        let mut summaries = Vec::new();
        loop {
            match rx.recv().await.unwrap() {
                Event::ToolStarted { summary, .. } => summaries.push(summary),
                Event::StreamFinished { .. } => break,
                Event::StreamFailed { message, .. } => panic!("stream falhou: {message}"),
                _ => {}
            }
        }

        assert!(
            summaries
                .iter()
                .any(|summary| summary.contains("symbol=verify_token")),
            "{summaries:?}"
        );

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history[2].role, Role::Tool);
        assert!(history[2].content.contains("editado"), "{:?}", history[2]);
        assert_eq!(history[4].role, Role::Tool);
        assert!(
            history[4].content.contains("raw.len() > 3"),
            "{:?}",
            history[4]
        );
        assert!(
            !history[4].content.contains("!raw.is_empty()"),
            "{:?}",
            history[4]
        );
    }

    struct SlowTool;

    impl Tool for SlowTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "echo".into(),
                description: "demora".into(),
                schema: serde_json::json!({"type": "object"}),
            }
        }

        fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
            std::thread::sleep(std::time::Duration::from_millis(50));
            Ok("lento".into())
        }
    }

    #[tokio::test]
    async fn stream_error_after_tool_calls_closes_pending_calls() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            calls_event(vec![tool_call("c1", "echo", "{}")]),
            Err(ProviderError::Network("caiu".into())),
        ]]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 3);
        assert_eq!(history[1].tool_calls.len(), 1);
        assert_eq!(history[2].role, Role::Tool);
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert!(history[2].content.contains("interrompido"));
    }

    #[tokio::test]
    async fn cancel_during_tool_execution_closes_every_pending_call() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![calls_event(vec![
                tool_call("c1", "echo", "{}"),
                tool_call("c2", "echo", "{}"),
            ])],
            vec![text_event("depois")],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(SlowTool));

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage {
                text: "leia".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        // Espera a primeira tool começar e cancela enquanto ela roda.
        loop {
            match rx.recv().await.unwrap() {
                Event::ToolStarted { call_id, .. } if call_id == "c1" => break,
                Event::StreamFinished { .. } | Event::StreamFailed { .. } => {
                    panic!("turno terminou antes do cancelamento")
                }
                _ => {}
            }
        }

        let outcome = app
            .handle(Command::CancelStream { request_id })
            .await
            .unwrap();
        assert_eq!(outcome, CommandOutcome::Cancelled);
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        let tool_ids: Vec<&str> = history
            .iter()
            .filter(|message| message.role == Role::Tool)
            .filter_map(|message| message.tool_call_id.as_deref())
            .collect();
        assert_eq!(tool_ids, vec!["c1", "c2"]);
        assert_eq!(history[1].tool_calls.len(), 2);

        // O histórico está consistente: um novo turno é aceito e completa.
        let CommandOutcome::Started { .. } = app
            .handle(Command::SendMessage {
                text: "de novo".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };
        wait_for_end(&mut rx).await;

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.last().unwrap().content, "depois");
    }

    struct LoopingProvider {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Provider for LoopingProvider {
        fn id(&self) -> &str {
            "looping"
        }

        async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
            Ok(Vec::new())
        }

        async fn stream_chat(
            &self,
            _request: ChatRequest,
        ) -> Result<ProviderStream, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let items = vec![calls_event(vec![tool_call(&format!("c{n}"), "echo", "{}")])];
            Ok(Box::pin(futures_util::stream::iter(items)))
        }
    }

    #[tokio::test]
    async fn step_limit_synthesizes_results_for_unanswered_calls() {
        let bus = EventBus::new(64);
        let mut rx = bus.subscribe();
        let provider = Arc::new(LoopingProvider {
            calls: AtomicUsize::new(0),
        });
        let mut tools = ToolRegistry::new();
        tools.register(Arc::new(EchoTool::new()));
        let app = App::with_tools(
            registry(provider),
            "looping",
            "modelo".into(),
            bus,
            None,
            tools,
            String::new(),
        );

        let CommandOutcome::Started { request_id } = app
            .handle(Command::SendMessage {
                text: "leia".into(),
            })
            .await
            .unwrap()
        else {
            panic!("esperava Started");
        };

        let mut failed = None;
        for _ in 0..40 {
            let event = rx.recv().await.unwrap();
            if let Event::StreamFailed { message, .. } = event {
                failed = Some(message);
                break;
            }
        }
        assert!(failed.unwrap().contains("limite"));

        let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
            panic!("esperava History");
        };
        assert_eq!(history.len(), 1 + MAX_TOOL_STEPS * 2);
        let last = history.last().unwrap();
        assert_eq!(last.role, Role::Tool);
        assert!(last.content.contains("limite de passos"));
        let _ = request_id;
    }

    fn usage_event(tokens: u64) -> Result<ProviderEvent, ProviderError> {
        Ok(ProviderEvent::Usage {
            completion_tokens: tokens,
        })
    }

    #[tokio::test]
    async fn usage_from_every_step_is_summed() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![
                text_event("vou ler"),
                usage_event(10),
                calls_event(vec![tool_call("c1", "echo", "{\"n\":1}")]),
            ],
            vec![text_event("fim"), usage_event(5)],
        ]));
        let app = app_with_tools(provider, bus, Arc::new(EchoTool::new()));

        app.handle(Command::SendMessage {
            text: "leia".into(),
        })
        .await
        .unwrap();

        let mut total = None;
        for _ in 0..32 {
            if let Event::StreamFinished {
                completion_tokens, ..
            } = rx.recv().await.unwrap()
            {
                total = completion_tokens;
                break;
            }
        }

        assert_eq!(total, Some(15));
    }
}
