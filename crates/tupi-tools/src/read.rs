use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blake3::Hash;
use serde::Deserialize;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::ast::AstChunker;
use crate::budget::{self, Section};
use crate::ledger::{self, Delivery, DeliveryLedger, Seen, Verdict};
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct ReadParams {
    pub path: String,
    pub symbol: Option<String>,
    pub budget_tokens: Option<usize>,
    /// `"allow"` pede o corpo mesmo se esta sessão já o recebeu.
    pub repeat: Option<String>,
    /// Opcional: lê `rev:path` (commit, tag ou ref) em vez da árvore de trabalho.
    pub rev: Option<String>,
}

/// Uma seção e o que dizer ao ledger quando se decidir se ela entrou na resposta.
pub(crate) struct Rendered {
    pub(crate) section: Section,
    pub(crate) pending: Option<Pending>,
}

#[derive(Clone)]
pub(crate) struct Pending {
    path: PathBuf,
    id: String,
    file_hash: Hash,
    text: String,
    complete: bool,
    omitted: bool,
}

/// Lê um arquivo como esqueleto, ou um símbolo, em vez do arquivo inteiro.
pub struct ReadTool {
    root_dir: PathBuf,
    pub(crate) store: Arc<ResponseStore>,
    ledger: Arc<DeliveryLedger>,
    default_budget_tokens: usize,
}

impl ReadTool {
    pub fn new(
        root_dir: PathBuf,
        store: Arc<ResponseStore>,
        ledger: Arc<DeliveryLedger>,
        default_budget_tokens: usize,
    ) -> Self {
        Self {
            root_dir,
            store,
            ledger,
            default_budget_tokens,
        }
    }

    pub fn read(&self, params: ReadParams) -> Result<String, ToolError> {
        if let Some(rev) = params
            .rev
            .as_deref()
            .map(str::trim)
            .filter(|rev| !rev.is_empty())
        {
            let (reference, sections) = self.rev_sections(&params, rev)?;
            let rendered = sections
                .into_iter()
                .map(|section| Rendered {
                    section,
                    pending: None,
                })
                .collect();
            return Ok(format!(
                "{reference}\n\n{}",
                self.finish(rendered, params.budget_tokens)
            ));
        }

        let (path, content, relative) = self.prepare(&params)?;
        let rendered = self.render(&params, &path, &content, &relative)?;

        if rendered.is_empty() {
            return Err(ToolError::SymbolNotFound(format!(
                "símbolo `{}` não encontrado em `{relative}`",
                params.symbol.as_deref().unwrap_or_default()
            )));
        }

        let hash = crate::ledger::short_hash(&crate::ledger::content_hash(&content));
        let mut reference = format!("[ref] {relative} @{hash}");
        if let Some(symbol) = &params.symbol {
            reference.push_str(" :: ");
            reference.push_str(symbol);
        }

        Ok(format!(
            "{reference}\n\n{}",
            self.finish(rendered, params.budget_tokens)
        ))
    }

    pub(crate) fn prepare_rev(
        &self,
        params: &ReadParams,
        rev: &str,
    ) -> Result<(PathBuf, String, String), ToolError> {
        let root = self
            .root_dir
            .canonicalize()
            .map_err(|_| ToolError::OutsideRoot(params.path.clone()))?;
        let candidate = Path::new(&params.path);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        let normalized = crate::paths::normalize(&joined);
        if !normalized.starts_with(&root) {
            return Err(ToolError::OutsideRoot(params.path.clone()));
        }
        let relative = normalized
            .strip_prefix(&root)
            .unwrap_or(&normalized)
            .display()
            .to_string();
        let output = crate::proc::program(
            "git",
            &["show".to_string(), format!("{rev}:{relative}")],
            &root,
            std::time::Duration::from_secs(30),
        )?;
        if !output.success {
            return Err(ToolError::NotFound(format!("{rev}:{relative}")));
        }
        Ok((normalized, output.stdout, format!("{relative}@{rev}")))
    }

