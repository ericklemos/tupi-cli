//! Provedor `openai-plan`: assinatura ChatGPT (Plus/Pro/Team) via OAuth.
//!
//! O backend de assinatura fala a Responses API (`{base}/responses`), não Chat
//! Completions, e autentica com o access token do login OAuth no navegador mais
//! o `chatgpt-account-id` extraído do `id_token`. O fluxo é PKCE público: não
//! existe `client_secret` no binário nem no repositório.
//!
//! Todas as constantes de rede ficam neste bloco porque os valores do cliente
//! oficial da OpenAI não são documentados para terceiros: cada uma tem default
//! razoável e pode ser sobrescrita por env (`TUPI_OPENAI_*`), sem recompilar.

use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use futures_util::stream::unfold;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use tupi_domain::{Message, Role};
use tupi_kernel::{ChatRequest, Provider, ProviderError, ProviderEvent, ProviderStream, ToolSpec};

use crate::deepseek::ToolCallAccumulator;
use crate::sse::{SseParser, ToolCallDelta};

/// Identificador do provedor no registro, na config e no redb.
pub const PROVIDER_ID: &str = "openai-plan";

/// Client id público do cliente oficial. Sem `client_secret`: PKCE público.
const DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const DEFAULT_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const DEFAULT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const DEFAULT_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
/// Endereço do listener local que recebe o retorno do login; precisa casar com
/// o `redirect_uri` acima.
const DEFAULT_CALLBACK_ADDR: &str = "127.0.0.1:1455";
const DEFAULT_ORIGINATOR: &str = "codex_cli_rs";

/// Catálogo embutido: só entra em cena quando a pergunta ao backend falha (ou
/// quando `TUPI_OPENAI_MODELS` fixa a lista). A fonte de verdade é o servidor —
/// regra do projeto: provedor sempre pergunta quais modelos a conta tem.
const DEFAULT_MODELS: &[&str] = &[
    "gpt-6.2-sol",
    "gpt-6.2-astra",
    "gpt-6.1-sol",
    "gpt-6-sol",
    "gpt-5.2-codex",
    "gpt-5.1-codex",
    "gpt-5.1-codex-mini",
    "gpt-5.1",
    "gpt-5-codex",
    "gpt-5",
    "codex-mini-latest",
];

const ENV_CLIENT_ID: &str = "TUPI_OPENAI_CLIENT_ID";
const ENV_AUTHORIZE_URL: &str = "TUPI_OPENAI_AUTHORIZE_URL";
const ENV_TOKEN_URL: &str = "TUPI_OPENAI_TOKEN_URL";
const ENV_BASE_URL: &str = "TUPI_OPENAI_BASE_URL";
const ENV_REDIRECT_URI: &str = "TUPI_OPENAI_REDIRECT_URI";
const ENV_CALLBACK_ADDR: &str = "TUPI_OPENAI_CALLBACK_ADDR";
const ENV_ORIGINATOR: &str = "TUPI_OPENAI_ORIGINATOR";
const ENV_MODELS: &str = "TUPI_OPENAI_MODELS";
/// Override do endpoint de catálogo: o caminho padrão não foi verificado, então
/// ele precisa ser corrigível sem rebuild.
const ENV_MODELS_URL: &str = "TUPI_OPENAI_MODELS_URL";
/// Caminho do catálogo dentro do `base_url`.
const DEFAULT_MODELS_PATH: &str = "models";

/// Escopos do login: `openid`/`profile`/`email` trazem o `id_token` que carrega
/// a conta ChatGPT, e `offline_access` é o que devolve o `refresh_token`.
const DEFAULT_SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];
const ENV_SCOPES: &str = "TUPI_OPENAI_SCOPES";

/// Claim do `id_token` que carrega os dados da conta ChatGPT.
const AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// Folga de renovação do access token: renova 5 min antes do vencimento.
const REFRESH_SKEW_SECS: u64 = 300;

/// Nome do arquivo de fallback, usado quando o redb não tem as credenciais.
const CREDENTIALS_FILE: &str = "openai-plan.json";

fn env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn network_constant(env_key: &str, default: &str) -> String {
    env_value(env_key).unwrap_or_else(|| default.to_string())
}

pub fn client_id() -> String {
    network_constant(ENV_CLIENT_ID, DEFAULT_CLIENT_ID)
}

pub fn authorize_url() -> String {
    network_constant(ENV_AUTHORIZE_URL, DEFAULT_AUTHORIZE_URL)
}

pub fn token_url() -> String {
    network_constant(ENV_TOKEN_URL, DEFAULT_TOKEN_URL)
}

pub fn base_url() -> String {
    network_constant(ENV_BASE_URL, DEFAULT_BASE_URL)
}

pub fn redirect_uri() -> String {
    network_constant(ENV_REDIRECT_URI, DEFAULT_REDIRECT_URI)
}

pub fn originator() -> String {
    network_constant(ENV_ORIGINATOR, DEFAULT_ORIGINATOR)
}

pub fn models() -> Vec<String> {
    match env_value(ENV_MODELS) {
        Some(raw) => {
            let parsed: Vec<String> = raw
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
            if parsed.is_empty() {
                default_models()
            } else {
                parsed
            }
        }
        None => default_models(),
    }
}

fn default_models() -> Vec<String> {
    DEFAULT_MODELS.iter().map(|model| model.to_string()).collect()
}

/// URL do catálogo de modelos do backend: `{base}/models`, salvo override em
/// `TUPI_OPENAI_MODELS_URL`. O caminho não foi verificado contra o servidor
/// real, então ele existe justamente para poder ser corrigido sem rebuild.
pub fn models_url_for(base: &str) -> String {
    env_value(ENV_MODELS_URL)
        .unwrap_or_else(|| format!("{}/{}", base.trim_end_matches('/'), DEFAULT_MODELS_PATH))
}

/// Extrai os ids de um catálogo remoto, tolerante ao formato: `{"models":[{…}]}`
/// (formato do cliente oficial), `{"data":[{…}]}` (formato OpenAI list) ou uma
/// lista de strings. Cada entrada pode nomear o modelo em `slug`, `id` ou
/// `model`. Devolve `None` quando o corpo não é JSON de catálogo.
pub fn extract_model_ids(body: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let entries = value
        .get("models")
        .or_else(|| value.get("data"))
        .or_else(|| value.get("items"))
        .cloned()
        .unwrap_or(value);
    let array = entries.as_array()?;

    let mut ids: Vec<String> = Vec::new();
    for entry in array {
        let id = entry.as_str().map(str::to_string).or_else(|| {
            ["slug", "id", "model"]
                .iter()
                .find_map(|key| entry.get(*key).and_then(Value::as_str))
                .map(str::to_string)
        });
        if let Some(id) = id.map(|id| id.trim().to_string())
            && !id.is_empty()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    Some(ids)
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

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

fn base64_url_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

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

/// `code_challenge` do PKCE (RFC 7636, método S256): SHA-256 em base64url sem
/// padding, do mesmo jeito que o `antigravity` monta o seu.
pub fn pkce_challenge(verifier: &str) -> String {
    use sha2::{Digest, Sha256};
    base64_url_encode(&Sha256::digest(verifier.as_bytes()))
}

/// Verifier do PKCE, entre 43 e 128 caracteres. Não há `rand` nas dependências:
/// o valor nasce de blake3 sobre o relógio, como em `antigravity`.
pub fn generate_verifier() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}{}",
        blake3::hash(&now.to_le_bytes()).to_hex(),
        blake3::hash(b"tupi-openai-plan-pkce").to_hex()
    )
}

/// `state` do OAuth, conferido no callback para barrar CSRF.
pub fn generate_state() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    blake3::hash(&now.to_be_bytes()).to_hex().to_string()
}

pub fn scopes() -> Vec<String> {
    match env_value(ENV_SCOPES) {
        Some(raw) => {
            let parsed: Vec<String> = raw
                .split([' ', ','])
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect();
            if parsed.is_empty() {
                default_scopes()
            } else {
                parsed
            }
        }
        None => default_scopes(),
    }
}

fn default_scopes() -> Vec<String> {
    DEFAULT_SCOPES.iter().map(|scope| scope.to_string()).collect()
}

pub fn scope_string() -> String {
    scopes().join(" ")
}

/// URL de autorização do navegador: PKCE público, então só `client_id`,
/// `redirect_uri`, escopos, o challenge S256 e o `state`.
pub fn build_authorization_url(verifier: &str, state: &str) -> String {
    format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}&originator={}",
        authorize_url(),
        url_encode(&client_id()),
        url_encode(&redirect_uri()),
        url_encode(&scope_string()),
        url_encode(&pkce_challenge(verifier)),
        url_encode(state),
        url_encode(&originator()),
    )
}

