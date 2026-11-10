use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::ast::compact_signature;
use crate::budget::Section;
use crate::exec::finish;
use crate::graph::{CallGraph, Def};
use crate::proc;
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct DiffParams {
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_base() -> String {
    "HEAD".to_string()
}

pub struct DiffTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

struct Change {
    file: String,
    start: usize,
    end: usize,
}

impl DiffTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn parse_diff(diff: &str) -> Vec<Change> {
    let mut file = String::new();
    let mut changes = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let path = rest.split('\t').next().unwrap_or(rest).trim();
            if path != "/dev/null" {
                file = path.strip_prefix("b/").unwrap_or(path).to_string();
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("@@ ") else {
            continue;
        };
        let Some(plus) = rest.split_whitespace().find(|part| part.starts_with('+')) else {
            continue;
        };
        let spec = plus.trim_start_matches('+');
        let (start, count) = match spec.split_once(',') {
            Some((start, count)) => (start.parse::<usize>().ok(), count.parse::<usize>().ok()),
            None => (spec.parse::<usize>().ok(), Some(1)),
        };
        if let (Some(start), Some(count)) = (start, count)
            && count > 0
        {
            changes.push(Change {
                file: file.clone(),
                start,
                end: start + count - 1,
            });
        }
    }
    changes
}

impl Tool for DiffTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "diff".to_string(),
            description:
                "Símbolos alterados desde uma revisão, chamadores externos e testes afetados."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "base": {
                        "type": "string",
                        "default": "HEAD",
                        "description": "Revisão base do git. O padrão compara com a árvore de trabalho."
                    },
                    "path": {
                        "type": "string",
                        "description": "Opcional: restringe a um prefixo de caminho."
                    },
                    "include": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["impact", "tests"] },
                        "description": "Seções desejadas; padrão é ambas."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                }
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: DiffParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let base = params.base.trim();
        if base.is_empty() {
            return Err(ToolError::InvalidArguments("base vazia".to_string()));
        }

        let mut args = vec![
            "diff".to_string(),
            "--unified=0".to_string(),
            "--no-color".to_string(),
            base.to_string(),
            "--".to_string(),
        ];
        if let Some(path) = params
            .path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            args.push(path.to_string());
        }
        let output = proc::program("git", &args, &root, Duration::from_secs(60))?;
        if !output.success {
            return Err(ToolError::Other(format!(
                "git diff falhou: {}",
                crate::exec::clip(output.stderr.trim(), 800, 200)
            )));
        }

        let changes = parse_diff(&output.stdout);
        let mut files: Vec<&str> = changes.iter().map(|change| change.file.as_str()).collect();
        files.sort();
        files.dedup();

        let graph = CallGraph::build(&root);
        let mut changed: Vec<&Def> = Vec::new();
        let mut seen = HashSet::new();
        for change in &changes {
            for def in &graph.defs {
                if def.relative == change.file
                    && def.start_line <= change.end
                    && def.end_line >= change.start
                    && seen.insert((def.qualified.as_str(), def.relative.as_str()))
                {
                    changed.push(def);
                }
            }
        }

        let include_impact = params
            .include
            .as_ref()
            .is_none_or(|items| items.iter().any(|item| item == "impact"));
        let include_tests = params
            .include
            .as_ref()
            .is_none_or(|items| items.iter().any(|item| item == "tests"));

        let changed_names: HashSet<&str> = changed.iter().map(|def| def.name.as_str()).collect();
        let mut callers: Vec<&Def> = Vec::new();
        let mut tests: Vec<&Def> = Vec::new();
        let mut caller_seen = HashSet::new();
        for name in &changed_names {
            for caller in graph.callers(name) {
                if changed_names.contains(caller.name.as_str()) {
                    continue;
                }
                if !caller_seen.insert((caller.qualified.as_str(), caller.relative.as_str())) {
                    continue;
                }
                if caller.is_test {
                    tests.push(caller);
                } else {
                    callers.push(caller);
                }
            }
        }

        let mut summary = format!(
            "\n- arquivos alterados: {}\n- símbolos alterados: {}\n- base: {base}\n",
            files.len(),
            changed.len()
        );
        for def in changed.iter().take(20) {
            summary.push_str(&format!(
                "- {} ({}:{}) · {}\n",
                def.qualified,
                def.relative,
                def.start_line,
                compact_signature(&def.signature)
            ));
        }
        if changed.is_empty() && !files.is_empty() {
            for file in &files {
                summary.push_str(&format!("- {file} (sem símbolo no grafo)\n"));
            }
        }

        let mut sections = vec![Section {
            heading: "### diff".to_string(),
            body: summary,
            priority: 255,
        }];

        if include_impact && !callers.is_empty() {
            let mut body = String::new();
            for def in callers.iter().take(20) {
                body.push_str(&format!(
                    "- {} ({}:{})\n",
                    def.qualified, def.relative, def.start_line
                ));
            }
            sections.push(Section {
                heading: format!("#### chamadores externos ({})", callers.len()),
                body: format!("\n{body}"),
                priority: 240,
            });
        }
        if include_tests && !tests.is_empty() {
            let mut body = String::new();
            for def in tests.iter().take(20) {
                body.push_str(&format!(
                    "- {} ({}:{})\n",
                    def.qualified, def.relative, def.start_line
                ));
            }
            sections.push(Section {
                heading: format!("#### testes afetados ({})", tests.len()),
                body: format!("\n{body}"),
                priority: 230,
            });
        }

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let base = arguments
            .get("base")
            .and_then(Value::as_str)
            .unwrap_or("HEAD");
        let path = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
        format!("diff: {base}..{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn tool(dir: &std::path::Path) -> DiffTool {
        DiffTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        )
    }

    #[test]
    fn a_changed_symbol_and_its_caller_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "pub fn helper() { 1; }\n").unwrap();
        std::fs::write(
            dir.path().join("src/b.rs"),
            "pub fn caller() { helper(); }\n",
        )
        .unwrap();
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
                "init",
            ],
        );
        std::fs::write(dir.path().join("src/a.rs"), "pub fn helper() { 2; }\n").unwrap();

        let out = Tool::call(&tool(dir.path()), serde_json::json!({})).unwrap();
        assert!(out.contains("helper"));
        assert!(out.contains("caller"));
    }

    #[test]
    fn no_change_is_stated() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init"]);
        std::fs::write(dir.path().join("a.rs"), "pub fn a() {}\n").unwrap();
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
                "init",
            ],
        );
        let out = Tool::call(&tool(dir.path()), serde_json::json!({})).unwrap();
        assert!(out.contains("arquivos alterados: 0"));
    }
}
