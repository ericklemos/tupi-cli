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

use crate::antigravity_signature::{
    SignatureStore, build_body_with_signatures, collect_signatures, scope_of_envelope,
};
use crate::sse::SseParser;
use tupi_domain::{Role, ToolCall};
use tupi_kernel::{
    ChatRequest, Provider, ProviderError, ProviderEvent, ProviderRegistry, ProviderStream,
    ToolSpec,
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

/// System instruction que o cliente Hub manda em todo request de estilo
/// antigravity (referencia: `ANTIGRAVITY_SYSTEM_INSTRUCTION` em src/constants.ts
/// do plugin opencode-antigravity-auth). Vai como primeira parte do
/// `systemInstruction` com role=user; as system messages do tupi entram
/// depois, na ordem original.
const ANTIGRAVITY_SYSTEM_INSTRUCTION: &str = concat!(
    "You are Antigravity, a powerful agentic AI coding assistant designed by the Google Deepmind team working on Advanced Agentic Coding.",
    "You are pair programming with a USER to solve their coding task. The task may require creating a new codebase, modifying or debugging an existing codebase, or simply answering a question.",
    "The USER will send you requests, which you must always prioritize addressing.",
    "Always maintain a helpful and respectful tone in your responses, and avoid being overly verbose.",
    "\n",
    "**Absolute paths only**",
    "When using tools, always use absolute file paths.",
    "\n",
    "**Proactiveness**",
    "You are an agent - please keep going until the user's query is completely resolved, before ending your turn and yielding back to the user. Only terminate your turn when you are sure that the problem is solved.",
    "If you are not sure about file content or codebase structure pertaining to the user's request, use your tools to read files and gather the relevant information: do NOT guess or make up an answer.",
    "Your goal is to accomplish the user's task, NOT engage in a back and forth conversation.",
    "You MUST complete the task, and only then yield control back to the user and end your turn without text.",
);

/// Folga de renovacao do access token: renova 5 min antes do vencimento.
const REFRESH_SKEW_SECS: u64 = 300;

/// Jitter maximo somado a folga, derivado do proprio token, para espalhar a
/// renovacao entre contas sem decidir "renova / nao renova" ao acaso.
const REFRESH_JITTER_SECS: u64 = 30;

const ANTIGRAVITY_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

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

/// TTL do catalogo descoberto por `fetchAvailableModels`. A referencia
/// (opencode-antigravity-auth) refaz a consulta a cada ~1h; dentro da janela
/// `list_models` responde do cache sem gerar request nenhum.
const MODELS_CACHE_TTL_SECS: u64 = 3_600;

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

    let mut out = serde_json::Map::new();
    let Value::Object(map) = schema else {
        return match schema {
            Value::Array(arr) => Value::Array(arr.iter().map(clean_gemini_schema).collect()),
            other => other.clone(),
        };
    };
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
                        cleaned_props.insert(prop_key.clone(), clean_gemini_schema(prop_val));
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

    // Array sem `items` (ou com `items` vazio) e INVALID_ARGUMENT no Gemini; o
    // fallback da referencia e uma lista de strings.
    if out.get("type").and_then(Value::as_str) == Some("array") {
        let items_ok = out
            .get("items")
            .and_then(Value::as_object)
            .is_some_and(|items| !items.is_empty());
        if !items_ok {
            out.insert("items".to_string(), json!({ "type": "string" }));
        }
    }

    Value::Object(out)
}

/// Campo sentinela que abre `properties` quando a ferramenta nao declara
/// nenhum. O Google recusa (`INVALID_ARGUMENT`) `parameters` OBJECT sem
/// `properties`, e um schema vazio derruba o turno inteiro antes de qualquer
/// chamada; a referencia (plugin Hub) resolve com um campo ignoravel.
const EMPTY_SCHEMA_PLACEHOLDER_NAME: &str = "_placeholder";
const EMPTY_SCHEMA_PLACEHOLDER_DESCRIPTION: &str =
    "Ignored placeholder field required by the API; do not set it.";

fn empty_schema_properties() -> serde_json::Map<String, Value> {
    let mut properties = serde_json::Map::new();
    properties.insert(
        EMPTY_SCHEMA_PLACEHOLDER_NAME.to_string(),
        json!({
            "type": "boolean",
            "description": EMPTY_SCHEMA_PLACEHOLDER_DESCRIPTION,
        }),
    );
    properties
}

fn empty_object_parameters() -> Value {
    json!({
        "type": "object",
        "properties": Value::Object(empty_schema_properties()),
    })
}

/// Normaliza o `parameters` de uma functionDeclaration. Schema vazio (`{}`) ou
/// objeto sem `properties` vira OBJECT com o campo sentinela; tipo fora de
/// OBJECT nao existe como `parameters` no protocolo e cai no mesmo placeholder.
fn normalize_tool_parameters(schema: &Value) -> Value {
    let cleaned = clean_gemini_schema(schema);
    let Value::Object(mut map) = cleaned else {
        return empty_object_parameters();
    };
    match map.get("type").and_then(Value::as_str) {
        Some("object") | None => {}
        Some(_) => return empty_object_parameters(),
    }
    let has_properties = map
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|props| !props.is_empty());
    if has_properties {
        if !map.contains_key("type") {
            map.insert("type".to_string(), Value::String("object".to_string()));
        }
    } else {
        map.insert("type".to_string(), Value::String("object".to_string()));
        map.insert(
            "properties".to_string(),
            Value::Object(empty_schema_properties()),
        );
        // `required` so apontaria para propriedades inexistentes: sai.
        map.remove("required");
    }
    Value::Object(map)
}

fn normalize_function_declaration(tool: &ToolSpec) -> Value {
    json!({
        "name": tool.name.trim(),
        "description": tool.description,
        "parameters": normalize_tool_parameters(&tool.schema),
    })
}

