//! Testes dos endpoints HTTP do grafo de código.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

fn project(source: &str) -> (Arc<crate::DashboardContext>, tempfile::TempDir) {
    let (ctx, tmp) = crate::routes::test_support::test_context();
    std::fs::write(ctx.root.join("lib.rs"), source).unwrap();
    (ctx, tmp)
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn graph_endpoints_handle_empty_graph_and_unknown_symbol() {
    let (ctx, _tmp) = crate::routes::test_support::test_context();
    let app = crate::build_router(ctx);

    let (status, body) = get(app.clone(), "/api/graph/summary").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["node_count"], 0);
    assert_eq!(body["edge_count"], 0);

    let (status, body) = get(
        app,
        "/api/graph/neighborhood?symbol=funcao_que_nao_existe",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn graph_search_neighborhood_and_trace_return_expected_subgraph() {
    let (ctx, _tmp) = project("fn a() { b(); }\nfn b() { c(); }\nfn c() {}\n");
    let app = crate::build_router(ctx);

    let (status, body) = get(app.clone(), "/api/graph/search?q=a").await;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(results.iter().any(|item| item["name"] == "a"));

    let (status, body) = get(app.clone(), "/api/graph/neighborhood?symbol=b&depth=1").await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect();
    assert!(names.contains(&"a"), "nós: {names:?}");
    assert!(names.contains(&"b"), "nós: {names:?}");
    assert!(names.contains(&"c"), "nós: {names:?}");
    assert_eq!(body["truncated"], false);

    let (status, body) = get(app, "/api/graph/trace?from=a&to=c").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["found"], true);
    let path: Vec<&str> = body["path"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item.as_str())
        .collect();
    assert_eq!(path, vec!["a", "b", "c"]);
}

#[tokio::test]
async fn neighborhood_clamps_nodes_at_limit_and_marks_truncated() {
    let mut source = String::from("fn hub() {\n");
    for index in 0..15 {
        source.push_str(&format!("    leaf_{index}();\n"));
    }
    source.push_str("}\n");
    for index in 0..15 {
        source.push_str(&format!("fn leaf_{index}() {{}}\n"));
    }

    let (ctx, _tmp) = project(&source);
    let app = crate::build_router(ctx);

    let (status, body) = get(app, "/api/graph/neighborhood?symbol=hub&limit=5").await;
    assert_eq!(status, StatusCode::OK);
    let nodes = body["nodes"].as_array().unwrap();
    assert!(nodes.len() <= 5);
    assert_eq!(body["truncated"], true);
    assert!(body["total_candidates"].as_u64().unwrap() >= 15);
}
