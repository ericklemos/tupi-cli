//! Leitura em lote: o lote inteiro divide um único orçamento de tokens entre os
//! itens. Caminho inexistente, leitura repetida ou rev desconhecido abortam o
//! lote inteiro; símbolo não encontrado vira aviso no topo e os demais itens
//! continuam sendo entregues.

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
    #[serde(default)]
    pub rev: Option<String>,
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
                "\u{0060}reads\u{0060} está vazio: nada a ler.".to_string(),
            ));
        }

        let mut rendered: Vec<Rendered> = Vec::new();
        let mut errors = Vec::new();
        let mut missing = Vec::new();
        let mut seen: HashSet<(PathBuf, Option<String>, Option<String>)> = HashSet::new();

        for (index, item) in params.reads.into_iter().enumerate() {
            let request = ReadParams {
                path: item.path.clone(),
                symbol: item.symbol.clone(),
                budget_tokens: None,
                repeat: params.repeat.clone(),
                rev: item.rev.clone(),
            };

            if let Some(rev) = item
                .rev
                .as_deref()
                .map(str::trim)
                .filter(|rev| !rev.is_empty())
            {
                let path = match self.inner.prepare_rev(&request, rev) {
                    Ok((path, _, _)) => path,
                    Err(error) => {
                        errors.push(format!(
                            "- item {} (\u{0060}{}\u{0060}): {error}",
                            index + 1,
                            item.path
                        ));
                        continue;
                    }
                };
                if !seen.insert((path, item.symbol.clone(), Some(rev.to_string()))) {
                    errors.push(format!(
                        "- item {} (\u{0060}{}\u{0060}): leitura repetida no lote.",
                        index + 1,
                        item.path
                    ));
                    continue;
                }
                match self.inner.rev_sections(&request, rev) {
                    Ok((reference, sections)) => {
                        rendered.push(Rendered {
                            section: crate::budget::Section {
                                heading: format!("#### {reference}"),
                                body: String::new(),
                                priority: 250,
                            },
                            pending: None,
                        });
                        rendered.extend(sections.into_iter().map(|section| Rendered {
                            section,
                            pending: None,
                        }))
                    }
                    Err(error) => errors.push(format!(
                        "- item {} (\u{0060}{}\u{0060}): {error}",
                        index + 1,
                        item.path
                    )),
                }
                continue;
            }

            let (path, content, relative) = match self.inner.prepare(&request) {
                Ok(read) => read,
                Err(error) => {
                    errors.push(format!(
                        "- item {} (\u{0060}{}\u{0060}): {error}",
                        index + 1,
                        item.path
                    ));
                    continue;
                }
            };

            if !seen.insert((path.clone(), item.symbol.clone(), None)) {
                errors.push(format!(
                    "- item {} (\u{0060}{}\u{0060}): leitura repetida no lote.",
                    index + 1,
                    item.path
                ));
                continue;
            }

            match self.inner.render(&request, &path, &content, &relative) {
                Ok(sections) if !sections.is_empty() => rendered.extend(sections),
                Ok(_) => missing.push(format!(
                    "- item {} (\u{0060}{}\u{0060}): símbolo \u{0060}{}\u{0060} não encontrado em \u{0060}{relative}\u{0060}.",
                    index + 1,
                    item.path,
                    item.symbol.as_deref().unwrap_or_default()
                )),
                Err(error) => {
                    errors.push(format!("- item {} (\u{0060}{}\u{0060}): {error}", index + 1, item.path))
                }
            }
        }

        if !errors.is_empty() {
            return Err(ToolError::Other(format!(
                "### \u{0060}read_many\u{0060} — nada foi lido\n\n{}\n",
                errors.join("\n")
            )));
        }

        if missing.is_empty() {
            return Ok(self.inner.finish(rendered, params.budget_tokens));
        }

        if rendered.is_empty() {
            return Err(ToolError::Other(format!(
                "### `read_many` — nada foi lido\n\n{}\n",
                missing.join("\n")
            )));
        }

        let heading = if missing.len() == 1 {
            "#### leitura não encontrada"
        } else {
            "#### leituras não encontradas"
        };
        let mut with_notice = vec![Rendered {
            section: crate::budget::Section {
                heading: heading.to_string(),
                body: missing.join("\n"),
                priority: 255,
            },
            pending: None,
        }];
        with_notice.extend(rendered);

        Ok(self.inner.finish(with_notice, params.budget_tokens))
    }
}

