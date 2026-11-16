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

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Deserialize)]
pub struct ShellParams {
    pub command: String,
    /// Filtro duro: substring, case-insensitive, OR entre os termos.
    #[serde(default)]
    pub terms: Vec<String>,
    /// Ranking mole dos blocos por relevancia lexical.
    #[serde(default)]
    pub question: Option<String>,
    /// Linhas de contexto em volta de cada casamento.
    #[serde(default)]
    pub context: Option<usize>,
    /// Teto de blocos devolvidos.
    #[serde(default)]
    pub max_blocks: Option<usize>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct ShellTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl ShellTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

/// bash -lc sem sandbox. So formatador entra: qualquer composicao de
/// comandos e recusada antes de chegar ao shell.
pub(crate) fn allowed(command: &str, allow: &[String]) -> bool {
    /// Programas que executam outro comando: liberar um liberaria tudo.
    const WRAPPERS: &[&str] = &[
        "sh", "bash", "zsh", "dash", "fish", "env", "xargs", "sudo", "doas", "su", "nohup",
        "timeout", "nice", "watch", "parallel", "eval", "exec", "python", "python3", "node",
        "deno", "bun", "perl", "ruby", "php", "lua", "make", "just",
    ];

    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed
        .chars()
        .any(|c| matches!(c, ';' | '&' | '|' | '>' | '<' | '$' | '\n' | '\r'))
        || trimmed.contains('\u{0060}')
        || trimmed.contains("--manifest-path")
    {
        return false;
    }

    // Formatador e o contrato antigo da tool: vale com qualquer lista.
    if trimmed == "cargo fmt" || trimmed.starts_with("cargo fmt ") {
        return true;
    }
    if let Some(rest) = trimmed.strip_prefix("rustfmt") {
        let rest = rest.trim();
        if rest.is_empty() || rest.split_whitespace().all(|token| token.starts_with('-')) {
            return true;
        }
    }

    let program = trimmed
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default();
    // git -c e --config-env aceitam alias que rodam comando: a allowlist do
    // binario seria contornada por dentro dele.
    if program == "git"
        && trimmed
            .split_whitespace()
            .skip(1)
            .any(|arg| arg.starts_with("-c") || arg.starts_with("--config-env"))
    {
        return false;
    }
    !WRAPPERS.contains(&program) && allow.iter().any(|name| name == program)
}

impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description: "Roda um comando no root e devolve so os trechos da saida que casam com terms/question. O binario precisa estar na allowlist do projeto (ai.project.json, chave shell.allow); composicao de comandos nao e permitida.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Comando unico executado por bash -lc, com cwd no diretorio raiz. Sem ; & | > < $ crase nem --manifest-path."
                    },
                    "terms": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filtro duro: devolve so os blocos cujas linhas contem algum termo (substring, case-insensitive). Nao e regex."
                    },
                    "question": {
                        "type": "string",
                        "description": "Ranking lexical dos blocos. Sem terms, a saida e fatiada em paragrafos e ranqueada por esta pergunta."
                    },
                    "context": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Linhas de contexto em volta de cada casamento (grep -C). Padrao 2, teto 20."
                    },
                    "max_blocks": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de blocos devolvidos. Padrao 6, teto 40."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tempo para este comando. Padrao 120s."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["command"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        struct Chunk {
            start: usize,
            end: usize,
            text: String,
        }

        fn wanted(terms: &[String]) -> Vec<String> {
            terms
                .iter()
                .map(|term| term.trim().to_lowercase())
                .filter(|term| !term.is_empty())
                .collect()
        }

        fn hit(line: &str, terms: &[String]) -> bool {
            let lower = line.to_lowercase();
            terms.iter().any(|term| lower.contains(term.as_str()))
        }

        fn count_matches(text: &str, terms: &[String]) -> usize {
            let terms = wanted(terms);
            if terms.is_empty() {
                return 0;
            }
            text.lines().filter(|line| hit(line, &terms)).count()
        }

        fn chunk_lines(text: &str, terms: &[String], context: usize) -> Vec<Chunk> {
            let terms = wanted(terms);
            if terms.is_empty() {
                return Vec::new();
            }
            let lines: Vec<&str> = text.lines().collect();
            if lines.is_empty() {
                return Vec::new();
            }
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if !hit(line, &terms) {
                    continue;
                }
                let start = index.saturating_sub(context);
                let end = (index + context).min(lines.len() - 1);
                match ranges.last_mut() {
                    Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
                    _ => ranges.push((start, end)),
                }
            }
            ranges
                .into_iter()
                .map(|(start, end)| Chunk {
                    start: start + 1,
                    end: end + 1,
                    text: lines[start..=end].join("\n"),
                })
                .collect()
        }

        fn flush_group(
            lines: &[&str],
            group: &mut Vec<usize>,
            max_lines: usize,
            out: &mut Vec<Chunk>,
        ) {
            for piece in group.chunks(max_lines) {
                if let (Some(first), Some(last)) = (piece.first(), piece.last()) {
                    out.push(Chunk {
                        start: first + 1,
                        end: last + 1,
                        text: lines[*first..=*last].join("\n"),
                    });
                }
            }
            group.clear();
        }

        fn paragraph_chunks(text: &str, max_lines: usize) -> Vec<Chunk> {
            let lines: Vec<&str> = text.lines().collect();
            let max_lines = max_lines.max(1);
            let mut out = Vec::new();
            let mut group: Vec<usize> = Vec::new();
            for (index, line) in lines.iter().enumerate() {
                if line.trim().is_empty() {
                    flush_group(&lines, &mut group, max_lines, &mut out);
                } else {
                    group.push(index);
                }
            }
            flush_group(&lines, &mut group, max_lines, &mut out);
            out
        }

        /// Binarios liberados quando o projeto nao declara `shell.allow`.
        const DEFAULT_ALLOW: &[&str] = &[
            "git", "rg", "grep", "ls", "cat", "head", "tail", "wc", "find", "jq", "sed", "awk",
            "sort", "uniq", "diff", "file", "stat", "du", "df", "tree", "echo", "pwd", "date",
            "cargo",
        ];

        let params: ShellParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;

        let project_allow = || -> Option<Vec<String>> {
            #[derive(serde::Deserialize)]
            struct Root {
                #[serde(default)]
                shell: Option<ShellBlock>,
            }

            #[derive(serde::Deserialize)]
            struct ShellBlock {
                #[serde(default)]
                allow: Option<Vec<String>>,
            }

            let text = std::fs::read_to_string(self.root_dir.join("ai.project.json")).ok()?;
            let root: Root = serde_json::from_str(&text).ok()?;
            root.shell?.allow
        };

        let allow: Vec<String> = project_allow()
            .unwrap_or_else(|| DEFAULT_ALLOW.iter().map(|name| name.to_string()).collect());
        if !allowed(&params.command, &allow) {
            return Err(ToolError::InvalidArguments(format!(
                "comando recusado: {}. Libere o binario na chave shell.allow de ai.project.json; composicao nao e permitida",
                params.command.trim()
            )));
        }

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let timeout = Duration::from_secs(
            params
                .timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        );
        let raw = proc::bash(params.command.trim(), &root, timeout)?;
        let status = if raw.timed_out {
            "timeout"
        } else if raw.success {
            "ok"
        } else {
            "falhou"
        };

        let question = params
            .question
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let context = params.context.unwrap_or(2).min(20);
        let max_blocks = params.max_blocks.unwrap_or(6).clamp(1, 40);
        let filtered = !params.terms.is_empty() || question.is_some();

        let mut candidates: Vec<crate::support::web::Block> = Vec::new();
        let mut matched = 0usize;
        let mut total = 0usize;
        for (stream, text) in [
            ("stdout", raw.stdout.as_str()),
            ("stderr", raw.stderr.as_str()),
        ] {
            total += text.lines().count();
            let chunks = if !params.terms.is_empty() {
                matched += count_matches(text, &params.terms);
                chunk_lines(text, &params.terms, context)
            } else if question.is_some() {
                paragraph_chunks(text, 40)
            } else {
                Vec::new()
            };
            for chunk in chunks {
                candidates.push(crate::support::web::Block {
                    anchor: format!("{stream} {}-{}", chunk.start, chunk.end),
                    text: chunk.text,
                });
            }
        }
        let ranked: Vec<crate::support::web::Block> =
            crate::support::web::rank(candidates, question)
                .into_iter()
                .take(max_blocks)
                .collect();

        let combined = format!("{}{}", raw.stdout, raw.stderr);
        let mut summary = format!(
            "\n- comando: {}\n- status: {status} · {:.1}s\n",
            params.command.trim(),
            raw.duration.as_secs_f64()
        );
        if !params.terms.is_empty() {
            summary.push_str(&format!(
                "- linhas: {matched} de {total} · {} blocos\n",
                ranked.len()
            ));
        } else if question.is_some() {
            summary.push_str(&format!("- blocos: {}\n", ranked.len()));
        } else {
            summary.push_str(&format!(
                "\n~~~text\n{}\n~~~\n",
                if combined.trim().is_empty() {
                    "(sem saida)".to_string()
                } else {
                    crate::exec::clip(combined.trim_end(), 4_000, 2_000)
                }
            ));
        }

        let mut sections = vec![Section {
            heading: format!("### shell — {status}"),
            body: summary,
            priority: 255,
        }];

        if !params.terms.is_empty() && matched == 0 {
            let lines: Vec<&str> = combined.lines().collect();
            let body = if lines.is_empty() {
                "(sem saida)".to_string()
            } else {
                let head = lines.iter().take(5).copied().collect::<Vec<_>>().join("\n");
                let tail = lines[lines.len().saturating_sub(5)..].join("\n");
                format!("\n~~~text\n{head}\n...\n{tail}\n~~~\n")
            };
            sections.push(Section {
                heading: "#### 0 correspondencias".to_string(),
                body,
                priority: 240,
            });
        }

        for block in &ranked {
            sections.push(Section {
                heading: format!("#### {}", block.anchor),
                body: format!(
                    "\n~~~text\n{}\n~~~\n",
                    crate::exec::clip(&block.text, 4_000, 1_000)
                ),
                priority: 200,
            });
        }

        if !filtered {
            sections.push(Section {
                heading: "#### saida completa".to_string(),
                body: format!(
                    "\n~~~text\n{}\n~~~\n",
                    crate::exec::clip(combined.trim_end(), 20_000, 20_000)
                ),
                priority: 40,
            });
        }

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("shell");
        let terms: Vec<&str> = arguments
            .get("terms")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if terms.is_empty() {
            format!("shell: {command}")
        } else {
            format!("shell: {command} · termos: {}", terms.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn allow(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn tool_for(dir: &Path) -> ShellTool {
        ShellTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(Duration::from_secs(60), 1024)),
            4000,
        )
    }

    #[test]
    fn cargo_fmt_is_allowed() {
        assert!(allowed("cargo fmt", &[]));
        assert!(allowed("cargo fmt -- --check", &[]));
        assert!(allowed("cargo fmt --check", &[]));
    }

    #[test]
    fn rustfmt_without_a_file_is_allowed() {
        assert!(allowed("rustfmt --version", &[]));
        assert!(allowed("rustfmt --check", &[]));
    }

    #[test]
    fn a_shell_metacharacter_is_refused() {
        assert!(!allowed("git status; rm -rf /", &allow(&["git"])));
        assert!(!allowed("git status && touch x", &allow(&["git"])));
        assert!(!allowed("git status | tee x", &allow(&["git"])));
        assert!(!allowed("git status $(id)", &allow(&["git"])));
    }

    #[test]
    fn anything_outside_the_formatter_allowlist_is_refused() {
        assert!(!allowed("cargo clean", &[]));
        assert!(!allowed("git status", &[]));
        assert!(!allowed("rustfmt src/main.rs", &[]));
        assert!(!allowed("", &[]));
        assert!(!allowed("cargo fmt --manifest-path /etc/x/Cargo.toml", &[]));
    }

    #[test]
    fn an_absolute_path_is_checked_by_its_basename() {
        assert!(allowed("/usr/bin/git status", &allow(&["git"])));
        assert!(!allowed("/usr/bin/rm -rf /", &allow(&["git"])));
    }

    #[test]
    fn a_wrapper_is_refused_even_when_configured() {
        for wrapper in ["env", "sh", "bash", "xargs", "sudo", "python3", "make"] {
            assert!(
                !allowed(&format!("{wrapper} --version"), &allow(&[wrapper])),
                "{wrapper} deveria ser recusado"
            );
        }
    }

    #[test]
    fn formatters_stay_allowed_when_the_allowlist_is_replaced() {
        assert!(allowed("cargo fmt --check", &allow(&["rg"])));
        assert!(!allowed("git status", &allow(&["rg"])));
    }

    #[test]
    fn a_binary_from_the_project_file_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ai.project.json"),
            r#"{"name":"x","shell":{"allow":["uname"]}}"#,
        )
        .unwrap();
        let out = tool_for(dir.path())
            .call(serde_json::json!({"command": "uname -s"}))
            .unwrap();
        assert!(out.contains("ok"), "{out}");
    }

    #[test]
    fn a_binary_outside_the_default_allowlist_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let error = tool_for(dir.path())
            .call(serde_json::json!({"command": "uname -s"}))
            .unwrap_err();
        assert!(error.to_string().contains("comando recusado"), "{error}");
    }

    #[test]
    fn describe_shows_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for(dir.path());
        assert_eq!(
            tool.describe_call(&serde_json::json!({"command": "cargo fmt"})),
            "shell: cargo fmt"
        );
    }

    fn log_tool(dir: &Path) -> ShellTool {
        std::fs::write(
            dir.join("ai.project.json"),
            r#"{"name":"x","shell":{"allow":["cat"]}}"#,
        )
        .unwrap();
        tool_for(dir)
    }

    fn log_file(dir: &Path, text: &str) -> String {
        let path = dir.join("log.txt");
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    #[test]
    fn terms_return_only_matching_blocks_with_context() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na2\na3 FAILED\na4\na5");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cat {log}"),
                "terms": ["FAILED"],
                "context": 1
            }))
            .unwrap();
        assert!(out.contains("3 FAILED"), "{out}");
        assert!(out.contains("a2"), "{out}");
        assert!(out.contains("a4"), "{out}");
        assert!(!out.contains("a1"), "{out}");
        assert!(out.contains("1 de 5"), "{out}");
    }

    #[test]
    fn no_match_returns_head_and_tail_and_the_counts() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na2\na3\na4\na5");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cat {log}"),
                "terms": ["NAOEXISTE"]
            }))
            .unwrap();
        assert!(out.contains("0 de 5"), "{out}");
        assert!(out.contains("a1"), "{out}");
        assert!(out.contains("a5"), "{out}");
    }

    #[test]
    fn no_filter_keeps_the_previous_summary() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na3 FAILED");
        let out = tool
            .call(serde_json::json!({"command": format!("cat {log}")}))
            .unwrap();
        assert!(out.contains("3 FAILED"), "{out}");
        assert!(!out.contains("linhas:"), "{out}");
    }

    #[test]
    fn a_question_ranks_the_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "ruido sem sentido\n\no token e validado aqui");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cat {log}"),
                "question": "onde o token e validado"
            }))
            .unwrap();
        assert!(
            out.find("token").unwrap() < out.find("ruido").unwrap(),
            "{out}"
        );
    }

    #[test]
    fn describe_shows_the_command_and_the_terms() {
        let dir = tempfile::tempdir().unwrap();
        let tool = tool_for(dir.path());
        assert_eq!(
            tool.describe_call(&serde_json::json!({"command": "git status", "terms": ["shell"]})),
            "shell: git status · termos: shell"
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({"command": "git status"})),
            "shell: git status"
        );
    }

    #[test]
    fn an_absurd_context_is_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let log = log_file(dir.path(), "a1\na2\na3 FAILED\na4\na5");
        let out = tool
            .call(serde_json::json!({
                "command": format!("cat {log}"),
                "terms": ["FAILED"],
                "context": 100000
            }))
            .unwrap();
        assert!(out.contains("a1"), "{out}");
        assert!(out.contains("a5"), "{out}");
    }

    #[test]
    fn a_huge_single_line_is_clipped() {
        let dir = tempfile::tempdir().unwrap();
        let tool = log_tool(dir.path());
        let huge = "X".repeat(200_000);
        let log = log_file(dir.path(), &format!("alvo {huge}"));
        let out = tool
            .call(serde_json::json!({"command": format!("cat {log}"), "terms": ["alvo"]}))
            .unwrap();
        assert!(out.contains("alvo"), "deveria manter o casamento");
        assert!(out.len() < 100_000, "saida sem teto: {}", out.len());
    }

    #[test]
    fn git_config_override_is_refused() {
        assert!(allowed("git status", &allow(&["git"])));
        assert!(!allowed("git -c alias.pwn=!id pwn", &allow(&["git"])));
        assert!(!allowed("git --config-env=x=Y status", &allow(&["git"])));
    }
}
