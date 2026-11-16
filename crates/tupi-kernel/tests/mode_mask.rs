use std::sync::Arc;

use tupi_kernel::{AgentMode, ModeMask, Tool, ToolError, ToolRegistry, ToolSpec};
use serde_json::Value;

struct ExecTool;

impl Tool for ExecTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "e".into(),
            description: String::new(),
            schema: serde_json::json!({}),
        }
    }

    fn call(&self, _: Value) -> Result<String, ToolError> {
        Ok(String::new())
    }
}

struct SharedTool;

impl Tool for SharedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "s".into(),
            description: String::new(),
            schema: serde_json::json!({}),
        }
    }

    fn call(&self, _: Value) -> Result<String, ToolError> {
        Ok(String::new())
    }

    fn mode_mask(&self) -> ModeMask {
        ModeMask::SDD.union(ModeMask::DOCS)
    }
}

fn names(registry: &ToolRegistry, mode: AgentMode) -> Vec<String> {
    registry
        .specs_for_mode(mode)
        .into_iter()
        .map(|spec| spec.name)
        .collect()
}

#[test]
fn mode_mask_filters_by_mode_and_defaults_to_execution() {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ExecTool));
    reg.register(Arc::new(SharedTool));

    assert_eq!(names(&reg, AgentMode::Execution), vec!["e"]);
    assert_eq!(names(&reg, AgentMode::Sdd), vec!["s"]);
    assert_eq!(names(&reg, AgentMode::Docs), vec!["s"]);
}
