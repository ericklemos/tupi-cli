use tupi_kernel::ToolRegistry;

#[test]
fn agent_registry_has_code_tools_and_no_orchestration_tools() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    tupi_tools::register_agent(
        dir.path().to_path_buf(),
        &mut registry,
        tupi_tools::AgentToolsOptions::default(),
    );
    for name in [
        "read",
        "lookup",
        "edit",
        "write",
        "exec",
        "test_many",
        "map",
        "peek",
    ] {
        assert!(registry.has(name), "faltou {name}");
    }
    for name in [
        "sdd",
        "read_many",
        "search",
        "pipeline",
        "workflow",
        "fetch",
        "websearch",
        "shell",
        "state",
        "setup",
    ] {
        assert!(!registry.has(name), "não deveria ter {name}");
    }
    // O atrito é registrado pelo agente principal: o subagente não ganha uma
    // segunda saída de escrita para .agents/tupi/debug/feedback.jsonl.
    assert!(!registry.has("feedback"), "subagente não registra atrito");
}

#[test]
fn two_agent_registries_do_not_share_the_delivery_ledger() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "pub fn a() {}\n").unwrap();
    let build = || {
        let mut registry = ToolRegistry::new();
        tupi_tools::register_agent(
            dir.path().to_path_buf(),
            &mut registry,
            tupi_tools::AgentToolsOptions::default(),
        );
        registry
    };
    let first = build()
        .get("read")
        .unwrap()
        .call(serde_json::json!({"path": "a.rs"}))
        .unwrap();
    let second = build()
        .get("read")
        .unwrap()
        .call(serde_json::json!({"path": "a.rs"}))
        .unwrap();
    assert!(first.contains("pub fn a"), "{first}");
    assert!(second.contains("pub fn a"), "{second}");
    assert!(!second.contains("Já entregue"), "{second}");
}
