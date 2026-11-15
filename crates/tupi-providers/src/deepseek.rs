use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::unfold;
use tupi_domain::ToolCall;
use tupi_kernel::{
    ChatRequest, Provider, ProviderError, ProviderEvent, ProviderRegistry, ProviderStream,
};

use crate::sse::{SseParser, ToolCallDelta, extract_chunk, is_done};

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STALL_TIMEOUT: Duration = Duration::from_secs(120);

pub struct DeepSeekProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    last_payload: std::sync::Mutex<Option<serde_json::Value>>,
}

impl DeepSeekProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("cliente HTTP válido");

        Self {
            client,
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            last_payload: std::sync::Mutex::new(None),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }
}

#[derive(Default)]
struct CallBuilder {
    id: Option<String>,
    name: String,
    arguments: String,
}

/// Remonta `tool_calls` fragmentados no stream, por índice.
#[derive(Default)]
pub(crate) struct ToolCallAccumulator {
    calls: BTreeMap<usize, CallBuilder>,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn merge(&mut self, delta: ToolCallDelta) {
        let builder = self.calls.entry(delta.index).or_default();
        if let Some(id) = delta.id.filter(|id| !id.is_empty()) {
            builder.id = Some(id);
        }
        if let Some(name) = delta.name {
            builder.name.push_str(&name);
        }
        if let Some(arguments) = delta.arguments {
            builder.arguments.push_str(&arguments);
        }
    }

    /// Devolve as calls completas, em ordem de índice, e esvazia o acumulador.
    pub fn finish(&mut self) -> Option<Vec<ToolCall>> {
        if self.calls.is_empty() {
            return None;
        }

        let calls: Vec<ToolCall> = std::mem::take(&mut self.calls)
            .into_iter()
            .map(|(index, builder)| ToolCall {
                id: builder.id.unwrap_or_else(|| format!("call_{index}")),
                name: builder.name,
                arguments: builder.arguments,
            })
            .collect();
        Some(calls)
    }
}

struct StreamState {
    response: reqwest::Response,
    parser: SseParser,
    pending: VecDeque<String>,
    calls: ToolCallAccumulator,
    /// Último `finish_reason` visto. "length" indica corte no teto de tokens.
    finish_reason: Option<String>,
    started: bool,
    done: bool,
}

#[async_trait]
impl Provider for DeepSeekProvider {
    fn id(&self) -> &str {
        "deepseek"
    }

