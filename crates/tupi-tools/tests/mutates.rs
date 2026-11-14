#[test]
fn mutating_tools_are_marked() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = tupi_kernel::ToolRegistry::new();
    tupi_tools::register(dir.path().to_path_buf(), &mut registry);

    let mut marked: Vec<String> = registry
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .filter(|name| registry.get(name).unwrap().mutates())
        .collect();
    marked.sort();

    assert_eq!(
        marked,
        vec![
            "edit",
            "move",
            "patch",
            "sdd",
            "setup",
            "shell",
            "state",
            "write",
            "write_many"
        ]
    );
}
