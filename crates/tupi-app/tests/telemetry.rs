use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tupi_app::app::App;
use tupi_app::telemetry::Telemetry;
use tupi_domain::{Event, ToolCall};
use tupi_kernel::{
    ChatRequest, Command, CommandHandler, CommandOutcome, EventBus, Provider, ProviderError,
    ProviderEvent, ProviderRegistry, ProviderStream, Tool, ToolError, ToolRegistry, ToolSpec,
};

struct Scripted {
    steps: Mutex<Vec<Vec<Result<ProviderEvent, ProviderError>>>>,
}

impl Scripted {
    fn new(steps: Vec<Vec<Result<ProviderEvent, ProviderError>>>) -> Self {
        Self {
            steps: Mutex::new(steps),
        }
    }
}

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &str {
        "fake"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(vec!["modelo".to_string()])
    }

    async fn stream_chat(&self, _request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let mut steps = self.steps.lock().unwrap();
        let next = steps.remove(0);
        Ok(Box::pin(futures_util::stream::iter(next)))
    }
}

struct Echo;

impl Tool for Echo {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "echo".to_string(),
            description: "devolve os argumentos".to_string(),
            schema: serde_json::json!({"type": "object"}),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        Ok(format!("eco:{arguments}"))
    }
}

fn usage() -> ProviderEvent {
    ProviderEvent::Usage {
        prompt_tokens: Some(10),
        cached_tokens: Some(2),
        completion_tokens: Some(3),
    }
}

#[tokio::test]
async fn a_turn_records_commands_tools_prompts_and_usage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("telemetry.jsonl");
    let telemetry = Telemetry::open(&path);
    let events = EventBus::new(64);
    let mut receiver = events.subscribe();

    let providers = ProviderRegistry::new();
    providers.register(Arc::new(Scripted::new(vec![
        vec![
            Ok(ProviderEvent::Started),
            Ok(ProviderEvent::Text("chamando".to_string())),
            Ok(ProviderEvent::ToolCalls(vec![ToolCall {
                id: "c1".to_string(),
                name: "echo".to_string(),
                arguments: "{\"value\":\"x\"}".to_string(),
            }])),
            Ok(usage()),
        ],
        vec![
            Ok(ProviderEvent::Started),
            Ok(ProviderEvent::Text("pronto".to_string())),
            Ok(usage()),
        ],
    ])));

    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Echo));

    let app = App::with_tools(
        providers,
        "fake",
        "modelo".to_string(),
        events,
        None,
        tools,
        String::new(),
    )
    .with_telemetry(telemetry.clone());

    let CommandOutcome::Started { .. } = app
        .handle(Command::SendMessage {
            text: "oi".to_string(),
        })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    loop {
        match receiver.recv().await.unwrap() {
            Event::StreamFinished { .. } => break,
            Event::StreamFailed { message, .. } => panic!("stream falhou: {message}"),
            _ => {}
        }
    }

    // A task do turno grava turn_finished logo depois de publicar o fim.
    for _ in 0..100 {
        if std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains("\"turn_finished\"")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    drop(app);
    drop(telemetry);

    let body = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|line| serde_json::from_str(line).expect("linha JSON"))
        .collect();
    let names: Vec<&str> = lines
        .iter()
        .map(|line| line["event"].as_str().expect("event"))
        .collect();

    for expected in [
        "command_started",
        "command_finished",
        "turn_started",
        "prompt_sent",
        "provider_response",
        "tool_started",
        "tool_finished",
        "turn_finished",
    ] {
        assert!(names.contains(&expected), "faltou {expected}: {names:?}");
    }

    let prompt = lines
        .iter()
        .find(|line| line["event"] == "prompt_sent")
        .unwrap();
    assert_eq!(prompt["body_hash"].as_str().unwrap().len(), 12);
    assert!(prompt["body_chars"].as_u64().unwrap() > 0);
    assert!(prompt["estimate_scaled"].as_u64().is_some());
    if std::env::var("TUPI_TELEMETRY_BODY").is_err() {
        assert!(prompt["body"].as_str().unwrap().is_empty());
        assert_eq!(prompt["body_omitted"], true);
    }
    assert!(
        prompt["tool_names"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "echo")
    );

    let tool = lines
        .iter()
        .find(|line| line["event"] == "tool_finished")
        .unwrap();
    assert_eq!(tool["ok"], true);
    assert_eq!(tool["name"], "echo");
    assert!(tool["result"].as_str().unwrap().contains("eco"));
    assert!(tool["duration_us"].as_u64().is_some());

    let turn = lines
        .iter()
        .find(|line| line["event"] == "turn_finished")
        .unwrap();
    assert_eq!(turn["ok"], true);
    assert_eq!(turn["tool_calls"], 1);
    assert_eq!(turn["total_prompt_tokens"], 20);
    assert_eq!(turn["total_completion_tokens"], 6);

    let responses: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["event"] == "provider_response")
        .collect();
    assert_eq!(responses.len(), 2);
    assert!(responses.iter().all(|line| line["ok"] == true));
    assert!(
        responses
            .iter()
            .all(|line| line["estimate_scaled"].as_u64().is_some())
    );
    assert!(
        responses
            .iter()
            .all(|line| line["first_token_us"].as_u64().is_some())
    );

    let seqs: Vec<u64> = lines
        .iter()
        .map(|line| line["seq"].as_u64().expect("seq"))
        .collect();
    assert!(seqs.windows(2).all(|pair| pair[1] == pair[0] + 1));
}