    fn last_payload(&self) -> Option<serde_json::Value> {
        self.last_payload
            .lock()
            .ok()
            .and_then(|payload| payload.clone())
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));

        let response = self
            .client
            .get(url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Http { status, body });
        }

        let body = response
            .text()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        parse_models(&body)
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let body = build_body(&request);
        if let Ok(mut last) = self.last_payload.lock() {
            *last = Some(body.clone());
        }

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Http { status, body });
        }

        let state = StreamState {
            response,
            parser: SseParser::new(),
            pending: VecDeque::new(),
            calls: ToolCallAccumulator::new(),
            finish_reason: None,
            started: false,
            done: false,
        };

        let stream = unfold(state, |mut state| async move {
            loop {
                if let Some(payload) = state.pending.pop_front() {
                    if is_done(&payload) {
                        state.done = true;
                        continue;
                    }

                    match extract_chunk(&payload) {
                        Ok(chunk) => {
                            for delta in chunk.tool_calls {
                                state.calls.merge(delta);
                            }
                            if chunk.finish_reason.is_some() {
                                state.finish_reason = chunk.finish_reason.clone();
                            }
                            if chunk.prompt_tokens.is_some()
                                || chunk.cached_tokens.is_some()
                                || chunk.usage.is_some()
                            {
                                return Some((
                                    Ok(ProviderEvent::Usage {
                                        prompt_tokens: chunk.prompt_tokens,
                                        cached_tokens: chunk.cached_tokens,
                                        completion_tokens: chunk.usage,
                                    }),
                                    state,
                                ));
                            }
                            if let Some(reasoning) = chunk.reasoning {
                                return Some((Ok(ProviderEvent::Reasoning(reasoning)), state));
                            }
                            if let Some(text) = chunk.text {
                                return Some((Ok(ProviderEvent::Text(text)), state));
                            }
                            continue;
                        }
                        Err(error) => return Some((Err(error), state)),
                    }
                }

                if state.done {
                    if let Some(calls) = state.calls.finish() {
                        return Some((Ok(ProviderEvent::ToolCalls(calls)), state));
                    }
                    if let Some(reason) = state.finish_reason.take() {
                        return Some((Ok(ProviderEvent::Finished(reason)), state));
                    }
                    return None;
                }

                match tokio::time::timeout(STALL_TIMEOUT, state.response.chunk()).await {
                    Err(_) => {
                        return Some((
                            Err(ProviderError::Network(
                                "stream sem dados por 120 segundos".to_string(),
                            )),
                            state,
                        ));
                    }
                    Ok(Err(error)) => {
                        return Some((Err(ProviderError::Network(error.to_string())), state));
                    }
                    Ok(Ok(None)) => state.done = true,
                    Ok(Ok(Some(bytes))) => {
                        state.pending.extend(state.parser.push(&bytes));
                        if !state.started {
                            state.started = true;
                            return Some((Ok(ProviderEvent::Started), state));
                        }
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

pub fn build_body(request: &ChatRequest) -> serde_json::Value {
    // Thinking mode + tools: o provedor exige `reasoning_content` em todo
    // turno do assistente, inclusive quando o modelo não emitiu reasoning
    // (aí o campo vai como string vazia). Sem ele a API responde 400.
    let has_tools = !request.tools.is_empty();
    let messages: Vec<serde_json::Value> = request
        .messages
        .iter()
        .map(|message| {
            let mut value = serde_json::json!({
                "role": message.role.as_str(),
                "content": message.content,
            });

            if !message.tool_calls.is_empty() {
                let calls: Vec<serde_json::Value> = message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments,
                            }
                        })
                    })
                    .collect();
                value["tool_calls"] = serde_json::Value::Array(calls);
            }

            if let Some(call_id) = &message.tool_call_id {
                value["tool_call_id"] = serde_json::Value::String(call_id.clone());
            }

            // Thinking mode + tools: o provedor exige o reasoning de volta,
            // inclusive em turnos em que o modelo não chamou tool.
            if message.role == tupi_domain::Role::Assistant {
                let reasoning = message.reasoning_content.as_deref().unwrap_or("");
                if has_tools || !reasoning.is_empty() {
                    value["reasoning_content"] = serde_json::Value::String(reasoning.to_string());
                }
            }

            value
        })
        .collect();

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });

    if !request.tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tupi_kernel::tools_payload(&request.tools));
    }

    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }

    body
}

pub fn parse_models(body: &str) -> Result<Vec<String>, ProviderError> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| ProviderError::Protocol(format!("JSON inválido em /models: {error}")))?;

    let data = value
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| ProviderError::Protocol("campo 'data' ausente em /models".to_string()))?;

    Ok(data
        .iter()
        .filter_map(|model| {
            model
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        })
        .collect())
}

