//! Embute `configs/modes/sdd.ron` no binário em tempo de compilação.
//!
//! O RON é parseado aqui e o resultado vira código Rust com literais; não há
//! leitura do arquivo em runtime nem override por projeto. RON inválido quebra
//! o build apontando o caminho do arquivo.

use std::{env, fmt::Write as _, fs, path::Path};

/// Ações do modo SDD quando o bloco `tool` é omitido.
const DEFAULT_PLANNING_ACTIONS: &[&str] = &[
    "upsert_spec",
    "set_tasks",
    "status",
    "list",
    "activate",
    "close",
    "progress",
    "note",
];

/// Ações do modo Execution quando o bloco `tool` é omitido.
const DEFAULT_EXECUTION_ACTIONS: &[&str] = &["status", "list", "activate", "progress", "execute"];

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct File {
    planning: Planning,
    execution: Execution,
    tool: Tool,
    runner: Runner,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Planning {
    posture: Option<String>,
    tools: Option<Vec<String>>,
    lock_mode_on_open: Option<bool>,
    tool_budget_tokens: Option<usize>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Execution {
    agent_prefix: Option<String>,
    docs_agent_prefix: Option<String>,
    subtask_brief: Option<String>,
    docs_brief: Option<String>,
    handoff_open: Option<String>,
    handoff_done: Option<String>,
    run_report: Option<String>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Tool {
    planning_actions: Option<Vec<String>>,
    execution_actions: Option<Vec<String>>,
    planning_description: Option<String>,
    execution_description: Option<String>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Runner {
    max_parallel: Option<usize>,
    agent_max_steps: Option<usize>,
    agent_timeout_secs: Option<u64>,
    auto_execute: Option<bool>,
    docs_agent: Option<bool>,
    mirror_markdown: Option<bool>,
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let path = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("configs")
        .join("modes")
        .join("sdd.ron");
    println!("cargo:rerun-if-changed={}", path.display());

    let raw = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "falha ao ler a config do modo SDD em {}: {error}",
            path.display()
        )
    });
    let file: File = ron::from_str(&raw).unwrap_or_else(|error| {
        panic!(
            "config do modo SDD inválida em {}: {error}",
            path.display()
        )
    });

    let mut code = String::from("// Gerado por build.rs a partir de configs/modes/sdd.ron.\n");
    code.push_str("pub(crate) fn embedded() -> SddConfig {\n    SddConfig {\n");
    code.push_str("        planning: SddPlanning {\n");
    push_str(&mut code, "posture", &file.planning.posture);
    push_list(&mut code, "tools", &file.planning.tools);
    push_line(
        &mut code,
        &format!(
            "lock_mode_on_open: {},",
            file.planning.lock_mode_on_open.unwrap_or(true)
        ),
    );
    push_line(
        &mut code,
        &match file.planning.tool_budget_tokens {
            Some(tokens) => format!("tool_budget_tokens: Some({tokens}),"),
            None => "tool_budget_tokens: None,".to_string(),
        },
    );
    code.push_str("        },\n");

    code.push_str("        execution: SddExecution {\n");
    push_str(&mut code, "agent_prefix", &file.execution.agent_prefix);
    push_str(
        &mut code,
        "docs_agent_prefix",
        &file.execution.docs_agent_prefix,
    );
    push_str(&mut code, "subtask_brief", &file.execution.subtask_brief);
    push_str(&mut code, "docs_brief", &file.execution.docs_brief);
    push_str(&mut code, "handoff_open", &file.execution.handoff_open);
    push_str(&mut code, "handoff_done", &file.execution.handoff_done);
    push_str(&mut code, "run_report", &file.execution.run_report);
    code.push_str("        },\n");

    code.push_str("        tool: SddToolConfig {\n");
    push_list_or(
        &mut code,
        "planning_actions",
        &file.tool.planning_actions,
        DEFAULT_PLANNING_ACTIONS,
    );
    push_list_or(
        &mut code,
        "execution_actions",
        &file.tool.execution_actions,
        DEFAULT_EXECUTION_ACTIONS,
    );
    push_str(
        &mut code,
        "planning_description",
        &file.tool.planning_description,
    );
    push_str(
        &mut code,
        "execution_description",
        &file.tool.execution_description,
    );
    code.push_str("        },\n");

    let runner = &file.runner;
    code.push_str("        runner: SddRunner {\n");
    push_line(
        &mut code,
        &format!(
            "max_parallel: {},",
            runner.max_parallel.unwrap_or(4)
        ),
    );
    push_line(
        &mut code,
        &format!(
            "agent_max_steps: {},",
            runner.agent_max_steps.unwrap_or(24)
        ),
    );
    push_line(
        &mut code,
        &format!(
            "agent_timeout_secs: {},",
            runner.agent_timeout_secs.unwrap_or(600)
        ),
    );
    push_line(
        &mut code,
        &format!(
            "auto_execute: {},",
            runner.auto_execute.unwrap_or(true)
        ),
    );
    push_line(
        &mut code,
        &format!("docs_agent: {},", runner.docs_agent.unwrap_or(false)),
    );
    push_line(
        &mut code,
        &format!(
            "mirror_markdown: {},",
            runner.mirror_markdown.unwrap_or(true)
        ),
    );
    code.push_str("        },\n");

    code.push_str("    }\n}\n");

    let out_dir = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out_dir).join("sdd_config_generated.rs"), code).unwrap();
}

fn push_line(code: &mut String, line: &str) {
    code.push_str("            ");
    code.push_str(line);
    code.push('\n');
}

fn push_str(code: &mut String, field: &str, value: &Option<String>) {
    let literal = value.as_deref().unwrap_or("");
    push_line(code, &format!("{field}: {literal:?},"));
}

fn push_list(code: &mut String, field: &str, value: &Option<Vec<String>>) {
    let names: Vec<&str> = value
        .as_ref()
        .map(|names| names.iter().map(String::as_str).collect())
        .unwrap_or_default();
    push_line(code, &format!("{field}: &[{}],", quote_list(&names)));
}

fn push_list_or(
    code: &mut String,
    field: &str,
    value: &Option<Vec<String>>,
    fallback: &[&str],
) {
    let names: Vec<&str> = value
        .as_ref()
        .map(|names| names.iter().map(String::as_str).collect())
        .filter(|names| !names.is_empty())
        .unwrap_or_else(|| fallback.to_vec());
    push_line(code, &format!("{field}: &[{}],", quote_list(&names)));
}

fn quote_list(names: &[&str]) -> String {
    let mut out = String::new();
    for name in names {
        let _ = write!(out, "{name:?}, ");
    }
    out
}