fn token_request_body(code: &str, verifier: &str) -> String {
    format!(
        "client_id={}&grant_type=authorization_code&code={}&redirect_uri={}&code_verifier={}",
        url_encode(&client_id()),
        url_encode(code),
        url_encode(&redirect_uri()),
        url_encode(verifier),
    )
}

/// Troca o `code` do callback pelas credenciais, batendo no `token_url()`
/// corrente (com o override de env valendo).
pub async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    verifier: &str,
    now_secs: u64,
) -> Result<OpenAiPlanCredentials, ProviderError> {
    exchange_code_at(client, &token_url(), code, verifier, now_secs).await
}

/// Igual a `exchange_code`, mas com o endpoint explícito: é o que os testes usam
/// para falar com um servidor local, sem tocar a rede.
pub async fn exchange_code_at(
    client: &reqwest::Client,
    token_endpoint: &str,
    code: &str,
    verifier: &str,
    now_secs: u64,
) -> Result<OpenAiPlanCredentials, ProviderError> {
    let response = client
        .post(token_endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(reqwest::header::ACCEPT, "application/json")
        .body(token_request_body(code, verifier))
        .send()
        .await
        .map_err(|error| ProviderError::Network(error.to_string()))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();

    // Falha de token é `Http` com status e corpo crus; o OAuth também pode
    // devolver `error` num 200, e aí o status real do corpo é o que importa.
    if !status.is_success() || has_oauth_error(&text) {
        return Err(ProviderError::Http {
            status: status.as_u16(),
            body: text,
        });
    }

    OpenAiPlanCredentials::from_token_response(&text, now_secs)
}

fn has_oauth_error(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|val| val.get("error").map(|error| !error.is_null()))
        .unwrap_or(false)
}

/// Corpo do pedido de renovação. PKCE público: `client_id` e mais nada de
/// segredo, porque não existe `client_secret` no binário.
fn refresh_request_body(refresh_token: &str) -> String {
    format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}",
        url_encode(&client_id()),
        url_encode(refresh_token),
    )
}

/// Renova as credenciais no `token_url()` corrente (com o override de env).
pub async fn refresh(
    client: &reqwest::Client,
    current: &OpenAiPlanCredentials,
    now_secs: u64,
) -> Result<OpenAiPlanCredentials, ProviderError> {
    refresh_at(client, &token_url(), current, now_secs).await
}

/// Igual a `refresh`, mas com o endpoint explícito: é o que os testes usam para
/// falar com um servidor local, sem tocar a rede.
///
/// O que a resposta não trouxer é herdado do valor corrente: o refresh pode
/// rotacionar o `refresh_token` e nem sempre devolve `id_token`, então conta e
/// e-mail sobrevivem à renovação.
pub async fn refresh_at(
    client: &reqwest::Client,
    token_endpoint: &str,
    current: &OpenAiPlanCredentials,
    now_secs: u64,
) -> Result<OpenAiPlanCredentials, ProviderError> {
    let refresh_token = current.refresh_token.trim();
    if refresh_token.is_empty() {
        return Err(ProviderError::Protocol(
            "refresh token ausente: refaça o login do openai-plan".to_string(),
        ));
    }

    let response = client
        .post(token_endpoint)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .header(reqwest::header::ACCEPT, "application/json")
        .body(refresh_request_body(refresh_token))
        .send()
        .await
        .map_err(|error| ProviderError::Network(error.to_string()))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();

    if !status.is_success() || has_oauth_error(&text) {
        return Err(ProviderError::Http {
            status: status.as_u16(),
            body: text,
        });
    }

    let mut renewed = OpenAiPlanCredentials::from_token_response(&text, now_secs)?;
    if renewed.refresh_token.is_empty() {
        renewed.refresh_token = current.refresh_token.clone();
    }
    if renewed.account_id.is_none() {
        renewed.account_id = current.account_id.clone();
    }
    if renewed.email.is_none() {
        renewed.email = current.email.clone();
    }

    Ok(renewed)
}

/// Destino da credencial renovada: o fallback em arquivo e o hook que grava no
/// redb (`with_on_refresh` em `tupi-app`).
pub type RefreshHook = std::sync::Arc<dyn Fn(&OpenAiPlanCredentials) + Send + Sync>;

/// Persiste a credencial nova nos dois destinos. Falha de arquivo só loga: o
/// token em memória segue válido e o próximo login reescreve o arquivo.
pub fn persist_credentials(
    credentials: &OpenAiPlanCredentials,
    save_path: Option<&Path>,
    on_refresh: Option<&RefreshHook>,
) {
    if let Some(path) = save_path
        && let Err(error) = credentials.save(path)
    {
        tracing::warn!(
            %error,
            path = %path.display(),
            "falha ao gravar credenciais do openai-plan"
        );
    }
    if let Some(hook) = on_refresh {
        hook(credentials);
    }
}

/// Credenciais da assinatura, persistidas como JSON no redb
/// (`store.provider_credentials(PROVIDER_ID)`) e, como fallback, em arquivo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiPlanCredentials {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    pub expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl OpenAiPlanCredentials {
    /// Janela efetiva de renovação: vence quando falta menos que o skew.
    pub fn is_expired(&self, now_secs: u64) -> bool {
        now_secs.saturating_add(REFRESH_SKEW_SECS) >= self.expires_at
    }

    /// Token ausente, JSON inválido ou campos obrigatórios faltando → `None`.
    pub fn from_json_str(text: &str) -> Option<Self> {
        let creds: Self = serde_json::from_str(text).ok()?;
        (!creds.access_token.trim().is_empty()).then_some(creds)
    }

    pub fn to_json_string(&self) -> Result<String, ProviderError> {
        serde_json::to_string_pretty(self).map_err(|error| {
            ProviderError::Protocol(format!("credenciais não serializáveis: {error}"))
        })
    }

    /// Caminho do fallback em arquivo dentro do diretório do tupi.
    pub fn path(tupi_dir: &Path) -> PathBuf {
        tupi_dir.join(CREDENTIALS_FILE)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, content)
    }

    /// Arquivo ausente, ilegível ou com JSON fora do formato → `None`.
    pub fn load_from_file(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        Self::from_json_str(&content)
    }

    pub fn load_from_dir(tupi_dir: &Path) -> Option<Self> {
        Self::load_from_file(&Self::path(tupi_dir))
    }

    /// Extrai credenciais de uma resposta de token (`access_token`,
    /// `refresh_token`, `id_token`, `expires_in`). O account id vem do JWT.
    pub fn from_token_response(payload: &str, now_secs: u64) -> Result<Self, ProviderError> {
        let val: Value = serde_json::from_str(payload)
            .map_err(|error| ProviderError::Protocol(format!("resposta de token inválida: {error}")))?;

        if let Some(error) = val.get("error").and_then(Value::as_str) {
            let desc = val
                .get("error_description")
                .and_then(Value::as_str)
                .unwrap_or(error);
            return Err(ProviderError::Protocol(format!("erro de autenticação: {desc}")));
        }

        let access_token = val
            .get("access_token")
            .and_then(Value::as_str)
            .and_then(non_empty)
            .ok_or_else(|| ProviderError::Protocol("access_token ausente".to_string()))?;

        let refresh_token = val
            .get("refresh_token")
            .and_then(Value::as_str)
            .and_then(non_empty)
            .unwrap_or_default();

        let expires_in = val
            .get("expires_in")
            .and_then(Value::as_u64)
            .filter(|secs| *secs > 0)
            .unwrap_or(3600);

        let claims = val
            .get("id_token")
            .and_then(Value::as_str)
            .and_then(jwt_claims);

        let account_id = claims.as_ref().and_then(account_id_from_claims);
        let email = claims.as_ref().and_then(email_from_claims);

        Ok(Self {
            access_token,
            refresh_token,
            expires_at: now_secs.saturating_add(expires_in),
            account_id,
            email,
        })
    }

    /// Renova a credencial vencida antes do uso e diz se o token foi trocado,
    /// para o chamador persistir (arquivo + redb) só quando houve renovação.
    ///
    /// `force` cobre o retry pós-401 de quem não guarda o token: o backend pode
    /// recusar um token que pelo relógio local ainda não venceu.
    pub async fn refresh_if_expired(
        &mut self,
        client: &reqwest::Client,
        token_endpoint: &str,
        now_secs: u64,
        force: bool,
    ) -> Result<bool, ProviderError> {
        if !force && !self.is_expired(now_secs) && !self.access_token.trim().is_empty() {
            return Ok(false);
        }

        let renewed = refresh_at(client, token_endpoint, self, now_secs).await?;
        let changed = *self != renewed;
        *self = renewed;

        Ok(changed)
    }
}

// ---------------------------------------------------------------------------
// Login OAuth: listener local, página devolvida ao navegador e abertura dele.
// ---------------------------------------------------------------------------

/// Espera máxima pelo callback no navegador antes de desistir do login.
const CALLBACK_TIMEOUT_SECS: u64 = 300;

