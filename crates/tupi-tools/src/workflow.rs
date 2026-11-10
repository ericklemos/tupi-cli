//! Descoberta dos workflows em cinco camadas, da mais fraca à mais forte:
//!
//! 1. **nativo** — embutido no binário por `include_str!`;
//! 2. **global `.agents`** — `~/.agents/workflows/*.md`;
//! 3. **global `.tupi`** — `~/.tupi/workflows/*.md`;
//! 4. **local `.agents`** — `<root>/.agents/workflows/*.md`;
//! 5. **local `.tupi`** — `<root>/.tupi/workflows/*.md`.
//!
//! O mais próximo do projeto vence; as camadas perdedoras ficam registradas em
//! [`Workflow::shadowed`]. `.agents` é o diretório compartilhado com outros
//! agentes; `.tupi` é a decisão deste agente e por isso vence no mesmo escopo.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use tupi_domain::{Workflow, WorkflowSource};
use tupi_kernel::{Tool, ToolError, ToolSpec, WorkflowIndex};

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;

/// Workflows que viajam dentro do binário. O nome é o do arquivo.
pub const NATIVE: &[(&str, &str)] = &[
    ("feature", include_str!("../workflows/feature.md")),
    ("fix-bug", include_str!("../workflows/fix-bug.md")),
    ("refactor", include_str!("../workflows/refactor.md")),
    ("review", include_str!("../workflows/review.md")),
];

const DESCRIPTION_CHARS: usize = 120;

fn home() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
    } else {
        std::env::var_os("HOME")
    };
    PathBuf::from(home.unwrap_or_else(|| ".".into()))
}

/// `~/.agents/workflows`, ou `%USERPROFILE%\.agents\workflows` no Windows.
pub fn global_agents_dir() -> PathBuf {
    home().join(".agents").join("workflows")
}

/// `~/.tupi/workflows`.
pub fn global_dir() -> PathBuf {
    home().join(".tupi").join("workflows")
}

/// `<root>/.agents/workflows`.
pub fn local_agents_dir(root: &Path) -> PathBuf {
    root.join(".agents").join("workflows")
}

/// `<root>/.tupi/workflows`.
pub fn local_dir(root: &Path) -> PathBuf {
    root.join(".tupi").join("workflows")
}

/// Catálogo do projeto: nativo, global e local, com o mais forte vencendo.
pub fn catalog(root: &Path) -> Vec<Workflow> {
    catalog_with(
        root,
        &[
            (global_agents_dir(), WorkflowSource::GlobalAgents),
            (global_dir(), WorkflowSource::GlobalTupi),
            (local_agents_dir(root), WorkflowSource::LocalAgents),
            (local_dir(root), WorkflowSource::LocalTupi),
        ],
    )
}

/// Mesma resolução de [`catalog`], com os diretórios injetados.
fn catalog_with(root: &Path, layers: &[(PathBuf, WorkflowSource)]) -> Vec<Workflow> {
    let mut merged: BTreeMap<String, Workflow> = BTreeMap::new();
    for (name, text) in NATIVE {
        merge(&mut merged, parse(name, text, None, WorkflowSource::Native));
    }
    for (dir, source) in layers {
        for workflow in read_dir(root, dir, *source) {
            merge(&mut merged, workflow);
        }
    }
    merged.into_values().collect()
}

fn merge(merged: &mut BTreeMap<String, Workflow>, incoming: Workflow) {
    let key = incoming.name.clone();
    match merged.remove(&key) {
        None => {
            merged.insert(key, incoming);
        }
        Some(current) if incoming.source > current.source => {
            let mut winner = incoming;
            winner.shadowed = current.shadowed;
            winner.shadowed.push(current.source);
            merged.insert(key, winner);
        }
        Some(mut current) => {
            current.shadowed.push(incoming.source);
            merged.insert(key, current);
        }
    }
}

/// Lê `*.md` de um diretório. Diretório ausente devolve vazio: workflow é
/// opcional por definição.
fn read_dir(root: &Path, dir: &Path, source: WorkflowSource) -> Vec<Workflow> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Workflow> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let relative = path.strip_prefix(root).map(Path::to_path_buf).ok();
        out.push(parse(name, &text, relative, source));
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn parse(name: &str, text: &str, path: Option<PathBuf>, source: WorkflowSource) -> Workflow {
    let (front, body) = split_front_matter(text);
    let description = front
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| first_paragraph(&body));
    Workflow {
        name: name.to_string(),
        description: truncate(&description, DESCRIPTION_CHARS),
        source,
        path,
        body,
        shadowed: Vec::new(),
    }
}

