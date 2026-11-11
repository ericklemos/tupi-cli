use tupi_tools::graph::GraphCache;

#[test]
fn repair_updates_one_file_without_rebuilding_everything() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let cache = GraphCache::new(dir.path().to_path_buf());
    assert!(cache.get().matching("alpha").len() == 1);

    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .unwrap();
    cache.repair(&[dir.path().join("a.rs")]);

    assert_eq!(cache.get().matching("beta").len(), 1);
}

#[test]
fn repair_removes_a_deleted_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let cache = GraphCache::new(dir.path().to_path_buf());
    assert_eq!(cache.get().matching("alpha").len(), 1);

    std::fs::remove_file(dir.path().join("a.rs")).unwrap();
    cache.repair(&[dir.path().join("a.rs")]);
    assert!(cache.get().matching("alpha").is_empty());
}
