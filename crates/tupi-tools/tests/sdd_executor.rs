use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use tupi_kernel::{AgentMode, Tool, ToolRegistry};
use tupi_tools::ToolsOptions;
use tupi_tools::sdd::{InMemorySddStore, SddPlan};

type Executor = Arc<dyn Fn(&SddPlan) -> Result<String, String> + Send + Sync>;

fn executor(hits: Arc<AtomicUsize>) -> Executor {
    Arc::new(move |plan: &SddPlan| {
        hits.fetch_add(1, Ordering::SeqCst);
        Ok(format!("Execução SDD: {} tasks ok", plan.tasks.len()))
    })
}

fn seed(tool: &Arc<dyn Tool>) {
    tool.call(serde_json::json!({
        "action": "upsert_spec", "slug": "auth", "title": "Auth", "goal": "OAuth"
    }))
    .unwrap();
    tool.call(serde_json::json!({
        "action": "set_tasks",
        "tasks": [{
            "id": "T1", "title": "Refresh", "details": "Renovar",
            "subtasks": [{ "id": "T1.1", "title": "Lock", "acceptance": "Passa" }]
        }]
    }))
    .unwrap();
}

#[test]
fn close_runs_the_executor_and_returns_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySddStore::default());
    let mode = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
    let hits = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    tupi_tools::register_with(
        dir.path().to_path_buf(),
        &mut registry,
        ToolsOptions {
            sdd_store: Some(store),
            mode_cell: Some(mode),
            sdd_executor: Some(executor(hits.clone())),
            ..ToolsOptions::default()
        },
    );
    let tool = registry.get("sdd").unwrap();
    seed(&tool);

    let out = tool.call(serde_json::json!({"action": "close"})).unwrap();

    assert!(out.contains("Execução SDD: 1 tasks ok"), "{out}");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn execute_action_reruns_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemorySddStore::default());
    let mode = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
    let hits = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    tupi_tools::register_with(
        dir.path().to_path_buf(),
        &mut registry,
        ToolsOptions {
            sdd_store: Some(store),
            mode_cell: Some(mode.clone()),
            sdd_executor: Some(executor(hits.clone())),
            ..ToolsOptions::default()
        },
    );
    let tool = registry.get("sdd").unwrap();
    seed(&tool);
    tool.call(serde_json::json!({"action": "close"})).unwrap();

    mode.store(AgentMode::Execution.as_u8(), Ordering::SeqCst);
    let out = tool.call(serde_json::json!({"action": "execute"})).unwrap();

    assert!(out.contains("Execução SDD"), "{out}");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
