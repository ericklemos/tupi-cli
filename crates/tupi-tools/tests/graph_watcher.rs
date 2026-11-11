use std::sync::Arc;
use std::time::{Duration, Instant};

use tupi_tools::graph::{GraphCache, GraphWatcher};

#[test]
fn an_external_write_updates_the_graph() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let cache = Arc::new(GraphCache::new(dir.path().to_path_buf()));
    assert!(cache.get().matching("beta").is_empty());
    let _watcher = GraphWatcher::start(dir.path().to_path_buf(), cache.clone()).unwrap();

    std::fs::write(
        dir.path().join("a.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if !cache.get().matching("beta").is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "watcher não aplicou a mudança");
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn an_external_delete_removes_the_symbols() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let cache = Arc::new(GraphCache::new(dir.path().to_path_buf()));
    assert_eq!(cache.get().matching("alpha").len(), 1);
    let _watcher = GraphWatcher::start(dir.path().to_path_buf(), cache.clone()).unwrap();

    std::fs::remove_file(dir.path().join("a.rs")).unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if cache.get().matching("alpha").is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "watcher não removeu o símbolo");
        std::thread::sleep(Duration::from_millis(25));
    }
}
