//! Duas leituras no mesmo passo rodam juntas, sem trocar a ordem declarada.
//!
//! O caminho serial pagava N vezes uma execução local que não tem dependência
//! entre si. O caminho paralelo não pode custar a ordem da história nem a ordem
//! dos eventos, que são contrato (`tool_parallelism.rs` guarda o lado mutante).

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

fn read_call(id: &str, tag: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "read".into(),
        arguments: serde_json::json!({ "tag": tag }).to_string(),
    }
}

/// Leitura lenta e sem efeito próprio: pode rodar junto da vizinha do passo.
#[derive(Default)]
struct ReadProbe {
    active: AtomicUsize,
    peak: AtomicUsize,
    entered: Mutex<Vec<String>>,
}

impl Tool for ReadProbe {
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::ReadOnly
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: "leitura lenta para expor paralelismo".into(),
            schema: serde_json::json!({"type": "object"}),
        }
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
        std::thread::sleep(Duration::from_millis(120));
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(format!("leu {tag}"))
    }
}

#[tokio::test]
async fn read_only_calls_of_the_same_step_run_together() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(Scripted::new(vec![
        vec![Ok(ProviderEvent::ToolCalls(vec![
            read_call("c1", "um"),
            read_call("c2", "dois"),
        ]))],
        vec![Ok(ProviderEvent::Text("fim".into()))],
    ]));

    let probe = Arc::new(ReadProbe::default());
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

    let started = std::time::Instant::now();
    app.handle(Command::SendMessage {
        text: "ler dois".into(),
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
    let elapsed = started.elapsed();

    assert_eq!(
        probe.peak.load(Ordering::SeqCst),
        2,
        "as duas leituras do mesmo passo não rodaram juntas"
    );
    assert!(
        elapsed < Duration::from_millis(210),
        "duas leituras de 120 ms levaram {elapsed:?}: não houve sobreposição"
    );
    assert_eq!(
        timeline,
        vec!["start c1", "end c1", "start c2", "end c2"],
        "a ordem declarada dos eventos é contrato"
    );

    let QueryResult::History(history) = app.query(Query::History).await.unwrap() else {
        panic!("esperava History");
    };
    let ids: Vec<&str> = history
        .iter()
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect();
    assert_eq!(
        ids,
        vec!["c1", "c2"],
        "a história grava na ordem declarada, não na ordem de conclusão"
    );
}
