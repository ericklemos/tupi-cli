use std::io::Write;
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

const MAX_PATCH_BYTES: usize = 1_000_000;
const TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Deserialize)]
pub struct PatchParams {
    pub patch: String,
    #[serde(default)]
    pub check_only: bool,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct PatchTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl PatchTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn outside(path: &str) -> bool {
    path.starts_with('/') || path.split('/').any(|component| component == "..")
}

fn validate_paths(root: &std::path::Path, patch: &str) -> Result<(), ToolError> {
    for line in patch.lines() {
        if line.starts_with("new file mode 120000")
            || line.starts_with("old mode 120000")
            || line.starts_with("new mode 120000")
            || (line.starts_with("index ") && line.ends_with(" 120000"))
        {
            return Err(ToolError::InvalidArguments(
                "patches de symlink (120000) nao sao permitidos".to_string(),
            ));
        }

        let headers = [
            ("--- ", Some("a/")),
            ("+++ ", Some("b/")),
            ("rename from ", None),
            ("rename to ", None),
            ("copy from ", None),
            ("copy to ", None),
        ];
        for (header, prefix) in headers {
            if let Some(rest) = line.strip_prefix(header) {
                let raw = rest
                    .split('\t')
                    .next()
                    .unwrap_or(rest)
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if raw.starts_with('"') || raw.ends_with('"') || raw.contains('\\') {
                    return Err(ToolError::InvalidArguments(format!(
                        "caminhos entre aspas ou escapados no patch nao sao permitidos: {raw}"
                    )));
                }
                if raw == "/dev/null" {
                    continue;
                }
                if outside(raw) {
                    return Err(ToolError::OutsideRoot(raw.to_string()));
                }
                let relative = match prefix {
                    Some(p) => raw.strip_prefix(p).unwrap_or(raw),
                    None => raw,
                };
                if outside(relative) {
                    return Err(ToolError::OutsideRoot(relative.to_string()));
                }
                let resolved = crate::paths::resolve_for_write(root, relative)?;
                if let Ok(meta) = std::fs::symlink_metadata(&resolved)
                    && meta.file_type().is_symlink()
                {
                    return Err(ToolError::OutsideRoot(relative.to_string()));
                }
            }
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            for token in rest.split_whitespace().take(2) {
                if token.starts_with('"') || token.ends_with('"') || token.contains('\\') {
                    return Err(ToolError::InvalidArguments(format!(
                        "caminhos entre aspas ou escapados no patch nao sao permitidos: {token}"
                    )));
                }
                if outside(token) {
                    return Err(ToolError::OutsideRoot(token.to_string()));
                }
                let stripped = token
                    .strip_prefix("a/")
                    .or_else(|| token.strip_prefix("b/"))
                    .unwrap_or(token);
                if outside(stripped) {
                    return Err(ToolError::OutsideRoot(stripped.to_string()));
                }
                let resolved = crate::paths::resolve_for_write(root, stripped)?;
                if let Ok(meta) = std::fs::symlink_metadata(&resolved)
                    && meta.file_type().is_symlink()
                {
                    return Err(ToolError::OutsideRoot(stripped.to_string()));
                }
            }
        }
    }
    Ok(())
}

fn failed_output(output: &proc::Output) -> String {
    let text = if output.stderr.trim().is_empty() {
        output.stdout.trim()
    } else {
        output.stderr.trim()
    };
    if text.is_empty() {
        "(sem saida)".to_string()
    } else {
        crate::exec::clip(text, 1_200, 400)
    }
}

fn touched_files(patch: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let path = rest
                .split('\t')
                .next()
                .unwrap_or(rest)
                .split_whitespace()
                .next()
                .unwrap_or("");
            let path = path.strip_prefix("b/").unwrap_or(path);
            if path != "/dev/null" && !files.iter().any(|existing| existing == path) {
                files.push(path.to_string());
            }
        }
    }
    files
}

fn stats(patch: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in patch.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    (added, removed)
}

