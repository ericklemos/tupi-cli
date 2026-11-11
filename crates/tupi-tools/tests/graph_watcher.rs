use std::sync::Arc;
use std::time::{Duration, Instant};

use tupi_tools::graph::{GraphCache, GraphWatcher};

#[test]
fn an_external_write_updates_the_graph() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "pub fn alpha() {}\n").unwrap();
    let cache = Arc::new(GraphCache::new(dir.path().to_path_buf()));
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
