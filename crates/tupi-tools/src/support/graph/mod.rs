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
        crate::support::effect::declared_paths(arguments)
    }
}

impl Tool for RepairingTool {
    fn spec(&self) -> ToolSpec {
        self.inner.spec()
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let effect = self.effect(&arguments);
        let result = self.inner.call(arguments)?;
        match &effect {
            tupi_kernel::Effect::ReadOnly => {}
            tupi_kernel::Effect::Writes(paths) => self.cache.repair(paths),
            tupi_kernel::Effect::Opaque => self.cache.repair(&[]),
        }
        Ok(result)
    }

    fn describe_call(&self, arguments: &Value) -> String {
        self.inner.describe_call(arguments)
    }

    fn effect(&self, arguments: &Value) -> tupi_kernel::Effect {
        match self.inner.effect(arguments) {
            tupi_kernel::Effect::ReadOnly => tupi_kernel::Effect::ReadOnly,
            tupi_kernel::Effect::Writes(paths) => tupi_kernel::Effect::Writes(paths),
            tupi_kernel::Effect::Opaque => {
                let declared = self.paths(arguments);
                if declared.is_empty() {
                    tupi_kernel::Effect::Opaque
                } else {
                    tupi_kernel::Effect::Writes(declared)
                }
            }
        }
    }

    fn mutates(&self) -> bool {
        true
    }
}
