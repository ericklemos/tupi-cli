//! Embute `configs/modes/sdd.ron` no binário em tempo de compilação.
//!
//! O RON é parseado aqui e o resultado vira código Rust com literais; não há
//! leitura do arquivo em runtime nem override por projeto. RON inválido quebra
//! o build apontando o caminho do arquivo. Campos omitidos caem no `Default`
//! de cada bloco, que reproduz o comportamento documentado no RON.

use std::{env, fs, path::Path};

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

#[derive(serde::Deserialize)]
#[serde(default)]
struct Planning {
    posture: String,
    tools: Vec<String>,
    lock_mode_on_open: bool,
    /// Zero deixa o padrão global das tools.
    tool_budget_tokens: usize,
}

impl Default for Planning {
    fn default() -> Self {
        Self {
            posture: String::new(),
            tools: Vec::new(),
            lock_mode_on_open: true,
            tool_budget_tokens: 0,
        }
    }
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Execution {
    agent_prefix: String,
    docs_agent_prefix: String,
    subtask_brief: String,
    docs_brief: String,
    handoff_open: String,
    handoff_done: String,
    run_report: String,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Tool {
    planning_actions: Vec<String>,
    execution_actions: Vec<String>,
    planning_description: String,
    execution_description: String,
}

#[derive(serde::Deserialize)]
#[serde(default)]
struct Runner {
    max_parallel: usize,
    agent_max_steps: usize,
    agent_timeout_secs: u64,
    auto_execute: bool,
    docs_agent: bool,
    mirror_markdown: bool,
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            max_parallel: 4,
            agent_max_steps: 24,
            agent_timeout_secs: 600,
            auto_execute: true,
            docs_agent: false,
            mirror_markdown: true,
        }
    }
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
    push_list(&mut code, "tools", names(&file.planning.tools, &[]));
    push_line(
        &mut code,
        &format!(
            "lock_mode_on_open: {},",
            file.planning.lock_mode_on_open
        ),
    );
    push_line(
        &mut code,
        &match file.planning.tool_budget_tokens {
            0 => "tool_budget_tokens: None,".to_string(),
            tokens => format!("tool_budget_tokens: Some({tokens}),"),
        },
    );
    code.push_str("        },\n");

    code.push_str("        execution: SddExecution {\n");
    let execution = &file.execution;
    push_str(&mut code, "agent_prefix", &execution.agent_prefix);
    push_str(&mut code, "docs_agent_prefix", &execution.docs_agent_prefix);
    push_str(&mut code, "subtask_brief", &execution.subtask_brief);
    push_str(&mut code, "docs_brief", &execution.docs_brief);
    push_str(&mut code, "handoff_open", &execution.handoff_open);
    push_str(&mut code, "handoff_done", &execution.handoff_done);
    push_str(&mut code, "run_report", &execution.run_report);
    code.push_str("        },\n");

    code.push_str("        tool: SddToolConfig {\n");
    push_list(
        &mut code,
        "planning_actions",
        names(&file.tool.planning_actions, DEFAULT_PLANNING_ACTIONS),
    );
    push_list(
        &mut code,
        "execution_actions",
        names(&file.tool.execution_actions, DEFAULT_EXECUTION_ACTIONS),
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
    push_line(&mut code, &format!("max_parallel: {},", runner.max_parallel));
    push_line(
        &mut code,
        &format!("agent_max_steps: {},", runner.agent_max_steps),
    );
    push_line(
        &mut code,
        &format!("agent_timeout_secs: {},", runner.agent_timeout_secs),
    );
    push_line(
        &mut code,
        &format!("auto_execute: {},", runner.auto_execute),
    );
    push_line(&mut code, &format!("docs_agent: {},", runner.docs_agent));
    push_line(
        &mut code,
        &format!("mirror_markdown: {},", runner.mirror_markdown),
    );
    code.push_str("        },\n");

    code.push_str("    }\n}\n");

    let out_dir = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out_dir).join("sdd_config_generated.rs"), code).unwrap();
}

fn names<'a>(value: &'a [String], fallback: &'a [&'a str]) -> Vec<&'a str> {
    if value.is_empty() {
        fallback.to_vec()
    } else {
        value.iter().map(String::as_str).collect()
    }
}

fn push_line(code: &mut String, line: &str) {
    code.push_str("            ");
    code.push_str(line);
    code.push('\n');
}

fn push_str(code: &mut String, field: &str, value: &str) {
    push_line(code, &format!("{field}: {value:?},"));
}

fn push_list(code: &mut String, field: &str, names: Vec<&str>) {
    let mut items = String::new();
    for name in names {
        items.push_str(&format!("{name:?}, "));
    }
    push_line(code, &format!("{field}: &[{items}],"));
}