/// Endereço do listener que espera o callback (sobrescrevível por
/// `TUPI_OPENAI_CALLBACK_ADDR`, e então precisa casar com o `redirect_uri`).
pub fn callback_addr() -> String {
    network_constant(ENV_CALLBACK_ADDR, DEFAULT_CALLBACK_ADDR)
}

/// Caminho do `redirect_uri` corrente (`/auth/callback` no default). O listener
/// só atende `GET` nesse caminho: favicon e afins não encerram a espera.
fn callback_path() -> String {
    let uri = redirect_uri();
    let rest = uri.split_once("://").map(|(_, rest)| rest).unwrap_or(&uri);
    let path = match rest.split_once('/') {
        Some((_, tail)) => format!("/{tail}"),
        None => "/".to_string(),
    };
    path.split('?').next().unwrap_or("/").to_string()
}

/// Alvo da linha de pedido HTTP (`/auth/callback?code=…`).
fn request_target(request: &str) -> Option<&str> {
    let line = request.lines().next()?;
    line.split_whitespace().nth(1)
}

fn target_path(target: &str) -> &str {
    target.split('?').next().unwrap_or(target)
}

fn query_value(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| percent_decode(value))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

/// `true` quando o pedido é para o caminho do callback; o resto recebe 404 e a
/// espera continua (navegador costuma pedir `/favicon.ico` junto).
pub fn is_callback_request(request: &str) -> bool {
    match request_target(request) {
        Some(target) => target_path(target) == callback_path(),
        None => false,
    }
}

/// Página HTML devolvida ao navegador ao fim do login. Nada do callback entra
/// aqui: é só o aviso de que o Tupi já pode voltar a receber comandos.
pub fn callback_page(success: bool) -> String {
    let message = if success {
        "<h2>Login concluído com sucesso!</h2><p>Você pode fechar esta aba e voltar ao Tupi.</p>"
    } else {
        "<h2>Não foi possível concluir o login.</h2><p>Volte ao Tupi e tente de novo.</p>"
    };

    format!(
        "<!DOCTYPE html><html lang=\"pt-BR\"><head><meta charset=\"utf-8\"><title>tupi · openai-plan</title></head><body style=\"font-family:sans-serif;text-align:center;padding:50px\">{message}</body></html>"
    )
}

fn http_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Extrai o `code` do `GET /auth/callback`. `state` diferente do gerado no
/// início, `error` do provedor ou ausência de `code` vira `Protocol`: o pedido
/// não veio de quem começou o login.
pub fn callback_code(request: &str, expected_state: &str) -> Result<String, ProviderError> {
    let target = request_target(request)
        .ok_or_else(|| ProviderError::Protocol("callback sem linha de pedido HTTP".to_string()))?;
    let query = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();

    if let Some(error) = query_value(query, "error") {
        let detail = query_value(query, "error_description").unwrap_or_default();
        let message = format!("login recusado: {error} {detail}");
        return Err(ProviderError::Protocol(message.trim_end().to_string()));
    }

    match query_value(query, "state") {
        Some(state) if state == expected_state => {}
        Some(_) => {
            return Err(ProviderError::Protocol(
                "state do callback não confere com o login iniciado".to_string(),
            ));
        }
        None => return Err(ProviderError::Protocol("callback sem state".to_string())),
    }

    query_value(query, "code")
        .filter(|code| !code.is_empty())
        .ok_or_else(|| {
            ProviderError::Protocol("código de autorização não encontrado no callback".to_string())
        })
}

/// Abre o listener do callback. Porta ocupada vira `Network` com o endereço na
/// mensagem, porque é a causa mais comum de o login nem começar.
pub async fn bind_callback_listener_at(
    addr: &str,
) -> Result<tokio::net::TcpListener, ProviderError> {
    tokio::net::TcpListener::bind(addr).await.map_err(|error| {
        ProviderError::Network(format!(
            "não foi possível abrir a porta do callback em {addr}: {error}"
        ))
    })
}

/// Igual a `bind_callback_listener_at`, no endereço de `callback_addr()`.
pub async fn bind_callback_listener() -> Result<tokio::net::TcpListener, ProviderError> {
    bind_callback_listener_at(&callback_addr()).await
}

async fn read_callback_request(
    socket: &mut tokio::net::TcpStream,
) -> Result<String, ProviderError> {
    use tokio::io::AsyncReadExt;

    let mut buffer = vec![0u8; 8192];
    let mut filled = 0;

    while filled < buffer.len() {
        let read = socket
            .read(&mut buffer[filled..])
            .await
            .map_err(|error| ProviderError::Network(format!("falha ao ler o callback: {error}")))?;
        if read == 0 {
            break;
        }
        filled += read;
        if buffer[..filled]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }

    Ok(String::from_utf8_lossy(&buffer[..filled]).into_owned())
}

async fn write_callback_response(socket: &mut tokio::net::TcpStream, status: &str, body: &str) {
    use tokio::io::AsyncWriteExt;

    let _ = socket.write_all(http_response(status, body).as_bytes()).await;
    let _ = socket.flush().await;
}

/// Espera o callback no listener e devolve o `code`. Pedido fora do caminho do
/// callback recebe 404 e a espera continua; `state` divergente ou `code`
/// ausente recebe a página de erro e vira `Protocol`.
pub async fn accept_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String, ProviderError> {
    loop {
        let (mut socket, _) = listener.accept().await.map_err(|error| {
            ProviderError::Network(format!("falha ao receber o callback: {error}"))
        })?;

        let request = read_callback_request(&mut socket).await?;

        if !is_callback_request(&request) {
            write_callback_response(&mut socket, "404 Not Found", &callback_page(false)).await;
            continue;
        }

        match callback_code(&request, expected_state) {
            Ok(code) => {
                write_callback_response(&mut socket, "200 OK", &callback_page(true)).await;
                return Ok(code);
            }
            Err(error) => {
                write_callback_response(&mut socket, "400 Bad Request", &callback_page(false))
                    .await;
                return Err(error);
            }
        }
    }
}

/// Comando do SO que abre a URL no navegador padrão, sem esperar por ele: o
/// navegador vive mais que o login.
fn browser_command(url: &str) -> Option<std::process::Command> {
    let mut cmd = {
        #[cfg(target_os = "linux")]
        {
            let mut cmd = std::process::Command::new("xdg-open");
            cmd.arg(url);
            cmd
        }

        #[cfg(target_os = "macos")]
        {
            let mut cmd = std::process::Command::new("open");
            cmd.arg(url);
            cmd
        }

        #[cfg(target_os = "windows")]
        {
            let mut cmd = std::process::Command::new("cmd");
            cmd.args(["/C", "start", "", url]);
            cmd
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            return None;
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

    Some(cmd)
}

/// Abre a URL de autorização no navegador do SO. `false` quando não deu para
/// disparar o comando: aí o chamador mostra a URL e o usuário abre à mão.
pub fn open_in_browser(url: &str) -> bool {
    let Some(mut cmd) = browser_command(url) else {
        return false;
    };

    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Login no navegador: abre o listener, avisa a URL, abre o navegador (quando o
/// chamador pede), espera o callback e troca o `code` pelas credenciais.
pub async fn run_login_flow<F>(
    open_browser: bool,
    notify_url: F,
) -> Result<OpenAiPlanCredentials, ProviderError>
where
    F: Fn(&str),
{
    let client = reqwest::Client::new();

    run_login_flow_at(
        open_browser,
        &callback_addr(),
        &token_url(),
        &client,
        notify_url,
    )
    .await
}

/// Igual a `run_login_flow`, com o endereço do listener e o endpoint de token
/// explícitos: é o que os testes usam para falar com um servidor local.
pub async fn run_login_flow_at<F>(
    open_browser: bool,
    addr: &str,
    token_endpoint: &str,
    client: &reqwest::Client,
    notify_url: F,
) -> Result<OpenAiPlanCredentials, ProviderError>
where
    F: Fn(&str),
{
    let listener = bind_callback_listener_at(addr).await?;

    let verifier = generate_verifier();
    let state = generate_state();
    let auth_url = build_authorization_url(&verifier, &state);

    notify_url(&auth_url);
    if open_browser {
        open_in_browser(&auth_url);
    }

    let code = tokio::time::timeout(
        Duration::from_secs(CALLBACK_TIMEOUT_SECS),
        accept_callback(listener, &state),
    )
    .await
    .map_err(|_| {
        ProviderError::Network(format!(
            "tempo esgotado esperando o callback em {addr} ({CALLBACK_TIMEOUT_SECS}s)"
        ))
    })??;

    exchange_code_at(client, token_endpoint, &code, &verifier, now_secs()).await
}

/// Payload (claims) de um JWT, sem validar assinatura: o token veio do
/// endpoint de token por TLS, e aqui só interessa o conteúdo.
pub fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.trim().split('.').nth(1)?;
    let bytes = base64_url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// `chatgpt_account_id` do claim `https://api.openai.com/auth`, com fallback
/// para `sub`. Token ausente/ inválido → `None`.
pub fn account_id_from_id_token(id_token: &str) -> Option<String> {
    account_id_from_claims(&jwt_claims(id_token)?)
}

pub fn account_id_from_claims(claims: &Value) -> Option<String> {
    let nested = claims
        .get(AUTH_CLAIM)
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .and_then(non_empty);

    nested.or_else(|| {
        claims
            .get("sub")
            .and_then(Value::as_str)
            .and_then(non_empty)
    })
}

fn email_from_claims(claims: &Value) -> Option<String> {
    claims
        .get("email")
        .and_then(Value::as_str)
        .and_then(non_empty)
}

fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;

    for byte in text.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;

        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }

    Some(out)
}

/// Array `tools` da Responses API. Aqui não existe o envelope `function` do
/// Chat Completions: nome, descrição e schema ficam no mesmo nível.
pub fn tools_payload(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "type": "function",
                "name": spec.name,
                "description": spec.description,
                "parameters": spec.schema,
            })
        })
        .collect()
}

