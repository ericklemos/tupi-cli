use std::sync::Arc;

use tupi_kernel::{Tool, ToolRegistry};

#[test]
fn docs_agent_registry_has_only_allowed_tools() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let mut registry = ToolRegistry::new();
    let docs = tupi_tools::register_docs_agent(
        dir.path().to_path_buf(),
        &mut registry,
        tupi_tools::DocsAgentOptions {
            docs_folders: vec![dir.path().join("docs")],
            embedder: None,
            generator: None,
        },
    );
    let names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();
    assert_eq!(
        names,
        vec!["read", "read_many", "search", "map", "peek", "docs"]
    );
    assert!(!registry.has("write"));
    assert!(!registry.has("edit"));
    assert!(!registry.has("exec"));
    assert!(docs.written_files().is_empty());
}

#[test]
fn docs_agent_can_write_docs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    let mut registry = ToolRegistry::new();
    let docs = tupi_tools::register_docs_agent(
        dir.path().to_path_buf(),
        &mut registry,
        tupi_tools::DocsAgentOptions {
            docs_folders: vec![dir.path().join("docs")],
            embedder: None,
            generator: None,
        },
    );
    let tool: Arc<dyn Tool> = registry.get("docs").unwrap();
    tool.call(serde_json::json!({
        "action": "write",
        "path": "docs/a.md",
        "content": "# A\n"
    }))
    .unwrap();
    assert_eq!(docs.written_files(), vec!["docs/a.md"]);
}