impl Tool for ReadManyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_many".to_string(),
            description: "Lê vários arquivos ou símbolos como um lote, com um único orçamento de tokens entre os itens. Caminho inexistente, leitura repetida ou rev desconhecido recusam o lote inteiro; símbolo não encontrado vira aviso no topo e não derruba os demais itens.".to_string(),
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
                                },
                                "rev": {
                                    "type": "string",
                                    "description": "Opcional: lê rev:path (commit, tag ou ref) em vez da árvore de trabalho."
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

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let Some(reads) = arguments.get("reads").and_then(|value| value.as_array()) else {
            return "read_many".to_string();
        };
        if reads.is_empty() {
            return "read_many".to_string();
        }

        let labels: Vec<String> = reads
            .iter()
            .take(3)
            .map(|item| {
                let path = item
                    .get("path")
                    .and_then(|value| value.as_str())
                    .unwrap_or("?");
                let label = match item.get("symbol").and_then(|value| value.as_str()) {
                    Some(symbol) => format!("{path}#{symbol}"),
                    None => path.to_string(),
                };
                match item
                    .get("rev")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|rev| !rev.is_empty())
                {
                    Some(rev) => format!("{rev}:{label}"),
                    None => label,
                }
            })
            .collect();

        let noun = if reads.len() == 1 {
            "leitura"
        } else {
            "leituras"
        };
        let mut out = format!("read_many: {} {noun} · {}", reads.len(), labels.join(" · "));
        let rest = reads.len().saturating_sub(labels.len());
        if rest > 0 {
            out.push_str(&format!(" · +{rest}"));
        }
        out
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
            rev: None,
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
    fn an_unknown_symbol_is_reported_without_dropping_the_rest() {
        let dir = fixture();
        let out = tool_for(dir.path())
            .read_many(batch(vec![
                item("auth.rs", Some("verify_token")),
                item("session.rs", Some("does_not_exist")),
            ]))
            .unwrap();

        assert!(out.contains("leitura não encontrada"), "{out}");
        assert!(out.contains("does_not_exist"), "{out}");
        assert!(out.contains("session.rs"), "{out}");
        assert!(out.contains("!raw.is_empty()"), "{out}");

        // Quando nenhum item é encontrado não sobra conteúdo: continua sendo erro.
        let error = tool_for(dir.path())
            .read_many(batch(vec![item("auth.rs", Some("does_not_exist"))]))
            .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("nada foi lido"), "{message}");
        assert!(message.contains("does_not_exist"), "{message}");
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
            vec![
                "read",
                "read_many",
                "search",
                "workflow",
                "edit",
                "write",
                "write_many",
                "move",
                "patch",
                "exec",
                "test_many",
                "shell",
                "map",
                "assert",
                "impact_check",
                "context_graph",
                "trace",
                "diff",
                "why",
                "fetch",
                "websearch",
                "research",
                "ledger",
                "state",
                "env",
                "peek",
                "setup",
            ]
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

    #[test]
    fn describe_call_summarizes_the_batch() {
        let dir = fixture();
        let tool = tool_for(dir.path());

        assert_eq!(
            tool.describe_call(&serde_json::json!({
                "reads": [{ "path": "auth.rs", "symbol": "verify_token" }]
            })),
            "read_many: 1 leitura · auth.rs#verify_token"
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({
                "reads": [
                    { "path": "auth.rs", "symbol": "verify_token" },
                    { "path": "lib.rs" },
                    { "path": "src/main.rs", "rev": "HEAD" }
                ]
            })),
            "read_many: 3 leituras · auth.rs#verify_token · lib.rs · HEAD:src/main.rs"
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({
                "reads": [
                    { "path": "a.rs" }, { "path": "b.rs" },
                    { "path": "c.rs" }, { "path": "d.rs" }
                ]
            })),
            "read_many: 4 leituras · a.rs · b.rs · c.rs · +1"
        );
        assert_eq!(tool.describe_call(&serde_json::json!({})), "read_many");
    }

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

    #[test]
    fn a_rev_item_reads_the_commit_not_the_worktree() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {\n    1\n}\n").unwrap();
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
                "x",
            ],
        );
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {\n    2\n}\n").unwrap();

        let mut params = batch(vec![item("src/a.rs", Some("a"))]);
        params.reads[0].rev = Some("HEAD".to_string());
        let out = tool_for(dir.path()).read_many(params).unwrap();
        assert!(out.contains("    1"));
        assert!(!out.contains("    2"));
        assert!(out.contains("somente leitura"));
    }
}