    pub(crate) fn rev_sections(
        &self,
        params: &ReadParams,
        rev: &str,
    ) -> Result<(String, Vec<Section>), ToolError> {
        let (path, content, relative) = self.prepare_rev(params, rev)?;
        let request = ReadParams {
            path: params.path.clone(),
            symbol: params.symbol.clone(),
            budget_tokens: params.budget_tokens,
            repeat: Some("allow".to_string()),
            rev: None,
        };
        let rendered = self.render(&request, &path, &content, &relative)?;
        if rendered.is_empty() {
            return Err(ToolError::SymbolNotFound(format!(
                "símbolo `{}` não encontrado em `{relative}`",
                params.symbol.as_deref().unwrap_or_default()
            )));
        }
        let hash = crate::ledger::short_hash(&crate::ledger::content_hash(&content));
        let mut reference = format!("[ref] {relative} @{hash} (somente leitura)");
        if let Some(symbol) = &params.symbol {
            reference.push_str(" :: ");
            reference.push_str(symbol);
        }
        Ok((
            reference,
            rendered.into_iter().map(|item| item.section).collect(),
        ))
    }

    /// Resolve o caminho contra o root e lê o texto. Rejeita o que escapa.
    pub(crate) fn prepare(
        &self,
        params: &ReadParams,
    ) -> Result<(PathBuf, String, String), ToolError> {
        let candidate = Path::new(&params.path);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root_dir.join(candidate)
        };

        let canonical = joined
            .canonicalize()
            .map_err(|_| ToolError::NotFound(params.path.clone()))?;
        let root = self
            .root_dir
            .canonicalize()
            .map_err(|_| ToolError::OutsideRoot(params.path.clone()))?;

        if !canonical.starts_with(&root) {
            return Err(ToolError::OutsideRoot(params.path.clone()));
        }

