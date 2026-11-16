use std::sync::{Arc, Mutex};

use serde_json::Value;
use tupi_app::docs::DOCS_AGENT_PREFIX;
use tupi_domain::Event;
use tupi_kernel::{
    AgentMode, ChatRequest, Command, CommandHandler, EventBus, ModeMask, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream, Tool, ToolError, ToolRegistry, ToolSpec,
};

struct CaptureProvider {
    requests: Mutex<Vec<ChatRequest>>,
}

#[async_trait::async_trait]
impl Provider for CaptureProvider {
    fn id(&self) -> &str {
        "capture"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["m".into()])
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        self.requests.lock().unwrap().push(request);
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(ProviderEvent::Started),
            Ok(ProviderEvent::Text("ok".into())),
            Ok(ProviderEvent::Finished("stop".into())),
        ])))
    }
}

struct DocsTool;

impl Tool for DocsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "docs_stub".into(),
            description: String::new(),
            schema: serde_json::json!({}),
        }
    }
    fn call(&self, _: Value) -> Result<String, ToolError> {
        Ok(String::new())
    }
    fn mode_mask(&self) -> ModeMask {
        ModeMask::DOCS
    }
}

struct ExecTool;

impl Tool for ExecTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "exec_stub".into(),
            description: String::new(),
            schema: serde_json::json!({}),
        }
    }
    fn call(&self, _: Value) -> Result<String, ToolError> {
        Ok(String::new())
    }
}

fn registry(provider: Arc<dyn Provider>) -> ProviderRegistry {
    let registry = ProviderRegistry::new();
    registry.register(provider);
    registry
}

#[tokio::test]
async fn docs_mode_sends_the_docs_prefix_and_only_docs_tools() {
    let provider = Arc::new(CaptureProvider {
        requests: Mutex::new(Vec::new()),
    });
    let bus = EventBus::new(16);
    let mut rx = bus.subscribe();

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(DocsTool));
    tools.register(Arc::new(ExecTool));

    let app = tupi_app::App::with_tools(
        registry(provider.clone()),
        "capture",
        "m".into(),
        bus,
        None,
        tools,
        "BASE".into(),
    )
    .with_mode_cell(Arc::new(std::sync::atomic::AtomicU8::new(
        AgentMode::Docs.as_u8(),
    )));

    app.handle(Command::SendMessage {
        text: "investigue".into(),
    })
    .await
    .unwrap();

    loop {
        match rx.recv().await.unwrap() {
            Event::StreamFinished { .. } => break,
            Event::StreamFailed { message, .. } => panic!("turno falhou: {message}"),
            _ => {}
        }
    }

    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].messages[0].content.contains("BASE"));
    assert!(requests[0].messages[0].content.contains(DOCS_AGENT_PREFIX));
    let names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["docs_stub"]);
}
