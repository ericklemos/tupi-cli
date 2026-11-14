//! Testes dos endpoints de memória, estado e telemetria.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use tupi_app::telemetry::Telemetry;
use tupi_kernel::{EventBus, Query, QueryBus, QueryError, QueryHandler, QueryResult};
use tupi_tools::graph::GraphCache;

use crate::DashboardContext;

struct StubQueries;

#[async_trait::async_trait]
impl QueryHandler for StubQueries {
    async fn query(&self, _query: Query) -> Result<QueryResult, QueryError> {
        Ok(QueryResult::History(Vec::new()))
    }
}

fn context(telemetry: Telemetry) -> (Arc<DashboardContext>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let state_dir = root.join("state");
    std::fs::create_dir_all(&state_dir).unwrap();

    let ctx = DashboardContext {
        graph: GraphCache::shared(&root),
        root,
        state_dir,
        events: EventBus::new(32),
        queries: QueryBus::new(Arc::new(StubQueries)),
        telemetry,
        model: "fake-model".to_string(),
        provider: "fake".to_string(),
    };
    (Arc::new(ctx), tmp)
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
async fn memory_and_state_endpoints_return_empty_lists_when_files_absent() {
    let (ctx, _tmp) = context(Telemetry::disabled());
    let app = crate::build_router(ctx);

    let (status, body) = get(app.clone(), "/api/memory").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["active_facts"].as_array().unwrap().len(), 0);
    assert_eq!(body["forgotten_facts"].as_array().unwrap().len(), 0);
    assert_eq!(body["open_pendings"].as_array().unwrap().len(), 0);
    assert_eq!(body["closed_pendings"].as_array().unwrap().len(), 0);

    let (status, body) = get(app, "/api/state").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["entries"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn memory_endpoint_separates_active_forgotten_and_pendings() {
    use tupi_domain::memory;

    let (ctx, _tmp) = context(Telemetry::disabled());
    let root = ctx.root.clone();

    memory::record(&root, "fato vivo", vec!["tag".to_string()], "teste").unwrap();
    let forgotten = memory::record(&root, "fato esquecido", Vec::new(), "teste")
        .unwrap()
        .unwrap();
    assert!(memory::forget(&root, &forgotten.id).unwrap());

    let mut pendings = Vec::new();
    assert!(memory::open_pending(&mut pendings, "p1", "kind", "sessao"));
    assert!(memory::open_pending(&mut pendings, "p2", "kind", "sessao"));
    memory::close_pending(&mut pendings, "p2", "teste", "resolvido");
    memory::save_pendings(&root, &pendings).unwrap();

    let app = crate::build_router(ctx);
    let (status, body) = get(app, "/api/memory").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["active_facts"].as_array().unwrap().len(), 1);
    assert_eq!(body["forgotten_facts"].as_array().unwrap().len(), 1);
    assert_eq!(body["open_pendings"].as_array().unwrap().len(), 1);
    assert_eq!(body["closed_pendings"].as_array().unwrap().len(), 1);
    assert_eq!(body["active_facts"][0]["text"], "fato vivo");
}

#[tokio::test]
async fn state_endpoint_lists_notes_written_by_the_state_tool() {
    let (ctx, _tmp) = context(Telemetry::disabled());
    let root = ctx.root.clone();
    let state_dir = ctx.state_dir.clone();

    let mut registry = tupi_kernel::ToolRegistry::new();
    tupi_tools::register_with(
        root.clone(),
        &mut registry,
        tupi_tools::ToolsOptions::hermetic(state_dir),
    );
    let state_tool = registry.get("state").unwrap();
    tupi_kernel::Tool::call(
        state_tool.as_ref(),
        serde_json::json!({ "action": "note", "text": "nota do dashboard" }),
    )
    .unwrap();

    let app = crate::build_router(ctx);
    let (status, body) = get(app, "/api/state").await;
    assert_eq!(status, StatusCode::OK);
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["text"], "nota do dashboard");
}

#[tokio::test]
async fn telemetry_recent_works_when_disabled() {
    let (ctx, _tmp) = context(Telemetry::disabled());
    let app = crate::build_router(ctx);

    let (status, body) = get(app, "/api/telemetry/recent").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["entries"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn telemetry_recent_returns_parsed_entries_when_file_exists() {
    let path = std::env::temp_dir().join(format!(
        "tupi-web-telemetry-{}-{}.jsonl",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &path,
        "{\"kind\":\"run_started\"}\n{\"kind\":\"turn_started\"}\n",
    )
    .unwrap();

    let (ctx, _tmp) = context(Telemetry::open(&path));
    let app = crate::build_router(ctx);
    let (status, body) = get(app, "/api/telemetry/recent").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], true);
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["kind"], "run_started");
    let _ = std::fs::remove_file(&path);
}
