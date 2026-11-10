use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::{self, Section};
use crate::proc::{self, Output};
use crate::store::ResponseStore;

const DEFAULT_TIMEOUT_SECS: u64 = 900;
const MAX_TIMEOUT_SECS: u64 = 3600;
const MAX_ERRORS_SHOWN: usize = 6;
const MAX_WARNINGS_SHOWN: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecKind {
    Build,
    Check,
    Test,
    Clippy,
    Xtask,
}

#[derive(Debug, Deserialize)]
pub struct ExecParams {
    pub kind: ExecKind,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub exact: bool,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub refresh: bool,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

impl ExecParams {
    pub fn test(
        package: Option<String>,
        target: Option<String>,
        filter: Option<String>,
        exact: bool,
    ) -> Self {
        Self {
            kind: ExecKind::Test,
            package,
            target,
            filter,
            exact,
            task: None,
            refresh: false,
            timeout_secs: None,
            budget_tokens: None,
        }
    }
}

#[derive(Clone)]
struct Cached {
    key: String,
    output: Output,
}

pub struct ExecTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
    cache: Mutex<Option<Cached>>,
}

impl ExecTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
            cache: Mutex::new(None),
        }
    }
}

pub(crate) fn build_args(params: &ExecParams) -> Result<Vec<String>, ToolError> {
    let mut args: Vec<String> = Vec::new();

    if params.kind == ExecKind::Xtask {
        let task = params
            .task
            .as_deref()
            .map(str::trim)
            .filter(|task| !task.is_empty())
            .ok_or_else(|| {
                ToolError::InvalidArguments("task e obrigatorio quando kind e xtask".to_string())
            })?;
        args.push("xtask".to_string());
        args.push(task.to_string());
        return Ok(args);
    }
    if params.task.is_some() {
        return Err(ToolError::InvalidArguments(
            "task so vale para kind xtask".to_string(),
        ));
    }

    args.push(
        match params.kind {
            ExecKind::Build => "build",
            ExecKind::Check => "check",
            ExecKind::Test => "test",
            ExecKind::Clippy => "clippy",
            ExecKind::Xtask => unreachable!(),
        }
        .to_string(),
    );

    match params
        .package
        .as_deref()
        .map(str::trim)
        .filter(|package| !package.is_empty())
    {
        Some(package) => {
            args.push("-p".to_string());
            args.push(package.to_string());
        }
        None => args.push("--workspace".to_string()),
    }

    if let Some(target) = params
        .target
        .as_deref()
        .map(str::trim)
        .filter(|target| !target.is_empty())
    {
        match target {
            "lib" => args.push("--lib".to_string()),
            "bins" => args.push("--bins".to_string()),
            "tests" => args.push("--tests".to_string()),
            "all-targets" => args.push("--all-targets".to_string()),
            "doc" => {
                if params.kind != ExecKind::Test {
                    return Err(ToolError::InvalidArguments(
                        "target doc so vale para kind test".to_string(),
                    ));
                }
                args.push("--doc".to_string());
            }
            name => {
                if params.kind != ExecKind::Test {
                    return Err(ToolError::InvalidArguments(format!(
                        "target {name} so vale para kind test; use lib, bins, tests ou all-targets"
                    )));
                }
                args.push("--test".to_string());
                args.push(name.to_string());
            }
        }
    }

    match params
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|filter| !filter.is_empty())
    {
        Some(filter) => {
            if params.kind != ExecKind::Test {
                return Err(ToolError::InvalidArguments(
                    "filter so vale para kind test".to_string(),
                ));
            }
            args.push("--".to_string());
            args.push(filter.to_string());
            if params.exact {
                args.push("--exact".to_string());
            }
        }
        None if params.exact => {
            return Err(ToolError::InvalidArguments(
                "exact exige filter".to_string(),
            ));
        }
        None => {}
    }

    Ok(args)
}

pub(crate) fn command_line(args: &[String]) -> String {
    format!("cargo {}", args.join(" "))
}

pub(crate) fn execute(
    root: &Path,
    params: &ExecParams,
    timeout: Duration,
) -> Result<Output, ToolError> {
    let args = build_args(params)?;
    proc::cargo(&args, root, timeout)
}

pub(crate) fn timeout_for(params: &ExecParams) -> Duration {
    Duration::from_secs(
        params
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS),
    )
}

struct Diagnostics {
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn diagnostics(stderr: &str) -> Diagnostics {
    let lines: Vec<&str> = stderr.lines().collect();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        let is_error = line.starts_with("error");
        let is_warning = line.starts_with("warning");
        if !is_error && !is_warning {
            index += 1;
            continue;
        }

        let mut block = String::new();
        while index < lines.len() {
            let current = lines[index];
            if !block.is_empty() && (current.starts_with("error") || current.starts_with("warning"))
            {
                break;
            }
            block.push_str(current);
            block.push('\n');
            index += 1;
            if block.len() > 4_000 || current.trim().is_empty() {
                break;
            }
        }

        if is_error {
            errors.push(block.trim_end().to_string());
        } else {
            warnings.push(block.trim_end().to_string());
        }
    }

