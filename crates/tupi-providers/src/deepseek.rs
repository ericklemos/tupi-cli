use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::unfold;
use tupi_kernel::{
    ChatRequest, Provider, ProviderError, ProviderEvent, ProviderRegistry, ProviderStream,
};

use crate::sse::{SseParser, extract_delta, is_done};

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

struct StreamState {
    response: reqwest::Response,
    parser: SseParser,
    pending: VecDeque<String>,
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
            done: false,
        };

        let stream = unfold(state, |mut state| async move {
            loop {
                if let Some(payload) = state.pending.pop_front() {
                    if is_done(&payload) {
                        return None;
                    }

                    match extract_delta(&payload) {
                        Ok(Some(text)) => return Some((Ok(ProviderEvent::Text(text)), state)),
                        Ok(None) => continue,
                        Err(error) => return Some((Err(error), state)),
                    }
                }

                if state.done {
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
            serde_json::json!({
                "role": message.role.as_str(),
                "content": message.content,
            })
        })
        .collect();

    serde_json::json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
    })
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
    use tupi_domain::{Message, Role};
    use tupi_kernel::{ChatRequest, ProviderRegistry};

    #[test]
    fn body_has_model_messages_and_stream() {
        let request = ChatRequest {
            model: "deepseek-flash".into(),
            messages: vec![Message {
                role: Role::User,
                content: "oi".into(),
            }],
        };

        let body = build_body(&request);

        assert_eq!(body["model"], "deepseek-flash");
        assert_eq!(body["stream"], true);
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
}
