use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use tupi_kernel::{AgentMode, Tool};
use tupi_tools::sdd::SddTool;

#[test]
fn set_tasks_accepts_parallel_touches_and_acceptance_test() {
    let dir = tempfile::tempdir().unwrap();
    let tool = SddTool::new(
        dir.path().to_path_buf(),
        Arc::new(tupi_tools::sdd::FsSddStore::new(dir.path())),
        Some(Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()))),
        2000,
    );

    tool.call(serde_json::json!({
        "action": "upsert_spec", "slug": "auth", "title": "Auth", "goal": "OAuth"
    }))
    .unwrap();
    tool.call(serde_json::json!({
        "action": "set_tasks",
        "tasks": [{
            "id": "T1", "title": "Refresh", "details": "Renovar", "parallel": true,
            "subtasks": [{
                "id": "T1.1", "title": "Lock", "acceptance": "Passa",
                "touches": ["src/store.rs"],
                "acceptance_test": { "package": "tupi-app", "filter": "sdd_run" }
            }]
        }]
    }))
    .unwrap();

    let tasks = std::fs::read_to_string(dir.path().join(".tupi/sdd/auth/tasks.json")).unwrap();
    assert!(tasks.contains("\"parallel\": true"), "{tasks}");
    assert!(tasks.contains("src/store.rs"), "{tasks}");
    assert!(tasks.contains("sdd_run"), "{tasks}");
}
