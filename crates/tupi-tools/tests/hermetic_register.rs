use std::sync::Arc;

use tupi_kernel::{Tool, ToolRegistry};
use tupi_tools::{ToolsOptions, register_with};

#[test]
fn hermetic_register_writes_state_under_the_given_dir() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    register_with(
        root.path().to_path_buf(),
        &mut registry,
        ToolsOptions::hermetic(state.path().to_path_buf()),
    );

    let state_tool: Arc<dyn Tool> = registry.get("state").expect("state tool");
    state_tool
        .call(serde_json::json!({"action": "note", "text": "nota hermética"}))
        .unwrap();

    let files: Vec<_> = std::fs::read_dir(state.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(
        files.iter().any(|path| path.to_string_lossy().contains("state-")),
        "esperava state-*.jsonl em {:?}",
        files
    );
}

#[test]
fn hermetic_register_keeps_the_tool_catalog() {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    register_with(
        root.path().to_path_buf(),
        &mut registry,
        ToolsOptions::hermetic(state.path().to_path_buf()),
    );
    assert!(registry.specs().len() >= 26);
}