#[tokio::test]
async fn a_provider_failure_is_recorded_with_kind_and_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("telemetry.jsonl");
    let telemetry = Telemetry::open(&path);
    let events = EventBus::new(64);
    let mut receiver = events.subscribe();

    let providers = ProviderRegistry::new();
    providers.register(Arc::new(Scripted::new(vec![vec![Err(
        ProviderError::Http {
            status: 429,
            body: "limite".to_string(),
        },
    )]])));

    let app = App::with_tools(
        providers,
        "fake",
        "modelo".to_string(),
        events,
        None,
        ToolRegistry::new(),
        String::new(),
    )
    .with_telemetry(telemetry.clone());

    let CommandOutcome::Started { .. } = app
        .handle(Command::SendMessage {
            text: "oi".to_string(),
        })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    loop {
        match receiver.recv().await.unwrap() {
            Event::StreamFailed { .. } => break,
            Event::StreamFinished { .. } => panic!("esperava falha"),
            _ => {}
        }
    }

    for _ in 0..100 {
        if std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains("\"turn_finished\"")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    drop(app);
    drop(telemetry);

    let body = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<serde_json::Value> = body
        .lines()
        .map(|line| serde_json::from_str(line).expect("linha JSON"))
        .collect();

    let response = lines
        .iter()
        .find(|line| line["event"] == "provider_response")
        .unwrap();
    assert_eq!(response["ok"], false);
    assert_eq!(response["error_kind"], "http");
    assert_eq!(response["http_status"], 429);
    assert!(response["first_token_us"].is_null());

    let turn = lines
        .iter()
        .find(|line| line["event"] == "turn_finished")
        .unwrap();
    assert_eq!(turn["ok"], false);
    assert!(turn["error"].as_str().unwrap().contains("429"));
}

#[tokio::test]
async fn a_truncated_response_records_the_finish_reason() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("telemetry.jsonl");
    let telemetry = Telemetry::open(&path);
    let events = EventBus::new(64);
    let mut receiver = events.subscribe();

    let providers = ProviderRegistry::new();
    providers.register(Arc::new(Scripted::new(vec![
        vec![
            Ok(ProviderEvent::Text("cortou".to_string())),
            Ok(ProviderEvent::Finished("length".to_string())),
            Ok(usage()),
        ],
        vec![
            Ok(ProviderEvent::Text("pronto".to_string())),
            Ok(ProviderEvent::Finished("stop".to_string())),
            Ok(usage()),
        ],
    ])));

    let app = App::with_tools(
        providers,
        "fake",
        "modelo".to_string(),
        events,
        None,
        ToolRegistry::new(),
        String::new(),
    )
    .with_telemetry(telemetry.clone());

    let CommandOutcome::Started { .. } = app
        .handle(Command::SendMessage {
            text: "oi".to_string(),
        })
        .await
        .unwrap()
    else {
        panic!("esperava Started");
    };

    loop {
        match receiver.recv().await.unwrap() {
            Event::StreamFinished { .. } => break,
            Event::StreamFailed { message, .. } => panic!("stream falhou: {message}"),
            _ => {}
        }
    }

    for _ in 0..100 {
        if std::fs::read_to_string(&path)
            .unwrap_or_default()
            .contains("\"turn_finished\"")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    drop(app);
    drop(telemetry);

    let body = std::fs::read_to_string(&path).unwrap();
    let reasons: Vec<String> = body
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("linha JSON"))
        .filter(|line| line["event"] == "provider_response")
        .map(|line| {
            line["finish_reason"]
                .as_str()
                .unwrap_or("<ausente>")
                .to_string()
        })
        .collect();

    assert_eq!(reasons, vec!["length", "stop"]);
}
