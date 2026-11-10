use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertKind {
    Exists,
    Public,
    UsedOutside,
}

#[derive(Debug, Deserialize)]
pub struct AssertParams {
    pub symbol: String,
    pub kind: AssertKind,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct AssertTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl AssertTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn evidence(defs: &[&crate::graph::Def]) -> String {
    let mut body = String::new();
    for def in defs.iter().take(6) {
        body.push_str(&format!(
            "- {} ({}:{}–{})\n",
            def.qualified, def.relative, def.start_line, def.end_line
        ));
    }
    body
}

impl Tool for AssertTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "assert".to_string(),
            description: "SIM/NÃO: o símbolo existe, é público ou é usado fora do módulo."
                .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Nome exato do símbolo (função, struct, método)."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["exists", "public", "used_outside"],
                        "description": "exists: aparece no grafo; public: tem pub; used_outside: há chamadas de fora do scope."
                    },
                    "scope": {
                        "type": "string",
                        "description": "Para used_outside: prefixo de caminho considerado dentro. Padrão: o arquivo que define o símbolo."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens."
                    }
                },
                "required": ["symbol", "kind"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: AssertParams = serde_json::from_value(arguments)
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
        let matching = graph.matching(symbol);

        let answer = match params.kind {
            AssertKind::Exists => {
                if matching.is_empty() {
                    format!("NÃO — {symbol} não aparece no grafo do repositório.\n")
                } else {
                    format!(
                        "SIM — {symbol} aparece {} vez(es).\n{}",
                        matching.len(),
                        evidence(&matching)
                    )
                }
            }
            AssertKind::Public => {
                let public: Vec<&crate::graph::Def> = matching
                    .iter()
                    .copied()
                    .filter(|def| def.is_public)
                    .collect();
                if public.is_empty() {
                    format!("NÃO — nenhuma definição de {symbol} é pública.\n")
                } else {
                    format!("SIM — {symbol} é pública.\n{}", evidence(&public))
                }
            }
            AssertKind::UsedOutside => {
                if matching.is_empty() {
                    format!("NÃO — {symbol} não aparece no grafo do repositório.\n")
                } else {
                    let inside = params
                        .scope
                        .as_deref()
                        .map(str::trim)
                        .filter(|scope| !scope.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| matching[0].relative.clone());
                    let callers: Vec<&crate::graph::Def> = graph
                        .callers(symbol)
                        .into_iter()
                        .filter(|def| !def.relative.starts_with(&inside))
                        .collect();
                    if callers.is_empty() {
                        format!("NÃO — nenhuma chamada a {symbol} vem de fora de {inside}.\n")
                    } else {
                        format!(
                            "SIM — {symbol} é usado fora de {inside} ({} chamador(es)).\n{}",
                            callers.len(),
                            evidence(&callers)
                        )
                    }
                }
            }
        };

        let section = Section {
            heading: format!(
                "### assert — {} {}",
                match params.kind {
                    AssertKind::Exists => "exists",
                    AssertKind::Public => "public",
                    AssertKind::UsedOutside => "used_outside",
                },
                symbol
            ),
            body: format!("\n{answer}"),
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
        let kind = arguments.get("kind").and_then(Value::as_str).unwrap_or("?");
        format!("assert: {kind} {symbol}")
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

    fn tool(dir: &std::path::Path) -> AssertTool {
        AssertTool::new(
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
            "pub fn helper() {}\nfn private() {}\n",
        );
        write(dir.path(), "src/b.rs", "pub fn caller() { helper(); }\n");
        write(
            dir.path(),
            "tests/it.rs",
            "pub fn outside() { helper(); }\n",
        );
        dir
    }

    #[test]
    fn exists_answers_yes_and_no() {
        let dir = fixture();
        let yes = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol": "helper", "kind": "exists"}),
        )
        .unwrap();
        assert!(yes.contains("SIM"));
        let no = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol": "nada", "kind": "exists"}),
        )
        .unwrap();
        assert!(no.contains("NÃO"));
    }

    #[test]
    fn public_checks_the_visibility() {
        let dir = fixture();
        let public = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol": "helper", "kind": "public"}),
        )
        .unwrap();
        assert!(public.contains("SIM"));
        let private = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol": "private", "kind": "public"}),
        )
        .unwrap();
        assert!(private.contains("NÃO"));
    }

    #[test]
    fn used_outside_respects_the_scope() {
        let dir = fixture();
        let outside = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol": "helper", "kind": "used_outside", "scope": "src/a"}),
        )
        .unwrap();
        assert!(outside.contains("SIM"));
        let inside = Tool::call(
            &tool(dir.path()),
            serde_json::json!({"symbol": "private", "kind": "used_outside", "scope": "src"}),
        )
        .unwrap();
        assert!(inside.contains("NÃO"));
    }
}
