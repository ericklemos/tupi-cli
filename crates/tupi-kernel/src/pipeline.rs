use serde::Deserialize;

use crate::tool::ToolSpec;

/// Nome da tool que o modelo chama; o app reconhece este nome e orquestra.
pub const PIPELINE_TOOL: &str = "pipeline";

/// Teto padrão da saída final quando o plano não traz `budget_tokens`.
pub const PIPELINE_DEFAULT_BUDGET_TOKENS: usize = 4000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelinePlan {
    pub steps: Vec<PipelineStep>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineStep {
    pub calls: Vec<PipelineCall>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineCall {
    pub tool: String,
    pub arguments: serde_json::Value,
}

/// A spec visível ao modelo. A execução não mora aqui: o app orquestra.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: PIPELINE_TOOL.to_string(),
        description: "Executa steps em ordem. As calls de uma step rodam juntas e a saída é concatenada; a step seguinte referencia o texto anterior com {{prev}} ou {{stepN}} (N 0-based). Devolve só a concatenação da última step.".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "required": ["steps"],
            "properties": {
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "required": ["calls"],
                        "properties": {
                            "calls": {
                                "type": "array",
                                "minItems": 1,
                                "items": {
                                    "type": "object",
                                    "required": ["tool", "arguments"],
                                    "properties": {
                                        "tool": {"type": "string"},
                                        "arguments": {"type": "object"}
                                    }
                                }
                            }
                        }
                    }
                },
                "budget_tokens": {"type": "integer", "minimum": 1}
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_named_pipeline_and_takes_steps() {
        let spec = spec();
        assert_eq!(spec.name, PIPELINE_TOOL);
        assert_eq!(spec.schema["required"][0], "steps");
    }

    #[test]
    fn a_plan_parses_steps_and_optional_budget() {
        let plan: PipelinePlan = serde_json::from_str(
            r#"{"steps":[{"calls":[{"tool":"read","arguments":{"path":"a.rs"}}]}]}"#,
        )
        .unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].calls[0].tool, "read");
        assert_eq!(plan.budget_tokens, None);

        let with_budget: PipelinePlan = serde_json::from_str(
            r#"{"steps":[{"calls":[{"tool":"read","arguments":{}}]}],"budget_tokens":900}"#,
        )
        .unwrap();
        assert_eq!(with_budget.budget_tokens, Some(900));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        assert!(
            serde_json::from_str::<PipelinePlan>(
                r#"{"steps":[{"calls":[{"tool":"read","arguments":{}}]}],"extra":1}"#
            )
            .is_err()
        );
    }
}