        let content = std::fs::read_to_string(&canonical).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ToolError::NotFound(params.path.clone())
            } else {
                ToolError::Other(format!("falha ao ler {}: {error}", canonical.display()))
            }
        })?;

        let relative = canonical
            .strip_prefix(&root)
            .unwrap_or(&canonical)
            .display()
            .to_string();

        Ok((canonical, content, relative))
    }

    pub(crate) fn render(
        &self,
        params: &ReadParams,
        path: &Path,
        content: &str,
        relative: &str,
    ) -> Result<Vec<Rendered>, ToolError> {
        match &params.symbol {
            Some(symbol) => self.symbol_sections(
                path,
                content,
                relative,
                symbol,
                params.repeat.as_deref() == Some("allow"),
            ),
            None => Ok(self.outline_sections(path, content, relative)),
        }
    }

    fn outline_sections(&self, path: &Path, content: &str, relative: &str) -> Vec<Rendered> {
        let snippets = AstChunker::new().snippets(path, content);
        if snippets.is_empty() {
            return vec![Rendered {
                section: whole_file_section(content, relative),
                pending: None,
            }];
        }

        let mut sections: Vec<Rendered> = snippets
            .into_iter()
            .map(|snippet| {
                let label = if snippet.signature.is_empty() {
                    snippet.symbol_name
                } else {
                    snippet.signature
                };
                Rendered {
                    section: Section {
                        heading: format!(
                            "- `{label}` (:{}-{})",
                            snippet.start_line, snippet.end_line
                        ),
                        body: String::new(),
                        priority: 200,
                    },
                    pending: None,
                }
            })
            .collect();

        sections.insert(
            0,
            Rendered {
                section: Section {
                    heading: format!(
                        "### `{relative}` — {} linhas · blake3 `{}`",
                        content.lines().count(),
                        ledger::short_hash(&ledger::content_hash(content))
                    ),
                    body: String::new(),
                    priority: 255,
                },
                pending: None,
            },
        );
        sections
    }

    fn symbol_sections(
        &self,
        path: &Path,
        content: &str,
        relative: &str,
        symbol: &str,
        repeat_allowed: bool,
    ) -> Result<Vec<Rendered>, ToolError> {
        let snippets = AstChunker::new().snippets(path, content);
        let file_hash = ledger::content_hash(content);
        let hash = ledger::short_hash(&file_hash);

        Ok(snippets
            .into_iter()
            .filter(|snippet| snippet.symbol_name == symbol || snippet.qualified_name == symbol)
            .map(|snippet| {
                let display = snippet.qualified_name.clone();
                let heading = format!(
                    "### `{display}` (`{relative}:{}-{}`) · blake3 `{hash}`",
                    snippet.start_line, snippet.end_line
                );
                let fence = format!("```rust\n{}\n```\n", snippet.code);
                let mut body = fence.clone();

                let mut pending = Pending {
                    path: path.to_path_buf(),
                    id: snippet.qualified_name.clone(),
                    file_hash,
                    text: snippet.code.clone(),
                    complete: true,
                    omitted: false,
                };

                if !repeat_allowed {
                    match self.ledger.check(
                        path,
                        &snippet.qualified_name,
                        &file_hash,
                        &snippet.code,
                        snippet.start_line,
                    ) {
                        Verdict::Seen(seen) => {
                            let handle = self.live_handle(&seen, path, &snippet.qualified_name);
                            body = ledger::pointer_body(&seen, &handle);
                            pending.omitted = true;
                        }
                        Verdict::Changed(changed) => {
                            body = ledger::changed_body(&changed);
                        }
                        Verdict::Full { reason, .. } => {
                            body = format!("{}\n{fence}", ledger::full_note(reason));
                        }
                        Verdict::Fresh => {}
                    }
                }

                Rendered {
                    section: Section {
                        heading,
                        body,
                        priority: 255,
                    },
                    pending: Some(pending),
                }
            })
            .collect())
    }

    fn live_handle(&self, seen: &Seen, path: &Path, id: &str) -> String {
        if let Some(handle) = &seen.handle
            && self.store.get(handle).is_some()
        {
            return handle.clone();
        }
        let handle = self.store.insert(seen.text.clone());
        self.ledger.remember_handle(path, id, &handle);
        handle
    }

    pub(crate) fn finish(&self, rendered: Vec<Rendered>, budget_tokens: Option<usize>) -> String {
        let budget = budget_tokens.unwrap_or(self.default_budget_tokens);
        let rendered: Vec<Rendered> = rendered
            .into_iter()
            .flat_map(|rendered| expand_over_budget(rendered, budget))
            .collect();

        let sections: Vec<Section> = rendered
            .iter()
            .map(|rendered| rendered.section.clone())
            .collect();
        let selection = budget::select(&sections, budget);
        let handle = if selection.omitted.is_empty() {
            None
        } else {
            Some(self.store.insert(selection.omitted_body()))
        };

        self.settle(&rendered, &selection);

        budget::render(&selection, handle.as_deref())
    }

    /// Registra só o que entrou de fato: uma seção cortada pelo orçamento não foi
    /// vista e não pode ser lembrada como entregue.
    fn settle(&self, rendered: &[Rendered], selection: &budget::Selection) {
        let included: HashSet<&str> = selection
            .included
            .iter()
            .map(|section| section.heading.as_str())
            .collect();

        let mut tally: HashMap<(&Path, &str), (usize, usize)> = HashMap::new();
        for rendered in rendered {
            let Some(pending) = &rendered.pending else {
                continue;
            };
            let entry = tally
                .entry((pending.path.as_path(), pending.id.as_str()))
                .or_insert((0, 0));
            entry.0 += 1;
            if included.contains(rendered.section.heading.as_str()) {
                entry.1 += 1;
            }
        }

        let mut handled: HashSet<(&Path, &str)> = HashSet::new();
        for rendered in rendered {
            let Some(pending) = &rendered.pending else {
                continue;
            };
            let key = (pending.path.as_path(), pending.id.as_str());
            if !handled.insert(key) {
                continue;
            }
            let (total, seen) = tally.get(&key).copied().unwrap_or((1, 0));
            if seen == 0 {
                continue;
            }
            if seen < total || pending.omitted {
                self.ledger.note_omitted(&pending.path, &pending.id);
            } else {
                self.ledger.record(Delivery {
                    tool: "read",
                    path: &pending.path,
                    id: &pending.id,
                    file_hash: pending.file_hash,
                    text: &pending.text,
                    complete: pending.complete,
                    handle: None,
                });
            }
        }
    }
}

fn expand_over_budget(rendered: Rendered, budget: usize) -> Vec<Rendered> {
    let parts = budget::split_section(&rendered.section, budget);
    if parts.len() <= 1 {
        return vec![rendered];
    }
    let pending = rendered.pending;
    parts
        .into_iter()
        .map(|section| Rendered {
            section,
            pending: pending.clone(),
        })
        .collect()
}

