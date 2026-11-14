use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use futures_util::stream::unfold;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::sse::SseParser;
use tupi_domain::{Role, ToolCall};
use tupi_kernel::{
    ChatRequest, Provider, ProviderError, ProviderEvent, ProviderRegistry, ProviderStream,
};

const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const ANTIGRAVITY_CLIENT_SECRET: &str = "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";
const ANTIGRAVITY_REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";
const ANTIGRAVITY_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
const ANTIGRAVITY_DEFAULT_PROJECT_ID: &str = "rising-fact-p41fc";
const ANTIGRAVITY_USER_AGENT: &str =
    "antigravity/hub/2.24.0 (aidev_client; os_type=darwin; arch=arm64; cl=1000335682)";
const ANTIGRAVITY_ONBOARD_USER_AGENT: &str = "antigravity/hub/2.24.0 (aidev_client; os_type=darwin; arch=arm64; cl=1000335682) google-api-nodejs-client/10.3.0";
const ANTIGRAVITY_GOOG_API_CLIENT: &str = "gl-node/22.21.1";
const SKIP_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

pub const DEFAULT_MODEL: &str = "gemini-3.8-flash-high";

const SUPPORTED_MODELS: &[&str] = &[
    "gemini-4-hydrogen-xhigh",
    "gemini-4-hydrogen-high",
    "gemini-4-hydrogen-medium",
    "gemini-4-hydrogen-low",
    "gemini-4-argon-high",
    "gemini-4-argon-medium",
    "gemini-4-argon-low",
    "gemini-3.8-flash-high",
    "gemini-3.8-flash-medium",
    "gemini-3.8-flash-low",
    "gemini-3.5-flash-lite",
    "gemini-3-flash",
    "gemini-3.1-pro-high",
    "gemini-3.1-pro-low",
    "gemini-2.5-pro",
    "gemini-2.5-flash",
    "claude-sonnet-5-5-high",
    "claude-sonnet-5-5-medium",
    "claude-sonnet-5-5-low",
    "claude-opus-5-5-high",
    "claude-opus-5-5-medium",
    "claude-opus-5-5-low",
    "gpt-oss-120b-medium",
];

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

pub type RefreshHook = Arc<dyn Fn(&AntigravityCredentials) + Send + Sync>;

fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

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

/// Resolve um nome de modelo (com ou sem prefixo 'antigravity-') para o ID
/// aceito pelo endpoint 'v1internal:streamGenerateContent'.
pub fn resolve_api_model(model: &str) -> String {
    let trimmed = model.trim();
    let without_quota = if trimmed.len() >= 12 && trimmed[..12].eq_ignore_ascii_case("antigravity-")
    {
        &trimmed[12..]
    } else {
        trimmed
    };

    match without_quota {
        "gemini-3.8-flash" => "gemini-3.8-flash-high".to_string(),
        "gemini-4-hydrogen" => "gemini-4-hydrogen-high".to_string(),
        "gemini-4-argon" => "gemini-4-argon-high".to_string(),
        "gemini-3-pro" => "gemini-3-pro-low".to_string(),
        "gemini-3.1-pro" => "gemini-3.1-pro-low".to_string(),
        "gemini-3-flash-low" | "gemini-3-flash-medium" | "gemini-3-flash-high" => {
            "gemini-3-flash".to_string()
        }
        other => other.to_string(),
    }
}

