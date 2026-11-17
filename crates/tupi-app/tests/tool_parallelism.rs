//! T5.2 — calls mutantes continuam seriais, na ordem declarada, mesmo com
//! `pipeline::MAX_PARALLEL > 1`.
//!
//! O paralelismo de calls read-only dentro do mesmo passo só pode juntar calls
//! em que `Tool::mutates()` é falso. Este teste falha se um lote paralelo
//! engolir duas calls mutantes (sobreposição) ou trocar a ordem declarada: a
//! timeline dos eventos `ToolStarted`/`ToolFinished` é contrato observável.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream;
use tupi_app::app::App;
use tupi_domain::{Event, ToolCall};
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, EventBus, Provider, ProviderError, ProviderEvent,
    ProviderRegistry, ProviderStream, Query, QueryHandler, QueryResult, Tool, ToolError,
    ToolRegistry, ToolSpec,
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

#[async_trait::async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["modelo".into()])
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let items = self.scripts.lock().unwrap().remove(0);
        Ok(Box::pin(stream::iter(items)))
    }
}

fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
    let registry = ProviderRegistry::new();
    registry.register(provider);
    registry
}

fn call(id: &str, tag: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "mutate".into(),
        arguments: serde_json::json!({ "tag": tag }).to_string(),
    }
}

/// Tool mutante que registra concorrência e ordem de entrada.
#[derive(Default)]
struct MutatingProbe {
    active: AtomicUsize,
    peak: AtomicUsize,
    entered: Mutex<Vec<String>>,
}

impl Tool for MutatingProbe {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mutate".into(),
            description: "mutação lenta para expor paralelismo".into(),
            schema: serde_json::json!({"type": "object"}),
        }
    }
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::Opaque
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        let tag = arguments
            .get("tag")
            .and_then(|tag| tag.as_str())
            .unwrap_or("?")
            .to_string();
        self.entered.lock().unwrap().push(tag.clone());
        std::thread::sleep(Duration::from_millis(40));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(format!("mutou {tag}"))
    }
}

#[tokio::test]
async fn mutating_calls_stay_serial_in_declared_order() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(Scripted::new(vec![
        vec![Ok(ProviderEvent::ToolCalls(vec![
            call("c1", "um"),
            call("c2", "dois"),
        ]))],
        vec![Ok(ProviderEvent::Text("fim".into()))],
    ]));

    let probe = Arc::new(MutatingProbe::default());
    let mut tools = ToolRegistry::new();
    tools.register(probe.clone());

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
        text: "mutar".into(),
    })
    .await
    .unwrap();

    let mut timeline = Vec::new();
    loop {
        match rx.recv().await {
            Ok(Event::ToolStarted { call_id, .. }) => timeline.push(format!("start {call_id}")),
            Ok(Event::ToolFinished { call_id, .. }) => timeline.push(format!("end {call_id}")),
            Ok(Event::StreamFinished { .. }) => break,
            Ok(Event::StreamFailed { message, .. }) => panic!("stream falhou: {message}"),
            Ok(_) => {}
            Err(error) => panic!("eventos encerraram cedo: {error}"),
        }
    }

    assert_eq!(
        timeline,
        vec!["start c1", "end c1", "start c2", "end c2"],
        "calls mutantes não podem se sobrepor nem trocar a ordem declarada"
    );
    assert_eq!(
        probe.peak.load(Ordering::SeqCst),
        1,
        "duas calls mutantes rodaram ao mesmo tempo"
    );
    assert_eq!(
        *probe.entered.lock().unwrap(),
        vec!["um", "dois"],
        "a ordem declarada é contrato"
    );

    let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
        panic!("esperava History");
    };
    let ids: Vec<&str> = history
        .iter()
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect();
    assert_eq!(ids, vec!["c1", "c2"]);
}