impl Tool for PatchTool {
    fn effect(&self, arguments: &Value) -> tupi_kernel::Effect {
        crate::support::effect::writes(arguments)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "patch".to_string(),
            description: "Aplica diff unificado sob o root; valida com git apply --check e recusa caminho fora do root.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "patch": {
                        "type": "string",
                        "description": "Diff unificado completo, no formato git diff (diff --git, ---, +++, hunks)."
                    },
                    "check_only": {
                        "type": "boolean",
                        "default": false,
                        "description": "So verifica se o patch aplica; nao escreve."
                    },
                    "reverse": {
                        "type": "boolean",
                        "default": false,
                        "description": "Aplica o patch ao contrario (git apply -R), util para desfazer."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["patch"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: PatchParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        if params.patch.trim().is_empty() {
            return Err(ToolError::InvalidArguments("patch vazio".to_string()));
        }
        if params.patch.len() > MAX_PATCH_BYTES {
            return Err(ToolError::InvalidArguments(format!(
                "patch maior que {MAX_PATCH_BYTES} bytes"
            )));
        }
        if !params.patch.contains("\n@@") && !params.patch.starts_with("@@") {
            return Err(ToolError::InvalidArguments("patch sem hunk @@".to_string()));
        }

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        validate_paths(&root, &params.patch)?;

        let mut temp = tempfile::NamedTempFile::new().map_err(|error| {
            ToolError::Other(format!("falha ao criar arquivo temporario: {error}"))
        })?;
        temp.write_all(params.patch.as_bytes()).map_err(|error| {
            ToolError::Other(format!("falha ao gravar o patch temporario: {error}"))
        })?;
        let patch_path = temp.path().to_string_lossy().to_string();
        let timeout = Duration::from_secs(TIMEOUT_SECS);

        let mut check = vec!["apply".to_string(), "--check".to_string()];
        if params.reverse {
            check.push("-R".to_string());
        }
        check.push(patch_path.clone());
        let checked = proc::program("git", &check, &root, timeout)?;
        if !checked.success {
            return Err(ToolError::InvalidArguments(format!(
                "patch nao aplica sobre o estado atual: {}",
                failed_output(&checked)
            )));
        }

        let files = touched_files(&params.patch);
        let (mut added, mut removed) = stats(&params.patch);
        if params.reverse {
            std::mem::swap(&mut added, &mut removed);
        }
        let mode = if params.check_only {
            "aplicavel"
        } else {
            "aplicado"
        };

        if !params.check_only {
            let mut apply = vec!["apply".to_string()];
            if params.reverse {
                apply.push("-R".to_string());
            }
            apply.push(patch_path);
            let applied = proc::program("git", &apply, &root, timeout)?;
            if !applied.success {
                return Err(ToolError::Other(format!(
                    "falha ao aplicar o patch: {}",
                    failed_output(&applied)
                )));
            }
        }

        let mut body = String::new();
        body.push_str(&format!(
            "\n- arquivos: {}\n- linhas: +{added}/-{removed}\n",
            files.len()
        ));
        for file in &files {
            body.push_str(&format!("  - {file}\n"));
        }
        let section = Section {
            heading: format!("### patch — {mode}"),
            body,
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let patch = arguments
            .get("patch")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let files = touched_files(patch);
        let check = arguments
            .get("check_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        format!(
            "patch: {} arquivo(s){}",
            files.len(),
            if check { " (check-only)" } else { "" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn tool(dir: &Path) -> PatchTool {
        PatchTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(Duration::from_secs(60), 1024 * 1024)),
            4000,
        )
    }

    fn diff() -> String {
        [
            "diff --git a/a.txt b/a.txt",
            "--- a/a.txt",
            "+++ b/a.txt",
            "@@ -1 +1 @@",
            "-antes",
            "+depois",
            "",
        ]
        .join("\n")
    }

    #[test]
    fn a_patch_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "antes\n").unwrap();
        Tool::call(&tool(dir.path()), serde_json::json!({"patch": diff()})).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "depois\n"
        );
    }

    #[test]
    fn check_only_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "antes\n").unwrap();
        Tool::call(
            &tool(dir.path()),
            serde_json::json!({"patch": diff(), "check_only": true}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "antes\n"
        );
    }

    #[test]
    fn a_stale_patch_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "outro\n").unwrap();
        assert!(Tool::call(&tool(dir.path()), serde_json::json!({"patch": diff()})).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "outro\n"
        );
    }

    #[test]
    fn a_escaping_path_is_rejected_before_git() {
        let dir = tempfile::tempdir().unwrap();
        let patch = "diff --git a/../fora.txt b/../fora.txt
--- a/../fora.txt
+++ b/../fora.txt
@@ -1 +1 @@
-a
+b
";
        assert!(Tool::call(&tool(dir.path()), serde_json::json!({"patch": patch})).is_err());

        let quoted = "diff --git \"a/../fora.txt\" \"b/../fora.txt\"
--- \"a/../fora.txt\"
+++ \"b/../fora.txt\"
@@ -1 +1 @@
-a
+b
";
        assert!(
            Tool::call(&tool(dir.path()), serde_json::json!({"patch": quoted})).is_err(),
            "quoted escaping path should be rejected"
        );

        let symlink_mode = "diff --git a/link b/link
new file mode 120000
--- /dev/null
+++ b/link
@@ -0,0 +1 @@
+/etc/passwd
";
        assert!(
            Tool::call(
                &tool(dir.path()),
                serde_json::json!({"patch": symlink_mode})
            )
            .is_err(),
            "symlink mode 120000 should be rejected"
        );

        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            let target = outside.path().join("secret.txt");
            std::fs::write(
                &target, "antes
",
            )
            .unwrap();
            std::os::unix::fs::symlink(outside.path(), dir.path().join("escape_dir")).unwrap();
            let via_symlink = "diff --git a/escape_dir/secret.txt b/escape_dir/secret.txt
--- a/escape_dir/secret.txt
+++ b/escape_dir/secret.txt
@@ -1 +1 @@
-antes
+pwned
";
            assert!(
                Tool::call(&tool(dir.path()), serde_json::json!({"patch": via_symlink})).is_err(),
                "patch traversing a symlink outside root must be rejected"
            );
            assert_eq!(
                std::fs::read_to_string(&target).unwrap(),
                "antes
"
            );
        }
    }

    #[test]
    fn a_reverse_patch_undoes_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "depois\n").unwrap();
        Tool::call(
            &tool(dir.path()),
            serde_json::json!({"patch": diff(), "reverse": true}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "antes\n"
        );
    }
}