    Diagnostics { errors, warnings }
}

fn test_lines(stdout: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("test result:")
            || trimmed == "failures:"
            || (trimmed.starts_with("---- ") && trimmed.ends_with(" stdout ----"))
        {
            lines.push(trimmed.to_string());
            if lines.len() >= 12 {
                break;
            }
        }
    }
    lines
}

pub(crate) fn clip(text: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= head + tail {
        return text.to_string();
    }
    let mut out: String = chars[..head].iter().collect();
    out.push_str(&format!(
        "\n... ({} caracteres omitidos)\n",
        chars.len() - head - tail
    ));
    out.extend(&chars[chars.len() - tail..]);
    out
}

pub(crate) fn sections_for(args: &[String], raw: &Output, cached: bool) -> Vec<Section> {
    let command = command_line(args);
    let status = if raw.timed_out {
        "timeout"
    } else if raw.success {
        "ok"
    } else {
        "falhou"
    };
    let diag = diagnostics(&raw.stderr);
    let tests = test_lines(&raw.stdout);

    let mut summary = String::new();
    summary.push('\n');
    summary.push_str(&format!("- comando: {command}\n"));
    summary.push_str(&format!(
        "- status: {status} · {:.1}s{}\n",
        raw.duration.as_secs_f64(),
        if cached { " · em cache" } else { "" }
    ));
    if !diag.errors.is_empty() || !diag.warnings.is_empty() {
        summary.push_str(&format!(
            "- erros: {} · avisos: {}\n",
            diag.errors.len(),
            diag.warnings.len()
        ));
    }
    if !tests.is_empty() {
        summary.push_str("- testes:\n");
        for line in &tests {
            summary.push_str(&format!("  - {line}\n"));
        }
    }

    for block in diag.errors.iter().take(MAX_ERRORS_SHOWN) {
        summary.push_str("\n~~~text\n");
        summary.push_str(&clip(block, 1_600, 200));
        summary.push_str("\n~~~\n");
    }
    if diag.errors.len() > MAX_ERRORS_SHOWN {
        summary.push_str(&format!(
            "\n(+{} erros na saida completa)\n",
            diag.errors.len() - MAX_ERRORS_SHOWN
        ));
    }
    for block in diag.warnings.iter().take(MAX_WARNINGS_SHOWN) {
        summary.push_str("\n~~~text\n");
        summary.push_str(&clip(block, 1_000, 0));
        summary.push_str("\n~~~\n");
    }

    let mut sections = vec![Section {
        heading: format!("### {command} — {status}"),
        body: summary,
        priority: 255,
    }];

    let combined = format!(
        "{}\n{}",
        clip(raw.stderr.trim_end(), 8_000, 2_000),
        clip(raw.stdout.trim_end(), 8_000, 8_000)
    );
    let needs_detail = diag.errors.len() > MAX_ERRORS_SHOWN
        || diag.warnings.len() > MAX_WARNINGS_SHOWN
        || raw.stdout.contains("failures:")
        || (!raw.success && diag.errors.is_empty() && !combined.trim().is_empty());

    if needs_detail {
        sections.push(Section {
            heading: "#### saida completa".to_string(),
            body: format!("\n~~~text\n{}\n~~~\n", combined.trim()),
            priority: 80,
        });
    }

    sections
}

pub(crate) fn finish(
    store: &ResponseStore,
    sections: Vec<Section>,
    budget_tokens: usize,
) -> String {
    let selection = budget::select(&sections, budget_tokens);
    let handle = if selection.omitted.is_empty() {
        None
    } else {
        Some(store.insert(selection.omitted_body()))
    };
    budget::render(&selection, handle.as_deref())
}

