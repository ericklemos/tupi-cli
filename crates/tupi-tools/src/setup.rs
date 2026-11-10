use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::paths;
use crate::store::ResponseStore;

const START: &str = "<!-- tupi-tools:start -->";
const END: &str = "<!-- tupi-tools:end -->";

#[derive(Debug, Deserialize)]
pub struct SetupParams {
    #[serde(default = "default_action")]
    pub action: String,
    #[serde(default = "default_agents")]
    pub agents_file: String,
    #[serde(default = "default_skill_dir")]
    pub skills_dir: String,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_action() -> String {
    "preview".to_string()
}

fn default_agents() -> String {
    "AGENTS.md".to_string()
}

fn default_skill_dir() -> String {
    "skills/tupi-tools".to_string()
}

pub struct SetupTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    specs: Vec<ToolSpec>,
    default_budget_tokens: usize,
}

impl SetupTool {
    pub fn new(
        root_dir: PathBuf,
        store: Arc<ResponseStore>,
        specs: Vec<ToolSpec>,
        default_budget_tokens: usize,
    ) -> Self {
        Self {
            root_dir,
            store,
            specs,
            default_budget_tokens,
        }
    }
}

fn block(specs: &[ToolSpec]) -> String {
    let mut out = format!("{START}\n## Tools do Tupi\n\n");
    for spec in specs {
        out.push_str(&format!("- **{}** — {}\n", spec.name, spec.description));
    }
    out.push_str(END);
    out
}

fn merge(existing: &str, block: &str) -> String {
    if let Some(start) = existing.find(START)
        && let Some(end) = existing[start..].find(END)
    {
        let end = start + end + END.len();
        let mut out = String::new();
        out.push_str(&existing[..start]);
        out.push_str(block);
        out.push_str(&existing[end..]);
        return out;
    }
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(block);
    out.push('\n');
    out
}

impl Tool for SetupTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "setup".to_string(),
            description: "Gera a documentação das tools (bloco em AGENTS.md + skill) a partir da lista viva. Começa em preview; só grava com action=apply.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["preview", "apply"],
                        "default": "preview",
                        "description": "preview mostra o que seria escrito; apply grava os arquivos."
                    },
                    "agents_file": {
                        "type": "string",
                        "default": "AGENTS.md",
                        "description": "Arquivo de instruções do agente."
                    },
                    "skills_dir": {
                        "type": "string",
                        "default": "skills/tupi-tools",
                        "description": "Diretório da skill, relativo ao root."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens."
                    }
                }
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: SetupParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let apply = match params.action.trim() {
            "preview" => false,
            "apply" => true,
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "action precisa ser preview ou apply: {other}"
                )));
            }
        };
        let block = block(&self.specs);
        let skill_path = format!("{}/SKILL.md", params.skills_dir.trim_end_matches('/'));
        let skill = format!(
            "---\nname: tupi-tools\ndescription: Tools do Tupi para leitura, escrita, execução e grafo do repositório.\n---\n\n# Tupi tools\n\n{} tools disponíveis.\n\n{block}\n",
            self.specs.len()
        );

        let mut body = format!(
            "\n- tools: {}\n- bloco: {} linhas\n- AGENTS.md: {}\n- skill: {}\n",
            self.specs.len(),
            block.lines().count(),
            params.agents_file,
            skill_path
        );
        if apply {
            let agents_path = paths::resolve_for_write(&self.root_dir, &params.agents_file)?;
            let existing = std::fs::read_to_string(&agents_path).unwrap_or_default();
            let merged = merge(&existing, &block);
            if let Some(parent) = agents_path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    ToolError::Other(format!("falha ao criar diretório de AGENTS.md: {error}"))
                })?;
            }
            paths::atomic_write(&agents_path, merged.as_bytes())
                .map_err(|error| ToolError::Other(format!("falha ao gravar AGENTS.md: {error}")))?;

            let skill_file = paths::resolve_for_write(&self.root_dir, &skill_path)?;
            if let Some(parent) = skill_file.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    ToolError::Other(format!("falha ao criar diretório da skill: {error}"))
                })?;
            }
            paths::atomic_write(&skill_file, skill.as_bytes())
                .map_err(|error| ToolError::Other(format!("falha ao gravar a skill: {error}")))?;
            body.push_str("- gravado\n");
        } else {
            body.push_str(&format!("\n{block}\n"));
        }

        let section = Section {
            heading: format!("### setup — {}", if apply { "aplicado" } else { "preview" }),
            body,
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("preview");
        format!("setup: {action}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(dir: &std::path::Path) -> SetupTool {
        SetupTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            vec![
                ToolSpec {
                    name: "read".to_string(),
                    description: "lê".to_string(),
                    schema: serde_json::json!({}),
                },
                ToolSpec {
                    name: "edit".to_string(),
                    description: "edita".to_string(),
                    schema: serde_json::json!({}),
                },
            ],
            4000,
        )
    }

    #[test]
    fn preview_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let out = Tool::call(&tool(dir.path()), serde_json::json!({})).unwrap();
        assert!(out.contains("preview"));
        assert!(out.contains("**read**"));
        assert!(!dir.path().join("AGENTS.md").exists());
    }

    #[test]
    fn apply_writes_once_and_updates_in_place() {
        let dir = tempfile::tempdir().unwrap();
        Tool::call(&tool(dir.path()), serde_json::json!({"action": "apply"})).unwrap();
        let first = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(first.contains(START));
        Tool::call(&tool(dir.path()), serde_json::json!({"action": "apply"})).unwrap();
        let second = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert_eq!(second.matches(START).count(), 1);
        assert!(dir.path().join("skills/tupi-tools/SKILL.md").exists());
    }

    #[test]
    fn an_invalid_action_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Tool::call(&tool(dir.path()), serde_json::json!({"action": "x"})).is_err());
    }
}
