use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::proc;
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct WhyParams {
    pub symbol: String,
    #[serde(default = "default_commits")]
    pub max_commits: usize,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_commits() -> usize {
    10
}

pub struct WhyTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl WhyTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for WhyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "why".to_string(),
            description: "Commits que tocaram um símbolo, sem abrir o log inteiro.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Nome exato da função/struct."
                    },
                    "max_commits": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 50,
                        "default": 10,
                        "description": "Máximo de commits por definição."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["symbol"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: WhyParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let symbol = params.symbol.trim();
        if symbol.is_empty() {
            return Err(ToolError::InvalidArguments("symbol vazio".to_string()));
        }
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let graph = crate::graph::GraphCache::shared(&root).get();
        let definitions = graph.matching(symbol);
        if definitions.is_empty() {
            return Err(ToolError::SymbolNotFound(format!(
                "{symbol} não encontrado no grafo"
            )));
        }

        let limit = params.max_commits.clamp(1, 50);
        let mut commits: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut errors: Vec<String> = Vec::new();

        for def in definitions.iter().take(3) {
            let range = format!("{},{}:{}", def.start_line, def.end_line, def.relative);
            let args = vec![
                "log".to_string(),
                "-L".to_string(),
                range,
                "--format=COMMIT %h %ad %s".to_string(),
                "--date=short".to_string(),
                "-n".to_string(),
                limit.to_string(),
            ];
            let output = proc::program("git", &args, &root, Duration::from_secs(60))?;
            if !output.success {
                errors.push(crate::exec::clip(output.stderr.trim(), 400, 100));
                continue;
            }
            for line in output.stdout.lines() {
                let Some(rest) = line.strip_prefix("COMMIT ") else {
                    continue;
                };
                let hash = rest.split_whitespace().next().unwrap_or("");
                if !hash.is_empty() && seen.insert(hash.to_string()) {
                    commits.push(format!("- {rest} ({})", def.relative));
                }
            }
        }

        let mut body = String::new();
        if commits.is_empty() {
            body.push_str(&format!("\nNÃO — nenhum commit tocou {symbol}.\n"));
            for error in &errors {
                body.push_str(&format!("- {error}\n"));
            }
        } else {
            body.push_str(&format!(
                "\n{} commit(s) tocaram {symbol}:\n",
                commits.len()
            ));
            for commit in &commits {
                body.push_str(commit);
                body.push('\n');
            }
        }

        let section = Section {
            heading: format!("### why — {symbol}"),
            body,
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let symbol = arguments
            .get("symbol")
            .and_then(Value::as_str)
            .unwrap_or("?");
        format!("why: {symbol}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn tool(dir: &std::path::Path) -> WhyTool {
        WhyTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        )
    }

    #[test]
    fn both_commits_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "pub fn helper() {\n    1\n}\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-m",
                "primeiro",
            ],
        );
        std::fs::write(dir.path().join("src/a.rs"), "pub fn helper() {\n    2\n}\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(
            dir.path(),
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-m",
                "segundo",
            ],
        );

        let out = Tool::call(&tool(dir.path()), serde_json::json!({"symbol": "helper"})).unwrap();
        assert!(out.contains("primeiro"));
        assert!(out.contains("segundo"));
    }

    #[test]
    fn an_unknown_symbol_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Tool::call(&tool(dir.path()), serde_json::json!({"symbol": "nada"})).is_err());
    }
}
