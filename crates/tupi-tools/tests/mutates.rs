#[test]
fn mutating_tools_are_marked() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = tupi_kernel::ToolRegistry::new();
    tupi_tools::register(dir.path().to_path_buf(), &mut registry);

    let mut marked: Vec<String> = registry
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .filter(|name| {
            let tool = registry.get(name).unwrap();
            !tool.effect(&serde_json::Value::Null).is_read_only()
        })
        .collect();
    marked.sort();

    assert_eq!(
        marked,
        vec![
            "edit",
            "exec",
            "feedback",
            "move",
            "patch",
            "sdd",
            "setup",
            "shell",
            "state",
            "test_many",
            "write",
            "write_many"
        ]
    );
}
