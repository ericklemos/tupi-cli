use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::ast::compact_signature;
use crate::budget::Section;
use crate::exec::finish;
use crate::graph::Def;
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct ImpactParams {
    pub symbol_name: String,
    #[serde(default)]
    pub include_tests: bool,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct ImpactTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl ImpactTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn line_for(def: &Def) -> String {
    format!(
        "- {} ({}:{}–{}) · {}",
        def.qualified,
        def.relative,
        def.start_line,
        def.end_line,
        compact_signature(&def.signature)
    )
}

impl Tool for ImpactTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "impact_check".to_string(),
            description: "Chamadores reversos que quebram se a assinatura mudar. Testes são contados e omitidos por padrão.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol_name": {
                        "type": "string",
                        "description": "Nome da função, método ou struct que será modificada."
                    },
                    "include_tests": {
                        "type": "boolean",
                        "default": false,
                        "description": "Inclui chamadores de teste."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["symbol_name"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: ImpactParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let symbol = params.symbol_name.trim();
        if symbol.is_empty() {
            return Err(ToolError::InvalidArguments("symbol_name vazio".to_string()));
        }
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let graph = crate::graph::GraphCache::shared(&root).get();
        let definitions = graph.matching(symbol);
        let callers = graph.callers(symbol);

        let mut tests = Vec::new();
        let external: Vec<&Def> = callers
            .iter()
            .copied()
            .filter(|def| {
                if def.is_test {
                    tests.push(*def);
                    false
                } else {
                    true
                }
            })
            .collect();

        let mut summary = String::new();
        summary.push('\n');
        if definitions.is_empty() {
            summary.push_str(&format!("- definição: {symbol} não encontrado no grafo\n"));
        } else {
            summary.push_str(&format!("- definições: {}\n", definitions.len()));
            for def in definitions.iter().take(5) {
                summary.push_str(&format!("  {}\n", line_for(def)));
            }
        }
        summary.push_str(&format!("- chamadores: {}\n", external.len()));
        if !params.include_tests {
            summary.push_str(&format!(
                "- testes que chamam: {} (omitidos)\n",
                tests.len()
            ));
        }

        let mut sections = vec![Section {
            heading: format!("### impacto — {symbol}"),
            body: summary,
            priority: 255,
        }];

        if !external.is_empty() {
            let mut body = String::new();
            for (index, def) in external.iter().enumerate() {
                if index >= 20 {
                    body.push_str(&format!("(+{} chamadores)\n", external.len() - 20));
                    break;
                }
                body.push_str(&line_for(def));
                body.push('\n');
            }
            sections.push(Section {
                heading: "#### chamadores".to_string(),
                body: format!("\n{body}"),
                priority: 240,
            });
        }

        if params.include_tests && !tests.is_empty() {
            let mut body = String::new();
            for def in tests.iter().take(20) {
                body.push_str(&line_for(def));
                body.push('\n');
            }
            sections.push(Section {
                heading: "#### testes".to_string(),
                body: format!("\n{body}"),
                priority: 230,
            });
        }

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let symbol = arguments
            .get("symbol_name")
            .and_then(Value::as_str)
            .unwrap_or("?");
        format!("impact_check: {symbol}")
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

    fn tool(dir: &std::path::Path) -> ImpactTool {
        ImpactTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        )
    }

    #[test]
    fn a_caller_is_listed_and_a_test_is_counted() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
        write(dir.path(), "src/b.rs", "pub fn caller() { helper(); }\n");
        write(
            dir.path(),
            "tests/it.rs",
            "#[test]\nfn it_works() { helper(); }\n",
        );
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol_name": "helper"}),
        )
        .unwrap();
        assert!(out.contains("chamadores: 1"));
        assert!(out.contains("caller"));
        assert!(out.contains("testes que chamam: 1"));
        assert!(!out.contains("it_works"));
    }

    #[test]
    fn include_tests_lists_them() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
        write(
            dir.path(),
            "tests/it.rs",
            "#[test]\nfn it_works() { helper(); }\n",
        );
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol_name": "helper", "include_tests": true}),
        )
        .unwrap();
        assert!(out.contains("it_works"));
    }

    #[test]
    fn an_unknown_symbol_is_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol_name": "nada"}),
        )
        .unwrap();
        assert!(out.contains("não encontrado"));
    }
}