fn clean_gemini_schema(schema: &Value) -> Value {
    const UNSUPPORTED: &[&str] = &[
        "$schema",
        "$defs",
        "definitions",
        "$ref",
        "$id",
        "$comment",
        "additionalProperties",
        "propertyNames",
        "title",
        "const",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "pattern",
        "minItems",
        "maxItems",
        "format",
        "default",
        "examples",
    ];

    match schema {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let prop_names: BTreeSet<String> = map
                .get("properties")
                .and_then(Value::as_object)
                .map(|props| props.keys().cloned().collect())
                .unwrap_or_default();

            for (key, value) in map {
                if UNSUPPORTED.contains(&key.as_str()) {
                    continue;
                }
                match key.as_str() {
                    "properties" => {
                        if let Some(props) = value.as_object() {
                            let mut cleaned_props = serde_json::Map::new();
                            for (prop_key, prop_val) in props {
                                cleaned_props
                                    .insert(prop_key.clone(), clean_gemini_schema(prop_val));
                            }
                            out.insert("properties".to_string(), Value::Object(cleaned_props));
                        }
                    }
                    "items" => {
                        out.insert("items".to_string(), clean_gemini_schema(value));
                    }
                    "anyOf" | "oneOf" | "allOf" => {
                        if let Some(arr) = value.as_array() {
                            out.insert(
                                key.clone(),
                                Value::Array(arr.iter().map(clean_gemini_schema).collect()),
                            );
                        }
                    }
                    "required" if !prop_names.is_empty() => {
                        if let Some(arr) = value.as_array() {
                            let valid: Vec<Value> = arr
                                .iter()
                                .filter(|v| v.as_str().is_some_and(|s| prop_names.contains(s)))
                                .cloned()
                                .collect();
                            if !valid.is_empty() {
                                out.insert("required".to_string(), Value::Array(valid));
                            }
                        }
                    }
                    _ => {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }

            if out.get("type").and_then(Value::as_str) == Some("array")
                && !out.contains_key("items")
            {
                out.insert("items".to_string(), json!({ "type": "string" }));
            }

            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(clean_gemini_schema).collect()),
        other => other.clone(),
    }
}

pub fn build_body(request: &ChatRequest, project_id: &str) -> Value {
    let mut system_parts: Vec<Value> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
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
                if !msg.content.is_empty() {
                    system_parts.push(json!({ "text": msg.content }));
                }
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
                for (idx, call) in msg.tool_calls.iter().enumerate() {
                    let args: Value =
                        serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                    let mut part = json!({
                        "functionCall": {
                            "id": call.id,
                            "name": call.name,
                            "args": args
                        }
                    });
                    if idx == 0 {
                        part["thoughtSignature"] =
                            Value::String(SKIP_THOUGHT_SIGNATURE.to_string());
                    }
                    parts.push(part);
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
                let part = json!({
                    "functionResponse": {
                        "id": call_id,
                        "name": name,
                        "response": {
                            "output": msg.content
                        }
                    }
                });
                let append_to_last_tool_turn = contents
                    .last()
                    .and_then(|c| c.get("parts"))
                    .and_then(Value::as_array)
                    .is_some_and(|parts| parts.iter().any(|p| p.get("functionResponse").is_some()));

                if append_to_last_tool_turn
                    && let Some(last) = contents.last_mut()
                    && let Some(parts) = last.get_mut("parts").and_then(Value::as_array_mut)
                {
                    parts.push(part);
                } else {
                    contents.push(json!({
                        "role": "user",
                        "parts": [part]
                    }));
                }
            }
        }
    }

    let mut request_payload = json!({
        "contents": contents,
    });

    if !system_parts.is_empty() {
        request_payload["systemInstruction"] = json!({
            "role": "user",
            "parts": system_parts
        });
    }

    if !request.tools.is_empty() {
        let decls: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": clean_gemini_schema(&tool.schema)
                })
            })
            .collect();

        request_payload["tools"] = json!([{
            "functionDeclarations": decls
        }]);
    }

    json!({
        "project": project_id,
        "model": resolve_api_model(&request.model),
        "request": request_payload,
        "requestType": "agent",
        "userAgent": "antigravity",
        "requestId": generate_request_id(),
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AntigravityChunk {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
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
                    let is_thought = part.get("thought").and_then(Value::as_bool) == Some(true);
                    if !is_thought {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            text_parts.push(text.to_string());
                        }
                    }
                    if let Some(fc) = part.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
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
        chunk.cached_tokens = usage.get("cachedContentTokenCount").and_then(Value::as_u64);
        chunk.completion_tokens = usage.get("candidatesTokenCount").and_then(Value::as_u64);
    }

    Ok(chunk)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            .get("managedProjectId")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                account
                    .get("projectId")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
            })
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

    pub fn parse_token_response(
        payload: &str,
        now_secs: u64,
    ) -> Result<(String, u64), ProviderError> {
        let val: Value = serde_json::from_str(payload)
            .map_err(|e| ProviderError::Protocol(format!("resposta de token invalida: {e}")))?;

        if let Some(error) = val.get("error").and_then(Value::as_str) {
            let desc = val
                .get("error_description")
                .and_then(Value::as_str)
                .unwrap_or(error);
            return Err(ProviderError::Protocol(format!(
                "erro de autenticacao: {desc}"
            )));
        }

        let access_token = val
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Protocol("access_token ausente".to_string()))?
            .to_string();

        let expires_in = val
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(3600);

        Ok((access_token, now_secs + expires_in))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, content)
    }

    pub fn load_from_dirs(tupi_dir: &Path, opencode_dir: Option<&Path>) -> Option<Self> {
        let tupi_path = tupi_dir.join("antigravity.json");
        if let Ok(content) = std::fs::read_to_string(&tupi_path) {
            if let Some(creds) = Self::from_json_str(&content) {
                return Some(creds);
            }
        }

        if let Some(op_dir) = opencode_dir {
            let op_path = op_dir.join("antigravity-accounts.json");
            if let Ok(content) = std::fs::read_to_string(&op_path) {
                if let Some(creds) = Self::from_opencode_json_str(&content) {
                    return Some(creds);
                }
            }
        }

        None
    }

    pub async fn refresh_if_expired(
        &mut self,
        client: &reqwest::Client,
        now_secs: u64,
    ) -> Result<(), ProviderError> {
        if !self.is_expired(now_secs) && !self.access_token.is_empty() {
            return Ok(());
        }

        let body = format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
            url_encode(&self.refresh_token),
            url_encode(ANTIGRAVITY_CLIENT_ID),
            url_encode(ANTIGRAVITY_CLIENT_SECRET)
        );

        let response = client
            .post("https://oauth2.googleapis.com/token")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(ProviderError::Http {
                status: status.as_u16(),
                body,
            });
        }

        let (new_access, new_expires_at) = Self::parse_token_response(&body, now_secs)?;
        self.access_token = new_access;
        self.expires_at = new_expires_at;

        Ok(())
    }
}

