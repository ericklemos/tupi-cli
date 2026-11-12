use crate::sse::SseParser;
use futures_util::stream::unfold;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::Mutex;
use tupi_domain::{Role, ToolCall};
use tupi_kernel::{ChatRequest, Provider, ProviderError, ProviderEvent, ProviderRegistry, ProviderStream};

pub const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
pub const ANTIGRAVITY_CLIENT_SECRET: &str = "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";
pub const ANTIGRAVITY_REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
pub const ANTIGRAVITY_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
pub const ANTIGRAVITY_DEFAULT_PROJECT_ID: &str = "rising-fact-p41fc";
pub const ANTIGRAVITY_USER_AGENT: &str =
    "antigravity/hub/2.24.0 (aidev_client; os_type=linux; arch=x86_64; cl=1000335682)";

pub const SUPPORTED_MODELS: &[&str] = &[
    "antigravity-gemini-3.8-flash",
    "antigravity-gemini-3-flash",
    "antigravity-gemini-3-pro",
    "antigravity-gemini-3.1-pro",
    "antigravity-claude-sonnet-4-6",
    "antigravity-claude-opus-4-6-thinking",
    "gemini-2.5-flash",
    "gemini-2.5-pro",
];

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_request_id() -> String {
    let count = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seed = format!("{now}:{count}");
    let hash = blake3::hash(seed.as_bytes());
    format!("agent-{}", hash.to_hex())
}

pub fn build_body(request: &ChatRequest, project_id: &str) -> Value {
    let mut system_instruction = None;
    let mut contents = Vec::new();
    let mut call_names_by_id: HashMap<String, String> = HashMap::new();

    // Map call IDs to names from assistant tool calls for tool responses
    for msg in &request.messages {
        if msg.role == Role::Assistant {
            for call in &msg.tool_calls {
                call_names_by_id.insert(call.id.clone(), call.name.clone());
            }
        }
    }

    for msg in &request.messages {
        match msg.role {
            Role::System => {
                system_instruction = Some(json!({
                    "role": "user",
                    "parts": [{ "text": msg.content }]
                }));
            }
            Role::User => {
                contents.push(json!({
                    "role": "user",
                    "parts": [{ "text": msg.content }]
                }));
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                if !msg.content.is_empty() {
                    parts.push(json!({ "text": msg.content }));
                }
                for call in &msg.tool_calls {
                    let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                    parts.push(json!({
                        "functionCall": {
                            "id": call.id,
                            "name": call.name,
                            "args": args
                        }
                    }));
                }
                contents.push(json!({
                    "role": "model",
                    "parts": parts
                }));
            }
            Role::Tool => {
                let call_id = msg.tool_call_id.clone().unwrap_or_default();
                let name = call_names_by_id
                    .get(&call_id)
                    .cloned()
                    .unwrap_or_else(|| call_id.clone());
                contents.push(json!({
                    "role": "function",
                    "parts": [{
                        "functionResponse": {
                            "id": call_id,
                            "name": name,
                            "response": {
                                "output": msg.content
                            }
                        }
                    }]
                }));
            }
        }
    }

    let mut request_payload = json!({
        "contents": contents,
    });

    if let Some(sys) = system_instruction {
        request_payload["systemInstruction"] = sys;
    }

    if !request.tools.is_empty() {
        let decls: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.schema
                })
            })
            .collect();

        request_payload["tools"] = json!([{
            "functionDeclarations": decls
        }]);
    }

    json!({
        "project": project_id,
        "model": request.model,
        "request": request_payload,
        "requestType": "agent",
        "userAgent": "antigravity",
        "requestId": generate_request_id(),
    })
}

