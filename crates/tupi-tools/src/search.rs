use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::ast::{AstChunker, AstSnippet, compact_signature};
use crate::budget::{self, Section, estimate_tokens};
use crate::ledger::{self, Delivery, DeliveryLedger, Seen, Verdict};
use crate::ranking::LexicalRanker;
use crate::scanner::LexicalScanner;
use crate::store::ResponseStore;
use crate::terms::extract_terms;

const MAX_SEARCH_BODY_LINES: usize = 80;
const MAX_FULL_BODIES_IN_AUTO: usize = 2;
const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_MAX_RESULTS: usize = 20;

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub query: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub max_results: Option<usize>,
    #[serde(default)]
    pub detail: Option<SearchDetail>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
    #[serde(default)]
    pub repeat: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchDetail {
    #[default]
    Auto,
    Full,
    Signatures,
}

struct Candidate {
    path: PathBuf,
    content: String,
    snippet: AstSnippet,
}

struct Pending {
    path: PathBuf,
    id: String,
    file_hash: blake3::Hash,
    text: String,
    complete: bool,
    omitted: bool,
}

struct Rendered {
    section: Section,
    pending: Option<Pending>,
}

/// Busca lexical no projeto: query natural, ranking por nome/assinatura/corpo.
pub struct SearchTool {
    root_dir: PathBuf,
    scanner: LexicalScanner,
    pub(crate) store: Arc<ResponseStore>,
    ledger: Arc<DeliveryLedger>,
    default_budget_tokens: usize,
}

impl SearchTool {
    pub fn new(
        root_dir: PathBuf,
        store: Arc<ResponseStore>,
        ledger: Arc<DeliveryLedger>,
        default_budget_tokens: usize,
    ) -> Self {
        Self {
            scanner: LexicalScanner::new(root_dir.clone()),
            root_dir,
            store,
            ledger,
            default_budget_tokens,
        }
    }

    pub fn search(&self, params: SearchParams) -> Result<String, ToolError> {
        let terms = extract_terms(&params.query, &params.keywords);
        if terms.is_empty() {
            return Err(ToolError::InvalidArguments(
                "query sem termos de busca (só stopwords ou vazia)".to_string(),
            ));
        }

        let max_results = params
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, MAX_MAX_RESULTS);
        let detail = params.detail.unwrap_or_default();
        let budget = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        let repeat_allowed = params.repeat.as_deref() == Some("allow");

        let matches = self.scanner.search(&terms, params.path.as_deref())?;
        if matches.is_empty() {
            return Ok(format!(
                "Nenhum resultado encontrado para `{}`.",
                params.query.trim()
            ));
        }

        let mut by_file: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for found in matches {
            by_file
                .entry(found.file_path)
                .or_default()
                .push(found.line_number);
        }

