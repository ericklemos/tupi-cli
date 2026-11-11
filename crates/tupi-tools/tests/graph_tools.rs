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

fn fixture(qualified: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/a")).unwrap();
    std::fs::create_dir_all(dir.path().join("src/b")).unwrap();
    std::fs::write(dir.path().join("src/a/mod.rs"), "pub fn new() {}\n").unwrap();
    std::fs::write(dir.path().join("src/b/mod.rs"), "pub fn new() {}\n").unwrap();
    let call = if qualified { "a::new();" } else { "new();" };
    std::fs::write(
        dir.path().join("src/main.rs"),
        format!("mod a;\nmod b;\npub fn caller() {{ {call} }}\n"),
    )
    .unwrap();
    dir
}

#[test]
fn impact_does_not_count_an_ambiguous_bare_call() {
    let dir = fixture(false);
    let impact = tool(dir.path(), "impact_check");
    let out = impact
        .call(serde_json::json!({"symbol_name": "new"}))
        .unwrap();
    assert!(out.contains("- chamadores: 0"), "{out}");
}

#[test]
fn context_graph_keeps_callees_per_definition() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub struct A;\npub struct B;\n\
         impl A { pub fn run(&self) { helper_a(); } }\n\
         impl B { pub fn run(&self) { helper_b(); } }\n\
         pub fn helper_a() {}\npub fn helper_b() {}\n",
    )
    .unwrap();

    let context = tool(dir.path(), "context_graph");
    let out = context
        .call(serde_json::json!({"target_symbol_or_file": "A::run"}))
        .unwrap_or_else(|error| panic!("context_graph falhou: {error:?}"));
    assert!(out.contains("helper_a"), "{out}");
    assert!(!out.contains("helper_b"), "{out}");
}

#[test]
fn impact_counts_a_qualified_call_once() {
    let dir = fixture(true);
    let impact = tool(dir.path(), "impact_check");
    let out = impact
        .call(serde_json::json!({"symbol_name": "new"}))
        .unwrap();
    let callers = out.split("#### chamadores").nth(1).unwrap_or("");
    assert!(!callers.contains("src/b/mod.rs"), "{out}");
    assert!(out.contains("- chamadores: 1"), "{out}");
}