pub struct AntigravityProvider {
    credentials: Arc<Mutex<AntigravityCredentials>>,
    client: reqwest::Client,
    base_url: String,
    save_path: Option<std::path::PathBuf>,
    on_refresh: Option<RefreshHook>,
    last_payload: std::sync::Mutex<Option<Value>>,
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
            save_path: None,
            on_refresh: None,
            last_payload: std::sync::Mutex::new(None),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_save_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.save_path = Some(path.into());
        self
    }

    pub fn with_on_refresh(
        mut self,
        hook: impl Fn(&AntigravityCredentials) + Send + Sync + 'static,
    ) -> Self {
        self.on_refresh = Some(Arc::new(hook));
        self
    }

    fn persist_credentials(&self, creds: &AntigravityCredentials) {
        if let Some(path) = &self.save_path {
            let _ = creds.save(path);
        }
        if let Some(hook) = &self.on_refresh {
            hook(creds);
        }
    }

    async fn ensure_auth(&self) -> Result<(String, String), ProviderError> {
        let now_secs = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut creds = self.credentials.lock().await;
        let mut changed = false;

        if creds.is_expired(now_secs) || creds.access_token.is_empty() {
            creds.refresh_if_expired(&self.client, now_secs).await?;
            changed = true;
        }

        if creds.project_id.trim().is_empty() {
            let resolved =
                resolve_project_id_at(&self.client, &self.base_url, &creds.access_token).await;
            if !resolved.is_empty() {
                creds.project_id = resolved;
                changed = true;
            }
        }

        if changed {
            self.persist_credentials(&creds);
        }

        Ok((creds.access_token.clone(), creds.project_id.clone()))
    }
}

struct StreamState {
    response: reqwest::Response,
    parser: SseParser,
    pending: VecDeque<String>,
    ready_events: VecDeque<ProviderEvent>,
    accumulated_calls: Vec<ToolCall>,
    last_usage: Option<(Option<u64>, Option<u64>, Option<u64>)>,
    done: bool,
}

fn is_selectable_chat_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    !lower.contains("image")
        && !lower.contains("imagen")
        && !lower.starts_with("chat_")
        && !lower.starts_with("tab_")
}

