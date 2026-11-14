use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tupi_app::telemetry::{StatsSnapshot, Telemetry};
use tupi_domain::{Message, Workflow};
use tupi_kernel::{EventBus, Query, QueryBus, QueryResult};
use tupi_tools::graph::GraphCache;

/// Handles vivos da sessão, montados por `crates/tupi/src/main.rs`.
pub struct DashboardContext {
    pub root: PathBuf,
    pub state_dir: PathBuf,
    pub graph: Arc<GraphCache>,
    pub events: EventBus,
    pub queries: QueryBus,
    pub telemetry: Telemetry,
    pub model: String,
    pub provider: String,
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) ctx: Arc<DashboardContext>,
    pub(crate) hub: crate::events::EventHub,
}

pub(crate) fn router(ctx: Arc<DashboardContext>) -> Router {
    let hub = crate::events::EventHub::new(&ctx.events, 200);
    Router::new()
        .route("/", get(crate::assets::index))
        .route("/styles.css", get(crate::assets::styles))
        .route("/app.js", get(crate::assets::app))
        .route("/graph.js", get(crate::assets::graph))
        .route("/api/overview", get(overview))
        .route("/api/history", get(history))
        .route("/api/workflows", get(workflows))
        .route("/api/graph/summary", get(crate::graph_api::summary))
        .route("/api/graph/search", get(crate::graph_api::search))
        .route("/api/graph/neighborhood", get(crate::graph_api::neighborhood))
        .route("/api/graph/trace", get(crate::graph_api::trace))
        .route("/api/memory", get(crate::inspect_api::memory))
        .route("/api/state", get(crate::inspect_api::state))
        .route("/api/telemetry/recent", get(crate::inspect_api::telemetry_recent))
        .route("/api/events", get(crate::events::events))
        .with_state(AppState { ctx, hub })
}

#[derive(Serialize)]
pub(crate) struct StatsDto {
    commands: u64,
    queries: u64,
    turns: u64,
    provider_requests: u64,
    tool_calls: u64,
    tool_failures: u64,
    prompt_tokens: u64,
    cached_tokens: u64,
    completion_tokens: u64,
    errors: u64,
    dropped: u64,
}

impl From<StatsSnapshot> for StatsDto {
    fn from(stats: StatsSnapshot) -> Self {
        Self {
            commands: stats.commands,
            queries: stats.queries,
            turns: stats.turns,
            provider_requests: stats.provider_requests,
            tool_calls: stats.tool_calls,
            tool_failures: stats.tool_failures,
            prompt_tokens: stats.prompt_tokens,
            cached_tokens: stats.cached_tokens,
            completion_tokens: stats.completion_tokens,
            errors: stats.errors,
            dropped: stats.dropped,
        }
    }
}

#[derive(Serialize)]
struct OverviewDto {
    provider: String,
    model: String,
    root: String,
    state_dir: String,
    telemetry_enabled: bool,
    telemetry_path: Option<String>,
    stats: StatsDto,
    fact_count: usize,
    open_pending_count: usize,
}

async fn overview(State(state): State<AppState>) -> Json<OverviewDto> {
    let ctx = state.ctx;
    let facts = tupi_domain::memory::records(&ctx.root);
    let pendings = tupi_domain::memory::pendings(&ctx.root);

    Json(OverviewDto {
        provider: ctx.provider.clone(),
        model: ctx.model.clone(),
        root: ctx.root.display().to_string(),
        state_dir: ctx.state_dir.display().to_string(),
        telemetry_enabled: ctx.telemetry.is_enabled(),
        telemetry_path: ctx
            .telemetry
            .path()
            .map(|path| path.display().to_string()),
        stats: ctx.telemetry.stats().into(),
        fact_count: facts.iter().filter(|fact| fact.live()).count(),
        open_pending_count: pendings.iter().filter(|pending| pending.open()).count(),
    })
}

#[derive(Serialize)]
struct MessageDto {
    role: &'static str,
    content: String,
    tool_calls: Vec<ToolCallDto>,
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct ToolCallDto {
    id: String,
    name: String,
    arguments: String,
}

fn role_name(role: tupi_domain::Role) -> &'static str {
    match role {
        tupi_domain::Role::System => "system",
        tupi_domain::Role::User => "user",
        tupi_domain::Role::Assistant => "assistant",
        tupi_domain::Role::Tool => "tool",
    }
}

fn message_dto(message: Message) -> MessageDto {
    MessageDto {
        role: role_name(message.role),
        content: message.content,
        tool_calls: message
            .tool_calls
            .into_iter()
            .map(|call| ToolCallDto {
                id: call.id,
                name: call.name,
                arguments: call.arguments,
            })
            .collect(),
        tool_call_id: message.tool_call_id,
    }
}

#[derive(Serialize)]
struct HistoryDto {
    messages: Vec<MessageDto>,
}

async fn history(State(state): State<AppState>) -> Json<HistoryDto> {
    let messages = match state.ctx.queries.dispatch(Query::History).await {
        Ok(QueryResult::History(messages)) => messages.into_iter().map(message_dto).collect(),
        _ => Vec::new(),
    };
    Json(HistoryDto { messages })
}

#[derive(Serialize)]
struct WorkflowDto {
    name: String,
    description: String,
    source: String,
    path: Option<String>,
    shadowed: Vec<String>,
}

fn workflow_source_name(source: &tupi_domain::WorkflowSource) -> String {
    format!("{source:?}")
}

fn workflow_dto(workflow: Workflow) -> WorkflowDto {
    WorkflowDto {
        name: workflow.name,
        description: workflow.description,
        source: workflow_source_name(&workflow.source),
        path: workflow.path.map(|path| path.display().to_string()),
        shadowed: workflow
            .shadowed
            .iter()
            .map(workflow_source_name)
            .collect(),
    }
}

#[derive(Serialize)]
struct WorkflowsDto {
    workflows: Vec<WorkflowDto>,
}

async fn workflows(State(state): State<AppState>) -> Json<WorkflowsDto> {
    let workflows = match state.ctx.queries.dispatch(Query::ListWorkflows).await {
        Ok(QueryResult::Workflows(workflows)) => workflows.into_iter().map(workflow_dto).collect(),
        _ => Vec::new(),
    };
    Json(WorkflowsDto { workflows })
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use tupi_kernel::{QueryError, QueryHandler};

    struct StubQueries;

    #[async_trait::async_trait]
    impl QueryHandler for StubQueries {
        async fn query(&self, _query: Query) -> Result<QueryResult, QueryError> {
            Ok(QueryResult::History(Vec::new()))
        }
    }

    pub(crate) fn test_context() -> (Arc<DashboardContext>, tempfile::TempDir) {
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
            telemetry: Telemetry::disabled(),
            model: "fake-model".to_string(),
            provider: "fake".to_string(),
        };
        (Arc::new(ctx), tmp)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::test_support::test_context;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn overview_history_and_workflows_endpoints_return_session_snapshot() {
        let (ctx, _tmp) = test_context();
        let app = crate::build_router(ctx);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/overview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["provider"], "fake");
        assert_eq!(body["model"], "fake-model");
        assert!(body["stats"]["turns"].is_number());

        let hist = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hist.status(), StatusCode::OK);

        let wf = app
            .oneshot(
                Request::builder()
                    .uri("/api/workflows")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wf.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn start_server_binds_strictly_to_loopback() {
        let (ctx, _tmp) = test_context();
        let handle = crate::start_server(Arc::clone(&ctx), 0).await.unwrap();
        assert!(handle.url().starts_with("http://127.0.0.1:"));
        handle.shutdown().await;
    }
}
