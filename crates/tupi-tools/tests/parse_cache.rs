use std::path::Path;
use std::sync::Arc;

use tupi_tools::ast::{AstChunker, Language};
use tupi_tools::graph::ParseCache;

#[test]
fn the_same_content_returns_the_same_tree() {
    let cache = Arc::new(ParseCache::new());
    let first = cache
        .parse(Path::new("a.rs"), Language::Rust, "pub fn a() {}")
        .unwrap();
    let second = cache
        .parse(Path::new("a.rs"), Language::Rust, "pub fn a() {}")
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn changed_content_returns_a_new_tree() {
    let cache = Arc::new(ParseCache::new());
    let first = cache
        .parse(Path::new("a.rs"), Language::Rust, "pub fn a() {}")
        .unwrap();
    let second = cache
        .parse(Path::new("a.rs"), Language::Rust, "pub fn b() {}")
        .unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
}

#[test]
fn the_chunker_uses_the_shared_cache() {
    let cache = Arc::new(ParseCache::new());
    let chunker = AstChunker::with_cache(cache.clone());
    let snippets = chunker.snippets(Path::new("a.rs"), "pub fn a() {}");
    assert_eq!(snippets.len(), 1);
    assert_eq!(cache.len(), 1);
}