fn fingerprint(root: &Path) -> String {
    use ignore::WalkBuilder;

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(true)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != "target");

    let mut entries: Vec<(PathBuf, u64, u64)> = Vec::new();
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if !matches!(extension, "rs" | "toml" | "lock") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        entries.push((path.to_path_buf(), metadata.len(), modified));
        if entries.len() >= 5_000 {
            break;
        }
    }
    entries.sort();

    let mut hasher = blake3::Hasher::new();
    for (path, length, modified) in entries {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&length.to_le_bytes());
        hasher.update(&modified.to_le_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

impl Tool for ExecTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "exec".to_string(),
            description: "Roda cargo check/build/test/clippy/xtask com erros agrupados e cache. Restrinja com package, target e filter para nao compilar o workspace inteiro.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["build", "check", "test", "clippy", "xtask"],
                        "description": "check e o mais rapido para erros de tipo; build liga artefatos; test roda a suite; clippy adiciona lints; xtask roda tarefas do workspace."
                    },
                    "package": {
                        "type": "string",
                        "description": "Restringe ao pacote (cargo -p). Sem ele, o comando vale para o workspace."
                    },
                    "target": {
                        "type": "string",
                        "description": "lib, bins, tests, all-targets, doc (so test) ou o nome de um teste de integracao (so test)."
                    },
                    "filter": {
                        "type": "string",
                        "description": "Filtro de nome de teste (substring). So para kind test."
                    },
                    "exact": {
                        "type": "boolean",
                        "default": false,
                        "description": "Exige casamento exato do filter. So para kind test e exige filter."
                    },
                    "task": {
                        "type": "string",
                        "description": "Comando do xtask. Obrigatorio quando kind e xtask."
                    },
                    "refresh": {
                        "type": "boolean",
                        "default": false,
                        "description": "Ignora o cache e reexecuta o cargo mesmo que nada tenha mudado."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tempo do processo. Padrao 900s."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["kind"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: ExecParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let args = build_args(&params)?;
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let key = format!("{}|{}", args.join(" "), fingerprint(&root));

        let cached = if params.refresh {
            None
        } else {
            self.cache
                .lock()
                .unwrap()
                .clone()
                .filter(|entry| entry.key == key)
                .map(|entry| entry.output)
        };

        let (output, from_cache) = match cached {
            Some(output) => (output, true),
            None => {
                let output = execute(&root, &params, timeout_for(&params))?;
                *self.cache.lock().unwrap() = Some(Cached {
                    key,
                    output: output.clone(),
                });
                (output, false)
            }
        };

        let sections = sections_for(&args, &output, from_cache);
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let Ok(params) = serde_json::from_value::<ExecParams>(arguments.clone()) else {
            return "exec".to_string();
        };
        match build_args(&params) {
            Ok(args) => command_line(&args),
            Err(_) => "exec".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(kind: ExecKind) -> ExecParams {
        ExecParams {
            kind,
            package: None,
            target: None,
            filter: None,
            exact: false,
            task: None,
            refresh: false,
            timeout_secs: None,
            budget_tokens: None,
        }
    }

    #[test]
    fn a_check_is_workspace_wide_by_default() {
        assert_eq!(
            build_args(&params(ExecKind::Check)).unwrap(),
            vec!["check", "--workspace"]
        );
    }

    #[test]
    fn a_package_replaces_the_workspace_flag() {
        let mut params = params(ExecKind::Check);
        params.package = Some("tupi-tools".to_string());
        assert_eq!(
            build_args(&params).unwrap(),
            vec!["check", "-p", "tupi-tools"]
        );
    }

    #[test]
    fn a_test_filter_goes_after_the_separator() {
        let mut params = params(ExecKind::Test);
        params.filter = Some("guard".to_string());
        params.exact = true;
        assert_eq!(
            build_args(&params).unwrap(),
            vec!["test", "--workspace", "--", "guard", "--exact"]
        );
    }

    #[test]
    fn exact_without_filter_is_rejected() {
        let mut params = params(ExecKind::Test);
        params.exact = true;
        assert!(build_args(&params).is_err());
    }

    #[test]
    fn a_target_name_is_only_valid_for_tests() {
        let mut params = params(ExecKind::Check);
        params.target = Some("auth".to_string());
        assert!(build_args(&params).is_err());
    }

    #[test]
    fn xtask_requires_a_task() {
        assert!(build_args(&params(ExecKind::Xtask)).is_err());
        let mut params = params(ExecKind::Xtask);
        params.task = Some("docs".to_string());
        assert_eq!(build_args(&params).unwrap(), vec!["xtask", "docs"]);
    }

    #[test]
    fn diagnostics_are_grouped() {
        let stderr = "error[E0308]: mismatched types\n --> src/a.rs:1:1\n  |\n1 | x\n  | ^ expected u8\n\nerror: could not compile x\n\nwarning: unused variable: y\n --> src/a.rs:2:1\n";
        let diag = diagnostics(stderr);
        assert_eq!(diag.errors.len(), 2);
        assert_eq!(diag.warnings.len(), 1);
        assert!(diag.errors[0].contains("mismatched types"));
    }

    #[test]
    fn a_successful_output_has_no_detail_section() {
        let raw = Output {
            stdout: String::new(),
            stderr: String::new(),
            success: true,
            timed_out: false,
            duration: Duration::from_millis(10),
        };
        let args = build_args(&params(ExecKind::Check)).unwrap();
        let sections = sections_for(&args, &raw, false);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].body.contains("ok"));
    }

    #[test]
    fn a_failure_carries_the_detail() {
        let raw = Output {
            stdout: String::new(),
            stderr: "error[E0308]: mismatched types\n".to_string(),
            success: false,
            timed_out: false,
            duration: Duration::from_millis(10),
        };
        let args = build_args(&params(ExecKind::Check)).unwrap();
        let sections = sections_for(&args, &raw, false);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].body.contains("mismatched types"));
    }

    #[test]
    fn describe_shows_the_cargo_command() {
        let tool = ExecTool::new(
            PathBuf::from("."),
            Arc::new(ResponseStore::new(Duration::from_secs(60), 1024)),
            4000,
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({"kind": "test", "filter": "x"})),
            "cargo test --workspace -- x"
        );
    }
}