fn whole_file_section(content: &str, relative: &str) -> Section {
    let lang = Path::new(relative)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    Section {
        heading: format!(
            "### `{relative}` — arquivo inteiro · blake3 `{}`",
            ledger::short_hash(&ledger::content_hash(content))
        ),
        body: format!("```{lang}\n{content}\n```\n"),
        priority: 255,
    }
}

impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: "Esqueleto do arquivo ou corpo de um símbolo, com blake3.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Caminho relativo ao diretório raiz do projeto."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Opcional: nome da função/struct, simples (`new`) ou qualificado (`Foo::new`), cujo corpo será devolvido."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    },
                    "repeat": {
                        "type": "string",
                        "enum": ["allow"],
                        "description": "`allow` devolve o corpo mesmo se esta sessão já o recebeu; o padrão aponta para o corpo anterior."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: ReadParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.read(params)
    }

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
            parts.push(format!("path={path}"));
        }
        if let Some(symbol) = arguments.get("symbol").and_then(|value| value.as_str()) {
            parts.push(format!("symbol={symbol}"));
        }
        if let Some(budget) = arguments
            .get("budget_tokens")
            .and_then(|value| value.as_u64())
        {
            parts.push(format!("budget={budget}"));
        }
        if let Some(repeat) = arguments.get("repeat").and_then(|value| value.as_str()) {
            parts.push(format!("repeat={repeat}"));
        }

        if parts.is_empty() {
            "read".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod ref_tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn tool_for(dir: &std::path::Path) -> ReadTool {
        ReadTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1024 * 1024,
            )),
            Arc::new(DeliveryLedger::new(64, 1 << 20)),
            crate::budget::DEFAULT_BUDGET_TOKENS,
        )
    }

    fn read(tool: &ReadTool, path: &str, symbol: Option<&str>) -> Result<String, ToolError> {
        tool.read(ReadParams {
            path: path.to_string(),
            symbol: symbol.map(str::to_string),
            budget_tokens: None,
            repeat: None,
            rev: None,
        })
    }

    #[test]
    fn read_starts_with_a_reference_line() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("auth.rs"), "pub fn alpha() {}\n").unwrap();
        let tool = tool_for(dir.path());
        let out = read(&tool, "auth.rs", None).unwrap();
        let first = out.lines().next().unwrap();
        assert!(first.starts_with("[ref] auth.rs @"), "{first}");
    }

    #[test]
    fn symbol_read_names_the_symbol_in_the_reference() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("auth.rs"), "pub fn alpha() {}\n").unwrap();
        let tool = tool_for(dir.path());
        let out = read(&tool, "auth.rs", Some("alpha")).unwrap();
        assert!(out.lines().next().unwrap().ends_with(" :: alpha"));
    }

    #[test]
    fn missing_file_is_not_found_even_if_the_index_lists_it() {
        let dir = tempdir().unwrap();
        let tool = tool_for(dir.path());
        assert!(matches!(
            read(&tool, "absent.rs", None),
            Err(ToolError::NotFound(_))
        ));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::{TempDir, tempdir};
    use tupi_kernel::{Tool, ToolError};

    use crate::budget::DEFAULT_BUDGET_TOKENS;

    const SOURCE: &str = "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n\npub fn logout() {\n}\n";

    fn tool_for(dir: &Path) -> ReadTool {
        ReadTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1024 * 1024,
            )),
            Arc::new(DeliveryLedger::new(64, 1 << 20)),
            DEFAULT_BUDGET_TOKENS,
        )
    }

    fn fixture() -> TempDir {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("auth.rs"), SOURCE).unwrap();
        dir
    }

    fn read(
        tool: &ReadTool,
        path: &str,
        symbol: Option<&str>,
        budget: Option<usize>,
        repeat: Option<&str>,
    ) -> Result<String, ToolError> {
        tool.read(ReadParams {
            path: path.to_string(),
            symbol: symbol.map(str::to_string),
            budget_tokens: budget,
            repeat: repeat.map(str::to_string),
            rev: None,
        })
    }

    #[test]
    fn read_describes_the_parameters_the_model_used() {
        let dir = fixture();
        let tool = tool_for(dir.path());

        assert_eq!(
            tool.describe_call(&serde_json::json!({
                "path": "auth.rs",
                "symbol": "verify_token"
            })),
            "path=auth.rs · symbol=verify_token"
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({
                "path": "auth.rs",
                "budget_tokens": 300,
                "repeat": "allow"
            })),
            "path=auth.rs · budget=300 · repeat=allow"
        );
    }

    #[test]
    fn outline_lists_every_symbol_without_bodies() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let text = read(&tool, "auth.rs", None, None, None).unwrap();

        assert!(text.contains("### `auth.rs`"), "{text}");
        assert!(text.contains("pub fn verify_token"), "{text}");
        assert!(text.contains("pub fn logout"), "{text}");
        assert!(!text.contains("!raw.is_empty()"), "{text}");
    }

    #[test]
    fn asking_for_a_symbol_returns_only_that_body() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let text = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();

        assert!(text.contains("!raw.is_empty()"), "{text}");
        assert!(!text.contains("pub fn logout"), "{text}");
        assert!(text.contains(":1-3`"), "{text}");
    }

    #[test]
    fn unknown_symbol_is_an_explicit_error() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let error = read(&tool, "auth.rs", Some("nope"), None, None).unwrap_err();
        assert!(matches!(error, ToolError::SymbolNotFound(_)));
    }

    #[test]
    fn path_outside_the_root_is_rejected() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let error = read(&tool, "../../etc/passwd", None, None, None).unwrap_err();
        assert!(matches!(error, ToolError::OutsideRoot(_)));
    }

    #[test]
    fn missing_file_is_an_explicit_error() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let error = read(&tool, "absent.rs", None, None, None).unwrap_err();
        assert!(matches!(error, ToolError::NotFound(_)));
    }

    #[test]
    fn non_utf8_file_is_an_explicit_error() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("bin.rs"), [0xff, 0xfe]).unwrap();
        let tool = tool_for(dir.path());
        let error = read(&tool, "bin.rs", None, None, None).unwrap_err();
        assert!(matches!(error, ToolError::Other(_)), "{error:?}");
    }

    #[test]
    fn the_second_read_of_an_unchanged_symbol_is_a_pointer_with_a_live_handle() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        let first = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
        assert!(first.contains("!raw.is_empty()"));

        let second = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
        assert!(second.contains("Já entregue nesta sessão"), "{second}");
        assert!(!second.contains("!raw.is_empty()"), "{second}");

        let handle = second
            .split("`peek` `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .unwrap();
        assert!(tool.store.get(handle).unwrap().contains("!raw.is_empty()"));
    }

    #[test]
    fn a_changed_symbol_returns_only_the_diff() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();

        fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.trim().is_empty()\n}\n\npub fn logout() {\n}\n",
        )
        .unwrap();

        let second = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
        assert!(second.contains("Mudou desde a entrega"), "{second}");
        assert!(second.contains("+    !raw.trim().is_empty()"), "{second}");
        assert!(second.contains("-    !raw.is_empty()"), "{second}");
    }

    #[test]
    fn repeat_allow_returns_the_body_and_registers_it_again() {
        let dir = fixture();
        let tool = tool_for(dir.path());
        read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();

        let again = read(&tool, "auth.rs", Some("verify_token"), None, Some("allow")).unwrap();
        assert!(again.contains("!raw.is_empty()"), "{again}");
        assert!(!again.contains("Já entregue"), "{again}");

        let third = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
        assert!(third.contains("Já entregue"), "{third}");
    }

    #[test]
    fn an_oversized_symbol_is_split_and_the_tail_is_recoverable() {
        let dir = tempdir().unwrap();
        let body: Vec<String> = (0..300).map(|i| format!("    let v{i} = {i};")).collect();
        fs::write(
            dir.path().join("long.rs"),
            format!(
                "pub fn long_fn() -> u32 {{\n{}\n    0\n}}\n",
                body.join("\n")
            ),
        )
        .unwrap();
        let tool = tool_for(dir.path());

        let text = read(&tool, "long.rs", Some("long_fn"), Some(300), None).unwrap();
        assert!(text.contains("parte 1/"), "{text}");

        let handle = text
            .split("handle: `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .unwrap();
        let tail = tool.store.get(handle).unwrap();
        assert!(tail.contains("let v299"), "{tail}");
        assert!(tail.contains("parte 2/"), "{tail}");
    }

    #[test]
    fn duplicate_names_return_every_matching_body() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.rs"),
            "impl A {\n    fn new() {}\n}\n\nimpl B {\n    fn new() {}\n}\n",
        )
        .unwrap();
        let tool = tool_for(dir.path());

        let text = read(&tool, "a.rs", Some("new"), None, None).unwrap();
        assert!(text.contains("`A::new`"), "{text}");
        assert!(text.contains("`B::new`"), "{text}");
    }

    #[test]
    fn a_qualified_symbol_reads_only_the_matching_container() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.rs"),
            "impl A {\n    fn new() -> u8 { 111 }\n}\n\nimpl B {\n    fn new() -> u8 { 222 }\n}\n",
        )
        .unwrap();
        let tool = tool_for(dir.path());

        let text = read(&tool, "a.rs", Some("B::new"), None, None).unwrap();
        assert!(text.contains("222"), "{text}");
        assert!(!text.contains("111"), "{text}");
        assert!(text.contains("B::new"), "{text}");
    }

    #[test]
    fn unsupported_files_fall_back_to_the_whole_content() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "# tupi\n\nleitura\n").unwrap();
        let tool = tool_for(dir.path());

        let text = read(&tool, "README.md", None, None, None).unwrap();
        assert!(text.contains("arquivo inteiro"), "{text}");
        assert!(text.contains("# tupi"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_escapes_the_root_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = fixture();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("secret.rs"), "pub fn secret() {}\n").unwrap();
        symlink(outside.path().join("secret.rs"), dir.path().join("link.rs")).unwrap();
        let tool = tool_for(dir.path());

        let error = read(&tool, "link.rs", None, None, None).unwrap_err();
        assert!(matches!(error, ToolError::OutsideRoot(_)), "{error:?}");
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
    fn a_rev_read_comes_from_the_commit_not_the_worktree() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init"]);
        fs::write(dir.path().join("a.rs"), "fn a() {\n    1\n}\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
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
        fs::write(dir.path().join("a.rs"), "fn a() {\n    2\n}\n").unwrap();
        let tool = tool_for(dir.path());

        let out = tool
            .read(ReadParams {
                path: "a.rs".to_string(),
                symbol: Some("a".to_string()),
                budget_tokens: None,
                repeat: None,
                rev: Some("HEAD".to_string()),
            })
            .unwrap();
        assert!(out.contains("somente leitura"));
        assert!(out.contains("    1"));
    }

    #[test]
    fn a_rev_read_does_not_mark_the_worktree_as_delivered() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init"]);
        fs::write(dir.path().join("a.rs"), "fn a() {\n    1\n}\n").unwrap();
        git(dir.path(), &["add", "a.rs"]);
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
        fs::write(dir.path().join("a.rs"), "fn a() {\n    2\n}\n").unwrap();
        let tool = tool_for(dir.path());

        tool.read(ReadParams {
            path: "a.rs".to_string(),
            symbol: Some("a".to_string()),
            budget_tokens: None,
            repeat: None,
            rev: Some("HEAD".to_string()),
        })
        .unwrap();

        let worktree = read(&tool, "a.rs", Some("a"), None, None).unwrap();
        assert!(worktree.contains("    2"));
        assert!(!worktree.contains("Já entregue"));
    }

    #[test]
    fn an_unknown_rev_is_not_found() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init"]);
        fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        let tool = tool_for(dir.path());
        let error = tool
            .read(ReadParams {
                path: "a.rs".to_string(),
                symbol: None,
                budget_tokens: None,
                repeat: None,
                rev: Some("nao-existe".to_string()),
            })
            .unwrap_err();
        assert!(matches!(error, ToolError::NotFound(_)), "{error:?}");
    }

    #[test]
    fn a_rev_path_outside_the_root_is_rejected() {
        let dir = tempdir().unwrap();
        git(dir.path(), &["init"]);
        let tool = tool_for(dir.path());
        let error = tool
            .read(ReadParams {
                path: "../fora.rs".to_string(),
                symbol: None,
                budget_tokens: None,
                repeat: None,
                rev: Some("HEAD".to_string()),
            })
            .unwrap_err();
        assert!(matches!(error, ToolError::OutsideRoot(_)), "{error:?}");
    }
}
