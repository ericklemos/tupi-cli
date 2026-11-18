//! Formato de erro do Cloud Code (`{"error":{code,status,message}}`): o payload
//! precisa virar `ProviderError` tipado com status e corpo originais, em vez de
//! chunk vazio.

use tupi_kernel::ProviderError;
use tupi_providers::antigravity::extract_chunk;

#[test]
fn chunk_reports_rate_limited_with_retry_after() {
    let payload = r#"{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"Resource has been exhausted (e.g. check quota).","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"37s"}]}}"#;

    match extract_chunk(payload).unwrap_err() {
        ProviderError::RateLimited {
            status,
            retry_after,
            message,
            body,
        } => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, Some(37));
            assert!(message.contains("exhausted"), "mensagem: {message}");
            assert_eq!(body, payload);
        }
        other => panic!("esperava RateLimited, veio {other:?}"),
    }
}

#[test]
fn chunk_reports_quota_exhausted_with_reset_from_wrapped_response() {
    let payload = r#"{"response":{"error":{"code":429,"status":"RESOURCE_EXHAUSTED","message":"You exceeded your current quota, please check your plan and billing details.","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"120s"}]}}}"#;

    match extract_chunk(payload).unwrap_err() {
        ProviderError::QuotaExhausted {
            status,
            reset,
            message,
            body,
        } => {
            assert_eq!(status, 429);
            assert_eq!(reset, Some(120));
            assert!(message.contains("quota"), "mensagem: {message}");
            assert_eq!(body, payload);
        }
        other => panic!("esperava QuotaExhausted, veio {other:?}"),
    }
}

#[test]
fn chunk_reports_invalid_argument() {
    let payload = r#"{"error":{"code":400,"status":"INVALID_ARGUMENT","message":"invalid model name"}}"#;

    match extract_chunk(payload).unwrap_err() {
        ProviderError::InvalidArgument {
            status,
            message,
            body,
        } => {
            assert_eq!(status, 400);
            assert_eq!(message, "invalid model name");
            assert_eq!(body, payload);
        }
        other => panic!("esperava InvalidArgument, veio {other:?}"),
    }
}

#[test]
fn chunk_reports_http_error_for_server_failure() {
    let payload = r#"{"error":{"code":500,"status":"INTERNAL","message":"internal error"}}"#;

    match extract_chunk(payload).unwrap_err() {
        ProviderError::Http { status, body } => {
            assert_eq!(status, 500);
            assert_eq!(body, payload);
        }
        other => panic!("esperava Http, veio {other:?}"),
    }
}