#[derive(Debug, Default, PartialEq)]
pub struct AntigravityChunk {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

pub fn extract_chunk(payload: &str) -> Result<AntigravityChunk, ProviderError> {
    let root: Value = serde_json::from_str(payload)
        .map_err(|e| ProviderError::Protocol(format!("json invalido: {e}")))?;
    let response = root.get("response").unwrap_or(&root);

    let mut chunk = AntigravityChunk::default();
    let mut text_parts = Vec::new();

    if let Some(candidates) = response.get("candidates").and_then(Value::as_array) {
        for candidate in candidates {
            if let Some(parts) = candidate
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(Value::as_array)
            {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        text_parts.push(text.to_string());
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        let id = fc
                            .get("id")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("call_{}", chunk.tool_calls.len()));
                        let arguments = fc
                            .get("args")
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "{}".to_string());

                        chunk.tool_calls.push(ToolCall {
                            id,
                            name,
                            arguments,
                        });
                    }
                }
            }
        }
    }

    if !text_parts.is_empty() {
        chunk.text = Some(text_parts.join(""));
    }

    if let Some(usage) = response.get("usageMetadata") {
        chunk.prompt_tokens = usage.get("promptTokenCount").and_then(Value::as_u64);
        chunk.completion_tokens = usage.get("candidatesTokenCount").and_then(Value::as_u64);
    }

    Ok(chunk)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AntigravityCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: u64,
    pub project_id: String,
    pub email: Option<String>,
}

impl AntigravityCredentials {
    pub fn is_expired(&self, now_secs: u64) -> bool {
        now_secs + 60 >= self.expires_at
    }

    pub fn from_json_str(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn from_opencode_json_str(text: &str) -> Option<Self> {
        let val: Value = serde_json::from_str(text).ok()?;
        let accounts = val.get("accounts")?.as_array()?;
        let active_idx = val.get("activeIndex").and_then(Value::as_u64).unwrap_or(0) as usize;
        let account = accounts.get(active_idx).or_else(|| accounts.first())?;

        let refresh_token = account.get("refreshToken")?.as_str()?.to_string();
        let project_id = account
            .get("projectId")
            .and_then(Value::as_str)
            .unwrap_or(ANTIGRAVITY_DEFAULT_PROJECT_ID)
            .to_string();
        let email = account
            .get("email")
            .and_then(Value::as_str)
            .map(|s| s.to_string());

        Some(Self {
            access_token: String::new(),
            refresh_token,
            expires_at: 0,
            project_id,
            email,
        })
    }
}

pub struct AntigravityProvider {
    credentials: Arc<Mutex<AntigravityCredentials>>,
    client: reqwest::Client,
    base_url: String,
}

impl AntigravityProvider {
    pub fn new(credentials: AntigravityCredentials) -> Self {
        let client = reqwest::Client::builder()
            .build()
            .expect("cliente HTTP valido");

        Self {
            credentials: Arc::new(Mutex::new(credentials)),
            client,
            base_url: ANTIGRAVITY_ENDPOINT.to_string(),
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
    accumulated_calls: Vec<ToolCall>,
}

#[async_trait::async_trait]
impl Provider for AntigravityProvider {
    fn id(&self) -> &str {
        "antigravity"
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect())
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let creds = self.credentials.lock().await.clone();
        let project_id = if creds.project_id.is_empty() {
            ANTIGRAVITY_DEFAULT_PROJECT_ID
        } else {
            &creds.project_id
        };

        let body = build_body(&request, project_id);
        let url = format!("{}/v1internal:streamGenerateContent?alt=sse", self.base_url.trim_end_matches('/'));

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", creds.access_token))
                .map_err(|e| ProviderError::Network(e.to_string()))?,
        );
        headers.insert(USER_AGENT, HeaderValue::from_static(ANTIGRAVITY_USER_AGENT));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        let response = self
            .client
            .post(url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Http { status, body });
        }

        let state = StreamState {
            response,
            parser: SseParser::new(),
            pending: VecDeque::new(),
            accumulated_calls: Vec::new(),
        };

