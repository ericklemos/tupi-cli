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
    done: bool,
}

#[async_trait]
impl Provider for DeepSeekProvider {
    fn id(&self) -> &str {
        "deepseek"
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

        let response = self
            .client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&build_body(&request))
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
            done: false,
        };

        let stream = unfold(state, |mut state| async move {
            loop {
                if let Some(payload) = state.pending.pop_front() {
                    if is_done(&payload) {
                        if let Some(calls) = state.calls.finish() {
                            return Some((Ok(ProviderEvent::ToolCalls(calls)), state));
                        }
                        return None;
                    }

                    match extract_chunk(&payload) {
                        Ok(chunk) => {
                            for delta in chunk.tool_calls {
                                state.calls.merge(delta);
                            }
                            if let Some(completion_tokens) = chunk.usage {
                                return Some((
                                    Ok(ProviderEvent::Usage { completion_tokens }),
                                    state,
                                ));
                            }
                            if let Some(text) = chunk.text {
                                return Some((Ok(ProviderEvent::Text(text)), state));
                            }
                            if chunk.finish_reason.is_some()
                                && let Some(calls) = state.calls.finish()
                            {
                                return Some((Ok(ProviderEvent::ToolCalls(calls)), state));
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
                    Ok(Ok(Some(bytes))) => state.pending.extend(state.parser.push(&bytes)),
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

pub fn build_body(request: &ChatRequest) -> serde_json::Value {
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
        let tools: Vec<serde_json::Value> = request
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.schema,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::Value::Array(tools);
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
}