        let chunker = AstChunker::new();
        let mut candidates = Vec::new();
        for (path, lines) in by_file {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for snippet in chunker.snippets_at_lines(&path, &content, &lines) {
                candidates.push(Candidate {
                    path: path.clone(),
                    content: content.clone(),
                    snippet,
                });
            }
        }
        candidates.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then_with(|| a.snippet.start_line.cmp(&b.snippet.start_line))
        });

        if candidates.is_empty() {
            return Ok(format!(
                "Termos encontrados, porém nenhum bloco de código pôde ser delimitado para `{}`.",
                params.query.trim()
            ));
        }

        let snippets: Vec<AstSnippet> = candidates
            .iter()
            .map(|candidate| candidate.snippet.clone())
            .collect();
        let ranked = LexicalRanker::new().rank_indices(&terms, &snippets);
        let total = candidates.len();
        let selected: Vec<(usize, f32)> = ranked.into_iter().take(max_results).collect();

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let mut rendered = Vec::with_capacity(selected.len());
        let mut full_bodies = 0usize;
        let mut full_tokens = 0usize;
        let mut signature_only = 0usize;

        for (position, (index, score)) in selected.iter().enumerate() {
            let candidate = &candidates[*index];
            let snippet = &candidate.snippet;
            let relative = candidate
                .path
                .strip_prefix(&root)
                .unwrap_or(&candidate.path)
                .display();

            let heading = format!(
                "#### {}. `{}` (`{}:{}-{}`)",
                position + 1,
                snippet.symbol_name,
                relative,
                snippet.start_line,
                snippet.end_line
            );

            let code_tokens = estimate_tokens(&snippet.code);
            let wants_full = match detail {
                SearchDetail::Full => true,
                SearchDetail::Signatures => false,
                SearchDetail::Auto => {
                    full_bodies < MAX_FULL_BODIES_IN_AUTO
                        && (full_bodies == 0 || full_tokens + code_tokens <= budget * 2 / 3)
                }
            };

            if !wants_full {
                signature_only += 1;
                let body = if snippet.signature.is_empty() {
                    String::new()
                } else {
                    format!("`{}`\n\n", compact_signature(&snippet.signature))
                };
                rendered.push(Rendered {
                    section: Section {
                        heading,
                        body,
                        priority: (score * 200.0) as u8,
                    },
                    pending: None,
                });
                continue;
            }

            full_bodies += 1;
            full_tokens += code_tokens + estimate_tokens(&heading);
            let (code, capped) = capped_body(snippet);
            let mut body = format!("```rust\n{code}\n```\n");
            let pending = self.apply_ledger(candidate, &mut body, capped, repeat_allowed);

            rendered.push(Rendered {
                section: Section {
                    heading,
                    body,
                    priority: (score * 200.0) as u8,
                },
                pending,
            });
        }

        let completeness = completeness_line(total, selected.len(), max_results, signature_only);
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

        let mut out = format!("### Resultados da busca\n{completeness}\n\n");
        if signature_only > 0 {
            out.push_str(&format!(
                "> {signature_only} resultado(s) só com assinatura.\n\n"
            ));
        }
        out.push_str(&budget::render(&selection, handle.as_deref()));
        Ok(out)
    }

    fn apply_ledger(
        &self,
        candidate: &Candidate,
        body: &mut String,
        capped: bool,
        repeat_allowed: bool,
    ) -> Option<Pending> {
        let snippet = &candidate.snippet;
        let range = ledger::line_span(&candidate.content, snippet.start_line, snippet.end_line)?;
        let span_text = candidate.content.get(range)?;
        let complete = !capped && span_text.trim_end() == snippet.code.trim_end();
        let file_hash = ledger::content_hash(&candidate.content);
        let id = snippet.qualified_name.clone();

        let draft = Pending {
            path: candidate.path.clone(),
            id: id.clone(),
            file_hash,
            text: span_text.to_string(),
            complete,
            omitted: false,
        };

        if repeat_allowed {
            return Some(draft);
        }

        match self.ledger.check(
            &candidate.path,
            &id,
            &file_hash,
            span_text,
            snippet.start_line,
        ) {
            Verdict::Seen(seen) => {
                let handle = self.live_handle(&seen, &candidate.path, &id);
                *body = ledger::pointer_body(&seen, &handle);
                Some(Pending {
                    omitted: true,
                    ..draft
                })
            }
            Verdict::Changed(changed) => {
                *body = ledger::changed_body(&changed);
                Some(draft)
            }
            Verdict::Full { reason, .. } => {
                *body = format!("{}\n{body}", ledger::full_note(reason));
                Some(draft)
            }
            Verdict::Fresh => Some(draft),
        }
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

    fn settle(&self, rendered: &[Rendered], selection: &budget::Selection) {
        let included: std::collections::HashSet<&str> = selection
            .included
            .iter()
            .map(|section| section.heading.as_str())
            .collect();

        let mut handled: HashMap<(&Path, &str), bool> = HashMap::new();
        for rendered in rendered {
            let Some(pending) = &rendered.pending else {
                continue;
            };
            let key = (pending.path.as_path(), pending.id.as_str());
            let included_here = included.contains(rendered.section.heading.as_str());

            match handled.get_mut(&key) {
                Some(seen) => *seen |= included_here,
                None => {
                    handled.insert(key, included_here);
                }
            }
        }

        for rendered in rendered {
            let Some(pending) = &rendered.pending else {
                continue;
            };
            let key = (pending.path.as_path(), pending.id.as_str());
            let Some(true) = handled.get(&key).copied() else {
                continue;
            };
            if handled.remove(&key).is_none() {
                continue;
            }
            if pending.omitted {
                self.ledger.note_omitted(&pending.path, &pending.id);
            } else {
                self.ledger.record(Delivery {
                    tool: "search",
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

fn capped_body(snippet: &AstSnippet) -> (String, bool) {
    let lines: Vec<&str> = snippet.code.lines().collect();
    if lines.len() <= MAX_SEARCH_BODY_LINES {
        return (snippet.code.clone(), false);
    }

    let remaining = lines.len() - MAX_SEARCH_BODY_LINES;
    (
        format!(
            "{}\n// ... (+{remaining} linhas; peça o corpo inteiro com read)",
            lines[..MAX_SEARCH_BODY_LINES].join("\n")
        ),
        true,
    )
}

fn completeness_line(
    total: usize,
    shown: usize,
    max_results: usize,
    signature_only: usize,
) -> String {
    let mut line = if shown < total {
        format!("#### {shown} de {total} resultados — max_results={max_results} cortou o resto.")
    } else {
        format!("#### Todas as {shown} candidatas casaram com a query.")
    };
    if signature_only > 0 {
        line.push_str(&format!(" {signature_only} só com assinatura."));
    }
    line.push_str(" lexical.");
    line
}

impl Tool for SearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search".to_string(),
            description: "Busca lexical no projeto por descrição natural ou nome: devolve corpo dos primeiros resultados e assinatura do resto, dentro do orçamento.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Descrição em linguagem natural do que se procura."
                    },
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Opcional: nomes/termos exatos que devem pesar mais."
                    },
                    "path": {
                        "type": "string",
                        "description": "Opcional: prefixo relativo ao root para restringir a varredura."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "default": 5,
                        "description": "Quantos resultados devolver."
                    },
                    "detail": {
                        "type": "string",
                        "enum": ["auto", "full", "signatures"],
                        "description": "auto: corpo dos 2 primeiros e assinatura do resto; full: corpo de todos; signatures: só assinaturas."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    },
                    "repeat": {
                        "type": "string",
                        "enum": ["allow"],
                        "description": "`allow` devolve corpos mesmo se esta sessão já os recebeu."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: SearchParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.search(params)
    }

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        if let Some(query) = arguments.get("query").and_then(|value| value.as_str()) {
            parts.push(format!("query={query}"));
        }
        if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
            parts.push(format!("path={path}"));
        }
        if let Some(detail) = arguments.get("detail").and_then(|value| value.as_str())
            && detail != "auto"
        {
            parts.push(format!("detail={detail}"));
        }
        if parts.is_empty() {
            "search".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::{TempDir, tempdir};

    use crate::budget::{DEFAULT_BUDGET_TOKENS, PEEK_TOOL_NAME};

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn tool_for(dir: &Path) -> SearchTool {
        SearchTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1024 * 1024,
            )),
            Arc::new(DeliveryLedger::new(64, 1 << 20)),
            DEFAULT_BUDGET_TOKENS,
        )
    }

    fn params(query: &str) -> SearchParams {
        SearchParams {
            query: query.to_string(),
            keywords: Vec::new(),
            path: None,
            max_results: None,
            detail: None,
            budget_tokens: None,
            repeat: None,
        }
    }

    fn auth_fixture() -> TempDir {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            "src/auth.rs",
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
        );
        write(dir.path(), "src/other.rs", "pub fn logout() {}\n");
        dir
    }

    #[test]
    fn a_natural_phrase_finds_the_symbol_and_returns_the_body() {
        let dir = auth_fixture();
        let out = tool_for(dir.path())
            .search(params("onde o token é validado"))
            .unwrap();

        assert!(out.contains("verify_token"), "{out}");
        assert!(out.contains("!raw.is_empty()"), "{out}");
        assert!(out.contains("Todas as"), "{out}");
        assert!(out.contains("lexical"), "{out}");
    }

    #[test]
    fn keywords_are_optional_and_boost_the_name() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a.rs", "pub fn verify_token_helper() {}\n");
        write(
            dir.path(),
            "b.rs",
            "pub fn other() {\n    let verify_token = 1;\n}\n",
        );
        let tool = tool_for(dir.path());
        let mut request = params("algo");
        request.keywords = vec!["verify_token".to_string()];

        let out = tool.search(request).unwrap();
        let name = out.find("verify_token_helper").unwrap();
        let body = out.find("fn other").unwrap_or(usize::MAX);

        assert!(name < body, "{out}");
    }

    #[test]
    fn an_exact_name_beats_a_body_occurrence() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a.rs", "pub fn other() {\n    token();\n}\n");
        write(dir.path(), "z.rs", "pub fn token() {}\n");
        let out = tool_for(dir.path()).search(params("token")).unwrap();

        let name = out.find("`token`").unwrap();
        let other = out.find("other").unwrap();
        assert!(name < other, "{out}");
    }

    #[test]
    fn auto_sends_two_bodies_and_signatures_for_the_rest() {
        let dir = tempdir().unwrap();
        for name in ["handler_a", "handler_b", "handler_c"] {
            write(
                dir.path(),
                &format!("{name}.rs"),
                &format!("pub fn {name}() -> u8 {{\n    1\n}}\n"),
            );
        }
        let out = tool_for(dir.path()).search(params("handler")).unwrap();

        assert_eq!(out.matches("```rust").count(), 2, "{out}");
        assert!(out.contains("1 só com assinatura"), "{out}");
    }

    #[test]
    fn signatures_detail_has_no_code_fence() {
        let dir = tempdir().unwrap();
        write(dir.path(), "a.rs", "pub fn handler() {}\n");
        let mut request = params("handler");
        request.detail = Some(SearchDetail::Signatures);
        let out = tool_for(dir.path()).search(request).unwrap();

        assert!(!out.contains("```rust"), "{out}");
        assert!(out.contains("fn handler"), "{out}");
    }

    #[test]
    fn full_detail_sends_a_body_for_every_result() {
        let dir = tempdir().unwrap();
        for name in ["handler_a", "handler_b", "handler_c"] {
            write(
                dir.path(),
                &format!("{name}.rs"),
                &format!("pub fn {name}() -> u8 {{\n    1\n}}\n"),
            );
        }
        let mut request = params("handler");
        request.detail = Some(SearchDetail::Full);
        let out = tool_for(dir.path()).search(request).unwrap();

        assert_eq!(out.matches("```rust").count(), 3, "{out}");
    }

    #[test]
    fn a_max_results_cut_is_declared() {
        let dir = tempdir().unwrap();
        for name in ["handler_a", "handler_b", "handler_c", "handler_d"] {
            write(
                dir.path(),
                &format!("{name}.rs"),
                &format!("pub fn {name}() {{}}\n"),
            );
        }
        let mut request = params("handler");
        request.max_results = Some(1);
        let out = tool_for(dir.path()).search(request).unwrap();

        assert!(out.contains("max_results=1 cortou o resto"), "{out}");
    }

    #[test]
    fn a_path_prefix_restricts_the_search() {
        let dir = auth_fixture();
        write(dir.path(), "top.rs", "pub fn verify_top() {}\n");
        let mut request = params("verify");
        request.path = Some("src".to_string());
        let out = tool_for(dir.path()).search(request).unwrap();

        assert!(!out.contains("top.rs"), "{out}");
        assert!(out.contains("auth.rs"), "{out}");
    }

    #[test]
    fn empty_or_stopword_only_queries_are_rejected() {
        let dir = auth_fixture();
        let error = tool_for(dir.path())
            .search(params("onde o é a"))
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn no_match_is_an_explicit_answer() {
        let dir = auth_fixture();
        let out = tool_for(dir.path()).search(params("zzznothing")).unwrap();
        assert!(out.contains("Nenhum resultado"), "{out}");
    }

    #[test]
    fn a_long_hit_is_capped_and_points_to_read() {
        let dir = tempdir().unwrap();
        let body: Vec<String> = (0..120).map(|i| format!("    let v{i} = {i};")).collect();
        write(
            dir.path(),
            "long.rs",
            &format!(
                "pub fn long_handler() -> u32 {{\n{}\n    0\n}}\n",
                body.join("\n")
            ),
        );
        let mut request = params("long_handler");
        request.detail = Some(SearchDetail::Full);
        let out = tool_for(dir.path()).search(request).unwrap();

        assert!(out.contains("peça o corpo inteiro com read"), "{out}");
        assert!(!out.contains("let v119"), "{out}");
    }

    #[test]
    fn a_small_budget_truncates_and_peek_recovers() {
        let dir = tempdir().unwrap();
        let body = |name: &str| {
            format!(
                "pub fn {name}() -> u8 {{\n{}\n    1\n}}\n",
                "    let _ = 1;\n".repeat(80)
            )
        };
        write(dir.path(), "one.rs", &body("handler_one"));
        write(dir.path(), "two.rs", &body("handler_two"));
        let tool = tool_for(dir.path());
        let mut request = params("handler");
        request.detail = Some(SearchDetail::Full);
        request.budget_tokens = Some(120);

        let out = tool.search(request).unwrap();

        assert!(out.contains("Resposta truncada"), "{out}");
        assert!(out.contains(PEEK_TOOL_NAME), "{out}");
        let handle = out
            .split("handle: `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .unwrap();
        assert!(tool.store.get(handle).is_some());
    }

    #[test]
    fn the_second_search_of_the_same_symbol_is_a_pointer() {
        let dir = auth_fixture();
        let tool = tool_for(dir.path());
        let first = tool.search(params("verify_token")).unwrap();
        assert!(first.contains("!raw.is_empty()"), "{first}");

        let second = tool.search(params("verify_token")).unwrap();
        assert!(second.contains("Já entregue nesta sessão"), "{second}");
        assert!(!second.contains("!raw.is_empty()"), "{second}");

        let handle = second
            .split("`peek` `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .unwrap();
        assert!(tool.store.get(handle).is_some());
    }

    #[test]
    fn a_changed_file_turns_the_hit_into_a_diff() {
        let dir = auth_fixture();
        let tool = tool_for(dir.path());
        tool.search(params("verify_token")).unwrap();

        write(
            dir.path(),
            "src/auth.rs",
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.trim().is_empty()\n}\n",
        );
        let second = tool.search(params("verify_token")).unwrap();

        assert!(second.contains("Mudou desde a entrega"), "{second}");
        assert!(second.contains("+    !raw.trim().is_empty()"), "{second}");
    }

    #[test]
    fn repeat_allow_returns_the_body_again() {
        let dir = auth_fixture();
        let tool = tool_for(dir.path());
        tool.search(params("verify_token")).unwrap();

        let mut request = params("verify_token");
        request.repeat = Some("allow".to_string());
        let again = tool.search(request).unwrap();

        assert!(again.contains("!raw.is_empty()"), "{again}");
        assert!(!again.contains("Já entregue"), "{again}");
    }

    #[test]
    fn a_read_delivery_makes_the_search_a_pointer() {
        use crate::read::{ReadParams, ReadTool};

        let dir = auth_fixture();
        let store = Arc::new(ResponseStore::new(
            std::time::Duration::from_secs(60),
            1024 * 1024,
        ));
        let ledger = Arc::new(DeliveryLedger::new(64, 1 << 20));
        ReadTool::new(
            dir.path().to_path_buf(),
            store.clone(),
            ledger.clone(),
            DEFAULT_BUDGET_TOKENS,
        )
        .read(ReadParams {
            path: "src/auth.rs".to_string(),
            symbol: Some("verify_token".to_string()),
            budget_tokens: None,
            repeat: None,
            rev: None,
        })
        .unwrap();

        let search = SearchTool::new(
            dir.path().to_path_buf(),
            store,
            ledger,
            DEFAULT_BUDGET_TOKENS,
        );
        let out = search.search(params("verify_token")).unwrap();

        assert!(out.contains("Já entregue nesta sessão"), "{out}");
    }

    #[test]
    fn a_path_outside_the_root_is_rejected() {
        let dir = auth_fixture();
        let mut request = params("token");
        request.path = Some("../fora".to_string());
        let error = tool_for(dir.path()).search(request).unwrap_err();
        assert!(matches!(error, ToolError::OutsideRoot(_)));
    }

    #[test]
    fn describe_shows_the_query_and_filters() {
        let dir = auth_fixture();
        let tool = tool_for(dir.path());
        let label = tool.describe_call(&serde_json::json!({
            "query": "onde valida token",
            "path": "src",
            "detail": "signatures"
        }));

        assert_eq!(
            label,
            "query=onde valida token · path=src · detail=signatures"
        );
    }
}
