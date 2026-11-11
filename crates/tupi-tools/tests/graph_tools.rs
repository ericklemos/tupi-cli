use std::sync::Arc;

use tupi_kernel::{Tool, ToolRegistry};
use tupi_tools::{ToolsOptions, register_with};

fn tool(root: &std::path::Path, name: &str) -> Arc<dyn Tool> {
    let state = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    register_with(
        root.to_path_buf(),
        &mut registry,
        ToolsOptions::hermetic(state.path().to_path_buf()),
    );
    registry.get(name).expect(name)
}

#[test]
fn impact_does_not_count_a_same_name_in_another_module() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("src/b")).unwrap();
    std::fs::write(dir.path().join("src/a/mod.rs"), "pub fn new() {}\n").unwrap();
    std::fs::write(dir.path().join("src/b/mod.rs"), "pub fn new() {}\n").unwrap();
    std::fs::write(
        dir.path().join("src/main.rs"),
        "mod a;\nmod b;\npub fn caller() { a::new(); }\n",
    )
    .unwrap();

    let impact = tool(dir.path(), "impact_check");
    let out = impact
        .call(serde_json::json!({"symbol_name": "new"}))
        .unwrap();
    let callers = out.split("#### chamadores").nth(1).unwrap_or("");
    assert!(!callers.contains("src/b/mod.rs"), "{out}");
    assert!(out.contains("- chamadores: 1"), "{out}");
}
