//! O espaço é opcional: sem embedder nada é disparado e a tool continua igual.

use std::sync::Arc;

use tupi_kernel::ToolRegistry;
use tupi_tools::{ToolsOptions, register_with};

#[test]
fn tools_options_default_has_no_space() {
    assert!(ToolsOptions::default().space.is_none());
}

#[test]
fn register_with_accepts_a_space_index_without_embedder() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    let options = ToolsOptions {
        space: Some(Arc::new(tupi_tools::space::SpaceIndex::new())),
        ..ToolsOptions::default()
    };
    register_with(dir.path().to_path_buf(), &mut registry, options);
    assert!(registry.get("search").is_some());
}
