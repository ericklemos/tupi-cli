use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct TraceParams {
    pub from: String,
    pub to: String,
    #[serde(default = "default_depth")]
    pub max_depth: usize,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_depth() -> usize {
    6
}

pub struct TraceTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl TraceTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for TraceTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "trace".to_string(),
            description:
                "Caminho de chamadas entre dois símbolos (A → … → B) em uma resposta curta."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "Símbolo de origem (nome ou caminho do arquivo)."
                    },
                    "to": {
                        "type": "string",
                        "description": "Símbolo de destino (nome ou caminho do arquivo)."
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 12,
                        "default": 6,
                        "description": "Máximo de saltos procurados."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens."
                    }
                },
                "required": ["from", "to"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: TraceParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let from = params.from.trim();
        let to = params.to.trim();
        if from.is_empty() || to.is_empty() {
            return Err(ToolError::InvalidArguments(
                "from e to não podem ser vazios".to_string(),
            ));
        }
        if from == to {
            return Err(ToolError::InvalidArguments(
                "from e to são o mesmo símbolo".to_string(),
            ));
        }
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let graph = crate::graph::GraphCache::shared(&root).get();
        let depth = params.max_depth.clamp(1, 12);

        let answer = match graph.trace(from, to, depth) {
            Some(path) => {
                let mut body = format!("SIM — {}\n\n", path.join(" → "));
                for name in &path {
                    if let Some(def) = graph.matching(name).first() {
                        body.push_str(&format!("- {name} ({}:{})\n", def.relative, def.start_line));
                    }
                }
                body
            }
            None => format!("NÃO — sem caminho de {from} até {to} em até {depth} salto(s).\n"),
        };

        let section = Section {
            heading: format!("### trace — {from} → {to}"),
            body: format!("\n{answer}"),
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let from = arguments.get("from").and_then(Value::as_str).unwrap_or("?");
        let to = arguments.get("to").and_then(Value::as_str).unwrap_or("?");
        format!("trace: {from} -> {to}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn tool(dir: &std::path::Path) -> TraceTool {
        TraceTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        )
    }

    #[test]
    fn a_path_is_reported_in_order() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/a.rs",
            "pub fn a() { b(); }\nfn b() { c(); }\nfn c() {}\n",
        );
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"from": "a", "to": "c"}),
        )
        .unwrap();
        assert!(out.contains("SIM — a → b → c"));
    }

    #[test]
    fn no_path_is_an_explicit_answer() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn a() {}\nfn b() {}\n");
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"from": "b", "to": "a"}),
        )
        .unwrap();
        assert!(out.contains("NÃO"));
    }
}
