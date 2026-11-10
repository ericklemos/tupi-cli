use tupi_kernel::pipeline::spec;
use tupi_kernel::{Tool, ToolError, ToolSpec};

/// A spec existe para o modelo; quem executa as steps é o app.
pub struct PipelineTool;

impl Tool for PipelineTool {
    fn spec(&self) -> ToolSpec {
        spec()
    }

    fn call(&self, _arguments: serde_json::Value) -> Result<String, ToolError> {
        Err(ToolError::Other(
            "pipeline é orquestrada pelo app".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pipeline_tool_only_exposes_the_spec() {
        let tool = PipelineTool;
        assert_eq!(tool.spec().name, "pipeline");
        assert!(matches!(
            tool.call(serde_json::json!({})),
            Err(ToolError::Other(_))
        ));
    }
}
