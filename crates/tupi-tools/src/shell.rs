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
pub(crate) fn allowed(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.chars().any(|c| {
        matches!(
            c,
            ';' | '&' | '|' | '\u{0060}' | '$' | '>' | '<' | '\n' | '\r'
        )
    }) {
        return false;
    }
    if trimmed.contains("--manifest-path") {
        return false;
    }

    if trimmed == "cargo fmt" || trimmed.starts_with("cargo fmt ") {
        return true;
    }

    if let Some(rest) = trimmed.strip_prefix("rustfmt") {
        let rest = rest.trim();
        return rest.is_empty() || rest.split_whitespace().all(|token| token.starts_with('-'));
    }

    false
}

impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "shell".to_string(),
            description: "Roda bash -lc no root, restrito a formatador (cargo fmt, rustfmt). Nao e sandbox: composicao de comandos e recusada.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Comando executado por bash -lc, com cwd no diretorio raiz. So cargo fmt ou rustfmt."
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
        let params: ShellParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        if !allowed(&params.command) {
            return Err(ToolError::InvalidArguments(format!(
                "comando recusado: {}. So cargo fmt e rustfmt sao permitidos, sem composicao",
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
        let combined = format!("{}{}", raw.stdout, raw.stderr);

        let summary = format!(
            "\n- comando: {}\n- status: {status} · {:.1}s\n\n~~~text\n{}\n~~~\n",
            params.command.trim(),
            raw.duration.as_secs_f64(),
            if combined.trim().is_empty() {
                "(sem saida)".to_string()
            } else {
                crate::exec::clip(combined.trim_end(), 4_000, 2_000)
            }
        );
        let sections = vec![
            Section {
                heading: format!("### shell — {status}"),
                body: summary,
                priority: 255,
            },
            Section {
                heading: "#### saida completa".to_string(),
                body: format!(
                    "\n~~~text\n{}\n~~~\n",
                    crate::exec::clip(combined.trim_end(), 20_000, 20_000)
                ),
                priority: 40,
            },
        ];

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let command = arguments
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("shell");
        format!("shell: {command}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_fmt_is_allowed() {
        assert!(allowed("cargo fmt"));
        assert!(allowed("cargo fmt -- --check"));
        assert!(allowed("cargo fmt --check"));
    }

    #[test]
    fn rustfmt_without_a_file_is_allowed() {
        assert!(allowed("rustfmt --version"));
        assert!(allowed("rustfmt --check"));
    }

    #[test]
    fn a_shell_metacharacter_is_refused() {
        assert!(!allowed("cargo fmt; rm -rf /"));
        assert!(!allowed("cargo fmt && touch x"));
        assert!(!allowed("cargo fmt | tee x"));
        assert!(!allowed("cargo fmt \u{0060}id\u{0060}"));
        assert!(!allowed("cargo fmt $(id)"));
    }

    #[test]
    fn anything_outside_the_formatter_allowlist_is_refused() {
        assert!(!allowed("cargo clean"));
        assert!(!allowed("git status"));
        assert!(!allowed("rustfmt src/main.rs"));
        assert!(!allowed(""));
        assert!(!allowed("cargo fmt --manifest-path /etc/x/Cargo.toml"));
    }

    #[test]
    fn describe_shows_the_command() {
        let tool = ShellTool::new(
            PathBuf::from("."),
            Arc::new(ResponseStore::new(Duration::from_secs(60), 1024)),
            4000,
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({"command": "cargo fmt"})),
            "shell: cargo fmt"
        );
    }
}