pub fn build_body(request: &ChatRequest, project_id: &str) -> Value {
    // A instrucao fixa do Antigravity abre o systemInstruction (role=user),
    // como no cliente Hub; as system messages do tupi vem depois, na ordem.
    let mut system_parts: Vec<Value> = vec![json!({ "text": ANTIGRAVITY_SYSTEM_INSTRUCTION })];
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
    // functionCall ja emitidos e ainda sem functionResponse, na ordem do
    // dialogo: e dessa fila que sai o par de um functionResponse cujo id nao
    // casa com nenhuma chamada (id vazio, trocado ou repetido).
    let mut open_calls: VecDeque<String> = VecDeque::new();

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
                    open_calls.push_back(call.id.clone());
                }
                contents.push(json!({
                    "role": "model",
                    "parts": parts
                }));
            }
            Role::Tool => {
                let requested_id = msg
                    .tool_call_id
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                // Id divergente nao pode subir como esta: functionResponse sem
                // functionCall correspondente vira INVALID_ARGUMENT em serie.
                // O Hub repara o par pela ordem dos functionCall abertos
                // (fixToolResponseGrouping/validateAndFixClaudeToolPairing) em
                // vez de propagar o id quebrado.
                let matched = (!requested_id.is_empty())
                    .then(|| {
                        open_calls
                            .iter()
                            .position(|id| id.trim() == requested_id)
                    })
                    .flatten();
                let call_id = match matched {
                    Some(idx) => match open_calls.remove(idx) {
                        Some(id) => id,
                        None => continue,
                    },
                    None => {
                        // Sem candidato aberto nao ha par possivel: o resultado
                        // solto e descartado, nunca enviado com id invalido.
                        let Some(repaired) = open_calls.pop_front() else {
                            tracing::debug!(
                                call_id = %requested_id,
                                "tool result sem functionCall correspondente descartado no antigravity"
                            );
                            continue;
                        };
                        if !requested_id.is_empty() {
                            tracing::debug!(
                                requested = %requested_id,
                                repaired = %repaired,
                                "functionResponse com id divergente reparado no antigravity"
                            );
                        }
                        repaired
                    }
                };
                let Some(name) = call_names_by_id.get(&call_id).cloned() else {
                    tracing::debug!(
                        call_id = %call_id,
                        "functionCall reparado sem nome conhecido: resultado descartado"
                    );
                    continue;
                };
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

    // system_parts nunca esta vazio: a instrucao do Antigravity e a primeira.
    request_payload["systemInstruction"] = json!({
        "role": "user",
        "parts": system_parts
    });

    if !request.tools.is_empty() {
        let decls: Vec<Value> = request
            .tools
            .iter()
            .map(normalize_function_declaration)
            .collect();

        request_payload["tools"] = json!([{
            "functionDeclarations": decls
        }]);
    }

    let mut generation_config = serde_json::Map::new();
    // Claude: o piso de saida cobre o orcamento de thinking mesmo quando o
    // chamador pede um `max_tokens` menor (ou nao pede nada).
    let requested_max_tokens = request.max_tokens.and_then(|tokens| u64::try_from(tokens).ok());
    if let Some(max_tokens) =
        crate::antigravity_thinking::claude_output_tokens(&request.model, requested_max_tokens)
    {
        generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
    }
    if let Some(tier) = crate::antigravity_thinking::resolve_thinking_tier(&request.model) {
        generation_config.insert(
            "thinkingConfig".to_string(),
            json!({
                "includeThoughts": true,
                "thinkingBudget": tier.budget_tokens(),
            }),
        );
    }
    if !generation_config.is_empty() {
        request_payload["generationConfig"] = Value::Object(generation_config);
    }

    // Identidade da conversa: semente ancorada no inicio da sessao do tupi
    // (primeira mensagem do usuario) e no projeto, que nao mudam entre turnos.
    // Turnos seguintes so acrescentam mensagens ao fim, entao o `conversationId`
    // se mantem e uma sessao nova com outro pedido gera outro id.
    let first_user = request
        .messages
        .iter()
        .find(|msg| msg.role == Role::User)
        .map(|msg| msg.content.as_str());
    let session = match first_user {
        Some(first_user) => {
            let hex = blake3::hash(format!("{project_id}\u{1f}{first_user}").as_bytes())
                .to_hex()
                .to_string();
            format!("session-{}", &hex[..32])
        }
        // Sem turno do usuario nao ha sessao a preservar: id descartavel.
        None => generate_request_id(),
    };
    // O `requestId` muda de turno em turno (cada mensagem do usuario abre um) e
    // fica estavel dentro do turno, para o retry reenviar o mesmo id.
    let turn = request
        .messages
        .iter()
        .filter(|msg| msg.role == Role::User)
        .count()
        .max(1);
    let request_id = {
        let hex = blake3::hash(format!("{session}:{turn}").as_bytes())
            .to_hex()
            .to_string();
        format!("agent-{}", &hex[..32])
    };

    json!({
        "project": project_id,
        "model": resolve_api_model(&request.model),
        "request": request_payload,
        "requestType": "agent",
        "userAgent": "antigravity",
        "conversationId": session,
        "requestId": request_id,
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AntigravityChunk {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub prompt_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Amostra truncada do payload não-JSON (keepalive/linha parcial) descartado.
    pub junk: Option<String>,
}

pub fn extract_chunk(payload: &str) -> Result<AntigravityChunk, ProviderError> {
    fn error_payload(root: &Value) -> Option<&Value> {
        root.get("error")
            .or_else(|| root.get("response").and_then(|response| response.get("error")))
    }

    fn error_status(error: &Value) -> u16 {
        if let Some(code) = error.get("code").and_then(Value::as_u64) {
            return u16::try_from(code).unwrap_or(500);
        }
        match error.get("status").and_then(Value::as_str).unwrap_or_default() {
            "RESOURCE_EXHAUSTED" => 429,
            "INVALID_ARGUMENT" | "FAILED_PRECONDITION" | "OUT_OF_RANGE" => 400,
            "UNAUTHENTICATED" => 401,
            "PERMISSION_DENIED" => 403,
            "NOT_FOUND" => 404,
            "UNAVAILABLE" => 503,
            _ => 500,
        }
    }

    fn error_message(error: &Value) -> String {
        error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.get("status").and_then(Value::as_str))
            .unwrap_or("erro sem mensagem do provedor")
            .to_string()
    }

    fn parse_retry_seconds(raw: &str) -> Option<u64> {
        let trimmed = raw.trim();
        let trimmed = trimmed.strip_suffix('s').unwrap_or(trimmed);
        integer_part(trimmed).parse::<u64>().ok()
    }

    fn integer_part(raw: &str) -> &str {
        raw.split('.').next().unwrap_or(raw)
    }

    fn is_retry_info(detail: &Value) -> bool {
        detail
            .get("@type")
            .and_then(Value::as_str)
            .map_or(false, |kind| kind.ends_with("RetryInfo"))
    }

    fn retry_hint_seconds(error: &Value) -> Option<u64> {
        if let Some(details) = error.get("details").and_then(Value::as_array) {
            let mut candidates: Vec<&Value> =
                details.iter().filter(|detail| is_retry_info(detail)).collect();
            candidates.extend(details.iter());
            for detail in candidates {
                for field in ["retryDelay", "retry_delay"] {
                    if let Some(seconds) = detail
                        .get(field)
                        .and_then(Value::as_str)
                        .and_then(parse_retry_seconds)
                    {
                        return Some(seconds);
                    }
                }
            }
        }
        ["retryDelay", "retry_delay"]
            .iter()
            .find_map(|field| error.get(*field).and_then(Value::as_str))
            .and_then(parse_retry_seconds)
    }

    /// Quota esgotada: só quando o corpo fala de cota/licença, não de throughput.
    /// `"Resource has been exhausted (e.g. check quota)."` é rate limit por minuto.
    fn looks_like_quota(error: &Value, message: &str) -> bool {
        let text = message.to_ascii_lowercase();
        if text.contains("exceeded your current quota")
            || text.contains("quota exceeded")
            || text.contains("quota metric")
            || text.contains("daily limit")
            || text.contains("requests per day")
        {
            return true;
        }
        error
            .get("details")
            .map(|details| details.to_string().to_ascii_lowercase())
            .map_or(false, |details| {
                details.contains("quota_exceeded")
                    || details.contains("daily_limit")
                    || details.contains("per_day")
                    || details.contains("perday")
            })
    }

    fn map_error_payload(error: &Value, raw: &str) -> ProviderError {
        let status = error_status(error);
        let message = error_message(error);
        let body = raw.to_string();
        match status {
            429 if looks_like_quota(error, &message) => ProviderError::QuotaExhausted {
                status,
                reset: retry_hint_seconds(error),
                message,
                body,
            },
            429 => ProviderError::RateLimited {
                status,
                retry_after: retry_hint_seconds(error),
                message,
                body,
            },
            400 => ProviderError::InvalidArgument {
                status,
                message,
                body,
            },
            _ => ProviderError::Http { status, body },
        }
    }

    let root: Value = match serde_json::from_str(payload) {
        Ok(root) => root,
        // Keepalive/linha parcial do Hub não pode derrubar o turno: vira chunk
        // marcado como lixo, contado e descartado pelo stream.
        Err(err) => {
            let mut sample: String = payload.chars().take(120).collect();
            if payload.chars().count() > 120 {
                sample.push('…');
            }
            return Ok(AntigravityChunk {
                junk: Some(format!("json invalido: {err}; amostra: {sample}")),
                ..AntigravityChunk::default()
            });
        }
    };
    if let Some(error) = error_payload(&root) {
        return Err(map_error_payload(error, payload));
    }
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

/// Relogio do provider, sobrescrito nos testes.
fn system_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Jitter deterministico (0..=REFRESH_JITTER_SECS) derivado do proprio token:
/// duas contas renovam em instantes diferentes, mas a mesma conta nao oscila
/// entre "renova" e "nao renova" a cada chamada.
fn refresh_jitter_secs(creds: &AntigravityCredentials) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(creds.refresh_token.as_bytes());
    hasher.update(&creds.expires_at.to_le_bytes());
    hasher.update(creds.email.as_deref().unwrap_or_default().as_bytes());
    let digest = hasher.finalize();
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(raw) % (REFRESH_JITTER_SECS + 1)
}

/// Janela efetiva de renovacao: skew de 5 min + jitter do token.
fn refresh_margin_secs(creds: &AntigravityCredentials) -> u64 {
    REFRESH_SKEW_SECS + refresh_jitter_secs(creds)
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
        now_secs.saturating_add(refresh_margin_secs(self)) >= self.expires_at
    }

    pub fn from_json_str(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub fn from_opencode_json_str(text: &str) -> Option<Self> {
        let (accounts, active_index) = crate::antigravity_accounts::parse_opencode_accounts(
            text,
            ANTIGRAVITY_DEFAULT_PROJECT_ID,
        );
        accounts.get(active_index).or_else(|| accounts.first()).cloned()
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

        // Store proprio: as contas do arquivo do plugin entram por copia e o
        // provider segue com a conta ativa local, nao com um emprestimo do
        // arquivo alheio (que nunca e aberto para escrita).
        let store_path = crate::antigravity_accounts::AntigravityAccountStore::path(tupi_dir);
        let mut local = crate::antigravity_accounts::AntigravityAccountStore::load(&store_path);
        let mut changed = false;
        if let Some(op_dir) = opencode_dir {
            let op_path = op_dir.join(crate::antigravity_accounts::AntigravityAccountStore::FILE_NAME);
            // Mesmo caminho nos dois lados: nao se escreve no arquivo alheio.
            if op_path != store_path {
                match local.import_from_opencode_file(&op_path, ANTIGRAVITY_DEFAULT_PROJECT_ID) {
                    Ok(added) => changed = added > 0,
                    Err(_) => {}
                }
            }
        }
        if changed
            && let Err(error) = local.save(&store_path)
        {
            tracing::warn!(
                %error,
                path = %store_path.display(),
                "falha ao gravar contas Antigravity importadas"
            );
        }

        local.active().cloned()
    }

    pub async fn refresh_if_expired(
        &mut self,
        client: &reqwest::Client,
        token_endpoint: &str,
        now_secs: u64,
        force: bool,
    ) -> Result<(), ProviderError> {
        if !force && !self.is_expired(now_secs) && !self.access_token.is_empty() {
            return Ok(());
        }

        if self.refresh_token.trim().is_empty() {
            return Err(ProviderError::Protocol(
                "refresh token ausente: refaca o login do antigravity".to_string(),
            ));
        }

        let body = format!(
            "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
            url_encode(&self.refresh_token),
            url_encode(ANTIGRAVITY_CLIENT_ID),
            url_encode(ANTIGRAVITY_CLIENT_SECRET)
        );

        let response = client
            .post(token_endpoint)
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
    token_endpoint: String,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    save_path: Option<std::path::PathBuf>,
    on_refresh: Option<RefreshHook>,
    last_payload: Arc<std::sync::Mutex<Option<Value>>>,
    /// Assinaturas de pensamento vistas nesta execucao, por sessao/modelo.
    signatures: Arc<std::sync::Mutex<SignatureStore>>,
    /// Ultimo catalogo de `fetchAvailableModels`, com TTL; vencido continua
    /// valendo como filtro de entitlement da conta.
    models_cache: Arc<std::sync::Mutex<Option<ModelsCache>>>,
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
            token_endpoint: ANTIGRAVITY_TOKEN_ENDPOINT.to_string(),
            clock: Arc::new(system_now_secs),
            save_path: None,
            on_refresh: None,
            last_payload: Arc::new(std::sync::Mutex::new(None)),
            signatures: Arc::new(std::sync::Mutex::new(SignatureStore::new())),
            models_cache: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Sobrescreve o endpoint de token (testes e ambientes isolados).
    pub fn with_token_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.token_endpoint = endpoint.into();
        self
    }

    fn now_secs(&self) -> u64 {
        (self.clock)()
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

    async fn ensure_auth(&self, force: bool) -> Result<(String, String), ProviderError> {
        // `force` (retry pos-401 de quem nao guarda o token): renova a nao ser
        // que outra task ja tenha trocado o token desde a leitura.
        let stale = if force {
            Some(self.credentials.lock().await.access_token.clone())
        } else {
            None
        };
        self.ensure_auth_after(stale).await
    }

    /// Resolve o token da conta selecionada segurando o lock de auth ate o fim.
    ///
    /// `stale_token` e o token que tomou 401: se uma task concorrente ja
    /// renovou, o token corrente difere do que falhou e este request apenas
    /// reaproveita o novo, sem segundo refresh com o mesmo refresh token.
    async fn ensure_auth_after(
        &self,
        stale_token: Option<String>,
    ) -> Result<(String, String), ProviderError> {
        // O lock e o ponto de serializacao: a primeira task renova e a segunda
        // acorda aqui com as credenciais ja renovadas.
        let mut creds = self.credentials.lock().await;
        let now_secs = self.now_secs();
        let mut changed = false;

        let retry_after_401 = stale_token.as_deref() == Some(creds.access_token.as_str());
        if retry_after_401 || creds.is_expired(now_secs) || creds.access_token.is_empty() {
            creds
                .refresh_if_expired(&self.client, &self.token_endpoint, now_secs, true)
                .await?;
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

/// Guarda o envelope do request que falhou junto do corpo cru do erro.
///
/// `last_payload` continua sendo o corpo enviado enquanto a chamada nao falha.
fn record_error_payload(slot: &std::sync::Mutex<Option<Value>>, envelope: &Value, raw: &str) {
    let payload = json!({
        "request": envelope,
        "error": serde_json::from_str::<Value>(raw).ok(),
        "raw": raw,
    });
    if let Ok(mut last) = slot.lock() {
        *last = Some(payload);
    }
}

struct StreamState {
    response: reqwest::Response,
    parser: SseParser,
    pending: VecDeque<String>,
    ready_events: VecDeque<ProviderEvent>,
    accumulated_calls: Vec<ToolCall>,
    last_usage: Option<(Option<u64>, Option<u64>, Option<u64>)>,
    started: bool,
    done: bool,
    failed: bool,
    /// Payloads não-JSON (keepalive/linha parcial) descartados neste turno.
    junk_payloads: usize,
}

/// Ultimo catalogo que o Hub devolveu para esta conta, com o instante da
/// resposta. Vale como fonte de entitlement mesmo depois de vencido: um
/// `fetchAvailableModels` que falha nao pode ressuscitar modelo bloqueado.
#[derive(Clone)]
struct ModelsCache {
    /// Catalogo na ordem do Hub (`agentModelSorts` / `tieredModelIds`).
    models: Vec<String>,
    fetched_at: u64,
}

/// Catalogo estatico restrito ao que a conta ja provou ter.
///
/// `observed` e o catalogo devolvido por `fetchAvailableModels` para esta conta
/// (mesmo vencido); modelo fora dele nao volta para a lista por fallback, senao
/// a UI oferece modelo que a conta nao tem entitlement de chamar. Sem nenhuma
/// resposta do servidor ainda (cold start), o catalogo estatico inteiro serve
/// de partida.
fn permitted_models(observed: Option<&[String]>) -> Vec<String> {
    let Some(observed) = observed else {
        return SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect();
    };
    let seen: BTreeSet<&str> = observed.iter().map(String::as_str).collect();
    let mut out: Vec<String> = SUPPORTED_MODELS
        .iter()
        .copied()
        .filter(|id| seen.contains(id))
        .map(str::to_string)
        .collect();
    // Modelo novo, que o catalogo estatico ainda nao conhece, nao pode sumir.
    out.extend(
        observed
            .iter()
            .filter(|id| !SUPPORTED_MODELS.contains(&id.as_str()))
            .cloned(),
    );
    out
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
        let now_secs = self.now_secs();
        let cached = self.models_cache.lock().ok().and_then(|slot| slot.clone());

        // Dentro do TTL a resposta sai do cache: `list_models` deixa de virar um
        // request por chamada (a referencia refaz a consulta a cada ~1h).
        if let Some(cache) = cached.as_ref() {
            if now_secs.saturating_sub(cache.fetched_at) < MODELS_CACHE_TTL_SECS {
                return Ok(cache.models.clone());
            }
        }

        // Fallback: catalogo estatico restrito ao que a conta ja devolveu, para
        // modelo sem entitlement nao reaparecer na lista.
        let fallback = || permitted_models(cached.as_ref().map(|cache| cache.models.as_slice()));
        let url = format!(
            "{}/v1internal:fetchAvailableModels",
            self.base_url.trim_end_matches('/')
        );

        // 401 com token que parecia valido: refaz o auth uma unica vez. Se a segunda
        // tentativa tambem falhar, a listagem cai no fallback como antes.
        let mut reauthenticated = false;
        loop {
            let Ok((access_token, project_id)) = self.ensure_auth(reauthenticated).await else {
                return Ok(fallback());
            };

            let body = if project_id.is_empty() {
                json!({})
            } else {
                json!({ "project": project_id })
            };

            let Ok(response) = self
                .client
                .post(url.clone())
                .header(AUTHORIZATION, format!("Bearer {access_token}"))
                .header(USER_AGENT, ANTIGRAVITY_USER_AGENT)
                .header(CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .await
            else {
                return Ok(fallback());
            };

            if response.status().as_u16() == 401 && !reauthenticated {
                reauthenticated = true;
                continue;
            }

            if !response.status().is_success() {
                return Ok(fallback());
            }

            let Ok(text) = response.text().await else {
                return Ok(fallback());
            };
            let models = parse_available_models(&text);
            if models.is_empty() {
                return Ok(fallback());
            }
            if let Ok(mut slot) = self.models_cache.lock() {
                *slot = Some(ModelsCache {
                    models: models.clone(),
                    fetched_at: now_secs,
                });
            }
            return Ok(models);
        }
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let url = format!(
            "{}/v1internal:streamGenerateContent?alt=sse",
            self.base_url.trim_end_matches('/')
        );

        // 401 com token que parecia valido (revogado, skew do relogio do Google):
        // refaz o auth uma unica vez e repete a chamada. Nenhum evento foi emitido
        // ainda, entao o consumidor ve uma resposta so.
        let mut reauthenticated = false;
        let (response, body) = loop {
            let (access_token, project_id) = self.ensure_auth(reauthenticated).await?;

            let effective_project_id = if project_id.is_empty() {
                ANTIGRAVITY_DEFAULT_PROJECT_ID
            } else {
                &project_id
            };

            let body = match self.signatures.lock() {
                Ok(signatures) => {
                    build_body_with_signatures(&request, effective_project_id, &signatures)
                }
                Err(_) => build_body(&request, effective_project_id),
            };
            if let Ok(mut last) = self.last_payload.lock() {
                *last = Some(body.clone());
            }

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
                .post(url.clone())
                .headers(headers)
                .json(&body)
                .send()
                .await
                .map_err(|e| ProviderError::Network(e.to_string()))?;

            if response.status().as_u16() == 401 && !reauthenticated {
                reauthenticated = true;
                continue;
            }

            if !response.status().is_success() {
                let status = response.status().as_u16();
                let raw = response.text().await.unwrap_or_default();
                // Envelope do request que falhou + corpo cru do erro: e o que sobrevive
                // para diagnosticar um 429 sem engolir o motivo do Google.
                record_error_payload(&self.last_payload, &body, &raw);
                return Err(ProviderError::Http { status, body: raw });
            }

            break (response, body);
        };

        let error_slot = Arc::clone(&self.last_payload);
        // Escopo/store das assinaturas desta sessao/modelo: as que vierem no
        // stream ficam guardadas para o proximo turno reenviar a real.
        let signature_scope = scope_of_envelope(&body);
        let signature_store = Arc::clone(&self.signatures);
        let envelope = body;
        let state = StreamState {
            response,
            parser: SseParser::new(),
            pending: VecDeque::new(),
            ready_events: VecDeque::new(),
            accumulated_calls: Vec::new(),
            last_usage: None,
            started: false,
            done: false,
            failed: false,
            junk_payloads: 0,
        };

        let stream = unfold(state, move |mut state| {
            let error_slot = Arc::clone(&error_slot);
            let envelope = envelope.clone();
            let signature_scope = signature_scope.clone();
            let signature_store = Arc::clone(&signature_store);
            async move {
            loop {
                if state.failed {
                    return None;
                }

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
                            if let Some(junk) = chunk.junk {
                                state.junk_payloads += 1;
                                tracing::debug!(
                                    junk_payloads = state.junk_payloads,
                                    amostra = %junk,
                                    "payload nao-JSON ignorado no stream do antigravity"
                                );
                                continue;
                            }
                            if let Some(scope) = signature_scope.as_deref()
                                && let Ok(mut store) = signature_store.lock()
                            {
                                // Assinatura real em cache substitui a sentinela
                                // no proximo turno; o resto vira fallback.
                                collect_signatures(&payload, scope, &mut store);
                            }
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
                        Err(err) => {
                            // Erro do Cloud Code no meio do stream: o consumidor precisa ver
                            // o ProviderError. Marca o estado como terminal para nao encerrar
                            // limpo nem emitir Usage/ToolCalls de um turno truncado.
                            record_error_payload(&error_slot, &envelope, &payload);
                            state.failed = true;
                            state.done = true;
                            state.last_usage = None;
                            state.accumulated_calls.clear();
                            return Some((Err(err), state));
                        }
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
                        if !state.started {
                            state.started = true;
                            state.ready_events.push_back(ProviderEvent::Started);
                        }
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

        // Sessao: dois turnos da mesma conversa compartilham o conversationId e
        // trocam de requestId; a sessao nova (outro pedido inicial) gera outro.
        let session_of = |body: &Value| body["conversationId"].as_str().unwrap().to_string();
        let request_of = |body: &Value| body["requestId"].as_str().unwrap().to_string();
        assert_eq!(session_of(&body), session_of(&body));

        let turn_two = ChatRequest::new(
            "antigravity-gemini-3.8-flash",
            vec![Message::user("ola"), Message::assistant("oi"), Message::user("e agora?")],
        );
        let body_two = build_body(&turn_two, "rising-fact-p41fc");
        assert_eq!(session_of(&body_two), session_of(&body));
        assert_ne!(request_of(&body_two), request_of(&body));
        // Retry do mesmo turno reenvia o mesmo requestId.
        assert_eq!(request_of(&build_body(&turn_two, "rising-fact-p41fc")), request_of(&body_two));

        let other_session =
            ChatRequest::new("antigravity-gemini-3.8-flash", vec![Message::user("outro pedido")]);
        let body_new = build_body(&other_session, "rising-fact-p41fc");
        assert_ne!(session_of(&body_new), session_of(&body));
    }

    #[test]
    fn body_carries_max_output_tokens() {
        let request = ChatRequest::new("gemini-3.8-flash-high", vec![Message::user("oi")])
            .with_max_tokens(900);
        let body = build_body(&request, "proj-1");

        assert_eq!(body["request"]["generationConfig"]["maxOutputTokens"], 900);
    }

    #[test]
    fn body_omits_max_output_tokens_when_unset() {
        let budget = |model: &str| {
            let request = ChatRequest::new(model, vec![Message::user("oi")]);
            let body = build_body(&request, "proj-1");
            body["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"].as_u64()
        };

        // Tier embutido no nome vira orcamento de thinking previsivel.
        assert_eq!(budget("gemini-3.8-flash-low"), Some(1_024));
        assert_eq!(budget("gemini-3.8-flash-medium"), Some(8_192));
        assert_eq!(budget("gemini-3.8-flash-high"), Some(24_576));
        assert_eq!(budget("antigravity-gemini-4-hydrogen-xhigh"), Some(32_768));
        assert_eq!(budget("claude-sonnet-5-5-thinking"), Some(32_768));
        // Modelo sem tier nao ganha thinkingConfig.
        assert_eq!(budget("gemini-2.5-pro"), None);

        // Sem tier e sem max_tokens nao existe generationConfig nenhum.
        let request = ChatRequest::new("gemini-2.5-pro", vec![Message::user("oi")]);
        let body = build_body(&request, "proj-1");
        assert!(body["request"].get("generationConfig").is_none());

        // Sem max_tokens, so o thinkingConfig vai no generationConfig.
        let request = ChatRequest::new("gemini-3.8-flash-high", vec![Message::user("oi")]);
        let body = build_body(&request, "proj-1");
        assert!(
            body["request"]["generationConfig"]
                .get("maxOutputTokens")
                .is_none()
        );
        assert_eq!(
            body["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            24_576
        );

        // Os dois convivem quando max_tokens existe.
        let request = ChatRequest::new("gemini-3.8-flash-high", vec![Message::user("oi")])
            .with_max_tokens(900);
        let body = build_body(&request, "proj-1");
        assert_eq!(body["request"]["generationConfig"]["maxOutputTokens"], 900);
        assert!(
            body["request"]["generationConfig"]["thinkingConfig"]["includeThoughts"]
                .as_bool()
                .unwrap_or(false)
        );
    }

    #[tokio::test]
    async fn stream_chat_sends_interleaved_thinking_beta_and_output_floor_for_claude() {
        let sse = concat!(
            "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]}}]}}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, server) = spawn_http_sequence(vec![(200, "text/event-stream", sse)]).await;
        let provider =
            AntigravityProvider::new(creds_expiring_at(u64::MAX)).with_base_url(base_url);
        let mut stream = provider
            .stream_chat(
                ChatRequest::new(
                    "antigravity-claude-sonnet-5-5-thinking",
                    vec![Message::user("oi")],
                )
                .with_max_tokens(900),
            )
            .await
            .unwrap();
        while let Some(item) = stream.next().await {
            item.unwrap();
        }
        let seen = server.await.unwrap();
        assert!(
            seen[0]
                .to_ascii_lowercase()
                .contains("anthropic-beta: interleaved-thinking-2025-05-14"),
            "header anthropic-beta ausente no request Claude: {}",
            seen[0]
        );
        assert!(
            seen[0].contains("\"maxOutputTokens\":64000"),
            "maxOutputTokens deveria subir ao piso para Claude: {}",
            seen[0]
        );

        // Gemini nao carrega o header nem herda o piso de saida.
        let (base_url, server) = spawn_http_sequence(vec![(200, "text/event-stream", sse)]).await;
        let provider =
            AntigravityProvider::new(creds_expiring_at(u64::MAX)).with_base_url(base_url);
        let mut stream = provider
            .stream_chat(
                ChatRequest::new("gemini-3.8-flash-high", vec![Message::user("oi")])
                    .with_max_tokens(900),
            )
            .await
            .unwrap();
        while let Some(item) = stream.next().await {
            item.unwrap();
        }
        let seen = server.await.unwrap();
        assert!(!seen[0].to_ascii_lowercase().contains("anthropic-beta"));
        assert!(seen[0].contains("\"maxOutputTokens\":900"));
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
        // A instrucao do Antigravity abre as partes; a system do tupi vem depois.
        assert_eq!(
            body["request"]["systemInstruction"]["parts"][0]["text"],
            ANTIGRAVITY_SYSTEM_INSTRUCTION
        );
        assert_eq!(
            body["request"]["systemInstruction"]["parts"][1]["text"],
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
                ANTIGRAVITY_SYSTEM_INSTRUCTION,
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
    fn empty_tool_schema_becomes_valid_placeholder_declaration() {
        // Schema vazio, objeto sem properties e properties vazio: os tres davam
        // INVALID_ARGUMENT no Google, porque OBJECT sem properties nao existe.
        let schemas = [
            json!({}),
            json!({ "type": "object" }),
            json!({ "type": "object", "properties": {} }),
        ];
        for schema in schemas {
            let spec = ToolSpec {
                name: "ping".into(),
                description: "sem argumentos".into(),
                schema: schema.clone(),
            };
            let request = ChatRequest::new("gemini-3.8-flash-high", vec![Message::user("ping")])
                .with_tools(vec![spec]);
            let body = build_body(&request, "rising-fact-p41fc");

            let params = &body["request"]["tools"][0]["functionDeclarations"][0]["parameters"];
            assert_eq!(params["type"], "object", "schema {schema}");
            let props = params["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("properties ausente no schema {schema}"));
            let placeholder = props
                .get(EMPTY_SCHEMA_PLACEHOLDER_NAME)
                .unwrap_or_else(|| panic!("placeholder ausente no schema {schema}"));
            assert_eq!(placeholder["type"], "boolean");
            assert!(placeholder["description"].is_string());
            // `required` nao pode apontar para propriedades que nao existem.
            assert!(params.get("required").is_none(), "schema {schema}");
        }
    }

    #[test]
    fn array_schema_without_items_gets_string_items() {
        let cleaned = clean_gemini_schema(&json!({ "type": "array" }));
        assert_eq!(cleaned["items"], json!({ "type": "string" }));

        // `items` vazio derruba o array do mesmo jeito.
        let cleaned = clean_gemini_schema(&json!({ "type": "array", "items": {} }));
        assert_eq!(cleaned["items"], json!({ "type": "string" }));

        // Aninhado num parametro a regra vale igual; `items` declarado fica.
        let cleaned = clean_gemini_schema(&json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array" },
                "ids": { "type": "array", "items": { "type": "integer" } }
            }
        }));
        assert_eq!(
            cleaned["properties"]["tags"]["items"],
            json!({ "type": "string" })
        );
        assert_eq!(
            cleaned["properties"]["ids"]["items"],
            json!({ "type": "integer" })
        );
    }

    #[test]
    fn body_keeps_tools_without_generation_config_for_plain_model() {
        let spec = ToolSpec {
            name: "read_file".into(),
            description: "le um arquivo".into(),
            schema: json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        };
        // Modelo sem tier de thinking e request sem max_tokens: as tools sobem,
        // mas nenhum generationConfig inventado vai junto.
        let request =
            ChatRequest::new("gemini-2.5-pro", vec![Message::user("leia")]).with_tools(vec![spec]);
        let body = build_body(&request, "rising-fact-p41fc");

        assert!(body["request"].get("tools").is_some());
        assert!(body["request"].get("generationConfig").is_none());
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
    fn body_groups_tool_results_and_drops_orphan() {
        let call1 = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into(),
        };
        let call2 = ToolCall {
            id: "call_2".into(),
            name: "grep".into(),
            arguments: r#"{"pattern":"b"}"#.into(),
        };
        let request = ChatRequest::new(
            "claude-sonnet-5-5-high",
            vec![
                Message::user("leia e busque"),
                Message::assistant_with_calls("", vec![call1, call2]),
                Message::tool("call_1", "conteudo A"),
                // Orfao no meio do turno: nao pode virar turno proprio nem
                // quebrar o agrupamento dos vizinhos.
                Message::tool("call_desconhecido", "resposta sem functionCall"),
                Message::tool("call_2", "match B"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        assert_eq!(contents[2]["role"], "user");
        let parts = contents[2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["functionResponse"]["id"], "call_1");
        assert_eq!(parts[0]["functionResponse"]["name"], "read_file");
        assert_eq!(parts[1]["functionResponse"]["id"], "call_2");
        assert_eq!(parts[1]["functionResponse"]["name"], "grep");
        // Nenhuma functionResponse referencia o id orfao.
        assert!(
            parts
                .iter()
                .all(|p| p["functionResponse"]["id"] != json!("call_desconhecido"))
        );
    }

    #[test]
    fn body_drops_tool_result_without_calls_at_all() {
        let request = ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![
                Message::user("ola"),
                Message::tool("call_perdido", "resultado solto"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "ola");
    }

    #[test]
    fn body_repairs_divergent_tool_response_id() {
        let call = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into(),
        };
        let request = ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![
                Message::user("leia o arquivo"),
                Message::assistant_with_calls("", vec![call]),
                // Id que nao casa com nenhum functionCall: o par e reparado com
                // o id da chamada aberta em vez de subir quebrado.
                Message::tool("toolu_01desconhecido", "conteudo A"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        let parts = contents[2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["functionResponse"]["id"], "call_1");
        assert_eq!(parts[0]["functionResponse"]["name"], "read_file");
        assert_eq!(
            parts[0]["functionResponse"]["response"]["output"],
            "conteudo A"
        );
        // Par valido: o functionResponse carrega o id do functionCall.
        assert_eq!(
            parts[0]["functionResponse"]["id"],
            contents[1]["parts"][0]["functionCall"]["id"]
        );
        assert!(serde_json::to_string(&body).is_ok());
    }

    #[test]
    fn body_repairs_empty_tool_response_id_without_duplicating_pairs() {
        let call1 = ToolCall {
            id: "call_1".into(),
            name: "read_file".into(),
            arguments: r#"{"path":"a.txt"}"#.into(),
        };
        let call2 = ToolCall {
            id: "call_2".into(),
            name: "grep".into(),
            arguments: r#"{"pattern":"b"}"#.into(),
        };
        let request = ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![
                Message::user("leia e busque"),
                Message::assistant_with_calls("", vec![call1, call2]),
                // Id vazio e depois id repetido: ambos caem na chamada aberta
                // na ordem original, sem duplicar par.
                Message::tool("", "conteudo A"),
                Message::tool("call_1", "match B"),
            ],
        );
        let body = build_body(&request, "rising-fact-p41fc");

        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3);
        let parts = contents[2]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["functionResponse"]["id"], "call_1");
        assert_eq!(parts[0]["functionResponse"]["name"], "read_file");
        assert_eq!(parts[1]["functionResponse"]["id"], "call_2");
        assert_eq!(parts[1]["functionResponse"]["name"], "grep");
        // Nenhum id repetido: cada functionCall tem exatamente um par.
        let mut ids: Vec<&str> = parts
            .iter()
            .filter_map(|p| p["functionResponse"]["id"].as_str())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids, vec!["call_1", "call_2"]);
        assert!(serde_json::to_string(&body).is_ok());
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
    fn chunk_treats_non_json_payload_as_ignorable_keepalive() {
        let chunk = extract_chunk("keepalive").unwrap();
        assert!(chunk.junk.is_some());
        assert!(chunk.text.is_none());
        assert!(chunk.tool_calls.is_empty());
        assert!(chunk.prompt_tokens.is_none());
    }

    #[tokio::test]
    async fn stream_chat_ignores_keepalive_payloads_and_delivers_valid_events() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let sse = concat!(
                "data: keepalive\n\n",
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ola\"}]}}]}}\n\n",
                "data: ping\n\n",
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" mundo\"}]}}]}}\n\n",
                "data: :heartbeat\n\n",
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
            events.push(item.expect("keepalive nao pode virar erro do provedor"));
        }
        assert_eq!(
            events,
            vec![
                ProviderEvent::Started,
                ProviderEvent::Text("ola".to_string()),
                ProviderEvent::Text(" mundo".to_string()),
            ]
        );
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
    fn is_expired_uses_five_minute_skew_plus_jitter() {
        let creds = AntigravityCredentials {
            access_token: "tok".into(),
            refresh_token: "ref".into(),
            expires_at: 1000,
            project_id: "p".into(),
            email: None,
        };

        let margin = refresh_margin_secs(&creds);
        assert!((REFRESH_SKEW_SECS..=REFRESH_SKEW_SECS + REFRESH_JITTER_SECS).contains(&margin));
        assert_eq!(margin, refresh_margin_secs(&creds), "jitter deterministico");

        // um segundo antes da janela: ainda vale
        assert!(!creds.is_expired(1000 - margin - 1));
        // na borda da janela: renova antes do request
        assert!(creds.is_expired(1000 - margin));
        // depois do vencimento: renova
        assert!(creds.is_expired(1050));
    }

    /// Sobe um servidor de token local; devolve (endpoint, handle com o POST cru).
    async fn spawn_token_server(token: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = format!("{{\"access_token\":\"{token}\",\"expires_in\":3600}}");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            request
        });
        (format!("http://{addr}/token"), handle)
    }

    fn creds_expiring_at(expires_at: u64) -> AntigravityCredentials {
        AntigravityCredentials {
            access_token: "old-token".into(),
            refresh_token: "1//refresh".into(),
            expires_at,
            project_id: "proj-1".into(),
            email: Some("dev@example.com".into()),
        }
    }

    #[tokio::test]
    async fn token_with_four_minutes_left_is_refreshed_before_request() {
        let now = 1_000_000u64;
        let (endpoint, server) = spawn_token_server("new-token").await;
        let mut provider =
            AntigravityProvider::new(creds_expiring_at(now + 240)).with_token_endpoint(endpoint);
        provider.clock = Arc::new(move || now);

        let (token, project_id) = provider.ensure_auth(false).await.unwrap();
        assert_eq!(token, "new-token");
        assert_eq!(project_id, "proj-1");

        let request = server.await.unwrap();
        assert!(request.starts_with("POST /token"));
        assert!(request.contains("grant_type=refresh_token"));
    }

    #[tokio::test]
    async fn force_auth_refreshes_valid_token() {
        let now = 2_000_000u64;
        let (endpoint, server) = spawn_token_server("forced-token").await;
        let mut provider =
            AntigravityProvider::new(creds_expiring_at(u64::MAX)).with_token_endpoint(endpoint);
        provider.clock = Arc::new(move || now);

        let (token, _) = provider.ensure_auth(true).await.unwrap();
        assert_eq!(token, "forced-token");
        assert!(server.await.unwrap().contains("grant_type=refresh_token"));
    }

    #[tokio::test]
    async fn ensure_auth_skips_refresh_outside_the_skew_window() {
        let now = 3_000_000u64;
        let mut provider = AntigravityProvider::new(creds_expiring_at(now + 3600))
            .with_token_endpoint("http://127.0.0.1:1/token");
        provider.clock = Arc::new(move || now);

        let (token, _) = provider.ensure_auth(false).await.unwrap();
        assert_eq!(token, "old-token");
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
                ProviderEvent::Started,
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
                ProviderEvent::Started,
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

    #[tokio::test]
    async fn stream_chat_surfaces_mid_stream_error_after_text() {
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
                "data: {\"error\":{\"code\":429,\"status\":\"RESOURCE_EXHAUSTED\",\"message\":\"Resource has been exhausted (e.g. check quota).\"}}\n\n",
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
        let mut error = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => events.push(event),
                Err(err) => {
                    error = Some(err);
                    break;
                }
            }
        }

        // Depois do erro o stream precisa acabar: nenhum Usage/ToolCalls de um turno
        // truncado pode sair como se a resposta tivesse terminado bem.
        assert!(
            stream.next().await.is_none(),
            "stream continuou depois do erro mid-stream"
        );

        assert_eq!(
            events,
            vec![ProviderEvent::Started, ProviderEvent::Text("ola".to_string())]
        );

        let described = format!("{error:?}");
        assert!(
            matches!(
                error.as_ref(),
                Some(ProviderError::RateLimited { status: 429, .. })
                    | Some(ProviderError::QuotaExhausted { status: 429, .. })
            ),
            "erro mid-stream precisa subir como ProviderError visivel, veio {described}"
        );
    }

    /// Regressao T2.3: numa unica sessao o Hub intercala keepalive, manda um
    /// evento com JSON truncado e so entao o erro final. Nada disso pode virar
    /// `ProviderError::Protocol` no meio do texto, e o erro do Cloud Code tem de
    /// subir para o consumidor em vez de ser engolido.
    #[tokio::test]
    async fn stream_chat_tolerates_keepalive_and_truncated_events_then_surfaces_final_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let sse = concat!(
                // keepalive: comentario SSE e payload nao-JSON que o Hub manda em ping
                ": keepalive\n\n",
                "data: ping\n\n",
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ola\"}]}}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":1}}}\n\n",
                // evento truncado: JSON cortado no meio, fechado pela linha em branco
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\" mun\n\n",
                // erro final do Cloud Code, depois do lixo todo
                "data: {\"error\":{\"code\":429,\"status\":\"RESOURCE_EXHAUSTED\",\"message\":\"Resource has been exhausted (e.g. check quota).\"}}\n\n",
                // sobra de linha parcial sem linha em branco: nunca fecha evento
                "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"nunca"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.flush().await;
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
        let mut error = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => events.push(event),
                Err(err) => {
                    error = Some(err);
                    break;
                }
            }
        }

        // Keepalive e JSON truncado sao descartados: o texto do turno chega
        // inteiro e o lixo nao aparece como erro de protocolo.
        assert_eq!(
            events,
            vec![ProviderEvent::Started, ProviderEvent::Text("ola".to_string())],
            "keepalive/evento truncado nao podem derrubar o turno"
        );

        // O turno termina no erro final, sem Usage de uma resposta truncada.
        assert!(
            stream.next().await.is_none(),
            "stream continuou depois do erro final"
        );

        let described = format!("{error:?}");
        assert!(
            matches!(
                error.as_ref(),
                Some(ProviderError::RateLimited { status: 429, .. })
                    | Some(ProviderError::QuotaExhausted { status: 429, .. })
            ),
            "erro final precisa subir como ProviderError visivel, veio {described}"
        );

        // O corpo cru do erro fica registrado para diagnostico, como no 429 de HTTP.
        let payload = provider.last_payload().expect("erro final registrado");
        let raw = payload
            .get("raw")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            raw.contains("RESOURCE_EXHAUSTED"),
            "payload registrado perdeu o erro do Google: {payload}"
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

    /// Sobe um servidor HTTP que responde uma vez por conexao, na ordem dada, e
    /// devolve o request cru de cada tentativa para conferir o Bearer usado.
    async fn spawn_http_sequence(
        responses: Vec<(u16, &'static str, &'static str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, content_type, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                seen.push(String::from_utf8_lossy(&buf[..n]).to_string());
                let reason = match status {
                    200 => "OK",
                    401 => "Unauthorized",
                    _ => "Error",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            }
            seen
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn stream_chat_reauthenticates_once_on_401_and_delivers_single_response() {
        let now = 4_000_000u64;
        let (token_endpoint, token_server) = spawn_token_server("fresh-token").await;
        let sse = concat!(
            "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]}}]}}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, server) = spawn_http_sequence(vec![
            (
                401,
                "application/json",
                "{\"error\":{\"code\":401,\"message\":\"invalid authentication credentials\"}}",
            ),
            (200, "text/event-stream", sse),
        ])
        .await;

        let mut provider = AntigravityProvider::new(creds_expiring_at(now + 3600))
            .with_base_url(base_url)
            .with_token_endpoint(token_endpoint);
        provider.clock = Arc::new(move || now);

        let mut stream = provider
            .stream_chat(ChatRequest::new(
                "gemini-3.8-flash-high",
                vec![Message::user("oi")],
            ))
            .await
            .expect("401 seguido de 200 precisa virar resposta");

        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.expect("a segunda tentativa nao pode falhar"));
        }
        assert_eq!(
            events,
            vec![ProviderEvent::Started, ProviderEvent::Text("ok".to_string())],
            "o retry nao pode duplicar eventos para o consumidor"
        );

        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 2, "401 precisa de exatamente uma nova tentativa");
        assert!(
            seen[0].contains("Bearer old-token"),
            "primeira tentativa usou outro token: {}",
            seen[0]
        );
        assert!(
            seen[1].contains("Bearer fresh-token"),
            "retry precisa sair com o token renovado: {}",
            seen[1]
        );
        assert!(token_server.await.unwrap().contains("grant_type=refresh_token"));
    }

    #[tokio::test]
    async fn stream_chat_surfaces_401_after_a_single_reauth() {
        let now = 5_000_000u64;
        let (token_endpoint, token_server) = spawn_token_server("fresh-token").await;
        let unauthorized =
            "{\"error\":{\"code\":401,\"message\":\"invalid authentication credentials\"}}";
        let (base_url, server) = spawn_http_sequence(vec![
            (401, "application/json", unauthorized),
            (401, "application/json", unauthorized),
        ])
        .await;

        let mut provider = AntigravityProvider::new(creds_expiring_at(now + 3600))
            .with_base_url(base_url)
            .with_token_endpoint(token_endpoint);
        provider.clock = Arc::new(move || now);

        let err = match provider
            .stream_chat(ChatRequest::new(
                "gemini-3.8-flash-high",
                vec![Message::user("oi")],
            ))
            .await
        {
            Ok(_) => panic!("401 repetido precisa subir como ProviderError"),
            Err(err) => err,
        };
        assert!(
            matches!(err, ProviderError::Http { status: 401, .. }),
            "erro final precisa ser visivel ao usuario, veio {err:?}"
        );

        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 2, "401 nao pode entrar em retry infinito");
        assert!(token_server.await.unwrap().contains("grant_type=refresh_token"));
    }

    #[tokio::test]
    async fn list_models_reauthenticates_once_on_401() {
        let now = 6_000_000u64;
        let (token_endpoint, token_server) = spawn_token_server("fresh-token").await;
        let (base_url, server) = spawn_http_sequence(vec![
            (401, "application/json", "{\"error\":{\"code\":401}}"),
            (
                200,
                "application/json",
                "{\"models\":{\"gemini-4-hydrogen-high\":{}}}",
            ),
        ])
        .await;

        let mut provider = AntigravityProvider::new(creds_expiring_at(now + 3600))
            .with_base_url(base_url)
            .with_token_endpoint(token_endpoint);
        provider.clock = Arc::new(move || now);

        let models = provider.list_models().await.unwrap();
        assert_eq!(
            models,
            vec!["gemini-4-hydrogen-high".to_string()],
            "retry do 401 precisa devolver o catalogo real, nao o fallback estatico"
        );

        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 2, "401 precisa de exatamente uma nova tentativa");
        assert!(
            seen[1].contains("Bearer fresh-token"),
            "retry precisa sair com o token renovado: {}",
            seen[1]
        );
        assert!(token_server.await.unwrap().contains("grant_type=refresh_token"));
    }

    #[test]
    fn permitted_models_keeps_static_order_and_appends_unknown_models() {
        let statics: Vec<String> = SUPPORTED_MODELS.iter().map(|s| s.to_string()).collect();
        assert_eq!(permitted_models(None), statics);
        let observed = vec!["gemini-5-ultra".to_string(), "gemini-3.8-flash-high".to_string()];
        assert_eq!(
            permitted_models(Some(&observed)),
            vec!["gemini-3.8-flash-high".to_string(), "gemini-5-ultra".to_string()]
        );
    }

    #[tokio::test]
    async fn list_models_serves_catalog_from_cache_within_ttl() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (hits, mut seen) = tokio::sync::mpsc::unbounded_channel::<()>();

        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"models":{"gemini-4-hydrogen-high":{},"gemini-3.8-flash-high":{}}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                if hits.send(()).is_err() {
                    break;
                }
            }
        });

        let mut provider = AntigravityProvider::new(creds_expiring_at(u64::MAX))
            .with_base_url(format!("http://{addr}"));
        let now = 1_000_000u64;
        provider.clock = Arc::new(move || now);

        let first = provider.list_models().await.unwrap();
        let expected = vec!["gemini-3.8-flash-high".to_string(), "gemini-4-hydrogen-high".to_string()];
        assert_eq!(first, expected);
        assert!(seen.recv().await.is_some(), "primeira chamada consulta o Hub");

        let second = provider.list_models().await.unwrap();
        assert_eq!(second, first, "dentro do TTL o catalogo vem do cache");
        let again = tokio::time::timeout(std::time::Duration::from_millis(200), seen.recv()).await;
        assert!(again.is_err(), "a segunda chamada nao pode gerar request HTTP");
    }

    #[tokio::test]
    async fn list_models_filters_static_catalog_by_entitlement_after_failure() {
        let (base_url, server) = spawn_http_sequence(vec![
            (200, "application/json", r#"{"models":{"gemini-4-hydrogen-high":{}}}"#),
            (500, "application/json", r#"{"error":{"code":500}}"#),
        ])
        .await;

        let now = Arc::new(AtomicU64::new(1_000_000));
        let mut provider = AntigravityProvider::new(creds_expiring_at(u64::MAX))
            .with_base_url(base_url);
        provider.clock = Arc::new({
            let now = now.clone();
            move || now.load(Ordering::Relaxed)
        });

        let fetched = provider.list_models().await.unwrap();
        assert_eq!(fetched, vec!["gemini-4-hydrogen-high".to_string()]);

        now.store(1_000_000 + MODELS_CACHE_TTL_SECS + 1, Ordering::Relaxed);
        let after_failure = provider.list_models().await.unwrap();
        assert_eq!(
            after_failure,
            vec!["gemini-4-hydrogen-high".to_string()],
            "conta sem entitlement nao ve modelo bloqueado no fallback"
        );
        assert_eq!(server.await.unwrap().len(), 2, "TTL vencido refaz a consulta");
    }

    mod fingerprint_tests;
}