        let stream = unfold(state, |mut state| async move {
            loop {
                if let Some(payload) = state.pending.pop_front() {
                    match extract_chunk(&payload) {
                        Ok(chunk) => {
                            if !chunk.tool_calls.is_empty() {
                                state.accumulated_calls.extend(chunk.tool_calls);
                            }
                            if chunk.prompt_tokens.is_some() || chunk.completion_tokens.is_some() {
                                return Some((
                                    Ok(ProviderEvent::Usage {
                                        prompt_tokens: chunk.prompt_tokens,
                                        cached_tokens: None,
                                        completion_tokens: chunk.completion_tokens,
                                    }),
                                    state,
                                ));
                            }
                            if let Some(text) = chunk.text {
                                return Some((Ok(ProviderEvent::Text(text)), state));
                            }
                        }
                        Err(err) => return Some((Err(err), state)),
                    }
                }

                match state.response.chunk().await {
                    Ok(Some(bytes)) => {
                        for event in state.parser.push(&bytes) {
                            state.pending.push_back(event);
                        }
                    }
                    Ok(None) => {
                        if !state.accumulated_calls.is_empty() {
                            let calls = std::mem::take(&mut state.accumulated_calls);
                            return Some((Ok(ProviderEvent::ToolCalls(calls)), state));
                        }
                        return None;
                    }
                    Err(err) => {
                        return Some((Err(ProviderError::Network(err.to_string())), state));
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

pub fn register_all(registry: &mut ProviderRegistry, credentials: AntigravityCredentials) {
    registry.register(Arc::new(AntigravityProvider::new(credentials)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::{Message, Role, ToolCall};
    use tupi_kernel::ToolSpec;

    #[test]
    fn body_wraps_in_antigravity_envelope() {
        let request = ChatRequest::new("antigravity-gemini-3.8-flash", vec![Message::user("ola")]);
        let body = build_body(&request, "rising-fact-p41fc");

        assert_eq!(body["project"], "rising-fact-p41fc");
        assert_eq!(body["model"], "antigravity-gemini-3.8-flash");
        assert_eq!(body["requestType"], "agent");
        assert_eq!(body["userAgent"], "antigravity");
        assert!(body["requestId"].as_str().unwrap().starts_with("agent-"));
        assert_eq!(body["request"]["contents"][0]["role"], "user");
        assert_eq!(body["request"]["contents"][0]["parts"][0]["text"], "ola");
    }

    #[test]
    fn body_serializes_system_instruction() {
        let request = ChatRequest::new(
            "antigravity-gemini-3.8-flash",
            vec![
                Message::new(Role::System, "voce e um assistente"),
                Message::user("oi"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        assert_eq!(
            body["request"]["systemInstruction"]["role"],
            "user"
        );
        assert_eq!(
            body["request"]["systemInstruction"]["parts"][0]["text"],
            "voce e um assistente"
        );
        assert_eq!(body["request"]["contents"].as_array().unwrap().len(), 1);
        assert_eq!(body["request"]["contents"][0]["role"], "user");
    }

    #[test]
    fn body_serializes_tools_as_function_declarations() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "le um arquivo".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        };
        let request = ChatRequest::new("antigravity-gemini-3.8-flash", vec![Message::user("leia")])
            .with_tools(vec![spec]);
        let body = build_body(&request, "rising-fact-p41fc");

        let decls = &body["request"]["tools"][0]["functionDeclarations"];
        assert_eq!(decls[0]["name"], "read_file");
        assert_eq!(decls[0]["description"], "le um arquivo");
        assert_eq!(decls[0]["parameters"]["type"], "object");
        assert_eq!(decls[0]["parameters"]["properties"]["path"]["type"], "string");
    }

    #[test]
    fn body_serializes_assistant_tool_calls_and_responses() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into(),
        };
        let request = ChatRequest::new(
            "antigravity-gemini-3.8-flash",
            vec![
                Message::user("leia o arquivo"),
                Message::assistant_with_calls("", vec![call]),
                Message::tool("call_1", "conteudo do arquivo"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);

        // Assistant model call
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["id"], "call_1");
        assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "read_file");
        assert_eq!(
            contents[1]["parts"][0]["functionCall"]["args"]["path"],
            "a.txt"
        );

        // Tool function response
        assert_eq!(contents[2]["role"], "function");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["id"], "call_1");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["output"],
            "conteudo do arquivo"
        );
    }

    #[test]
    fn chunk_extracts_text_delta() {
        let payload = r#"{"response":{"candidates":[{"content":{"parts":[{"text":"Ola mundo"}]}}]}}"#;
        let chunk = extract_chunk(payload).unwrap();
        assert_eq!(chunk.text, Some("Ola mundo".into()));
        assert!(chunk.tool_calls.is_empty());
    }

    #[test]
    fn chunk_extracts_function_calls() {
        let payload = r#"{"response":{"candidates":[{"content":{"parts":[{"functionCall":{"id":"c1","name":"read_file","args":{"path":"a.rs"}}}]}}]}}"#;
        let chunk = extract_chunk(payload).unwrap();
        assert_eq!(chunk.tool_calls.len(), 1);
        assert_eq!(chunk.tool_calls[0].id, "c1");
        assert_eq!(chunk.tool_calls[0].name, "read_file");
        assert_eq!(chunk.tool_calls[0].arguments, r#"{"path":"a.rs"}"#);
    }

    #[test]
    fn chunk_extracts_usage_metadata() {
        let payload = r#"{"response":{"candidates":[{"content":{"parts":[{"text":"ok"}]}}],"usageMetadata":{"promptTokenCount":120,"candidatesTokenCount":45}}}"#;
        let chunk = extract_chunk(payload).unwrap();
        assert_eq!(chunk.prompt_tokens, Some(120));
        assert_eq!(chunk.completion_tokens, Some(45));
    }

    #[test]
    fn loads_tupi_credentials_format() {
        let json = r#"{
            "access_token": "ya29.test",
            "refresh_token": "1//test_ref",
            "expires_at": 1700000000,
            "project_id": "p-123",
            "email": "user@example.com"
        }"#;

        let creds = AntigravityCredentials::from_json_str(json).unwrap();
        assert_eq!(creds.access_token, "ya29.test");
        assert_eq!(creds.refresh_token, "1//test_ref");
        assert_eq!(creds.expires_at, 1700000000);
        assert_eq!(creds.project_id, "p-123");
        assert_eq!(creds.email, Some("user@example.com".into()));
    }

    #[test]
    fn falls_back_to_opencode_accounts_format() {
        let json = r#"{
            "version": 4,
            "accounts": [
                {
                    "refreshToken": "1//opencode_refresh",
                    "projectId": "opencode-proj",
                    "email": "dev@gmail.com"
                }
            ],
            "activeIndex": 0
        }"#;

        let creds = AntigravityCredentials::from_opencode_json_str(json).unwrap();
        assert_eq!(creds.refresh_token, "1//opencode_refresh");
        assert_eq!(creds.project_id, "opencode-proj");
        assert_eq!(creds.email, Some("dev@gmail.com".into()));
        assert_eq!(creds.expires_at, 0); // Forces refresh
    }

    #[test]
    fn is_expired_detects_expiration_within_window() {
        let creds = AntigravityCredentials {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires_at: 1000,
            project_id: "p".into(),
            email: None,
        };

        // 100 seconds before: not expired (buffer is 60s)
        assert!(!creds.is_expired(900));
        // 55 seconds before: expired
        assert!(creds.is_expired(945));
        // after: expired
        assert!(creds.is_expired(1050));
    }

    #[tokio::test]
    async fn provider_id_and_models() {
        let creds = AntigravityCredentials {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires_at: 1000,
            project_id: "p".into(),
            email: None,
        };
        let provider = AntigravityProvider::new(creds.clone());
        assert_eq!(provider.id(), "antigravity");

        let models = provider.list_models().await.unwrap();
        assert!(models.contains(&"antigravity-gemini-3.8-flash".to_string()));

        let mut registry = ProviderRegistry::new();
        register_all(&mut registry, creds);
        assert_eq!(registry.ids(), vec!["antigravity".to_string()]);
    }
}
