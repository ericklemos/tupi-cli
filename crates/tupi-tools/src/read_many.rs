//! Leitura em lote: uma chamada valida todos os itens antes de responder, e o
//! lote inteiro divide um único orçamento de tokens.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::read::{ReadParams, ReadTool, Rendered};

#[derive(Debug, Deserialize)]
pub struct ReadItem {
    pub path: String,
    #[serde(default)]
    pub symbol: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReadManyParams {
    pub reads: Vec<ReadItem>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
    #[serde(default)]
    pub repeat: Option<String>,
}

pub struct ReadManyTool {
    inner: Arc<ReadTool>,
}

impl ReadManyTool {
    pub fn new(inner: Arc<ReadTool>) -> Self {
        Self { inner }
    }

    pub fn read_many(&self, params: ReadManyParams) -> Result<String, ToolError> {
        if params.reads.is_empty() {
            return Err(ToolError::InvalidArguments(
                "`reads` está vazio: nada a ler.".to_string(),
            ));
        }

        let mut rendered: Vec<Rendered> = Vec::new();
        let mut errors = Vec::new();
        let mut seen: HashSet<(PathBuf, Option<String>)> = HashSet::new();

        for (index, item) in params.reads.into_iter().enumerate() {
            let request = ReadParams {
                path: item.path.clone(),
                symbol: item.symbol.clone(),
                budget_tokens: None,
                repeat: params.repeat.clone(),
            };

            let (path, content, relative) = match self.inner.prepare(&request) {
                Ok(read) => read,
                Err(error) => {
                    errors.push(format!("- item {} (`{}`): {error}", index + 1, item.path));
                    continue;
                }
            };

            if !seen.insert((path.clone(), item.symbol.clone())) {
                errors.push(format!(
                    "- item {} (`{}`): leitura repetida no lote.",
                    index + 1,
                    item.path
                ));
                continue;
            }

            match self.inner.render(&request, &path, &content, &relative) {
                Ok(sections) if !sections.is_empty() => rendered.extend(sections),
                Ok(_) => errors.push(format!(
                    "- item {} (`{}`): símbolo `{}` não encontrado em `{relative}`.",
                    index + 1,
                    item.path,
                    item.symbol.as_deref().unwrap_or_default()
                )),
                Err(error) => {
                    errors.push(format!("- item {} (`{}`): {error}", index + 1, item.path))
                }
            }
        }

        if !errors.is_empty() {
            return Err(ToolError::Other(format!(
                "### `read_many` — nada foi lido\n\n{}\n",
                errors.join("\n")
            )));
        }

        Ok(self.inner.finish(rendered, params.budget_tokens))
    }
}

impl Tool for ReadManyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_many".to_string(),
            description: "Lê vários arquivos ou símbolos como um lote: valida todas as leituras antes de responder, recusa o lote inteiro se uma for inválida e divide um único orçamento de tokens entre os itens.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "reads": {
                        "type": "array",
                        "minItems": 1,
                        "description": "Leituras a executar, na ordem do relatório. A mesma combinação de caminho e símbolo é recusada duas vezes.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": {
                                    "type": "string",
                                    "description": "Caminho relativo ao diretório raiz do projeto."
                                },
                                "symbol": {
                                    "type": "string",
                                    "description": "Opcional: nome da função/struct, simples ou qualificado, cujo corpo será devolvido."
                                }
                            },
                            "required": ["path"]
                        }
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens compartilhado por todas as leituras do lote (excedente em peek)."
                    },
                    "repeat": {
                        "type": "string",
                        "enum": ["allow"],
                        "description": "`allow` devolve os corpos mesmo se esta sessão já os recebeu; o padrão aponta para o corpo anterior."
                    }
                },
                "required": ["reads"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: ReadManyParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.read_many(params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::{TempDir, tempdir};
    use tupi_kernel::{Tool, ToolError, ToolRegistry};

    use crate::budget::DEFAULT_BUDGET_TOKENS;
    use crate::ledger::DeliveryLedger;
    use crate::store::ResponseStore;

    fn tool_for(dir: &Path) -> ReadManyTool {
        let store = Arc::new(ResponseStore::new(
            std::time::Duration::from_secs(60),
            1024 * 1024,
        ));
        let ledger = Arc::new(DeliveryLedger::new(64, 1 << 20));
        ReadManyTool::new(Arc::new(ReadTool::new(
            dir.to_path_buf(),
            store,
            ledger,
            DEFAULT_BUDGET_TOKENS,
        )))
    }

    fn fixture() -> TempDir {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n\npub fn logout() {\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("session.rs"),
            "pub fn start_session() -> u8 {\n    1\n}\n",
        )
        .unwrap();
        dir
    }

    fn item(path: &str, symbol: Option<&str>) -> ReadItem {
        ReadItem {
            path: path.into(),
            symbol: symbol.map(str::to_string),
        }
    }

    fn batch(reads: Vec<ReadItem>) -> ReadManyParams {
        ReadManyParams {
            reads,
            budget_tokens: None,
            repeat: None,
        }
    }

    #[test]
    fn a_batch_reads_every_symbol() {
        let dir = fixture();
        let out = tool_for(dir.path())
            .read_many(batch(vec![
                item("auth.rs", Some("verify_token")),
                item("session.rs", Some("start_session")),
            ]))
            .unwrap();

        assert!(out.contains("!raw.is_empty()"), "{out}");
        assert!(out.contains("start_session"), "{out}");
    }

    #[test]
    fn the_same_file_may_appear_with_different_symbols() {
        let dir = fixture();
        let out = tool_for(dir.path())
            .read_many(batch(vec![
                item("auth.rs", Some("verify_token")),
                item("auth.rs", Some("logout")),
            ]))
            .unwrap();

        assert!(out.contains("!raw.is_empty()"), "{out}");
        assert!(out.contains("pub fn logout"), "{out}");
    }

    #[test]
    fn a_repeated_read_is_rejected_before_returning_anything() {
        let dir = fixture();
        let error = tool_for(dir.path())
            .read_many(batch(vec![
                item("auth.rs", Some("verify_token")),
                item("./auth.rs", Some("verify_token")),
            ]))
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("repetida"), "{message}");
        assert!(!message.contains("!raw.is_empty()"), "{message}");
    }

    #[test]
    fn a_missing_file_aborts_the_whole_batch() {
        let dir = fixture();
        let error = tool_for(dir.path())
            .read_many(batch(vec![
                item("auth.rs", Some("verify_token")),
                item("missing.rs", Some("nope")),
            ]))
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("nada foi lido"), "{message}");
        assert!(message.contains("missing.rs"), "{message}");
        assert!(!message.contains("!raw.is_empty()"), "{message}");
    }

    #[test]
    fn an_unknown_symbol_aborts_the_whole_batch() {
        let dir = fixture();
        let error = tool_for(dir.path())
            .read_many(batch(vec![
                item("auth.rs", Some("verify_token")),
                item("session.rs", Some("does_not_exist")),
            ]))
            .unwrap_err();

        assert!(error.to_string().contains("não encontrado"), "{error}");
    }

    #[test]
    fn an_empty_batch_is_an_explicit_error() {
        let dir = fixture();
        let error = tool_for(dir.path())
            .read_many(batch(Vec::new()))
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn the_batch_shares_one_budget_and_the_tail_is_recoverable() {
        let dir = tempdir().unwrap();
        let body = |name: &str| {
            format!(
                "pub fn {name}() -> u8 {{\n{}\n    1\n}}\n",
                "    let _ = 1;\n".repeat(80)
            )
        };
        fs::write(dir.path().join("one.rs"), body("first_long")).unwrap();
        fs::write(dir.path().join("two.rs"), body("second_long")).unwrap();

        let mut params = batch(vec![
            item("one.rs", Some("first_long")),
            item("two.rs", Some("second_long")),
        ]);
        params.budget_tokens = Some(80);

        let out = tool_for(dir.path()).read_many(params).unwrap();
        assert!(out.contains("Resposta truncada"), "{out}");
        assert!(out.contains("peek"), "{out}");
    }

    #[test]
    fn register_exposes_read_read_many_and_peek() {
        let dir = fixture();
        let mut registry = ToolRegistry::new();
        crate::register(dir.path().to_path_buf(), &mut registry);

        let names: Vec<String> = registry.specs().into_iter().map(|spec| spec.name).collect();
        assert_eq!(
            names,
            vec!["read", "read_many", "search", "edit", "peek"]
        );
    }

    #[test]
    fn the_tool_schema_requires_reads() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let spec = tool.spec();
        assert_eq!(spec.name, "read_many");
        assert_eq!(spec.schema["required"][0], "reads");
    }
}
