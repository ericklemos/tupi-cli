//! Testes dos assets embutidos do frontend.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

async fn fetch(app: axum::Router, uri: &str) -> (StatusCode, String, String) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, content_type, body)
}

#[tokio::test]
async fn static_assets_are_embedded_with_proper_mime_types_and_no_cdn() {
    let (ctx, _tmp) = crate::routes::test_support::test_context();
    let app = crate::build_router(ctx);

    for (uri, expected_mime) in [
        ("/", "text/html"),
        ("/styles.css", "text/css"),
        ("/app.js", "application/javascript"),
        ("/graph.js", "application/javascript"),
    ] {
        let (status, content_type, body) = fetch(app.clone(), uri).await;
        assert_eq!(status, StatusCode::OK, "falhou em {uri}");
        assert!(
            content_type.contains(expected_mime),
            "{uri} retornou {content_type}"
        );
        assert!(
            !body.contains("https://cdn"),
            "{uri} não deve usar CDN externa"
        );
        assert!(
            !body.contains("http://cdn"),
            "{uri} não deve usar CDN externa"
        );
    }
}

#[tokio::test]
async fn index_wires_the_module_scripts_and_canvas() {
    let (ctx, _tmp) = crate::routes::test_support::test_context();
    let app = crate::build_router(ctx);

    let (status, _, html) = fetch(app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("src=\"/app.js\""));
    assert!(html.contains("graph-canvas"));
    assert!(html.contains("panel-memory"));
    assert!(html.contains("panel-session"));
    assert!(html.contains("panel-workspace"));
    assert!(
        html.contains("role=\"tablist\""),
        "as abas precisam de tablist"
    );
    assert!(
        html.contains("aria-selected=\"true\""),
        "a aba ativa precisa expor aria-selected"
    );
    assert!(
        html.contains("aria-controls=\"panel-graph\""),
        "cada aba precisa apontar para seu painel"
    );
    assert!(
        html.contains("role=\"tabpanel\""),
        "os painéis precisam de tabpanel"
    );
}
