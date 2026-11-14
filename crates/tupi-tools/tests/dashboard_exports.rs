#[test]
fn graph_module_reexports_public_read_types() {
    let _: fn(&std::path::Path) -> std::sync::Arc<tupi_tools::graph::GraphCache> =
        tupi_tools::graph::GraphCache::shared;
    let _empty = tupi_tools::graph::ProjectCodeGraph::new();
    let _kind = tupi_tools::graph::RelationKind::Calls;
    let _section = tupi_tools::graph::SectionKind::Member;
    let _id = tupi_tools::graph::SectionId::new("a.rs", "foo");
    fn _accepts(
        _: Option<&tupi_tools::graph::SymbolNode>,
        _: Option<&tupi_tools::graph::Def>,
        _: Option<&tupi_tools::graph::RelationEdge>,
    ) {
    }
}

#[test]
fn read_state_entries_returns_empty_when_missing_and_reads_saved_notes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    let state_dir = tmp.path().join("state");
    std::fs::create_dir_all(&root).unwrap();

    assert!(tupi_tools::read_state_entries(&root, &state_dir).is_empty());

    let mut registry = tupi_kernel::ToolRegistry::new();
    tupi_tools::register_with(
        root.clone(),
        &mut registry,
        tupi_tools::ToolsOptions::hermetic(state_dir.clone()),
    );
    let state_tool = registry.get("state").unwrap();
    state_tool
        .call(serde_json::json!({ "action": "note", "text": "decisão registrada" }))
        .unwrap();

    let entries: Vec<tupi_tools::StateEntry> = tupi_tools::read_state_entries(&root, &state_dir);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, "note");
    assert_eq!(entries[0].text, "decisão registrada");
    assert!(!tupi_tools::default_state_dir().as_os_str().is_empty());
}
