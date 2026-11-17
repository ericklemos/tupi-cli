use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream;
use serde_json::{Value, json};
use tupi_app::app::App;
use tupi_domain::{Event, Message, Role, ToolCall};
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream, Query, QueryHandler, QueryResult, Tool,
    ToolError, ToolRegistry, ToolSpec,
};

struct ScriptedProvider {
    scripts: Mutex<VecDeque<Vec<Result<ProviderEvent, ProviderError>>>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Vec<Result<ProviderEvent, ProviderError>>>) -> Self {
        Self {
            scripts: Mutex::new(scripts.into()),
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

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::pin(stream::iter(script)))
    }
}

#[derive(Clone, Copy)]
enum Behavior {
    Echo,
    Record,
    Fail,
}

struct FakeTool {
    name: String,
    behavior: Behavior,
    mutating: bool,
    sleep: Duration,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<Value>>>,
}

impl FakeTool {
    fn new(name: &str, behavior: Behavior) -> Self {
        Self {
            name: name.to_string(),
            behavior,
            mutating: false,
            sleep: Duration::ZERO,
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_counters(mut self, active: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> Self {
        self.active = active;
        self.peak = peak;
        self
    }

    fn mutating(mut self) -> Self {
        self.mutating = true;
        self
    }

    fn sleeping(mut self, millis: u64) -> Self {
        self.sleep = Duration::from_millis(millis);
        self
    }

    fn seen(&self) -> Arc<Mutex<Vec<Value>>> {
        Arc::clone(&self.seen)
    }
}

impl Tool for FakeTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: String::new(),
            schema: json!({"type": "object"}),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        if !self.sleep.is_zero() {
            std::thread::sleep(self.sleep);
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.seen.lock().unwrap().push(arguments.clone());

        match self.behavior {
            Behavior::Echo => Ok(arguments
                .get("n")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string()),
            Behavior::Record => Ok("ok".to_string()),
            Behavior::Fail => Err(ToolError::Other("quebrou".to_string())),
        }
    }
    fn effect(&self, _arguments: &Value) -> tupi_kernel::Effect {
        if self.mutating { tupi_kernel::Effect::Opaque } else { tupi_kernel::Effect::ReadOnly }
    }
}

fn tool_call(id: &str, name: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn pipeline_call(id: &str, steps: Value) -> ToolCall {
    tool_call(id, "pipeline", json!({ "steps": steps }))
}

fn text_event(text: &str) -> Result<ProviderEvent, ProviderError> {
    Ok(ProviderEvent::Text(text.to_string()))
}

fn calls_event(calls: Vec<ToolCall>) -> Result<ProviderEvent, ProviderError> {
    Ok(ProviderEvent::ToolCalls(calls))
}

fn app(provider: Arc<dyn Provider>, bus: EventBus, tools: ToolRegistry) -> Arc<App> {
    let registry = ProviderRegistry::new();
    registry.register(provider);
    App::with_tools(
        registry,
        "scripted",
        "modelo".into(),
        bus,
        None,
        tools,
        String::new(),
    )
}

async fn wait_for_end(rx: &mut tokio::sync::broadcast::Receiver<Event>) {
    loop {
        match rx.recv().await.unwrap() {
            Event::StreamFinished { .. } | Event::StreamFailed { .. } => return,
            _ => {}
        }
    }
}

async fn history(app: &App) -> Vec<Message> {
    match app.query(Query::History).await.unwrap() {
        QueryResult::History(messages) => messages,
        other => panic!("esperava History, veio {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_runs_its_calls_together() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![calls_event(vec![pipeline_call(
            "p1",
            json!([{"calls": [
                {"tool": "count", "arguments": {}},
                {"tool": "count", "arguments": {}},
                {"tool": "count", "arguments": {}}
            ]}]),
        )])],
        vec![text_event("fim")],
    ]));

    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(
        FakeTool::new("count", Behavior::Record)
            .with_counters(active.clone(), peak.clone())
            .sleeping(100),
    ));

    let app = app(provider, bus, tools);
    app.handle(Command::SendMessage { text: "vai".into() })
        .await
        .unwrap();
    wait_for_end(&mut rx).await;

    assert_eq!(peak.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_with_a_mutating_tool_runs_in_series() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![calls_event(vec![pipeline_call(
            "p1",
            json!([{"calls": [
                {"tool": "write_like", "arguments": {}},
                {"tool": "count", "arguments": {}}
            ]}]),
        )])],
        vec![text_event("fim")],
    ]));

    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(
        FakeTool::new("write_like", Behavior::Record)
            .with_counters(active.clone(), peak.clone())
            .mutating()
            .sleeping(100),
    ));
    tools.register(Arc::new(
        FakeTool::new("count", Behavior::Record)
            .with_counters(active, peak.clone())
            .sleeping(100),
    ));

    let app = app(provider, bus, tools);
    app.handle(Command::SendMessage { text: "vai".into() })
        .await
        .unwrap();
    wait_for_end(&mut rx).await;

    assert_eq!(peak.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_next_step_receives_the_previous_concatenation() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![calls_event(vec![pipeline_call(
            "p1",
            json!([
                {"calls": [
                    {"tool": "echo", "arguments": {"n": "a"}},
                    {"tool": "echo", "arguments": {"n": "b"}}
                ]},
                {"calls": [
                    {"tool": "capture", "arguments": {"content": "{{prev}}"}}
                ]}
            ]),
        )])],
        vec![text_event("fim")],
    ]));

    let echo = FakeTool::new("echo", Behavior::Echo);
    let capture = FakeTool::new("capture", Behavior::Record);
    let seen = capture.seen();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(echo));
    tools.register(Arc::new(capture));

    let app = app(provider, bus, tools);
    app.handle(Command::SendMessage { text: "vai".into() })
        .await
        .unwrap();
    wait_for_end(&mut rx).await;

    let captured = seen.lock().unwrap();
    let content = captured[0]["content"].as_str().unwrap();
    assert!(content.contains("### echo"), "{content}");
    assert!(content.lines().any(|line| line == "a"), "{content}");
    assert!(content.lines().any(|line| line == "b"), "{content}");
}

