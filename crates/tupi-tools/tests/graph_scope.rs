use tupi_tools::graph::GraphBuilder;

fn write(root: &std::path::Path, relative: &str, content: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn an_explicit_use_resolves_to_the_imported_file() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/b.rs", "pub fn helper() {}\n");
    write(
        dir.path(),
        "src/c.rs",
        "use crate::a::helper;\npub fn caller() { helper(); }\n",
    );

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    let callers = graph.callers_in_file("helper", "src/c.rs");
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].relative, "src/c.rs");
    let targets = graph.callees("caller");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].relative, "src/a.rs");
}

#[test]
fn two_definitions_without_import_are_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/b.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/c.rs", "pub fn caller() { helper(); }\n");

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    assert!(graph.callers("helper").is_empty());
    assert_eq!(graph.dropped_for("helper"), 1);
}

#[test]
fn a_typed_receiver_resolves_the_method() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/lib.rs",
        "pub struct Builder;\npub struct Other;\n\
         impl Builder { pub fn build(&self) {} }\n\
         impl Other { pub fn build(&self) {} }\n\
         pub fn run() { let builder = Builder::new(); builder.build(); }\n",
    );
    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    let callers = graph.callers("build");
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].name, "run");
}