/// Corpo exato de `POST {base}/responses`.
///
/// A Responses API não tem `messages`: o texto de sistema vira `instructions` e
/// o diálogo vira itens em `input`, onde uma tool call é um `function_call` e o
/// resultado dela é um `function_call_output`. `store` é sempre `false`, porque
/// o backend de assinatura não guarda estado do lado do servidor.
pub fn build_body(request: &ChatRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "input": Value::Array(input_items(&request.messages)),
        "store": false,
        "stream": true,
    });

    let instructions = instructions(&request.messages);
    if !instructions.is_empty() {
        body["instructions"] = Value::String(instructions);
    }

    if !request.tools.is_empty() {
        body["tools"] = Value::Array(tools_payload(&request.tools));
    }

    // Sem teto a chave não vai: omitida, o backend escolhe o limite.
    if let Some(max_tokens) = request.max_tokens {
        body["max_output_tokens"] = json!(max_tokens);
    }

    body
}

/// Todo o texto de sistema acumulado, na ordem do diálogo, separado por linha
/// em branco. Vira `instructions`, que a Responses API trata à parte do `input`.
fn instructions(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|message| message.role == Role::System)
        .map(|message| message.content.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn input_items(messages: &[Message]) -> Vec<Value> {
    let mut items = Vec::new();

    for message in messages {
        match message.role {
            // Já promovido a `instructions`; repetir aqui duplicaria o texto.
            Role::System => {}
            Role::User => items.push(message_item("user", "input_text", &message.content)),
            Role::Assistant => {
                if !message.content.trim().is_empty() {
                    items.push(message_item("assistant", "output_text", &message.content));
                }
                for call in &message.tool_calls {
                    items.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments,
                    }));
                }
            }
            Role::Tool => {
                let call_id = message.tool_call_id.as_deref().unwrap_or_default().trim();
                // `function_call_output` sem `call_id` não emparelha com nada e
                // derruba a requisição inteira: sem par, o resultado é descartado.
                if call_id.is_empty() {
                    tracing::debug!("resultado de tool sem call_id descartado no openai-plan");
                    continue;
                }
                items.push(json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": message.content,
                }));
            }
        }
    }

    items
}

fn message_item(role: &str, part_type: &str, text: &str) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{ "type": part_type, "text": text }],
    })
}

/// Consumo reportado em `response.usage`, nos três campos que o
/// `ProviderEvent::Usage` do kernel espera.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

/// Evento reconhecido da Responses API.
///
/// Ao contrário do `extract_chunk` do Chat Completions (`crate::sse`), aqui cada
/// evento tem nome próprio e o `SseParser` só entrega o `data:`, então o
/// `type` sai do próprio JSON. O parser é stateless: acumular as calls é papel
/// do `ToolCallAccumulator` de quem consome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// `response.output_text.delta`.
    Text(String),
    /// `response.reasoning_summary_text.delta` e o reasoning cru
    /// (`response.reasoning_text.delta`).
    Reasoning(String),
    /// `response.output_item.added` de um item `function_call`.
    ToolCallStarted {
        index: usize,
        id: Option<String>,
        name: Option<String>,
    },
    /// `response.function_call_arguments.delta`: fragmento dos argumentos.
    ToolCallArguments { index: usize, delta: String },
    /// `response.output_item.done` de um item `function_call`, já com os
    /// argumentos completos — só deve preencher o que os deltas não trouxeram,
    /// porque o acumulador concatena.
    ToolCallFinished {
        index: usize,
        id: Option<String>,
        arguments: String,
    },
    /// `response.completed`/`response.incomplete`: o backend encerrou a resposta.
    Completed { reason: String, usage: Option<Usage> },
    /// `response.failed`/`error`: o backend encerrou com erro.
    Failed(String),
    /// Evento válido que não muda nada no stream (`response.created`, itens de
    /// mensagem, `[DONE]`…).
    Ignored,
}

/// Traduz um `data:` da Responses API em `StreamEvent`.
///
/// JSON inválido — ou evento sem `type` — é `ProviderError::Protocol`: o
/// backend mudou de formato e seguir adiante produziria resposta truncada.
pub fn extract_event(payload: &str) -> Result<StreamEvent, ProviderError> {
    let trimmed = payload.trim();
    if trimmed == "[DONE]" {
        return Ok(StreamEvent::Ignored);
    }

    let value: Value = serde_json::from_str(trimmed).map_err(|error| {
        ProviderError::Protocol(format!("JSON inválido no SSE do openai-plan: {error}"))
    })?;

    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        return Err(ProviderError::Protocol(
            "evento SSE do openai-plan sem `type`".to_string(),
        ));
    };

    let delta = || value.get("delta").and_then(Value::as_str).map(str::to_string);

    Ok(match kind {
        "response.output_text.delta" => match delta() {
            Some(text) => StreamEvent::Text(text),
            None => StreamEvent::Ignored,
        },
        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
            match delta() {
                Some(text) => StreamEvent::Reasoning(text),
                None => StreamEvent::Ignored,
            }
        }
        "response.output_item.added" => function_call_item(&value)
            .map(|(index, item)| StreamEvent::ToolCallStarted {
                index,
                id: call_id(item),
                name: item.get("name").and_then(Value::as_str).map(str::to_string),
            })
            .unwrap_or(StreamEvent::Ignored),
        "response.function_call_arguments.delta" => match delta() {
            Some(delta) => StreamEvent::ToolCallArguments {
                index: output_index(&value),
                delta,
            },
            None => StreamEvent::Ignored,
        },
        "response.output_item.done" => function_call_item(&value)
            .map(|(index, item)| StreamEvent::ToolCallFinished {
                index,
                id: call_id(item),
                arguments: item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
            .unwrap_or(StreamEvent::Ignored),
        "response.completed" => StreamEvent::Completed {
            reason: "stop".to_string(),
            usage: usage_of(&value),
        },
        "response.incomplete" => StreamEvent::Completed {
            reason: finish_reason_of(&value),
            usage: usage_of(&value),
        },
        "response.failed" | "error" => StreamEvent::Failed(failure_message(&value)),
        _ => StreamEvent::Ignored,
    })
}

/// Índice e item de um `output_item.*` que seja `function_call`.
fn function_call_item(value: &Value) -> Option<(usize, &Value)> {
    let item = value.get("item")?;
    if item.get("type").and_then(Value::as_str) != Some("function_call") {
        return None;
    }
    Some((output_index(value), item))
}

fn output_index(value: &Value) -> usize {
    value
        .get("output_index")
        .and_then(Value::as_u64)
        .unwrap_or_default() as usize
}

