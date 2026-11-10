use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::ast::compact_signature;
use crate::budget::Section;
use crate::exec::finish;
use crate::graph::{CallGraph, Def};
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct ContextParams {
    pub target_symbol_or_file: String,
    #[serde(default = "default_depth")]
    pub depth: usize,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_depth() -> usize {
    1
}

pub struct ContextTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl ContextTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn list_section(title: String, defs: &[&Def], priority: u8) -> Section {
    let mut body = String::new();
    for def in defs.iter().take(12) {
        body.push_str(&format!(
            "- {} ({}:{}) · {}\n",
            def.qualified,
            def.relative,
            def.start_line,
            compact_signature(&def.signature)
        ));
    }
    if defs.len() > 12 {
        body.push_str(&format!("(+{} itens)\n", defs.len() - 12));
    }
    Section {
        heading: title,
        body: format!("\n{body}"),
        priority,
    }
}

impl Tool for ContextTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "context_graph".to_string(),
            description: "Quem chama e o que o símbolo/arquivo consome, com assinaturas."
                .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "target_symbol_or_file": {
                        "type": "string",
                        "description": "Nome da função/struct (verify_token) ou caminho relativo do arquivo."
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 3,
                        "default": 1,
                        "description": "Número de saltos no grafo de chamadas."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["target_symbol_or_file"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: ContextParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let target = params.target_symbol_or_file.trim();
        if target.is_empty() {
            return Err(ToolError::InvalidArguments(
                "target_symbol_or_file vazio".to_string(),
            ));
        }
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let graph = CallGraph::build(&root);
        let depth = params.depth.clamp(1, 3);

        let is_file = target.ends_with(".rs") || target.contains('/');
        let roots: Vec<&Def> = if is_file {
            graph
                .defs
                .iter()
                .filter(|def| {
                    def.relative == target
                        || def
                            .relative
                            .starts_with(&format!("{}/", target.trim_end_matches('/')))
                })
                .collect()
        } else {
            graph.matching(target)
        };
        if roots.is_empty() {
            return Err(ToolError::SymbolNotFound(format!(
                "{target} não encontrado no grafo"
            )));
        }

        let mut sections = vec![Section {
            heading: format!(
                "### contexto — {} ({} raiz(es), profundidade {depth})",
                target,
                roots.len()
            ),
            body: String::new(),
            priority: 255,
        }];

        let mut seen: HashSet<(&str, &str)> = roots
            .iter()
            .map(|def| (def.qualified.as_str(), def.relative.as_str()))
            .collect();
        let mut current: Vec<&Def> = roots.clone();

        for level in 1..=depth {
            let mut callers: Vec<&Def> = Vec::new();
            let mut callees: Vec<&Def> = Vec::new();
            for def in &current {
                for caller in graph.callers(&def.name) {
                    if seen.insert((caller.qualified.as_str(), caller.relative.as_str())) {
                        callers.push(caller);
                    }
                }
                for callee in graph.callees(def) {
                    if seen.insert((callee.qualified.as_str(), callee.relative.as_str())) {
                        callees.push(callee);
                    }
                }
            }

            if !callers.is_empty() {
                sections.push(list_section(
                    format!("#### nível {level} — chamadores"),
                    &callers,
                    (240 - level as u8 * 10).max(40),
                ));
            }
            if !callees.is_empty() {
                sections.push(list_section(
                    format!("#### nível {level} — consumidores"),
                    &callees,
                    (235 - level as u8 * 10).max(40),
                ));
            }
            current = callers.into_iter().chain(callees).collect();
            if current.is_empty() {
                break;
            }
        }

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let target = arguments
            .get("target_symbol_or_file")
            .and_then(Value::as_str)
            .unwrap_or("?");
        format!("context_graph: {target}")
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

    fn tool(dir: &std::path::Path) -> ContextTool {
        ContextTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        )
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/a.rs",
            "pub fn a() { b(); }\npub fn b() { c(); }\npub fn c() {}\n",
        );
        dir
    }

    #[test]
    fn one_hop_shows_callers_and_callees() {
        let dir = fixture();
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"target_symbol_or_file": "b"}),
        )
        .unwrap();
        assert!(out.contains("chamadores"));
        assert!(out.contains("a (src/a.rs"));
        assert!(out.contains("consumidores"));
        assert!(out.contains("c (src/a.rs"));
    }

    #[test]
    fn two_hops_reach_the_grandparent() {
        let dir = fixture();
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"target_symbol_or_file": "c", "depth": 2}),
        )
        .unwrap();
        assert!(out.contains("niavel 2") || out.contains("nível 2"));
        assert!(out.contains("a (src/a.rs"));
    }

    #[test]
    fn a_file_target_uses_its_symbols_as_roots() {
        let dir = fixture();
        let out = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"target_symbol_or_file": "src/a.rs"}),
        )
        .unwrap();
        assert!(out.contains("3 raiz(es)"));
    }
}
