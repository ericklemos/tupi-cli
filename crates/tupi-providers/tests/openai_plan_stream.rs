//! Stream do `openai-plan` contra um servidor local: fixture SSE, 401 e stall.
//!
//! Nenhuma requisição sai da máquina: o provedor aponta para um `TcpListener`
//! em `127.0.0.1` que devolve o corpo fixo ou aceita e fica calado (stall).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use futures_util::StreamExt;
use tupi_domain::{Message, ToolCall};
use tupi_kernel::{ChatRequest, Provider, ProviderError, ProviderEvent};
use tupi_providers::openai_plan::{OpenAiPlanCredentials, OpenAiPlanProvider};

/// Requisição crua que o servidor recebeu, para conferir headers e corpo.
type Capture = Arc<Mutex<String>>;

/// Regra do projeto: o provedor pergunta quais modelos a conta tem em vez de
/// servir um literal. O `TcpListener` local faz o papel do backend.
#[tokio::test]
async fn list_models_asks_the_backend() {
    let body = r#"{"models":[{"slug":"gpt-6.2-sol"},{"slug":"gpt-6.2-astra"}]}"#;
    let (base_url, captured) = spawn_server("200 OK", body.to_string());

    let models = provider(base_url).list_models().await.unwrap();

    assert_eq!(
        models,
        vec!["gpt-6.2-sol".to_string(), "gpt-6.2-astra".to_string()]
    );
    let request = captured.lock().unwrap().clone();
    assert!(request.starts_with("GET /models "), "{request}");
}

/// 401 no catálogo não vira lista embutida: sobe como erro, para o login
/// aparecer em vez de uma lista que mente sobre a conta.
#[tokio::test]
async fn list_models_surfaces_unauthorized() {
    let (base_url, _captured) = spawn_server("401 Unauthorized", r#"{"error":"x"}"#.to_string());

    match provider(base_url).list_models().await {
        Err(ProviderError::Http { status, .. }) => assert_eq!(status, 401),
        other => panic!("esperava Http 401, veio {other:?}"),
    }
}

fn spawn_server(status_line: &'static str, body: String) -> (String, Capture) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind do servidor de teste");
    let addr = listener.local_addr().expect("endereco local");
    let captured: Capture = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&captured);

    thread::spawn(move || {
        if let Ok((mut socket, _)) = listener.accept() {
            let mut buf = vec![0u8; 16384];
            let read = socket.read(&mut buf).unwrap_or_default();
            if let Ok(mut sink) = sink.lock() {
                *sink = String::from_utf8_lossy(&buf[..read]).to_string();
            }
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes());
        }
    });

    (format!("http://{addr}"), captured)
}

/// Cabeçalho completo, corpo prometido e nada enviado: o cliente estaciona.
fn spawn_stall_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind do servidor de teste");
    let addr = listener.local_addr().expect("endereco local");

    thread::spawn(move || {
        if let Ok((mut socket, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf);
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n";
            let _ = socket.write_all(head.as_bytes());
            let _ = socket.flush();
            thread::sleep(Duration::from_secs(5));
        }
    });

    format!("http://{addr}")
}

fn credentials() -> OpenAiPlanCredentials {
    OpenAiPlanCredentials {
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        expires_at: u64::MAX,
        account_id: Some("acct_123".to_string()),
        email: None,
    }
}

fn provider(base_url: String) -> OpenAiPlanProvider {
    OpenAiPlanProvider::new(credentials()).with_base_url(base_url)
}

/// `Started` → `Text` → `ToolCalls` → `Usage` → `Finished`.
const SSE_BODY: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{}}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"output_index\":1,\"delta\":\"oi\"}\n\n",
    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read\"}}\n\n",
    "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"path\\\":\"}\n\n",
    "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"\\\"a.rs\\\"}\"}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":11,\"input_tokens_details\":{\"cached_tokens\":5},\"output_tokens\":7}}}\n\n",
);

#[tokio::test]
async fn stream_reports_started_text_tool_calls_usage_and_finished() {
    let (base_url, captured) = spawn_server("200 OK", SSE_BODY.to_string());
    let provider = provider(base_url);

    let mut stream = provider
        .stream_chat(ChatRequest::new(
            "gpt-5.1-codex",
            vec![Message::user("oi")],
        ))
        .await
        .expect("stream aberto");

    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        events.push(item.expect("evento do stream"));
    }

    assert_eq!(
        events,
        vec![
            ProviderEvent::Started,
            ProviderEvent::Text("oi".to_string()),
            ProviderEvent::ToolCalls(vec![ToolCall {
                id: "call_1".to_string(),
                name: "read".to_string(),
                arguments: "{\"path\":\"a.rs\"}".to_string(),
            }]),
            ProviderEvent::Usage {
                prompt_tokens: Some(11),
                cached_tokens: Some(5),
                completion_tokens: Some(7),
            },
            ProviderEvent::Finished("stop".to_string()),
        ]
    );

    let request = captured.lock().expect("captura").to_lowercase();
    assert!(request.starts_with("post /responses "), "{request}");
    assert!(request.contains("authorization: bearer access"), "{request}");
    assert!(request.contains("chatgpt-account-id: acct_123"), "{request}");
    assert!(request.contains("\"store\":false"), "{request}");
    assert!(request.contains("\"stream\":true"), "{request}");
    assert!(request.contains("\"model\":\"gpt-5.1-codex\""), "{request}");

    assert_eq!(provider.last_payload().expect("payload")["model"], "gpt-5.1-codex");
}

#[tokio::test]
async fn unauthorized_answer_surfaces_as_http_error() {
    let body = r#"{"error":{"message":"token inválido","code":"unauthorized"}}"#;
    let (base_url, _captured) = spawn_server("401 Unauthorized", body.to_string());

    let result = provider(base_url)
        .stream_chat(ChatRequest::new("gpt-5.1-codex", vec![Message::user("oi")]))
        .await;

    match result {
        Err(ProviderError::Http { status, body: raw }) => {
            assert_eq!(status, 401);
            assert!(raw.contains("token inválido"), "{raw}");
        }
        Ok(_) => panic!("401 precisa subir como ProviderError::Http, veio um stream"),
        Err(other) => panic!("401 precisa subir como ProviderError::Http, veio {other:?}"),
    }
}

#[tokio::test]
async fn stalled_stream_fails_with_network_error() {
    let base_url = spawn_stall_server();
    let provider = provider(base_url).with_stall_timeout(Duration::from_millis(200));

    let mut stream = provider
        .stream_chat(ChatRequest::new("gpt-5.1-codex", vec![Message::user("oi")]))
        .await
        .expect("stream aberto");

    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        events.push(item);
    }

    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(
        events[0].as_ref().expect("started"),
        &ProviderEvent::Started
    );
    match events[1].as_ref() {
        Err(ProviderError::Network(message)) => {
            assert!(message.contains("sem dados"), "{message}");
        }
        other => panic!("stream parado precisa estourar Network, veio {other:?}"),
    }
}
