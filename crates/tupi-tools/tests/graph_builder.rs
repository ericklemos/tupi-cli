use tupi_tools::graph::{GraphBuilder, ProjectCodeGraph};

fn write(root: &std::path::Path, relative: &str, content: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn a_unique_call_becomes_an_edge() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/b.rs", "pub fn caller() { helper(); }\n");

    let graph: ProjectCodeGraph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    let helper = graph.matching("helper");
    assert_eq!(helper.len(), 1);
    assert_eq!(graph.callers("helper").len(), 1);
    assert_eq!(graph.callers("helper")[0].name, "caller");
    assert_eq!(graph.callees("caller").len(), 1);
    assert_eq!(graph.callees("caller")[0].name, "helper");
}

#[test]
fn a_broken_file_does_not_take_the_other_files_down() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/broken.rs", "pub fn oops( {");

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    assert_eq!(graph.matching("helper").len(), 1);
}

#[test]
fn a_malformed_use_does_not_take_the_graph_down() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/bad.rs", "use a::}{b;\npub fn broken() {}\n");

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    assert_eq!(graph.matching("helper").len(), 1);
    assert_eq!(graph.matching("broken").len(), 1);
}

#[test]
fn a_duplicated_name_without_evidence_has_no_edge() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/b.rs", "pub fn helper() {}\n");
    write(dir.path(), "src/c.rs", "pub fn caller() { helper(); }\n");

    let graph = GraphBuilder::new().unwrap().build(dir.path()).unwrap();
    assert_eq!(graph.matching("helper").len(), 2);
    assert!(graph.callers("helper").is_empty());
    assert_eq!(graph.dropped_for("helper"), 1);
}
