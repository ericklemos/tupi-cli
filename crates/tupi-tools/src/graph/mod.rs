use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use tupi_kernel::{Tool, ToolError, ToolSpec};

mod builder;
mod cache;
mod model;
mod parse_cache;
mod queries;
mod scope;
mod watcher;

pub use builder::GraphBuilder;
pub use cache::GraphCache;
pub use model::{CallGraph, Def, ProjectCodeGraph, RelationEdge, RelationKind, SymbolNode};
pub use parse_cache::ParseCache;
pub use scope::{FileScope, ImportTarget, Resolution, ScopeResolver};
pub use watcher::GraphWatcher;

/// Depois de uma tool que muta, o grafo é reconstruído (com os parses em
/// cache) em vez de ficar com arestas velhas.
pub(crate) struct RepairingTool {
    inner: Arc<dyn Tool>,
    cache: Arc<GraphCache>,
}

impl RepairingTool {
    pub(crate) fn new(inner: Arc<dyn Tool>, cache: Arc<GraphCache>) -> Self {
        Self { inner, cache }
    }

    fn paths(&self, arguments: &Value) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(path) = arguments.get("path").and_then(Value::as_str) {
            paths.push(self.cache.root().join(path));
        }
        if let Some(writes) = arguments.get("writes").and_then(Value::as_array) {
            for write in writes {
                if let Some(path) = write.get("path").and_then(Value::as_str) {
                    paths.push(self.cache.root().join(path));
                }
            }
        }
        paths
    }
}

impl Tool for RepairingTool {
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let paths = self.paths(&arguments);
        let result = self.inner.call(arguments);
        if result.is_ok() {
            if paths.is_empty() {
                self.cache.invalidate();
            } else {
                self.cache.repair(&paths);
            }
        }
        result
    }

    fn describe_call(&self, arguments: &Value) -> String {
        self.inner.describe_call(arguments)
    }

    fn mutates(&self) -> bool {
        true
    }
}
