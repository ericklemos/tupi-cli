use tupi_tools::graph::GraphBuilder;

#[test]
fn kotlin_defs_and_calls_are_indexed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Greeter.kt"),
        "class Greeter { fun greet() {} }\nfun run() { Greeter().greet() }\n",
    )
    .unwrap();

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    assert_eq!(graph.matching("Greeter").len(), 1);
    assert_eq!(graph.matching("greet").len(), 1);
    assert_eq!(graph.callers("greet").len(), 1);
}

#[test]
fn a_rust_and_a_kotlin_name_do_not_become_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "pub fn helper() {}\n").unwrap();
    std::fs::write(dir.path().join("b.kt"), "fun helper() {}\n").unwrap();
    std::fs::write(dir.path().join("c.rs"), "pub fn caller() { helper(); }\n").unwrap();

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    assert_eq!(graph.callers("helper").len(), 1);
    assert_eq!(graph.callers("helper")[0].relative, "c.rs");
}