pub fn register_all(registry: &mut ProviderRegistry, api_key: &str) {
    registry.register(Arc::new(DeepSeekProvider::new(api_key)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::Message;
    use tupi_kernel::{ChatRequest, ProviderRegistry};

    #[test]
    fn body_has_model_messages_and_stream() {
        let request = ChatRequest::new("deepseek-flash", vec![Message::user("oi")]);

        let body = build_body(&request);

        assert_eq!(body["model"], "deepseek-flash");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "oi");
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn register_all_registers_deepseek() {
        let mut registry = ProviderRegistry::new();
        register_all(&mut registry, "sk-test");

        assert_eq!(registry.ids(), vec!["deepseek".to_string()]);
    }

    #[test]
    fn parse_models_reads_ids_and_skips_invalid_entries() {
        let body = r#"{
            "object": "list",
            "data": [
                {"id": "deepseek-flash", "object": "model"},
                {"id": "deepseek-v4-pro", "object": "model"},
                {"object": "model"}
            ]
        }"#;

        let models = parse_models(body).unwrap();

        assert_eq!(
            models,
            vec!["deepseek-flash".to_string(), "deepseek-v4-pro".to_string()]
        );
    }

    #[test]
    fn parse_models_without_data_is_protocol_error() {
        let error = parse_models("{}").unwrap_err();
        assert!(matches!(error, ProviderError::Protocol(_)));
    }

    use crate::sse::ToolCallDelta;
    use tupi_domain::ToolCall;
    use tupi_kernel::ToolSpec;

    fn read_spec() -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: "lê".into(),
            schema: serde_json::json!({"type": "object"}),
        }
    }

    #[test]
    fn body_includes_tools_when_present() {
        let request = ChatRequest::new("deepseek-flash", vec![Message::user("oi")])
            .with_tools(vec![read_spec()]);
        let body = build_body(&request);

        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "read");
    }

    #[test]
    fn body_omits_tools_when_empty() {
        let request = ChatRequest::new("deepseek-flash", vec![Message::user("oi")]);
        let body = build_body(&request);

        assert!(body.get("tools").is_none());
    }

    #[test]
    fn body_carries_the_configured_max_tokens() {
        let request =
            ChatRequest::new("deepseek-flash", vec![Message::user("oi")]).with_max_tokens(1234);
        let body = build_body(&request);

        assert_eq!(body["max_tokens"], 1234);
    }

    #[test]
    fn body_omits_max_tokens_when_unset_or_zero() {
        let unset = build_body(&ChatRequest::new(
            "deepseek-flash",
            vec![Message::user("oi")],
        ));
        assert!(unset.get("max_tokens").is_none());

        let zero = ChatRequest::new("deepseek-flash", vec![Message::user("oi")]).with_max_tokens(0);
        assert!(build_body(&zero).get("max_tokens").is_none());
    }

    #[test]
    fn assistant_tool_calls_are_serialized() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: "{\"path\":\"a.rs\"}".into(),
        };
        let request = ChatRequest::new(
            "deepseek-flash",
            vec![Message::assistant_with_calls("", vec![call])],
        );
        let body = build_body(&request);

        assert_eq!(body["messages"][0]["role"], "assistant");
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "c1");
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["name"],
            "read"
        );
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a.rs\"}"
        );
    }

    #[test]
    fn body_passes_back_reasoning_content_of_assistant_messages() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: "{}".into(),
        };
        let request = ChatRequest::new(
            "deepseek-flash",
            vec![
                Message::assistant_with_calls("", vec![call]).with_reasoning("penso no passo"),
                Message::tool("c1", "resultado"),
            ],
        );
        let body = build_body(&request);

        assert_eq!(body["messages"][0]["reasoning_content"], "penso no passo");
        assert!(body["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn body_omits_reasoning_content_when_absent_or_empty() {
        let request = ChatRequest::new(
            "deepseek-flash",
            vec![
                Message::assistant("sem pensar"),
                Message::assistant("vazio").with_reasoning(""),
            ],
        );
        let body = build_body(&request);

        assert!(body["messages"][0].get("reasoning_content").is_none());
        assert!(body["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn body_fills_empty_reasoning_content_on_assistant_turns_with_tools() {
        let request = ChatRequest::new(
            "deepseek-flash",
            vec![
                Message::user("oi"),
                Message::assistant_with_calls(
                    "",
                    vec![ToolCall {
                        id: "c1".into(),
                        name: "read".into(),
                        arguments: "{}".into(),
                    }],
                ),
                Message::tool("c1", "resultado"),
            ],
        )
        .with_tools(vec![read_spec()]);
        let body = build_body(&request);

        assert_eq!(body["messages"][1]["reasoning_content"], "");
        assert!(body["messages"][0].get("reasoning_content").is_none());
        assert!(body["messages"][2].get("reasoning_content").is_none());
    }

    #[test]
    fn tool_message_serializes_call_id() {
        let request = ChatRequest::new("deepseek-flash", vec![Message::tool("c1", "resultado")]);
        let body = build_body(&request);

        assert_eq!(body["messages"][0]["role"], "tool");
        assert_eq!(body["messages"][0]["tool_call_id"], "c1");
        assert_eq!(body["messages"][0]["content"], "resultado");
    }

    #[test]
    fn accumulator_reassembles_fragmented_arguments() {
        let mut accumulator = ToolCallAccumulator::new();
        accumulator.merge(ToolCallDelta {
            index: 0,
            id: Some("c1".into()),
            name: Some("read".into()),
            arguments: Some("{\"path\":\"a".into()),
        });
        accumulator.merge(ToolCallDelta {
            index: 0,
            id: None,
            name: None,
            arguments: Some(".rs\"}".into()),
        });

        let calls = accumulator.finish().expect("calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "c1");
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].arguments, "{\"path\":\"a.rs\"}");
        assert!(accumulator.finish().is_none());
    }

    #[test]
    fn accumulator_orders_calls_by_index() {
        let mut accumulator = ToolCallAccumulator::new();
        accumulator.merge(ToolCallDelta {
            index: 1,
            id: Some("b".into()),
            name: Some("second".into()),
            arguments: Some("{}".into()),
        });
        accumulator.merge(ToolCallDelta {
            index: 0,
            id: Some("a".into()),
            name: Some("first".into()),
            arguments: Some("{}".into()),
        });

        let calls = accumulator.finish().unwrap();
        assert_eq!(calls[0].name, "first");
        assert_eq!(calls[1].name, "second");
    }

    #[tokio::test]
    async fn stream_starts_with_started_before_the_first_text() {
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let sse = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ola\"}}]}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let provider = DeepSeekProvider::new("sk-test").with_base_url(format!("http://{addr}"));
        let mut stream = provider
            .stream_chat(ChatRequest::new(
                "deepseek-flash",
                vec![Message::user("oi")],
            ))
            .await
            .unwrap();

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first, ProviderEvent::Started);
        let second = stream.next().await.unwrap().unwrap();
        assert_eq!(second, ProviderEvent::Text("ola".to_string()));
    }

    #[tokio::test]
    async fn stream_reports_reasoning_content_before_the_text() {
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let sse = concat!(
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"penso\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"ola\"}}]}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let provider = DeepSeekProvider::new("sk-test").with_base_url(format!("http://{addr}"));
        let mut stream = provider
            .stream_chat(ChatRequest::new(
                "deepseek-flash",
                vec![Message::user("oi")],
            ))
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.unwrap());
        }

        assert_eq!(events[0], ProviderEvent::Started);
        assert_eq!(events[1], ProviderEvent::Reasoning("penso".to_string()));
        assert_eq!(events[2], ProviderEvent::Text("ola".to_string()));
    }

    #[test]
    fn accumulator_generates_an_id_when_the_provider_omits_it() {
        let mut accumulator = ToolCallAccumulator::new();
        accumulator.merge(ToolCallDelta {
            index: 3,
            id: None,
            name: Some("read".into()),
            arguments: Some("{}".into()),
        });

        let calls = accumulator.finish().unwrap();
        assert_eq!(calls[0].id, "call_3");
    }

    #[tokio::test]
    async fn stream_reports_the_finish_reason_after_tool_calls() {
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let sse = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"cortando\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\"\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let provider = DeepSeekProvider::new("sk-test").with_base_url(format!("http://{addr}"));
        let mut stream = provider
            .stream_chat(ChatRequest::new(
                "deepseek-flash",
                vec![Message::user("oi")],
            ))
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.unwrap());
        }

        assert_eq!(events[0], ProviderEvent::Started);
        assert!(events.contains(&ProviderEvent::Text("cortando".to_string())));
        assert!(events.contains(&ProviderEvent::ToolCalls(vec![ToolCall {
            id: "c1".to_string(),
            name: "read".to_string(),
            arguments: "{\"path\"".to_string(),
        }])));
        assert_eq!(
            events.last(),
            Some(&ProviderEvent::Finished("length".to_string())),
            "events: {events:?}"
        );
    }
}
