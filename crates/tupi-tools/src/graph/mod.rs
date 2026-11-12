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
pub use model::{
    CallGraph, Def, ProjectCodeGraph, RelationEdge, RelationKind, SectionId, SectionKind,
    SectionNode, SymbolNode,
};
pub use parse_cache::ParseCache;
pub use scope::{FileScope, ImportTarget, Resolution, ScopeResolver};
pub use watcher::GraphWatcher;

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
        for key in ["path", "from", "to"] {
            if let Some(path) = arguments.get(key).and_then(Value::as_str) {
                paths.push(self.cache.root().join(path));
            }
        }
        if let Some(writes) = arguments.get("writes").and_then(Value::as_array) {
            for write in writes {
                if let Some(path) = write.get("path").and_then(Value::as_str) {
                    paths.push(self.cache.root().join(path));
                }
            }
        }
        if let Some(patch) = arguments.get("patch").and_then(Value::as_str) {
            for line in patch.lines() {
                if let Some(rest) = line
                    .strip_prefix("+++ ")
                    .or_else(|| line.strip_prefix("--- "))
                {
                    let raw = rest.split('\t').next().unwrap_or(rest).trim();
                    if raw != "/dev/null" && !raw.is_empty() {
                        let relative = raw
                            .strip_prefix("b/")
                            .or_else(|| raw.strip_prefix("a/"))
                            .unwrap_or(raw);
                        let candidate = self.cache.root().join(relative);
                        if !paths.contains(&candidate) {
                            paths.push(candidate);
                        }
                    }
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
        let result = self.inner.call(arguments)?;
        self.cache.repair(&paths);
        Ok(result)
    }

    fn describe_call(&self, arguments: &Value) -> String {
        self.inner.describe_call(arguments)
    }

    fn mutates(&self) -> bool {
        true
    }
}