#[tokio::test]
async fn a_failing_call_becomes_a_section_and_the_step_continues() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![calls_event(vec![pipeline_call(
            "p1",
            json!([{"calls": [
                {"tool": "fail", "arguments": {}},
                {"tool": "echo", "arguments": {"n": "ok"}}
            ]}]),
        )])],
        vec![text_event("fim")],
    ]));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(FakeTool::new("fail", Behavior::Fail)));
    tools.register(Arc::new(FakeTool::new("echo", Behavior::Echo)));

    let app = app(provider, bus, tools);
    app.handle(Command::SendMessage { text: "vai".into() })
        .await
        .unwrap();
    wait_for_end(&mut rx).await;

    let messages = history(&app).await;
    let tool = messages
        .iter()
        .find(|message| message.role == Role::Tool)
        .unwrap();
    assert!(tool.content.contains("falhou"), "{}", tool.content);
    assert!(
        tool.content.lines().any(|line| line == "ok"),
        "{}",
        tool.content
    );
}

#[tokio::test]
async fn the_pipeline_appends_one_tool_message_and_emits_inner_ids() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![calls_event(vec![pipeline_call(
            "p1",
            json!([{"calls": [
                {"tool": "echo", "arguments": {"n": "a"}},
                {"tool": "echo", "arguments": {"n": "b"}}
            ]}]),
        )])],
        vec![text_event("fim")],
    ]));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(FakeTool::new("echo", Behavior::Echo)));

    let app = app(provider, bus, tools);
    app.handle(Command::SendMessage { text: "vai".into() })
        .await
        .unwrap();

    let mut inner_ids = Vec::new();
    loop {
        match rx.recv().await.unwrap() {
            Event::ToolStarted { call_id, .. } if call_id.starts_with("p1#") => {
                inner_ids.push(call_id);
            }
            Event::StreamFinished { .. } | Event::StreamFailed { .. } => break,
            _ => {}
        }
    }

    inner_ids.sort();
    assert_eq!(inner_ids, vec!["p1#0.0", "p1#0.1"]);

    let messages = history(&app).await;
    let tool_messages: Vec<&Message> = messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .collect();
    assert_eq!(tool_messages.len(), 1);
    assert_eq!(tool_messages[0].tool_call_id.as_deref(), Some("p1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_mid_pipeline_closes_the_pending_call() {
    let bus = EventBus::new(64);
    let mut rx = bus.subscribe();
    let provider = Arc::new(ScriptedProvider::new(vec![
        vec![calls_event(vec![pipeline_call(
            "p1",
            json!([{"calls": [{"tool": "slow", "arguments": {}}]}]),
        )])],
        vec![text_event("depois")],
    ]));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(
        FakeTool::new("slow", Behavior::Record).sleeping(300),
    ));

    let app = app(provider, bus, tools);
    let CommandOutcome::Started { request_id } = app
        .handle(Command::SendMessage { text: "vai".into() })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    loop {
        match rx.recv().await.unwrap() {
            Event::ToolStarted { call_id, .. } if call_id.starts_with("p1#") => break,
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

    let messages = history(&app).await;
    let tool_ids: Vec<&str> = messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect();
    assert_eq!(tool_ids, vec!["p1"]);

    app.handle(Command::SendMessage {
        text: "de novo".into(),
    })
    .await
    .unwrap();
    wait_for_end(&mut rx).await;

    let messages = history(&app).await;
    assert_eq!(messages.last().unwrap().content, "depois");
}
