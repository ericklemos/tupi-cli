//! Contrato de fingerprint do provider Antigravity (T10.1).
//!
//! O Hub oficial manda so `User-Agent` em generate/loadCodeAssist/
//! fetchAvailableModels; `X-Goog-Api-Client` existe apenas no onboardUser.
//! Header a mais nessas rotas e fingerprint que o Hub nao carrega, e nada
//! impede a regressao voltar calada — por isso os testes abaixo olham o
//! request cru que sai no socket, nao a constante que a gente acha que mandou.

use super::*;
use futures_util::StreamExt;
use tupi_kernel::{ChatRequest, Provider};

/// Headers do transporte e do protocolo. Qualquer outro nome fora deste
/// conjunto e regressao de fingerprint.
const UA_ONLY_HEADERS: &[&str] = &[
    "accept",
    "authorization",
    "content-length",
    "content-type",
    "host",
    "user-agent",
];

/// Headers de identidade de cliente que o Hub nao manda nas rotas UA-only.
const FINGERPRINT_HEADERS: &[&str] = &[
    "client-metadata",
    "x-goog-api-client",
    "x-goog-api-format-version",
    "x-goog-api-key",
    "x-goog-request-params",
    "x-goog-user-project",
];

fn request_headers(raw: &str) -> Vec<(String, String)> {
    let head = raw.split("\r\n\r\n").next().unwrap_or(raw);
    head.lines()
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// `allowed_extra` e o unico header extra que a rota legitima (ex.:
/// `x-goog-api-client` no onboardUser).
fn assert_ua_only_contract(raw: &str, endpoint: &str, allowed_extra: &[&str]) {
    let headers = request_headers(raw);
    let names: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();

    for forbidden in FINGERPRINT_HEADERS {
        if allowed_extra.contains(forbidden) {
            continue;
        }
        assert!(
            !names.contains(forbidden),
            "{endpoint} nao pode mandar `{forbidden}` (o Hub nao manda): {raw}"
        );
    }
    for name in &names {
        assert!(
            UA_ONLY_HEADERS.contains(name) || allowed_extra.contains(name),
            "{endpoint} ganhou o header `{name}`, fora do conjunto UA-only do Hub: {raw}"
        );
    }

    let ua = header_value(&headers, "user-agent").unwrap_or_default();
    assert!(
        ua.starts_with("antigravity/hub/") && ua.contains("aidev_client") && ua.contains("cl="),
        "{endpoint} precisa do UA do Hub, veio `{ua}`"
    );
    assert_eq!(
        header_value(&headers, "content-type"),
        Some("application/json"),
        "{endpoint} deve mandar JSON: {raw}"
    );
    assert!(
        header_value(&headers, "authorization")
            .unwrap_or_default()
            .starts_with("Bearer "),
        "{endpoint} deve mandar o bearer token: {raw}"
    );
}

#[tokio::test]
async fn generate_request_carries_user_agent_only() {
    let sse = concat!(
        "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]}}]}}\n\n",
        "data: [DONE]\n\n"
    );
    let (base_url, server) = spawn_http_sequence(vec![(200, "text/event-stream", sse)]).await;
    let provider = AntigravityProvider::new(creds_expiring_at(u64::MAX)).with_base_url(base_url);
    let mut stream = provider
        .stream_chat(ChatRequest::new("gemini-3.8-flash-high", Vec::new()))
        .await
        .unwrap();
    while let Some(item) = stream.next().await {
        item.unwrap();
    }

    let seen = server.await.unwrap();
    assert!(
        seen[0].starts_with("POST /v1internal:streamGenerateContent"),
        "{}",
        seen[0]
    );
    assert_ua_only_contract(&seen[0], "generate", &[]);
    assert_eq!(
        header_value(&request_headers(&seen[0]), "accept"),
        Some("text/event-stream"),
        "generate e SSE"
    );
}

#[tokio::test]
async fn load_code_assist_carries_user_agent_only() {
    let (base_url, server) = spawn_http_sequence(vec![(
        200,
        "application/json",
        r#"{"cloudaicompanionProject":{"id":"proj-from-load"}}"#,
    )])
    .await;

    let client = reqwest::Client::new();
    let proj = resolve_project_id_at(&client, &base_url, "tok").await;
    assert_eq!(proj, "proj-from-load");

    let seen = server.await.unwrap();
    assert!(
        seen[0].starts_with("POST /v1internal:loadCodeAssist"),
        "{}",
        seen[0]
    );
    assert_ua_only_contract(&seen[0], "loadCodeAssist", &[]);
}

#[tokio::test]
async fn onboard_user_adds_goog_api_client_and_nothing_else() {
    let (base_url, server) = spawn_http_sequence(vec![
        (
            200,
            "application/json",
            r#"{"allowedTiers":[{"id":"free-tier","isDefault":true}]}"#,
        ),
        (
            200,
            "application/json",
            r#"{"done":true,"response":{"cloudaicompanionProject":{"id":"proj-onboarded"}}}"#,
        ),
    ])
    .await;

    let client = reqwest::Client::new();
    let proj = resolve_project_id_at(&client, &base_url, "tok").await;
    assert_eq!(proj, "proj-onboarded");

    let seen = server.await.unwrap();
    assert_ua_only_contract(&seen[0], "loadCodeAssist", &[]);
    assert!(
        seen[1].starts_with("POST /v1internal:onboardUser"),
        "{}",
        seen[1]
    );
    assert_ua_only_contract(&seen[1], "onboardUser", &["x-goog-api-client"]);

    let headers = request_headers(&seen[1]);
    assert_eq!(
        header_value(&headers, "x-goog-api-client"),
        Some(ANTIGRAVITY_GOOG_API_CLIENT),
        "onboard manda o X-Goog-Api-Client do Node"
    );
    let ua = header_value(&headers, "user-agent").unwrap_or_default();
    assert!(
        ua.starts_with("antigravity/hub/") && ua.ends_with(" google-api-nodejs-client/10.3.0"),
        "onboard usa o UA do Hub + cliente Node, veio `{ua}`"
    );
}

#[tokio::test]
async fn fetch_available_models_carries_user_agent_only() {
    let (base_url, server) = spawn_http_sequence(vec![(
        200,
        "application/json",
        r#"{"models":{"gemini-4-hydrogen-high":{}}}"#,
    )])
    .await;

    let provider = AntigravityProvider::new(creds_expiring_at(u64::MAX)).with_base_url(base_url);
    let models = provider.list_models().await.unwrap();
    assert_eq!(models, vec!["gemini-4-hydrogen-high".to_string()]);

    let seen = server.await.unwrap();
    assert!(
        seen[0].starts_with("POST /v1internal:fetchAvailableModels"),
        "{}",
        seen[0]
    );
    assert_ua_only_contract(&seen[0], "fetchAvailableModels", &[]);
}