/// Front-matter opcional `---\ndescription: …\n---`; o corpo sai sem ele.
fn split_front_matter(text: &str) -> (Option<String>, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
    else {
        return (None, text.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (None, text.to_string());
    };
    let head = &rest[..end];
    let body = rest[end..]
        .trim_start_matches("\n---")
        .trim_start_matches(|c| c == '\r' || c == '\n')
        .to_string();
    let description = head.lines().find_map(|line| {
        let value = line.trim().strip_prefix("description:")?.trim();
        let value = value.trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_string())
    });
    (description, body)
}

/// Descrição de reserva para arquivo sem front-matter: o primeiro parágrafo,
/// caindo para o título quando o arquivo só tem títulos.
fn first_paragraph(body: &str) -> String {
    let mut title = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('>') || line.starts_with("---") {
            continue;
        }
        if line.starts_with('#') {
            if title.is_empty() {
                title = line.trim_start_matches('#').trim().to_string();
            }
            continue;
        }
        return line.to_string();
    }
    title
}

fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (count, ch) in text.chars().enumerate() {
        if count == max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

/// Bloco do prompt de sistema: um nome por workflow, sem descrição nem corpo.
///
/// Custa uma linha por sessão e evita um passo de `workflow` só para descobrir
/// o que existe.
pub fn prompt_section(workflows: &[Workflow]) -> String {
    if workflows.is_empty() {
        return String::new();
    }
    let names: Vec<&str> = workflows
        .iter()
        .map(|workflow| workflow.name.as_str())
        .collect();
    format!(
        "## Workflows\n\n\
         Procedimentos prontos (nativo, global ou local). O usuário invoca com\n\
         `/workflows/<nome>`; a tool `workflow` lista as descrições\n\
         (`action=list`) e carrega os passos (`action=read`). Disponíveis: {}.",
        names.join(", ")
    )
}

/// Catálogo de arquivos do projeto: atende a porta que o `App` e a TUI usam.
#[derive(Debug, Clone)]
pub struct Catalog {
    root: PathBuf,
}

impl Catalog {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl WorkflowIndex for Catalog {
    fn list(&self) -> Vec<Workflow> {
        catalog(&self.root)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowAction {
    List,
    Read,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowParams {
    pub action: WorkflowAction,
    pub name: Option<String>,
    pub budget_tokens: Option<usize>,
}

pub struct WorkflowTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl WorkflowTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for WorkflowTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "workflow".to_string(),
            description: "Procedimentos em markdown em três escopos: nativo (embutido no binário), global (~/.agents/workflows e ~/.tupi/workflows) e local (.agents/workflows e .tupi/workflows). list mostra as descrições; read devolve os passos do workflow escolhido.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["list", "read"],
                        "description": "list cataloga; read carrega o corpo de um workflow."
                    },
                    "name": {
                        "type": "string",
                        "description": "Nome do arquivo sem .md; exigido por read."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: WorkflowParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let workflows = catalog(&self.root_dir);

        let (heading, body) = match params.action {
            WorkflowAction::List => {
                let mut body = String::from("\n");
                for workflow in &workflows {
                    body.push_str(&format!(
                        "- `{}` ({})",
                        workflow.name,
                        workflow.source.as_str()
                    ));
                    if !workflow.description.is_empty() {
                        body.push_str(&format!(": {}", workflow.description));
                    }
                    body.push('\n');
                }
                ("### workflow — catálogo".to_string(), body)
            }
            WorkflowAction::Read => {
                let name = params
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("read exige name".to_string()))?;
                match workflows
                    .iter()
                    .find(|workflow| workflow.name.eq_ignore_ascii_case(name))
                {
                    Some(workflow) => {
                        let mut body = String::from("\n");
                        match &workflow.path {
                            Some(path) => {
                                body.push_str(&format!("> arquivo: `{}`\n", path.display()))
                            }
                            None => body.push_str("> arquivo: embutido no binário\n"),
                        }
                        if !workflow.shadowed.is_empty() {
                            let sources: Vec<&str> = workflow
                                .shadowed
                                .iter()
                                .map(|source| source.as_str())
                                .collect();
                            body.push_str(&format!("> sombreia: {}\n", sources.join(", ")));
                        }
                        body.push('\n');
                        body.push_str(workflow.body.trim());
                        body.push('\n');
                        (
                            format!(
                                "### workflow — `{}` ({})",
                                workflow.name,
                                workflow.source.as_str()
                            ),
                            body,
                        )
                    }
                    None => {
                        let names: Vec<String> = workflows
                            .iter()
                            .map(|workflow| format!("`{}`", workflow.name))
                            .collect();
                        (
                            "### workflow — não encontrado".to_string(),
                            format!(
                                "\nNÃO — nenhum workflow com o nome `{name}`. Disponíveis: {}.\n",
                                names.join(", ")
                            ),
                        )
                    }
                }
            }
        };

        let section = Section {
            heading,
            body,
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("?");
        match arguments.get("name").and_then(Value::as_str) {
            Some(name) => format!("workflow: {action} {name}"),
            None => format!("workflow: {action}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{name}.md")), text).unwrap();
    }

    fn tool(root: &Path) -> WorkflowTool {
        WorkflowTool::new(
            root.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1024 * 1024,
            )),
            4000,
        )
    }

    fn call(tool: &WorkflowTool, value: serde_json::Value) -> Result<String, ToolError> {
        Tool::call(tool, value)
    }

    /// As quatro camadas de arquivo, como fixture dentro de um tempdir.
    fn layers(root: &Path) -> Vec<(PathBuf, WorkflowSource)> {
        vec![
            (root.join("global-agents"), WorkflowSource::GlobalAgents),
            (root.join("global-tupi"), WorkflowSource::GlobalTupi),
            (root.join("local-agents"), WorkflowSource::LocalAgents),
            (root.join("local-tupi"), WorkflowSource::LocalTupi),
        ]
    }

    #[test]
    fn every_native_workflow_has_a_description_and_a_body() {
        for (name, text) in NATIVE {
            let workflow = parse(name, text, None, WorkflowSource::Native);
            assert!(!workflow.description.is_empty(), "{name} sem descrição");
            assert!(!workflow.body.trim().is_empty(), "{name} sem corpo");
            assert!(
                !workflow.body.starts_with("---"),
                "{name} manteve o front-matter"
            );
        }
    }

    #[test]
    fn the_closest_layer_wins() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("global-agents"),
            "review",
            "review global .agents\n",
        );
        write(&root.join("global-tupi"), "review", "review global .tupi\n");
        write(
            &root.join("local-agents"),
            "review",
            "review local .agents\n",
        );
        write(&root.join("local-tupi"), "review", "review local .tupi\n");

        let workflows = catalog_with(root, &layers(root));
        let review = workflows
            .iter()
            .find(|workflow| workflow.name == "review")
            .unwrap();
        assert_eq!(review.source, WorkflowSource::LocalTupi);
        assert_eq!(review.body.trim(), "review local .tupi");
        assert_eq!(
            review.shadowed,
            vec![
                WorkflowSource::Native,
                WorkflowSource::GlobalAgents,
                WorkflowSource::GlobalTupi,
                WorkflowSource::LocalAgents,
            ]
        );
    }

    #[test]
    fn tupi_beats_agents_in_the_same_scope() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("local-agents"), "feature", "dos agentes\n");
        write(&root.join("local-tupi"), "feature", "do tupi\n");

        let workflows = catalog_with(root, &layers(root));
        let feature = workflows
            .iter()
            .find(|workflow| workflow.name == "feature")
            .unwrap();
        assert_eq!(feature.source, WorkflowSource::LocalTupi);
        assert_eq!(feature.body.trim(), "do tupi");
        assert_eq!(
            feature.shadowed,
            vec![WorkflowSource::Native, WorkflowSource::LocalAgents]
        );
    }

    #[test]
    fn an_agents_workflow_beats_the_native_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("local-agents"),
            "fix-bug",
            "---\ndescription: meu fix\n---\n\ncorpo\n",
        );

        let workflows = catalog_with(root, &layers(root));
        let fix = workflows
            .iter()
            .find(|workflow| workflow.name == "fix-bug")
            .unwrap();
        assert_eq!(fix.source, WorkflowSource::LocalAgents);
        assert_eq!(fix.description, "meu fix");
        assert_eq!(fix.shadowed, vec![WorkflowSource::Native]);
    }

    #[test]
    fn a_missing_directory_is_silently_empty() {
        let missing = Path::new("/nao/existe");
        let workflows = catalog_with(
            missing,
            &[
                (missing.join("a"), WorkflowSource::GlobalAgents),
                (missing.join("b"), WorkflowSource::GlobalTupi),
                (missing.join("c"), WorkflowSource::LocalAgents),
                (missing.join("d"), WorkflowSource::LocalTupi),
            ],
        );
        assert_eq!(workflows.len(), NATIVE.len());
        assert!(
            workflows
                .iter()
                .all(|workflow| workflow.source == WorkflowSource::Native)
        );
    }

    #[test]
    fn a_file_without_front_matter_uses_the_first_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("local-tupi"),
            "enxuto",
            "# Enxuto\n\n> nota\n\nFaz pouca coisa.\n",
        );
        let workflows = catalog_with(dir.path(), &layers(dir.path()));
        let enxuto = workflows
            .iter()
            .find(|workflow| workflow.name == "enxuto")
            .unwrap();
        assert_eq!(enxuto.description, "Faz pouca coisa.");
    }

    #[test]
    fn a_title_only_file_falls_back_to_the_title() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("local-tupi"), "curto", "# Só o título\n");
        let workflows = catalog_with(dir.path(), &layers(dir.path()));
        let curto = workflows
            .iter()
            .find(|workflow| workflow.name == "curto")
            .unwrap();
        assert_eq!(curto.description, "Só o título");
    }

    #[test]
    fn a_windows_front_matter_is_understood() {
        let (description, body) =
            split_front_matter("---\r\ndescription: windows\r\n---\r\n\r\ncorpo");
        assert_eq!(description.as_deref(), Some("windows"));
        assert_eq!(body, "corpo");
    }

    #[test]
    fn non_markdown_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("local-tupi")).unwrap();
        std::fs::write(dir.path().join("local-tupi/nota.txt"), "texto").unwrap();
        let workflows = catalog_with(dir.path(), &layers(dir.path()));
        assert!(workflows.iter().all(|workflow| workflow.name != "nota"));
    }

    #[test]
    fn list_shows_source_and_read_returns_the_body() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".tupi/workflows"),
            "deploy",
            "---\ndescription: sobe a versão\n---\n\n# Deploy\n\n1. suba\n",
        );
        let tool = tool(dir.path());

        let list = call(&tool, serde_json::json!({"action": "list"})).unwrap();
        assert!(list.contains("`deploy` (local (.tupi)): sobe a versão"));
        assert!(list.contains("`feature` (nativo)"));

        let read = call(
            &tool,
            serde_json::json!({"action": "read", "name": "deploy"}),
        )
        .unwrap();
        assert!(read.contains("`deploy` (local (.tupi))"));
        assert!(read.contains("> arquivo: `.tupi/workflows/deploy.md`"));
        assert!(read.contains("1. suba"));
        assert!(!read.contains("description:"));
    }

    #[test]
    fn read_requires_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let error = call(&tool(dir.path()), serde_json::json!({"action": "read"})).unwrap_err();
        assert!(error.to_string().contains("name"));
    }

    #[test]
    fn an_unknown_name_lists_what_exists() {
        let dir = tempfile::tempdir().unwrap();
        let out = call(
            &tool(dir.path()),
            serde_json::json!({"action": "read", "name": "nao-existe"}),
        )
        .unwrap();
        assert!(out.contains("NÃO"));
        assert!(out.contains("`feature`"));
    }

    #[test]
    fn the_catalog_reads_the_real_project_directories() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".agents/workflows"),
            "so-agents",
            "# A\n\nfaz\n",
        );
        let index = Catalog::new(dir.path());
        let found = index
            .list()
            .into_iter()
            .find(|workflow| workflow.name == "so-agents")
            .expect("o workflow local de .agents aparece");

        assert_eq!(found.source, WorkflowSource::LocalAgents);
        assert_eq!(
            found.path.as_deref(),
            Some(Path::new(".agents/workflows/so-agents.md"))
        );
        assert!(index.find("SO-AGENTS").is_some());
    }

    #[test]
    fn the_prompt_section_lists_names_without_bodies() {
        let section = prompt_section(&catalog(Path::new("/nao/existe")));
        assert!(section.starts_with("## Workflows"));
        assert!(section.contains("feature"));
        assert!(!section.contains("impact_check"));
    }
}