pub fn parse_available_models(payload: &str) -> Vec<String> {
    let Ok(root) = serde_json::from_str::<Value>(payload) else {
        return Vec::new();
    };
    let Some(models_map) = root.get("models").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();

    if let Some(sorts) = root.get("agentModelSorts").and_then(Value::as_array) {
        for sort in sorts {
            if let Some(groups) = sort.get("groups").and_then(Value::as_array) {
                for group in groups {
                    if let Some(ids) = group.get("modelIds").and_then(Value::as_array) {
                        for id in ids.iter().filter_map(Value::as_str) {
                            let disabled = models_map
                                .get(id)
                                .and_then(|e| e.get("disabled"))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            if !disabled
                                && models_map.contains_key(id)
                                && is_selectable_chat_model(id)
                                && seen.insert(id.to_string())
                            {
                                ordered.push(id.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(tiered) = root.get("tieredModelIds").and_then(Value::as_object) {
        for ids in tiered.values().filter_map(Value::as_array) {
            for id in ids.iter().filter_map(Value::as_str) {
                let disabled = models_map
                    .get(id)
                    .and_then(|e| e.get("disabled"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !disabled
                    && models_map.contains_key(id)
                    && is_selectable_chat_model(id)
                    && seen.insert(id.to_string())
                {
                    ordered.push(id.to_string());
                }
            }
        }
    }

    let mut remaining: Vec<String> = models_map
        .iter()
        .filter_map(|(id, entry)| {
            let disabled = entry
                .get("disabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !disabled && is_selectable_chat_model(id) && !seen.contains(id) {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect();
    remaining.sort();
    ordered.extend(remaining);

    ordered
}

#[async_trait::async_trait]
impl Provider for AntigravityProvider {
    fn id(&self) -> &str {
        "antigravity"
    }

    fn last_payload(&self) -> Option<Value> {
        self.last_payload
            .lock()
            .ok()
            .and_then(|payload| payload.clone())
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let fallback = || SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect();

        let Ok((access_token, project_id)) = self.ensure_auth().await else {
            return Ok(fallback());
        };

        let url = format!(
            "{}/v1internal:fetchAvailableModels",
            self.base_url.trim_end_matches('/')
        );
        let body = if project_id.is_empty() {
            json!({})
        } else {
            json!({ "project": project_id })
        };

        let Ok(response) = self
            .client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(USER_AGENT, ANTIGRAVITY_USER_AGENT)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await
        else {
            return Ok(fallback());
        };

        if !response.status().is_success() {
            return Ok(fallback());
        }

        let Ok(text) = response.text().await else {
            return Ok(fallback());
        };
        let models = parse_available_models(&text);
        if models.is_empty() {
            Ok(fallback())
        } else {
            Ok(models)
        }
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let (access_token, project_id) = self.ensure_auth().await?;

        let effective_project_id = if project_id.is_empty() {
            ANTIGRAVITY_DEFAULT_PROJECT_ID
        } else {
            &project_id
        };

        let body = build_body(&request, effective_project_id);
        if let Ok(mut last) = self.last_payload.lock() {
            *last = Some(body.clone());
        }
        let url = format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            self.base_url.trim_end_matches('/')
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", access_token))
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
            ready_events: VecDeque::new(),
            accumulated_calls: Vec::new(),
            last_usage: None,
            done: false,
        };

        let stream = unfold(state, |mut state| async move {
            loop {
                if let Some(event) = state.ready_events.pop_front() {
                    return Some((Ok(event), state));
                }

                if let Some(payload) = state.pending.pop_front() {
                    if payload.trim() == "[DONE]" {
                        state.done = true;
                        continue;
                    }
                    match extract_chunk(&payload) {
                        Ok(chunk) => {
                            if !chunk.tool_calls.is_empty() {
                                state.accumulated_calls.extend(chunk.tool_calls);
                            }
                            if let Some(text) = chunk.text {
                                state.ready_events.push_back(ProviderEvent::Text(text));
                            }
                            if chunk.prompt_tokens.is_some()
                                || chunk.cached_tokens.is_some()
                                || chunk.completion_tokens.is_some()
                            {
                                state.last_usage = Some((
                                    chunk.prompt_tokens,
                                    chunk.cached_tokens,
                                    chunk.completion_tokens,
                                ));
                            }
                            continue;
                        }
                        Err(err) => return Some((Err(err), state)),
                    }
                }

                if state.done {
                    if let Some((prompt_tokens, cached_tokens, completion_tokens)) =
                        state.last_usage.take()
                    {
                        return Some((
                            Ok(ProviderEvent::Usage {
                                prompt_tokens,
                                cached_tokens,
                                completion_tokens,
                            }),
                            state,
                        ));
                    }
                    if !state.accumulated_calls.is_empty() {
                        let calls = std::mem::take(&mut state.accumulated_calls);
                        return Some((Ok(ProviderEvent::ToolCalls(calls)), state));
                    }
                    return None;
                }

                match state.response.chunk().await {
                    Ok(Some(bytes)) => {
                        for event in state.parser.push(&bytes) {
                            state.pending.push_back(event);
                        }
                    }
                    Ok(None) => {
                        state.done = true;
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

pub fn register_provider(registry: &ProviderRegistry, provider: AntigravityProvider) {
    registry.register(Arc::new(provider));
}

pub fn register_all(registry: &ProviderRegistry, credentials: AntigravityCredentials) {
    register_provider(registry, AntigravityProvider::new(credentials));
}

fn base64_url_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };

        result.push(CHARS[((b0 >> 2) & 0x3F) as usize] as char);
        result.push(CHARS[(((b0 & 0x03) << 4) | ((b1 >> 4) & 0x0F)) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[(((b1 & 0x0F) << 2) | ((b2 >> 6) & 0x03)) as usize] as char);
        }
        if chunk.len() > 2 {
            result.push(CHARS[(b2 & 0x3F) as usize] as char);
        }
    }
    result
}

pub fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(verifier.as_bytes());
    base64_url_encode(&hash)
}

pub fn build_authorization_url(verifier: &str, state: &str) -> String {
    let challenge = pkce_challenge(verifier);
    let scopes = [
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
        "https://www.googleapis.com/auth/cclog",
        "https://www.googleapis.com/auth/experimentsandconfigs",
    ]
    .join(" ");

    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        url_encode(ANTIGRAVITY_CLIENT_ID),
        url_encode(ANTIGRAVITY_REDIRECT_URI),
        url_encode(&scopes),
        url_encode(&challenge),
        url_encode(state),
    )
}

pub fn parse_exchange_response(
    payload: &str,
    now_secs: u64,
) -> Result<(String, String, u64), ProviderError> {
    let val: Value = serde_json::from_str(payload)
        .map_err(|e| ProviderError::Protocol(format!("resposta de token invalida: {e}")))?;

    if let Some(err) = val.get("error").and_then(Value::as_str) {
        let desc = val
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or(err);
        return Err(ProviderError::Protocol(format!("erro OAuth: {desc}")));
    }

    let access_token = val
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Protocol("access_token ausente".to_string()))?
        .to_string();

    let refresh_token = val
        .get("refresh_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Protocol("refresh_token ausente".to_string()))?
        .to_string();

    let expires_in = val
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or(3600);

    Ok((access_token, refresh_token, now_secs + expires_in))
}

pub async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    verifier: &str,
    now_secs: u64,
) -> Result<(String, String, u64), ProviderError> {
    let body = format!(
        "client_id={}&client_secret={}&code={}&grant_type=authorization_code&redirect_uri={}&code_verifier={}",
        url_encode(ANTIGRAVITY_CLIENT_ID),
        url_encode(ANTIGRAVITY_CLIENT_SECRET),
        url_encode(code),
        url_encode(ANTIGRAVITY_REDIRECT_URI),
        url_encode(verifier)
    );

    let response = client
        .post("https://oauth2.googleapis.com/token")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderError::Http {
            status: status.as_u16(),
            body: text,
        });
    }

    parse_exchange_response(&text, now_secs)
}

fn extract_project_field(val: &Value) -> Option<String> {
    if let Some(id) = val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(id.to_string());
    }
    if let Some(id) = val
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(id.to_string());
    }
    None
}

fn select_onboard_tier(payload: &Value) -> String {
    if let Some(tiers) = payload.get("allowedTiers").and_then(Value::as_array) {
        for tier in tiers {
            if tier.get("isDefault").and_then(Value::as_bool) == Some(true)
                && let Some(id) = tier.get("id").and_then(Value::as_str)
                && !id.is_empty()
            {
                return id.to_string();
            }
        }
        if let Some(id) = tiers
            .first()
            .and_then(|t| t.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            return id.to_string();
        }
    }

    if let Some(id) = payload
        .get("currentTier")
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        return id.to_string();
    }

    "free-tier".to_string()
}

pub async fn resolve_project_id(client: &reqwest::Client, access_token: &str) -> String {
    resolve_project_id_at(client, ANTIGRAVITY_ENDPOINT, access_token).await
}

async fn resolve_project_id_at(
    client: &reqwest::Client,
    base_url: &str,
    access_token: &str,
) -> String {
    let base = base_url.trim_end_matches('/');
    let load_url = format!("{base}/v1internal:loadCodeAssist");
    let body = json!({
        "metadata": {
            "ideType": "ANTIGRAVITY"
        }
    });

    let load_data = match client
        .post(load_url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(USER_AGENT, ANTIGRAVITY_USER_AGENT)
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response.json::<Value>().await.ok(),
        _ => None,
    };

    if let Some(data) = load_data.as_ref()
        && let Some(proj) = data.get("cloudaicompanionProject")
        && let Some(id) = extract_project_field(proj)
    {
        return id;
    }

    let tier_id = load_data
        .as_ref()
        .map(select_onboard_tier)
        .unwrap_or_else(|| "free-tier".to_string());

    let onboard_url = format!("{base}/v1internal:onboardUser");
    let onboard_body = json!({
        "tier_id": tier_id,
        "metadata": {
            "ide_type": "ANTIGRAVITY",
            "ide_version": "2.24.0",
            "ide_name": "antigravity"
        }
    });

    for attempt in 0..4 {
        let Ok(resp) = client
            .post(&onboard_url)
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(USER_AGENT, ANTIGRAVITY_ONBOARD_USER_AGENT)
            .header("X-Goog-Api-Client", ANTIGRAVITY_GOOG_API_CLIENT)
            .header(CONTENT_TYPE, "application/json")
            .json(&onboard_body)
            .send()
            .await
        else {
            break;
        };

        if !resp.status().is_success() {
            break;
        }

        if let Ok(data) = resp.json::<Value>().await {
            if data.get("done").and_then(Value::as_bool) == Some(true) {
                if let Some(proj) = data
                    .get("response")
                    .and_then(|r| r.get("cloudaicompanionProject"))
                    && let Some(id) = extract_project_field(proj)
                {
                    return id;
                }
                break;
            }
        }

        if attempt < 3 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    ANTIGRAVITY_DEFAULT_PROJECT_ID.to_string()
}

pub async fn run_login_flow<F>(
    open_browser: bool,
    notify_url: F,
) -> Result<AntigravityCredentials, ProviderError>
where
    F: Fn(&str),
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:51121")
        .await
        .map_err(|e| ProviderError::Network(format!("não foi possível abrir porta 51121: {e}")))?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let verifier = format!(
        "{}{}",
        blake3::hash(&now.to_le_bytes()).to_hex(),
        blake3::hash(b"tupi-pkce").to_hex()
    );
    let state = blake3::hash(&now.to_be_bytes()).to_hex().to_string();
    let auth_url = build_authorization_url(&verifier, &state);

    notify_url(&auth_url);

    if open_browser {
        let mut cmd = {
            #[cfg(target_os = "linux")]
            {
                let mut c = std::process::Command::new("xdg-open");
                c.arg(&auth_url);
                c
            }
            #[cfg(target_os = "macos")]
            {
                let mut c = std::process::Command::new("open");
                c.arg(&auth_url);
                c
            }
            #[cfg(target_os = "windows")]
            {
                let mut c = std::process::Command::new("cmd");
                c.args(["/C", "start", "", &auth_url]);
                c
            }
        };

        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        if let Ok(mut child) = cmd.spawn() {
            tokio::task::spawn_blocking(move || {
                let _ = child.wait();
            });
        }
    }

    let (mut socket, _) = listener
        .accept()
        .await
        .map_err(|e| ProviderError::Network(format!("falha ao receber callback: {e}")))?;

    let mut buffer = [0u8; 4096];
    let n = socket
        .read(&mut buffer)
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;

    let req_str = String::from_utf8_lossy(&buffer[..n]);
    let code = extract_query_param(&req_str, "code").ok_or_else(|| {
        ProviderError::Protocol("código de autorização não encontrado no callback".to_string())
    })?;

    let response_html = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<!DOCTYPE html><html><body style='font-family:sans-serif;text-align:center;padding:50px'><h2>Autenticação concluída com sucesso!</h2><p>Você pode fechar esta aba e retornar ao Tupi.</p></body></html>";
    let _ = socket.write_all(response_html.as_bytes()).await;

    let client = reqwest::Client::new();
    let now_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let (access_token, refresh_token, expires_at) =
        exchange_code(&client, &code, &verifier, now_secs).await?;
    let project_id = resolve_project_id(&client, &access_token).await;

    Ok(AntigravityCredentials {
        access_token,
        refresh_token,
        expires_at,
        project_id,
        email: None,
    })
}

fn extract_query_param(req: &str, param: &str) -> Option<String> {
    let first_line = req.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    let query = path.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == param {
                return Some(v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tupi_domain::Message;
    use tupi_kernel::ToolSpec;

    #[test]
    fn body_wraps_in_antigravity_envelope_and_resolves_model() {
        let request = ChatRequest::new("antigravity-gemini-3.8-flash", vec![Message::user("ola")]);
        let body = build_body(&request, "rising-fact-p41fc");

        assert_eq!(body["project"], "rising-fact-p41fc");
        assert_eq!(body["model"], "gemini-3.8-flash-high");
        assert_eq!(body["requestType"], "agent");
        assert_eq!(body["userAgent"], "antigravity");
        assert!(body["requestId"].as_str().unwrap().starts_with("agent-"));
        assert_eq!(body["request"]["contents"][0]["role"], "user");
        assert_eq!(body["request"]["contents"][0]["parts"][0]["text"], "ola");
    }

    #[test]
    fn resolve_api_model_strips_prefix_and_keeps_valid_ids() {
        assert_eq!(
            resolve_api_model("antigravity-gemini-3.8-flash-medium"),
            "gemini-3.8-flash-medium"
        );
        assert_eq!(
            resolve_api_model("gemini-3.8-flash-high"),
            "gemini-3.8-flash-high"
        );
        assert_eq!(
            resolve_api_model("antigravity-gemini-4-hydrogen-high"),
            "gemini-4-hydrogen-high"
        );
        assert_eq!(
            resolve_api_model("antigravity-gemini-3-flash-high"),
            "gemini-3-flash"
        );
    }

    #[test]
    fn body_serializes_system_instruction() {
        let request = ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![
                Message::new(Role::System, "voce e um assistente"),
                Message::user("oi"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        assert_eq!(body["request"]["systemInstruction"]["role"], "user");
        assert_eq!(
            body["request"]["systemInstruction"]["parts"][0]["text"],
            "voce e um assistente"
        );
        assert_eq!(body["request"]["contents"].as_array().unwrap().len(), 1);
        assert_eq!(body["request"]["contents"][0]["role"], "user");
    }

    #[test]
    fn body_preserves_multiple_system_messages_in_order() {
        let request = ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![
                Message::new(Role::System, "instrucao principal"),
                Message::new(Role::System, "memoria relevante"),
                Message::user("oi"),
                Message::new(Role::System, "freio de passos"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        let parts = body["request"]["systemInstruction"]["parts"]
            .as_array()
            .expect("systemInstruction.parts");
        let texts: Vec<&str> = parts.iter().filter_map(|p| p["text"].as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "instrucao principal",
                "memoria relevante",
                "freio de passos"
            ]
        );
    }

    #[test]
    fn body_serializes_tools_as_function_declarations_and_cleans_schema() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "le um arquivo".into(),
            schema: json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "minLength": 1 },
                    "limit": { "type": "integer", "minimum": 1, "default": 10 }
                },
                "required": ["path", "ghost"]
            }),
        };
        let request = ChatRequest::new("gemini-3.8-flash-high", vec![Message::user("leia")])
            .with_tools(vec![spec]);
        let body = build_body(&request, "rising-fact-p41fc");

        let decls = &body["request"]["tools"][0]["functionDeclarations"];
        assert_eq!(decls[0]["name"], "read_file");
        assert_eq!(decls[0]["description"], "le um arquivo");
        assert_eq!(decls[0]["parameters"]["type"], "object");
        assert!(decls[0]["parameters"].get("$schema").is_none());
        assert!(decls[0]["parameters"].get("additionalProperties").is_none());
        assert!(
            decls[0]["parameters"]["properties"]["path"]
                .get("minLength")
                .is_none()
        );
        assert!(
            decls[0]["parameters"]["properties"]["limit"]
                .get("minimum")
                .is_none()
        );
        assert_eq!(decls[0]["parameters"]["required"], json!(["path"]));
    }

    #[test]
    fn body_serializes_assistant_tool_calls_and_responses() {
        let call1 = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into(),
        };
        let call2 = ToolCall {
            id: "call_2".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"b.txt"}"#.into(),
        };
        let request = ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![
                Message::user("leia os arquivos"),
                Message::assistant_with_calls("", vec![call1, call2]),
                Message::tool("call_1", "conteudo A"),
                Message::tool("call_2", "conteudo B"),
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
        assert_eq!(
            contents[1]["parts"][0]["thoughtSignature"],
            SKIP_THOUGHT_SIGNATURE
        );
        assert!(contents[1]["parts"][1].get("thoughtSignature").is_none());

        // Tool function responses grouped under role "user"
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(contents[2]["parts"][0]["functionResponse"]["id"], "call_1");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["output"],
            "conteudo A"
        );
        assert_eq!(contents[2]["parts"][1]["functionResponse"]["id"], "call_2");
        assert_eq!(
            contents[2]["parts"][1]["functionResponse"]["response"]["output"],
            "conteudo B"
        );
    }

    #[test]
    fn chunk_extracts_text_delta_and_skips_thought_parts() {
        let payload = r#"{"response":{"candidates":[{"content":{"parts":[{"thought":true,"text":"pensando..."},{"text":"Ola mundo"}]}}]}}"#;
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
        let payload = r#"{"response":{"candidates":[{"content":{"parts":[{"text":"ok"}]}}],"usageMetadata":{"promptTokenCount":120,"candidatesTokenCount":45,"cachedContentTokenCount":30}}}"#;
        let chunk = extract_chunk(payload).unwrap();
        assert_eq!(chunk.text, Some("ok".into()));
        assert_eq!(chunk.prompt_tokens, Some(120));
        assert_eq!(chunk.completion_tokens, Some(45));
        assert_eq!(chunk.cached_tokens, Some(30));
    }

    #[test]
    fn parse_available_models_prefers_selectable_and_filters_disabled_or_image() {
        let payload = r#"{
            "models": {
                "gemini-3.8-flash-high": { "displayName": "Gemini 3.8 Flash (High)" },
                "gemini-4-hydrogen-high": { "displayName": "Gemini 4 Hydrogen" },
                "gemini-3-pro-image": { "displayName": "Image" },
                "disabled-model": { "disabled": true }
            },
            "agentModelSorts": [
                { "groups": [{ "modelIds": ["gemini-4-hydrogen-high", "gemini-3.8-flash-high", "gemini-3-pro-image", "disabled-model"] }] }
            ]
        }"#;
        let models = parse_available_models(payload);
        assert_eq!(
            models,
            vec![
                "gemini-4-hydrogen-high".to_string(),
                "gemini-3.8-flash-high".to_string()
            ]
        );
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
    fn falls_back_to_opencode_accounts_format_and_prefers_managed_project() {
        let json = r#"{
            "version": 4,
            "accounts": [
                {
                    "refreshToken": "1//opencode_refresh",
                    "projectId": "opencode-proj",
                    "managedProjectId": "managed-proj-1",
                    "email": "dev@gmail.com"
                }
            ],
            "activeIndex": 0
        }"#;

        let creds = AntigravityCredentials::from_opencode_json_str(json).unwrap();
        assert_eq!(creds.refresh_token, "1//opencode_refresh");
        assert_eq!(creds.project_id, "managed-proj-1");
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

    #[test]
    fn parse_token_response_success() {
        let json = r#"{"access_token":"ya29.new_token","expires_in":3600,"token_type":"Bearer"}"#;
        let (token, expires_at) = AntigravityCredentials::parse_token_response(json, 1000).unwrap();
        assert_eq!(token, "ya29.new_token");
        assert_eq!(expires_at, 4600);
    }

    #[test]
    fn parse_token_response_error() {
        let json =
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#;
        let err = AntigravityCredentials::parse_token_response(json, 1000).unwrap_err();
        match err {
            ProviderError::Protocol(msg) => {
                assert!(msg.contains("Token has been expired or revoked"));
            }
            _ => panic!("esperava ProviderError::Protocol"),
        }
    }

    #[test]
    fn credentials_file_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let creds = AntigravityCredentials {
            access_token: "ya29.saved".into(),
            refresh_token: "1//refresh".into(),
            expires_at: 2000,
            project_id: "my-project".into(),
            email: Some("me@test.com".into()),
        };

        let file_path = dir.path().join("antigravity.json");
        creds.save(&file_path).unwrap();

        let loaded = AntigravityCredentials::load_from_dirs(dir.path(), None).unwrap();
        assert_eq!(loaded, creds);
    }

    #[test]
    fn load_from_dirs_falls_back_to_opencode_file() {
        let tupi_dir = tempfile::tempdir().unwrap();
        let opencode_dir = tempfile::tempdir().unwrap();

        let opencode_json = r#"{
            "version": 4,
            "accounts": [
                {
                    "refreshToken": "1//opencode_saved",
                    "projectId": "opencode-proj",
                    "email": "user@gmail.com"
                }
            ],
            "activeIndex": 0
        }"#;
        std::fs::write(
            opencode_dir.path().join("antigravity-accounts.json"),
            opencode_json,
        )
        .unwrap();

        let loaded =
            AntigravityCredentials::load_from_dirs(tupi_dir.path(), Some(opencode_dir.path()))
                .unwrap();
        assert_eq!(loaded.refresh_token, "1//opencode_saved");
        assert_eq!(loaded.project_id, "opencode-proj");
        assert_eq!(loaded.email, Some("user@gmail.com".into()));
    }

    #[tokio::test]
    async fn provider_id_and_models() {
        let creds = AntigravityCredentials {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires_at: u64::MAX,
            project_id: "p".into(),
            email: None,
        };
        let provider = AntigravityProvider::new(creds.clone()).with_base_url("http://127.0.0.1:1");
        assert_eq!(provider.id(), "antigravity");

        let models = provider.list_models().await.unwrap();
        assert!(models.contains(&"gemini-3.8-flash-high".to_string()));
        assert!(models.contains(&"gemini-4-hydrogen-high".to_string()));
        assert!(models.contains(&"gemini-4-hydrogen-xhigh".to_string()));
        assert!(models.contains(&"gemini-4-argon-high".to_string()));

        let mut registry = ProviderRegistry::new();
        register_all(&mut registry, creds);
        assert_eq!(registry.ids(), vec!["antigravity".to_string()]);
    }

    #[tokio::test]
    async fn list_models_fetches_available_models_from_endpoint() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let body = r#"{"models":{"gemini-4-hydrogen-high":{},"gemini-3.8-flash-high":{}}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let creds = AntigravityCredentials {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires_at: u64::MAX,
            project_id: "proj-1".into(),
            email: None,
        };
        let provider = AntigravityProvider::new(creds).with_base_url(format!("http://{addr}"));
        let models = provider.list_models().await.unwrap();
        assert_eq!(
            models,
            vec![
                "gemini-3.8-flash-high".to_string(),
                "gemini-4-hydrogen-high".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn resolve_project_id_onboards_when_load_has_no_project() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]);
                let body = if req.contains("loadCodeAssist") {
                    r#"{"allowedTiers":[{"id":"free-tier","isDefault":true}]}"#
                } else {
                    r#"{"done":true,"response":{"cloudaicompanionProject":{"id":"onboarded-proj-42"}}}"#
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
        });

        let client = reqwest::Client::new();
        let proj = resolve_project_id_at(&client, &format!("http://{addr}"), "tok").await;
        assert_eq!(proj, "onboarded-proj-42");
    }

    #[tokio::test]
    async fn stream_chat_emits_both_text_and_usage_from_same_chunk() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let sse = concat!(
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ola\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":5}}}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let creds = AntigravityCredentials {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires_at: u64::MAX,
            project_id: "proj-1".into(),
            email: None,
        };
        let provider = AntigravityProvider::new(creds).with_base_url(format!("http://{addr}"));
        let mut stream = provider
            .stream_chat(ChatRequest::new(
                "gemini-3.8-flash-high",
                vec![Message::user("oi")],
            ))
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.unwrap());
        }
        assert_eq!(
            events,
            vec![
                ProviderEvent::Text("ola".to_string()),
                ProviderEvent::Usage {
                    prompt_tokens: Some(10),
                    cached_tokens: None,
                    completion_tokens: Some(5),
                }
            ]
        );
    }

    #[tokio::test]
    async fn stream_chat_emits_only_final_usage_across_multiple_chunks() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let sse = concat!(
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ola\"}]}}],\"usageMetadata\":{\"promptTokenCount\":1000,\"candidatesTokenCount\":2}}}\n\n",
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" mundo\"}]}}],\"usageMetadata\":{\"promptTokenCount\":1000,\"candidatesTokenCount\":5,\"cachedContentTokenCount\":400}}}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let creds = AntigravityCredentials {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires_at: u64::MAX,
            project_id: "proj-1".into(),
            email: None,
        };
        let provider = AntigravityProvider::new(creds).with_base_url(format!("http://{addr}"));
        let mut stream = provider
            .stream_chat(ChatRequest::new(
                "gemini-3.8-flash-high",
                vec![Message::user("oi")],
            ))
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.unwrap());
        }
        assert_eq!(
            events,
            vec![
                ProviderEvent::Text("ola".to_string()),
                ProviderEvent::Text(" mundo".to_string()),
                ProviderEvent::Usage {
                    prompt_tokens: Some(1000),
                    cached_tokens: Some(400),
                    completion_tokens: Some(5),
                },
            ]
        );
    }

    #[test]
    fn authorization_url_contains_client_id_and_pkce_challenge() {
        let url = build_authorization_url("test_verifier_12345", "test_state");
        assert!(url.contains("client_id=1071006060591"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=test_state"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A51121%2Foauth-callback"));
    }

    #[test]
    fn parse_exchange_response_extracts_tokens() {
        let json = r#"{"access_token":"ya29.acc","refresh_token":"1//ref","expires_in":3600}"#;
        let (acc, ref_tok, exp) = parse_exchange_response(json, 1000).unwrap();
        assert_eq!(acc, "ya29.acc");
        assert_eq!(ref_tok, "1//ref");
        assert_eq!(exp, 4600);
    }
}
