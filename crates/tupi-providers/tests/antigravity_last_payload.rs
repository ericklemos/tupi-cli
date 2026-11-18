//! `last_payload` precisa guardar o envelope do ultimo request que falhou
//! junto do corpo cru do erro: e o unico rastro de diagnostico quando o Cloud
//! Code responde 429 e o erro sobe como ProviderError.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use tupi_domain::Message;
use tupi_kernel::{ChatRequest, Provider, ProviderError};
use tupi_providers::antigravity::{AntigravityCredentials, AntigravityProvider};

const ERROR_BODY: &str = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"Resource has been exhausted (e.g. check quota)."}}"#;

fn spawn_error_server(status_line: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind do servidor de teste");
    let addr = listener.local_addr().expect("endereco local");

    thread::spawn(move || {
        if let Ok((mut socket, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes());
        }
    });

    format!("http://{addr}")
}

fn credentials() -> AntigravityCredentials {
    AntigravityCredentials {
        access_token: "tok".into(),
        refresh_token: "ref".into(),
        expires_at: u64::MAX,
        project_id: "proj-1".into(),
        email: None,
    }
}

#[tokio::test]
async fn last_payload_holds_failed_request_envelope_and_raw_error() {
    let base_url = spawn_error_server("429 Too Many Requests", ERROR_BODY);
    let provider = AntigravityProvider::new(credentials()).with_base_url(base_url);

    let result = provider
        .stream_chat(ChatRequest::new(
            "gemini-3.8-flash-high",
            vec![Message::user("oi")],
        ))
        .await;

    assert!(
        matches!(result, Err(ProviderError::Http { status: 429, .. })),
        "erro HTTP do Cloud Code precisa subir como ProviderError visivel"
    );

    let stored = provider
        .last_payload()
        .expect("last_payload precisa guardar o request que falhou");

    // Envelope exato do request que falhou.
    assert_eq!(stored["request"]["project"], "proj-1");
    assert_eq!(stored["request"]["requestType"], "agent");
    assert_eq!(stored["request"]["userAgent"], "antigravity");
    assert_eq!(stored["request"]["contents"][0]["parts"][0]["text"], "oi");

    // Corpo cru do erro preservado, e decodificado quando e JSON.
    assert_eq!(stored["raw"], ERROR_BODY);
    assert_eq!(stored["error"]["error"]["code"], 429);
    assert_eq!(stored["error"]["error"]["status"], "RESOURCE_EXHAUSTED");
}