/// A Responses API nomeia a call em `call_id`; o id do item (`fc_…`) é o
/// fallback.
fn call_id(item: &Value) -> Option<String> {
    item.get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn usage_of(value: &Value) -> Option<Usage> {
    let usage = value
        .get("response")
        .and_then(|response| response.get("usage"))?;
    let count = |path: &str| usage.pointer(path).and_then(Value::as_u64);
    Some(Usage {
        prompt_tokens: count("/input_tokens"),
        cached_tokens: count("/input_tokens_details/cached_tokens"),
        completion_tokens: count("/output_tokens"),
    })
}

/// `max_output_tokens` é o nome que o backend dá ao corte no teto de tokens; o
/// kernel chama o mesmo caso de `length`.
fn finish_reason_of(value: &Value) -> String {
    match value
        .pointer("/response/incomplete_details/reason")
        .and_then(Value::as_str)
    {
        Some("max_output_tokens") => "length".to_string(),
        Some(other) => other.to_string(),
        None => "incomplete".to_string(),
    }
}

fn failure_message(value: &Value) -> String {
    value
        .pointer("/response/error/message")
        .or_else(|| value.pointer("/error/message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "resposta falhou no openai-plan".to_string())
}

/// Timeouts do cliente HTTP, os mesmos números do `deepseek`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Provedor `openai-plan` sobre a Responses API.
///
/// O token não é uma chave estática: vem do login OAuth e é renovado com o
/// `refresh_token` quando venceu, com o hook de gravação disparado a cada
/// renovação para o redb não ficar com o token velho.
pub struct OpenAiPlanProvider {
    client: reqwest::Client,
    credentials: Mutex<OpenAiPlanCredentials>,
    base_url: String,
    stall_timeout: Duration,
    on_refresh: Option<RefreshHook>,
    last_payload: Mutex<Option<Value>>,
}

impl OpenAiPlanProvider {
    pub fn new(credentials: OpenAiPlanCredentials) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .expect("cliente HTTP válido");

        Self {
            client,
            credentials: Mutex::new(credentials),
            base_url: base_url(),
            stall_timeout: STALL_TIMEOUT,
            on_refresh: None,
            last_payload: Mutex::new(None),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Teto de espera por dados do stream. Existe para o teste do stall: 120 s
    /// de espera real não cabem na suite.
    pub fn with_stall_timeout(mut self, stall_timeout: Duration) -> Self {
        self.stall_timeout = stall_timeout;
        self
    }

    /// Chamado a cada token renovado, para regravar as credenciais.
    pub fn with_on_refresh(mut self, hook: RefreshHook) -> Self {
        self.on_refresh = Some(hook);
        self
    }

    fn locked_credentials(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, OpenAiPlanCredentials>, ProviderError> {
        self.credentials.lock().map_err(|_| {
            ProviderError::Protocol("credenciais do openai-plan envenenadas".to_string())
        })
    }

    /// Access token utilizável e o account id do header. Token vencido é
    /// renovado aqui — o guard do `Mutex` não atravessa o `await`.
    async fn access(&self) -> Result<(String, Option<String>), ProviderError> {
        let now = now_secs();

        {
            let guard = self.locked_credentials()?;
            if !guard.is_expired(now) {
                return Ok((guard.access_token.clone(), guard.account_id.clone()));
            }
        }

        let mut renewed = self.locked_credentials()?.clone();
        renewed
            .refresh_if_expired(&self.client, &token_url(), now, true)
            .await?;
        let access = renewed.access_token.clone();
        let account_id = renewed.account_id.clone();
        *self.locked_credentials()? = renewed.clone();
        persist_credentials(&renewed, None, self.on_refresh.as_ref());

        Ok((access, account_id))
    }
}

#[async_trait]
impl Provider for OpenAiPlanProvider {
    fn id(&self) -> &str {
        PROVIDER_ID
    }

    fn last_payload(&self) -> Option<Value> {
        self.last_payload
            .lock()
            .ok()
            .and_then(|payload| payload.clone())
    }

    /// Pergunta o catálogo ao backend: só ele sabe quais modelos a conta tem.
    /// Falha de rede ou de caminho não é catálogo vazio — cai no embutido e
    /// registra o aviso —, mas 401/403 sobe como `Http` para o login aparecer.
    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let (access_token, account_id) = self.access().await?;

        let mut builder = self
            .client
            .get(models_url_for(&self.base_url))
            .bearer_auth(access_token)
            .header("originator", originator())
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(account_id) = account_id.filter(|id| !id.is_empty()) {
            builder = builder.header("chatgpt-account-id", account_id);
        }

        let response = builder
            .send()
            .await
            .map_err(|error| ProviderError::Network(error.to_string()))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(ProviderError::Http {
                status: status.as_u16(),
                body: text,
            });
        }

        if status.is_success()
            && let Some(ids) = extract_model_ids(&text)
            && !ids.is_empty()
        {
            return Ok(ids);
        }

        tracing::warn!(
            status = status.as_u16(),
            "catálogo remoto do openai-plan indisponível; usando o catálogo embutido"
        );
        Ok(models())
    }

    async fn stream_chat(&self, request: ChatRequest) -> Result<ProviderStream, ProviderError> {
        let url = format!("{}/responses", self.base_url.trim_end_matches('/'));
        let body = build_body(&request);
        if let Ok(mut last) = self.last_payload.lock() {
            *last = Some(body.clone());
        }

        let (access_token, account_id) = self.access().await?;

        let mut builder = self
            .client
            .post(url)
            .bearer_auth(access_token)
            .header("originator", originator())
            .json(&body);
        if let Some(account_id) = account_id.filter(|id| !id.is_empty()) {
            builder = builder.header("chatgpt-account-id", account_id);
        }

        let response = builder
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
            with_arguments: BTreeSet::new(),
            usage: None,
            finish_reason: None,
            started: false,
            closed: false,
            done: false,
            failed: false,
        };

        let stall_timeout = self.stall_timeout;
        let stream = unfold(state, move |mut state| async move {
            loop {
                // Já encerrado: drena tool calls, consumo e motivo, nessa ordem,
                // que é a que o kernel espera (`ToolCalls` → `Usage` → `Finished`).
                if state.done {
                    if state.failed {
                        return None;
                    }
                    if let Some(calls) = state.calls.finish() {
                        return Some((Ok(ProviderEvent::ToolCalls(calls)), state));
                    }
                    if let Some(usage) = state.usage.take() {
                        return Some((
                            Ok(ProviderEvent::Usage {
                                prompt_tokens: usage.prompt_tokens,
                                cached_tokens: usage.cached_tokens,
                                completion_tokens: usage.completion_tokens,
                            }),
                            state,
                        ));
                    }
                    if let Some(reason) = state.finish_reason.take() {
                        return Some((Ok(ProviderEvent::Finished(reason)), state));
                    }
                    return None;
                }

                if let Some(payload) = state.pending.pop_front() {
                    let event = match extract_event(&payload) {
                        Ok(event) => event,
                        Err(error) => {
                            state.done = true;
                            state.failed = true;
                            return Some((Err(error), state));
                        }
                    };
                    match state.apply(event) {
                        Ok(Some(event)) => return Some((Ok(event), state)),
                        Ok(None) => continue,
                        Err(error) => return Some((Err(error), state)),
                    }
                }

                // Fim do corpo sem evento de encerramento: ainda vale drenar o
                // que ficou no buffer do parser.
                if state.closed {
                    state.done = true;
                    continue;
                }

                match tokio::time::timeout(stall_timeout, state.response.chunk()).await {
                    Err(_) => {
                        state.done = true;
                        state.failed = true;
                        let message = format!(
                            "stream do openai-plan sem dados por {} segundos",
                            stall_timeout.as_secs()
                        );
                        return Some((Err(ProviderError::Network(message)), state));
                    }
                    Ok(Err(error)) => {
                        state.done = true;
                        state.failed = true;
                        return Some((Err(ProviderError::Network(error.to_string())), state));
                    }
                    Ok(Ok(None)) => {
                        let tail = state.parser.finish();
                        state.closed = true;
                        state.pending.extend(tail);
                    }
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

/// Estado do stream de uma resposta da Responses API.
struct StreamState {
    response: reqwest::Response,
    parser: SseParser,
    pending: VecDeque<String>,
    calls: ToolCallAccumulator,
    /// Índices que já receberam `function_call_arguments.delta`: o item final
    /// repete os argumentos completos e concatenar duplicaria o JSON.
    with_arguments: BTreeSet<usize>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
    started: bool,
    closed: bool,
    done: bool,
    failed: bool,
}

impl StreamState {
    /// Traduz um evento da Responses API em evento do kernel. `Ok(None)` é
    /// evento que só alimenta estado; `Err` encerra o stream.
    fn apply(&mut self, event: StreamEvent) -> Result<Option<ProviderEvent>, ProviderError> {
        match event {
            StreamEvent::Text(text) => Ok(Some(ProviderEvent::Text(text))),
            StreamEvent::Reasoning(text) => Ok(Some(ProviderEvent::Reasoning(text))),
            StreamEvent::ToolCallStarted { index, id, name } => {
                self.calls.merge(ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments: None,
                });
                Ok(None)
            }
            StreamEvent::ToolCallArguments { index, delta } => {
                self.with_arguments.insert(index);
                self.calls.merge(ToolCallDelta {
                    index,
                    id: None,
                    name: None,
                    arguments: Some(delta),
                });
                Ok(None)
            }
            StreamEvent::ToolCallFinished {
                index,
                id,
                arguments,
            } => {
                let arguments = (!self.with_arguments.contains(&index) && !arguments.is_empty())
                    .then_some(arguments);
                self.calls.merge(ToolCallDelta {
                    index,
                    id,
                    name: None,
                    arguments,
                });
                Ok(None)
            }
            StreamEvent::Completed { reason, usage } => {
                self.finish_reason = Some(reason);
                if usage.is_some() {
                    self.usage = usage;
                }
                self.done = true;
                Ok(None)
            }
            StreamEvent::Failed(message) => {
                self.done = true;
                self.failed = true;
                Err(ProviderError::Protocol(format!(
                    "resposta falhou no openai-plan: {message}"
                )))
            }
            StreamEvent::Ignored => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tupi_domain::ToolCall;

    fn encode_url(bytes: &[u8]) -> String {
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut result = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0];
            let b1 = chunk.get(1).copied().unwrap_or(0);
            let b2 = chunk.get(2).copied().unwrap_or(0);
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

    fn jwt_with_claims(claims: &str) -> String {
        let header = encode_url(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = encode_url(claims.as_bytes());
        format!("{header}.{payload}.signature")
    }

    fn credentials() -> OpenAiPlanCredentials {
        OpenAiPlanCredentials {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: 1_000_000,
            account_id: Some("acct_123".to_string()),
            email: Some("pessoa@example.com".to_string()),
        }
    }

    #[test]
    fn account_id_comes_from_the_openai_auth_claim() {
        let token = jwt_with_claims(
            r#"{"sub":"sub_fallback","https://api.openai.com/auth":{"chatgpt_account_id":"acct_nested"}}"#,
        );
        assert_eq!(
            account_id_from_id_token(&token).as_deref(),
            Some("acct_nested")
        );
    }

    #[test]
    fn account_id_falls_back_to_sub() {
        let token = jwt_with_claims(r#"{"sub":"sub_fallback"}"#);
        assert_eq!(
            account_id_from_id_token(&token).as_deref(),
            Some("sub_fallback")
        );
    }

    #[test]
    fn invalid_token_yields_none() {
        assert_eq!(account_id_from_id_token("não-é-jwt"), None);
        assert_eq!(account_id_from_id_token("a.!!!.c"), None);
        assert_eq!(account_id_from_id_token("a.bm90LWpzb24.c"), None);
        assert_eq!(jwt_claims("sem-pontos"), None);
        assert_eq!(jwt_claims("a.b.c"), None);
    }

    #[test]
    fn credentials_round_trip_through_serde() {
        let creds = credentials();
        let json = serde_json::to_string(&creds).expect("serializa");
        assert_eq!(
            OpenAiPlanCredentials::from_json_str(&json).as_ref(),
            Some(&creds)
        );
        assert_eq!(
            OpenAiPlanCredentials::from_json_str(&creds.to_json_string().unwrap()).as_ref(),
            Some(&creds)
        );
    }

    #[test]
    fn credentials_without_access_token_are_rejected() {
        assert!(OpenAiPlanCredentials::from_json_str("{}").is_none());
        assert!(OpenAiPlanCredentials::from_json_str("não é json").is_none());
        assert!(
            OpenAiPlanCredentials::from_json_str(
                r#"{"access_token":"  ","refresh_token":"r","expires_at":1}"#
            )
            .is_none()
        );
    }

    #[test]
    fn optional_fields_are_omitted_from_json() {
        let creds = OpenAiPlanCredentials {
            account_id: None,
            email: None,
            ..credentials()
        };
        let json = serde_json::to_string(&creds).expect("serializa");
        assert!(!json.contains("account_id"));
        assert!(!json.contains("email"));
        let parsed = OpenAiPlanCredentials::from_json_str(&json).expect("volta");
        assert_eq!(parsed, creds);
    }

    #[test]
    fn is_expired_applies_the_skew() {
        let mut creds = credentials();
        creds.expires_at = 1_000;
        assert!(!creds.is_expired(1_000 - REFRESH_SKEW_SECS - 1));
        assert!(creds.is_expired(1_000 - REFRESH_SKEW_SECS));
        assert!(creds.is_expired(1_000));
        assert!(creds.is_expired(5_000));
    }

    #[test]
    fn missing_file_yields_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(OpenAiPlanCredentials::load_from_dir(dir.path()).is_none());
        assert!(
            OpenAiPlanCredentials::load_from_file(&dir.path().join("openai-plan.json")).is_none()
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let creds = credentials();
        creds
            .save(&OpenAiPlanCredentials::path(dir.path()))
            .expect("grava");
        assert_eq!(
            OpenAiPlanCredentials::load_from_dir(dir.path()),
            Some(creds)
        );
    }

    #[test]
    fn token_response_carries_account_id_and_expiry() {
        let id_token = jwt_with_claims(
            r#"{"email":"pessoa@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"acct_nested"}}"#,
        );
        let payload = format!(
            r#"{{"access_token":"a","refresh_token":"r","expires_in":120,"id_token":"{id_token}"}}"#
        );
        let creds = OpenAiPlanCredentials::from_token_response(&payload, 10).expect("credenciais");
        assert_eq!(creds.expires_at, 130);
        assert_eq!(creds.account_id.as_deref(), Some("acct_nested"));
        assert_eq!(creds.email.as_deref(), Some("pessoa@example.com"));

        let sem_id = r#"{"access_token":"a","refresh_token":"r","expires_in":60}"#;
        let creds = OpenAiPlanCredentials::from_token_response(sem_id, 0).expect("credenciais");
        assert_eq!(creds.account_id, None);
        assert_eq!(creds.expires_at, 60);
    }

    #[test]
    fn token_response_with_error_or_without_access_token_is_protocol_error() {
        let error = OpenAiPlanCredentials::from_token_response(
            r#"{"error":"invalid_grant","error_description":"refresh expirado"}"#,
            0,
        )
        .expect_err("erro de autenticação");
        assert!(matches!(error, ProviderError::Protocol(_)));

        let missing =
            OpenAiPlanCredentials::from_token_response(r#"{"refresh_token":"r"}"#, 0).expect_err(
                "access_token ausente",
            );
        assert!(matches!(missing, ProviderError::Protocol(_)));
    }

    #[test]
    fn network_constants_have_defaults() {
        assert!(!client_id().is_empty());
        assert!(authorize_url().starts_with("http"));
        assert!(token_url().starts_with("http"));
        assert!(base_url().starts_with("http"));
        assert!(redirect_uri().starts_with("http"));
        assert!(!originator().is_empty());
        assert!(models().iter().any(|model| model.contains("codex")));
    }

    #[test]
    fn pkce_challenge_matches_the_rfc7636_vector() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_pkce_material_respects_the_verifier_bounds() {
        let verifier = generate_verifier();
        assert!(
            (43..=128).contains(&verifier.len()),
            "verifier fora do RFC 7636: {} chars",
            verifier.len()
        );
        assert!(
            verifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')),
            "verifier com caractere fora do unreserved: {verifier}"
        );
        assert_eq!(pkce_challenge(&verifier).len(), 43);
        assert_ne!(generate_state(), generate_verifier());
        assert_eq!(generate_state().len(), 64);
    }

    #[test]
    fn authorization_url_carries_client_id_pkce_state_and_scopes() {
        let url = build_authorization_url("test_verifier_12345", "test_state");

        assert!(url.starts_with(&format!("{}?", authorize_url())));
        assert!(url.contains(&format!("client_id={}", client_id())));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!(
            "code_challenge={}",
            pkce_challenge("test_verifier_12345")
        )));
        assert!(url.contains("state=test_state"));
        assert!(url.contains(&format!("redirect_uri={}", url_encode(&redirect_uri()))));
        assert!(url.contains(&format!("scope={}", url_encode(&scope_string()))));
        assert!(url.contains("scope=openid%20profile%20email%20offline_access"));
        assert!(!url.contains("client_secret"));
    }

    /// Servidor de token local: responde uma vez e devolve o request cru para
    /// conferir o corpo do `POST`.
    async fn spawn_token_server(
        status: u16,
        body: String,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            let resp = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            request
        });
        (format!("http://{addr}/oauth/token"), handle)
    }

    #[tokio::test]
    async fn exchange_code_turns_expires_in_into_expires_at() {
        let id_token = jwt_with_claims(
            r#"{"email":"pessoa@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"acct_nested"}}"#,
        );
        let body = format!(
            r#"{{"access_token":"acc","refresh_token":"ref","expires_in":1800,"id_token":"{id_token}"}}"#
        );
        let (endpoint, server) = spawn_token_server(200, body).await;

        let creds = exchange_code_at(&reqwest::Client::new(), &endpoint, "the-code", "the-verifier", 500)
            .await
            .expect("exchange válido");

        assert_eq!(creds.access_token, "acc");
        assert_eq!(creds.refresh_token, "ref");
        assert_eq!(creds.expires_at, 2300);
        assert_eq!(creds.account_id.as_deref(), Some("acct_nested"));
        assert_eq!(creds.email.as_deref(), Some("pessoa@example.com"));

        let request = server.await.unwrap();
        assert!(request.starts_with("POST /oauth/token "), "{request}");
        assert!(request.contains("application/x-www-form-urlencoded"));
        assert!(request.contains("grant_type=authorization_code"));
        assert!(request.contains("code=the-code"));
        assert!(request.contains("code_verifier=the-verifier"));
        assert!(request.contains(&format!("client_id={}", client_id())));
        assert!(request.contains(&format!("redirect_uri={}", url_encode(&redirect_uri()))));
        assert!(!request.contains("client_secret"));
    }

    #[tokio::test]
    async fn exchange_code_maps_token_failures_to_http() {
        let (endpoint, server) = spawn_token_server(
            400,
            r#"{"error":"invalid_grant","error_description":"código expirado"}"#.to_string(),
        )
        .await;
        let error = exchange_code_at(&reqwest::Client::new(), &endpoint, "bad", "verifier", 0)
            .await
            .expect_err("400 precisa subir como Http");
        match error {
            ProviderError::Http { status, body } => {
                assert_eq!(status, 400);
                assert!(body.contains("invalid_grant"), "{body}");
            }
            other => panic!("erro inesperado: {other:?}"),
        }
        server.await.unwrap();

        // OAuth também pode devolver `error` num 200: continua sendo `Http`.
        let (endpoint, server) = spawn_token_server(
            200,
            r#"{"error":"invalid_request","error_description":"PKCE ausente"}"#.to_string(),
        )
        .await;
        let error = exchange_code_at(&reqwest::Client::new(), &endpoint, "code", "verifier", 0)
            .await
            .expect_err("error no corpo precisa subir como Http");
        match error {
            ProviderError::Http { status, body } => {
                assert_eq!(status, 200);
                assert!(body.contains("invalid_request"), "{body}");
            }
            other => panic!("erro inesperado: {other:?}"),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn refresh_if_expired_leaves_a_fresh_token_alone() {
        let original = credentials();
        let mut creds = original.clone();
        creds.expires_at = 1_000;

        // Endpoint que não atende: se houvesse request, viria `Network`.
        let changed = creds
            .refresh_if_expired(
                &reqwest::Client::new(),
                "http://127.0.0.1:1/oauth/token",
                10,
                false,
            )
            .await
            .expect("token fresco não renova");

        assert!(!changed);
        assert_eq!(creds, original);
    }

    #[tokio::test]
    async fn expired_token_is_renewed_before_use() {
        let (endpoint, server) = spawn_token_server(
            200,
            r#"{"access_token":"novo-acesso","refresh_token":"novo-refresh","expires_in":3600}"#
                .to_string(),
        )
        .await;

        let mut creds = credentials();
        creds.expires_at = 1_000;
        let changed = creds
            .refresh_if_expired(&reqwest::Client::new(), &endpoint, 5_000, false)
            .await
            .expect("renova");

        assert!(changed);
        assert_eq!(creds.access_token, "novo-acesso");
        assert_eq!(creds.refresh_token, "novo-refresh");
        assert_eq!(creds.expires_at, 8_600);
        // Sem `id_token` no refresh: conta e e-mail sobrevivem à renovação.
        assert_eq!(creds.account_id.as_deref(), Some("acct_123"));
        assert_eq!(creds.email.as_deref(), Some("pessoa@example.com"));

        let request = server.await.unwrap();
        assert!(request.contains("application/x-www-form-urlencoded"), "{request}");
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=refresh"));
        assert!(request.contains(&format!("client_id={}", client_id())));
        assert!(!request.contains("client_secret"));
    }

    #[tokio::test]
    async fn refresh_failure_maps_to_http_and_keeps_the_old_credentials() {
        let (endpoint, server) = spawn_token_server(
            401,
            r#"{"error":"invalid_grant","error_description":"refresh revogado"}"#.to_string(),
        )
        .await;

        let mut creds = credentials();
        let error = creds
            .refresh_if_expired(&reqwest::Client::new(), &endpoint, 2_000, true)
            .await
            .expect_err("401 precisa subir como Http");

        match error {
            ProviderError::Http { status, body } => {
                assert_eq!(status, 401);
                assert!(body.contains("invalid_grant"), "{body}");
            }
            other => panic!("erro inesperado: {other:?}"),
        }
        // Nada foi trocado pela metade: a credencial antiga continua em uso.
        assert_eq!(creds, credentials());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn transport_failure_maps_to_network() {
        let addr = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap()
        };
        let endpoint = format!("http://{addr}/oauth/token");

        let error = refresh_at(&reqwest::Client::new(), &endpoint, &credentials(), 0)
            .await
            .expect_err("conexão recusada");

        assert!(matches!(error, ProviderError::Network(_)), "{error:?}");
    }

    #[tokio::test]
    async fn refresh_without_refresh_token_is_protocol_error() {
        let mut creds = credentials();
        creds.refresh_token = "  ".to_string();

        let error = creds
            .refresh_if_expired(
                &reqwest::Client::new(),
                "http://127.0.0.1:1/oauth/token",
                2_000,
                true,
            )
            .await
            .expect_err("sem refresh token");

        assert!(matches!(error, ProviderError::Protocol(_)), "{error:?}");
    }

    #[test]
    fn persist_credentials_writes_the_file_and_fires_the_hook() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = OpenAiPlanCredentials::path(dir.path());
        let renewed = OpenAiPlanCredentials {
            access_token: "acesso-renovado".to_string(),
            expires_at: 8_600,
            ..credentials()
        };

        let seen = std::sync::Arc::new(std::sync::Mutex::new(None::<OpenAiPlanCredentials>));
        let slot = std::sync::Arc::clone(&seen);
        let hook: RefreshHook = std::sync::Arc::new(move |creds: &OpenAiPlanCredentials| {
            *slot.lock().unwrap() = Some(creds.clone());
        });

        persist_credentials(&renewed, Some(&path), Some(&hook));

        assert_eq!(
            OpenAiPlanCredentials::load_from_file(&path),
            Some(renewed.clone())
        );
        assert_eq!(*seen.lock().unwrap(), Some(renewed));
    }

    #[test]
    fn persist_credentials_without_destination_is_a_no_op() {
        persist_credentials(&credentials(), None, None);
    }

    fn tool_spec() -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: "lê um arquivo".to_string(),
            schema: json!({ "type": "object", "properties": {} }),
        }
    }

    #[test]
    fn system_messages_go_into_instructions() {
        let request = ChatRequest::new(
            "gpt-5-codex",
            vec![
                Message::new(Role::System, "  primeira regra  "),
                Message::new(Role::System, "   "),
                Message::new(Role::System, "segunda regra"),
                Message::user("oi"),
            ],
        );

        let body = build_body(&request);

        assert_eq!(body["instructions"], json!("primeira regra\n\nsegunda regra"));
        // O sistema não se repete no input: só sobra o turno do usuário.
        let input = body["input"].as_array().expect("input é array");
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], json!("message"));
        assert_eq!(input[0]["role"], json!("user"));
        assert_eq!(input[0]["content"][0]["type"], json!("input_text"));
    }

    #[test]
    fn tool_calls_become_function_call_and_results_function_call_output() {
        let request = ChatRequest::new(
            "gpt-5-codex",
            vec![
                Message::user("lê o arquivo"),
                Message::assistant_with_calls(
                    "",
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "read".to_string(),
                        arguments: "{\"path\":\"a.txt\"}".to_string(),
                    }],
                ),
                Message::tool("call_1", "conteúdo"),
            ],
        );

        let body = build_body(&request);
        let input = body["input"].as_array().expect("input é array");

        // Turno do assistente sem texto não gera item `message` vazio.
        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], json!("function_call"));
        assert_eq!(input[1]["call_id"], json!("call_1"));
        assert_eq!(input[1]["name"], json!("read"));
        assert_eq!(input[1]["arguments"], json!("{\"path\":\"a.txt\"}"));
        assert_eq!(input[2]["type"], json!("function_call_output"));
        assert_eq!(input[2]["call_id"], json!("call_1"));
        assert_eq!(input[2]["output"], json!("conteúdo"));
    }

    #[test]
    fn the_body_is_stateless_and_streams() {
        let body = build_body(&ChatRequest::new("gpt-5-codex", vec![Message::user("oi")]));

        assert_eq!(body["store"], json!(false));
        assert_eq!(body["stream"], json!(true));
        // A chave só existe quando há system message.
        assert!(body.get("instructions").is_none());
        assert!(body.get("messages").is_none());
    }

    #[test]
    fn tools_are_sent_without_the_function_envelope() {
        let request = ChatRequest::new("gpt-5-codex", vec![Message::user("oi")])
            .with_tools(vec![tool_spec()]);

        let body = build_body(&request);
        let tools = body["tools"].as_array().expect("tools é array");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], json!("function"));
        assert_eq!(tools[0]["name"], json!("read"));
        assert_eq!(tools[0]["description"], json!("lê um arquivo"));
        assert_eq!(tools[0]["parameters"]["type"], json!("object"));
        assert!(tools[0].get("function").is_none());
    }

    #[test]
    fn body_omits_max_output_tokens_when_there_is_no_cap() {
        let request = ChatRequest::new("gpt-5-codex", vec![Message::user("oi")]);
        assert!(build_body(&request).get("max_output_tokens").is_none());

        let zeroed = ChatRequest::new("gpt-5-codex", vec![Message::user("oi")]).with_max_tokens(0);
        assert!(build_body(&zeroed).get("max_output_tokens").is_none());
    }

    #[test]
    fn body_carries_max_output_tokens_when_capped() {
        let request =
            ChatRequest::new("gpt-5-codex", vec![Message::user("oi")]).with_max_tokens(512);

        assert_eq!(build_body(&request)["max_output_tokens"], json!(512));
    }

    fn event(payload: &str) -> StreamEvent {
        extract_event(payload).expect("evento válido")
    }

    #[test]
    fn text_delta_becomes_text() {
        assert_eq!(
            event(r#"{"type":"response.output_text.delta","delta":"olá"}"#),
            StreamEvent::Text("olá".to_string())
        );
    }

    #[test]
    fn reasoning_summary_delta_becomes_reasoning() {
        assert_eq!(
            event(r#"{"type":"response.reasoning_summary_text.delta","delta":"pensando"}"#),
            StreamEvent::Reasoning("pensando".to_string())
        );
        assert_eq!(
            event(r#"{"type":"response.reasoning_text.delta","delta":"cru"}"#),
            StreamEvent::Reasoning("cru".to_string())
        );
    }

    #[test]
    fn function_call_start_arguments_and_end_are_reported() {
        assert_eq!(
            event(
                r#"{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}"#
            ),
            StreamEvent::ToolCallStarted {
                index: 1,
                id: Some("call_1".to_string()),
                name: Some("read".to_string()),
            }
        );

        assert_eq!(
            event(
                r#"{"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc_1","delta":"{\"path\":"}"#
            ),
            StreamEvent::ToolCallArguments {
                index: 1,
                delta: "{\"path\":".to_string(),
            }
        );

        assert_eq!(
            event(
                r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","arguments":"{\"path\":\"AGENTS.md\"}"}}"#
            ),
            StreamEvent::ToolCallFinished {
                index: 1,
                id: Some("call_1".to_string()),
                arguments: "{\"path\":\"AGENTS.md\"}".to_string(),
            }
        );
    }

    #[test]
    fn non_function_call_items_are_ignored() {
        assert_eq!(
            event(
                r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant"}}"#
            ),
            StreamEvent::Ignored
        );
        assert_eq!(
            event(r#"{"type":"response.created","response":{}}"#),
            StreamEvent::Ignored
        );
        assert_eq!(event("[DONE]"), StreamEvent::Ignored);
    }

    #[test]
    fn completed_carries_usage() {
        assert_eq!(
            event(
                r#"{"type":"response.completed","response":{"usage":{"input_tokens":120,"output_tokens":8,"input_tokens_details":{"cached_tokens":64}}}}"#
            ),
            StreamEvent::Completed {
                reason: "stop".to_string(),
                usage: Some(Usage {
                    prompt_tokens: Some(120),
                    cached_tokens: Some(64),
                    completion_tokens: Some(8),
                }),
            }
        );
    }

    #[test]
    fn incomplete_maps_the_token_cap_to_length() {
        assert_eq!(
            event(
                r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}"#
            ),
            StreamEvent::Completed {
                reason: "length".to_string(),
                usage: None,
            }
        );
        assert_eq!(
            event(
                r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}"#
            ),
            StreamEvent::Completed {
                reason: "content_filter".to_string(),
                usage: None,
            }
        );
    }

    #[test]
    fn failed_and_error_events_carry_the_message() {
        assert_eq!(
            event(
                r#"{"type":"response.failed","response":{"error":{"code":"server_error","message":"deu ruim"}}}"#
            ),
            StreamEvent::Failed("deu ruim".to_string())
        );
        assert_eq!(
            event(r#"{"type":"error","message":"conta sem permissão"}"#),
            StreamEvent::Failed("conta sem permissão".to_string())
        );
        assert_eq!(
            event(r#"{"type":"response.failed","response":{}}"#),
            StreamEvent::Failed("resposta falhou no openai-plan".to_string())
        );
    }

    #[test]
    fn callback_page_tells_success_from_failure() {
        let ok = callback_page(true);
        assert!(ok.starts_with("<!DOCTYPE html>"));
        assert!(ok.contains("sucesso"));
        assert!(ok.contains("fechar esta aba"));
        assert!(ok.contains("charset=\"utf-8\""));

        let failed = callback_page(false);
        assert!(failed.contains("Não foi possível"));
        assert!(!failed.contains("sucesso"));
    }

    #[test]
    fn http_response_carries_length_and_closes_the_connection() {
        let response = http_response("200 OK", "<p>ok</p>");

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("Content-Length: 9"));
        assert!(response.contains("Connection: close"));
        assert!(response.ends_with("\r\n\r\n<p>ok</p>"));
    }

    #[test]
    fn callback_path_comes_from_the_redirect_uri() {
        assert_eq!(callback_path(), "/auth/callback");
        assert!(is_callback_request(
            "GET /auth/callback?code=x HTTP/1.1\r\nHost: localhost\r\n\r\n"
        ));
        assert!(!is_callback_request(
            "GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n"
        ));
        assert!(!is_callback_request(
            "GET /auth/callback-extra HTTP/1.1\r\n\r\n"
        ));
        assert!(!is_callback_request(""));
    }

    #[test]
    fn callback_code_requires_matching_state_and_a_code() {
        let request =
            |query: &str| format!("GET /auth/callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n");

        assert_eq!(
            callback_code(&request("code=abc&state=s1"), "s1").unwrap(),
            "abc"
        );
        assert_eq!(
            callback_code(&request("state=s1&code=ab%2Bcd"), "s1").unwrap(),
            "ab+cd"
        );

        assert!(matches!(
            callback_code(&request("code=abc&state=outro"), "s1"),
            Err(ProviderError::Protocol(_))
        ));
        assert!(matches!(
            callback_code(&request("code=abc"), "s1"),
            Err(ProviderError::Protocol(_))
        ));
        assert!(matches!(
            callback_code(&request("state=s1"), "s1"),
            Err(ProviderError::Protocol(_))
        ));
        assert!(matches!(
            callback_code(&request("code=&state=s1"), "s1"),
            Err(ProviderError::Protocol(_))
        ));
        assert!(matches!(
            callback_code("GET", "s1"),
            Err(ProviderError::Protocol(_))
        ));

        match callback_code(
            &request("error=access_denied&error_description=negado&state=s1"),
            "s1",
        ) {
            Err(ProviderError::Protocol(message)) => {
                assert!(message.contains("access_denied"), "{message}");
                assert!(message.contains("negado"), "{message}");
            }
            other => panic!("esperado Protocol, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn busy_port_maps_to_network_with_the_port_in_the_message() {
        let held = bind_callback_listener_at("127.0.0.1:0")
            .await
            .expect("primeira porta");
        let addr = held.local_addr().expect("endereço local").to_string();
        let port = addr.rsplit(':').next().unwrap().to_string();

        let error = bind_callback_listener_at(&addr)
            .await
            .expect_err("porta ocupada precisa falhar");

        match error {
            ProviderError::Network(message) => assert!(message.contains(&port), "{message}"),
            other => panic!("esperado Network, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn accept_callback_skips_foreign_requests_and_answers_with_the_success_page() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let listener = bind_callback_listener_at("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { accept_callback(listener, "state-ok").await });

        // O navegador pede o favicon antes/junto do callback: 404 e a espera segue.
        let mut probe = TcpStream::connect(addr).await.unwrap();
        probe
            .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut skipped = String::new();
        probe.read_to_string(&mut skipped).await.unwrap();
        assert!(skipped.starts_with("HTTP/1.1 404"), "{skipped}");
        assert!(!server.is_finished());

        let mut socket = TcpStream::connect(addr).await.unwrap();
        socket
            .write_all(b"GET /auth/callback?code=the-code&state=state-ok HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("Content-Type: text/html"));
        assert!(response.contains("sucesso"));
        assert_eq!(server.await.unwrap().unwrap(), "the-code");
    }

    #[tokio::test]
    async fn accept_callback_rejects_foreign_state_with_protocol_and_error_page() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        let listener = bind_callback_listener_at("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { accept_callback(listener, "expected").await });

        let mut socket = TcpStream::connect(addr).await.unwrap();
        socket
            .write_all(b"GET /auth/callback?code=the-code&state=other HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert!(response.contains("Não foi possível"));
        assert!(matches!(
            server.await.unwrap(),
            Err(ProviderError::Protocol(_))
        ));
    }

    #[test]
    fn invalid_json_or_missing_type_is_protocol_error() {
        assert!(matches!(
            extract_event("{não é json"),
            Err(ProviderError::Protocol(_))
        ));
        assert!(matches!(
            extract_event(r#"{"delta":"sem type"}"#),
            Err(ProviderError::Protocol(_))
        ));
    }
}
