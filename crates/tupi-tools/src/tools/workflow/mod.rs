use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use tupi_domain::{Workflow, WorkflowSource};
use tupi_kernel::{Tool, ToolError, ToolSpec, WorkflowIndex};

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;

const DESCRIPTION_CHARS: usize = 120;

include!(concat!(env!("OUT_DIR"), "/native_workflows.rs"));

fn home() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
    } else {
        std::env::var_os("HOME")
    };
    PathBuf::from(home.unwrap_or_else(|| ".".into()))
}

pub fn global_agents_dir() -> PathBuf {
    home().join(".agents").join("workflows")
}

pub fn global_dir() -> PathBuf {
    home().join(".tupi").join("workflows")
}

pub fn local_agents_dir(root: &Path) -> PathBuf {
    root.join(".agents").join("workflows")
}

pub fn local_dir(root: &Path) -> PathBuf {
    root.join(".tupi").join("workflows")
}

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

fn catalog_with(root: &Path, layers: &[(PathBuf, WorkflowSource)]) -> Vec<Workflow> {
    let mut merged: BTreeMap<String, Workflow> = BTreeMap::new();
    for (default_name, text) in NATIVE {
        merge(
            &mut merged,
            parse(default_name, text, None, WorkflowSource::Native),
        );
    }
    for (dir, source) in layers {
        for workflow in read_dir(root, dir, *source) {
            merge(&mut merged, workflow);
        }
    }
    merged.into_values().collect()
}

fn merge(merged: &mut BTreeMap<String, Workflow>, incoming: Workflow) {
    match merged.get_mut(&incoming.name) {
        Some(existing) => {
            if incoming.source > existing.source {
                let mut shadowed = existing.shadowed.clone();
                shadowed.push(existing.source);
                let mut replaced = incoming;
                replaced.shadowed = shadowed;
                *existing = replaced;
            } else {
                existing.shadowed.push(incoming.source);
            }
        }
        None => {
            merged.insert(incoming.name.clone(), incoming);
        }
    }
}

fn read_dir(root: &Path, dir: &Path, source: WorkflowSource) -> Vec<Workflow> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Workflow> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|ext| ext.to_str());
        if ext != Some("md") && ext != Some("ron") {
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct WorkflowRon {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub agent: Option<String>,
    pub body: String,
}

fn parse(
    default_name: &str,
    text: &str,
    path: Option<PathBuf>,
    source: WorkflowSource,
) -> Workflow {
    let (name, description, agent, body) =
        if let Ok(ron_parsed) = ron::from_str::<WorkflowRon>(text) {
            let name = if !ron_parsed.name.trim().is_empty() {
                ron_parsed.name
            } else {
                default_name.to_string()
            };
            let desc = if !ron_parsed.description.trim().is_empty() {
                ron_parsed.description
            } else {
                first_paragraph(&ron_parsed.body)
            };
            (name, desc, ron_parsed.agent, ron_parsed.body)
        } else {
            let (front_description, front_agent, body) = split_front_matter(text);
            let desc = front_description
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| first_paragraph(&body));
            (default_name.to_string(), desc, front_agent, body)
        };
    let agent = agent
        .or_else(|| matches!(&source, WorkflowSource::Native).then(|| "execution".to_string()));
    Workflow {
        name,
        description: truncate(&description, DESCRIPTION_CHARS),
        agent,
        source,
        path,
        body,
        shadowed: Vec::new(),
    }
}

fn split_front_matter(text: &str) -> (Option<String>, Option<String>, String) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
    else {
        return (None, None, text.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (None, None, text.to_string());
    };
    let head = &rest[..end];
    let body = rest[end..]
        .trim_start_matches("\n---")
        .trim_start_matches(['\r', '\n'])
        .to_string();
    let extract = |key: &str| {
        head.lines().find_map(|line| {
            let value = line.trim().strip_prefix(key)?.trim();
            let value = value.trim_matches('"').trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    (extract("description:"), extract("agent:"), body)
}

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
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn docs_directive(root: &Path, folders: &[PathBuf]) -> Option<String> {
    let first = folders.first()?;
    let display = |folder: &Path| {
        folder
            .strip_prefix(root)
            .unwrap_or(folder)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let list: Vec<String> = folders
        .iter()
        .map(|folder| format!("`{}`", display(folder)))
        .collect();
    let first_display = display(first);
    let used = if folders.len() > 1 {
        " (a primeira é a usada)"
    } else {
        ""
    };
    Some(format!(
        "Documentação de projeto produzida por `archivist`/`scribe` deve ser gravada em: {}{used}. \
         O scribe preenche os arquivos canônicos em `{first_display}/` (`index.md`, `architecture.md`, \
         `business.md`, `conventions.md` e `decisions/`) e o archivist grava regras em \
         `{first_display}/rules/`. Consulte antes com `docs(action=\"map\")` e reindexe com \
         `docs(action=\"refresh\")` após escrever. Não grave esses artefatos em `.agents/`.",
        list.join(", ")
    ))
}

pub fn prompt_section(workflows: &[Workflow], docs_folders: &[PathBuf], root: &Path) -> String {
    if workflows.is_empty() {
        return String::new();
    }
    let names: Vec<String> = workflows
        .iter()
        .map(|workflow| match &workflow.agent {
            Some(agent) => format!("{} ({agent})", workflow.name),
            None => workflow.name.clone(),
        })
        .collect();
    let mut section = format!(
        "## Workflows\n\n\
         Procedimentos prontos (nativo, global ou local). O usuário invoca com\n\
         `/workflows/<nome>`; a tool `workflow` lista as descrições\n\
         (`action=list`) e carrega os passos (`action=read`). Disponíveis: {}.",
        names.join(", ")
    );
    if let Some(directive) = docs_directive(root, docs_folders) {
        section.push_str("\n\n");
        section.push_str(&directive);
    }
    section
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowAction {
    List,
    Read,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkflowParams {
    pub action: WorkflowAction,
    pub name: Option<String>,
    pub budget_tokens: Option<usize>,
}

pub struct WorkflowTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
    docs_folders: Vec<PathBuf>,
}

impl WorkflowTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
            docs_folders: Vec::new(),
        }
    }

    pub fn with_docs_folders(mut self, docs_folders: Vec<PathBuf>) -> Self {
        self.docs_folders = docs_folders;
        self
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
                        "description": "Nome do arquivo sem extensão; exigido por read."
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
                    if let Some(agent) = &workflow.agent {
                        body.push_str(&format!(" — agente: {agent}"));
                    }
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
                        if let Some(agent) = &workflow.agent {
                            body.push_str(&format!("> agente: {agent}\n"));
                        }
                        body.push('\n');
                        if let Some(directive) = docs_directive(&self.root_dir, &self.docs_folders)
                        {
                            body.push_str(&format!("> documentação: {directive}\n\n"));
                        }
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

    fn sdd_allowed(&self) -> bool {
        true
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
            "sentinel",
            "sentinel global .agents\n",
        );
        write(
            &root.join("global-tupi"),
            "sentinel",
            "sentinel global .tupi\n",
        );
        write(
            &root.join("local-agents"),
            "sentinel",
            "sentinel local .agents\n",
        );
        write(
            &root.join("local-tupi"),
            "sentinel",
            "sentinel local .tupi\n",
        );

        let workflows = catalog_with(root, &layers(root));
        let sentinel = workflows
            .iter()
            .find(|workflow| workflow.name == "sentinel")
            .unwrap();
        assert_eq!(sentinel.source, WorkflowSource::LocalTupi);
        assert_eq!(sentinel.body.trim(), "sentinel local .tupi");
        assert_eq!(
            sentinel.shadowed,
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
        write(&root.join("local-agents"), "skeptic", "dos agentes\n");
        write(&root.join("local-tupi"), "skeptic", "do tupi\n");

        let workflows = catalog_with(root, &layers(root));
        let skeptic = workflows
            .iter()
            .find(|workflow| workflow.name == "skeptic")
            .unwrap();
        assert_eq!(skeptic.source, WorkflowSource::LocalTupi);
        assert_eq!(skeptic.body.trim(), "do tupi");
        assert_eq!(
            skeptic.shadowed,
            vec![WorkflowSource::Native, WorkflowSource::LocalAgents]
        );
    }

    #[test]
    fn an_agents_workflow_beats_the_native_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("local-agents"),
            "bughunter",
            "---\ndescription: meu fix\n---\n\ncorpo\n",
        );

        let workflows = catalog_with(root, &layers(root));
        let bughunter = workflows
            .iter()
            .find(|workflow| workflow.name == "bughunter")
            .unwrap();
        assert_eq!(bughunter.source, WorkflowSource::LocalAgents);
        assert_eq!(bughunter.description, "meu fix");
        assert_eq!(bughunter.shadowed, vec![WorkflowSource::Native]);
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
        let (description, agent, body) =
            split_front_matter("---\r\ndescription: windows\r\n---\r\n\rcorpo");
        assert_eq!(description.as_deref(), Some("windows"));
        assert_eq!(agent, None);
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
        assert!(list.contains("`bolt` (nativo)"));

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
        assert!(out.contains("`spark`"));
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
        let workflows = catalog(Path::new("/nao/existe"));
        let section = prompt_section(&workflows, &[], Path::new("/projeto"));
        assert!(section.starts_with("## Workflows"));
        assert!(section.contains("alchemist"));
        assert!(section.contains("skeptic"));
        assert!(!section.contains("impact_check"));
        assert!(!section.contains("falsificador"));

        let skeptic = workflows
            .iter()
            .find(|workflow| workflow.name == "skeptic")
            .expect("o skeptic viaja com o binário");
        assert_eq!(skeptic.source, WorkflowSource::Native);
        assert!(!skeptic.description.is_empty());
        assert!(skeptic.body.contains("The Skeptic 🤨"));
        assert!(skeptic.body.contains("THE SKEPTIC'S DAILY PROCESS:"));
        assert!(!skeptic.body.starts_with("---"));
    }

    #[test]
    fn a_ron_workflow_file_is_parsed_with_name_description_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let ron_content = "(name: \"custom-call\", description: \"workflow em ron\", body: \"# Custom\\n\\n1. faca\\n\")";
        std::fs::create_dir_all(dir.path().join("local-tupi")).unwrap();
        std::fs::write(dir.path().join("local-tupi/ignored_stem.ron"), ron_content).unwrap();

        let workflows = catalog_with(dir.path(), &layers(dir.path()));
        let custom = workflows
            .iter()
            .find(|workflow| workflow.name == "custom-call")
            .expect("workflow com name no .ron deve usar esse nome");
        assert_eq!(custom.source, WorkflowSource::LocalTupi);
        assert_eq!(custom.description, "workflow em ron");
        assert_eq!(custom.body.trim(), "# Custom\n\n1. faca");

        // Sem name no .ron, usa o stem do arquivo
        let ron_no_name = "(description: \"sem nome\", body: \"# Sem Nome\\n\\n1. faca\\n\")";
        std::fs::write(dir.path().join("local-tupi/fallback_stem.ron"), ron_no_name).unwrap();
        let workflows2 = catalog_with(dir.path(), &layers(dir.path()));
        let fallback = workflows2
            .iter()
            .find(|workflow| workflow.name == "fallback_stem")
            .expect("workflow sem name no .ron deve usar o stem do arquivo");
        assert_eq!(fallback.name, "fallback_stem");
        assert_eq!(fallback.description, "sem nome");
    }

    #[test]
    fn every_native_workflow_is_valid_ron_in_configs() {
        assert!(
            NATIVE.len() >= 5,
            "os workflows nativos viajam com o binário"
        );
        for (name, text) in NATIVE {
            let parsed = ron::from_str::<WorkflowRon>(text);
            assert!(
                parsed.is_ok(),
                "workflow nativo {name} não é RON válido: {:?}",
                parsed.err()
            );
            let ron = parsed.unwrap();
            assert_eq!(ron.name.as_str(), *name);
            let agent = ron.agent.as_deref().unwrap_or_default();
            assert!(
                matches!(agent, "execution" | "sdd"),
                "workflow nativo {name} precisa declarar agent (execution ou sdd); achei {agent:?}"
            );
            assert!(!ron.body.trim().is_empty(), "corpo vazio em {name}");
        }
    }

    /// O catálogo que viaja no binário é contrato com três consumidores: o
    /// prompt do sistema (via `prompt_section`), o autocomplete da TUI e os
    /// testes de precedência. Um `.ron` removido ou renomeado em
    /// `configs/workflows/` desaparece dos três em silêncio, então a lista de
    /// nomes fica fixada aqui.
    #[test]
    fn the_shipped_catalog_is_exactly_the_agent_workflows() {
        let names: Vec<&str> = NATIVE.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            vec![
                "alchemist",
                "archivist",
                "artisan",
                "bolt",
                "bughunter",
                "disco",
                "gatekeeper",
                "janitor",
                "observer",
                "optimizer",
                "palette",
                "scribe",
                "sentinel",
                "skeptic",
                "solver",
                "spark",
            ]
        );
    }

    #[test]
    fn the_prompt_section_lists_the_configured_docs_folders() {
        let root = Path::new("/projeto");
        let workflows = catalog(Path::new("/nao/existe"));
        let folders = vec![PathBuf::from("/projeto/docs")];
        let section = prompt_section(&workflows, &folders, root);
        assert!(section.starts_with("## Workflows"));
        assert!(section.contains("`docs`"));
        assert!(section.contains("architecture.md"));
        assert!(section.contains("decisions/"));
        assert!(section.contains("docs/rules/"));
        assert!(!section.contains("docs/scribe/"));
    }

    #[test]
    fn the_prompt_section_marks_the_first_folder_as_the_used_one() {
        let root = Path::new("/projeto");
        let workflows = catalog(Path::new("/nao/existe"));
        let folders = vec![
            PathBuf::from("/projeto/docs"),
            PathBuf::from("/projeto/wiki"),
        ];
        let section = prompt_section(&workflows, &folders, root);
        assert!(section.contains("`docs`, `wiki` (a primeira é a usada)"));
    }

    #[test]
    fn the_prompt_section_omits_the_docs_line_without_folders() {
        let workflows = catalog(Path::new("/nao/existe"));
        let section = prompt_section(&workflows, &[], Path::new("/projeto"));
        assert!(!section.contains("docs/scribe/"));
    }

    #[test]
    fn read_reports_the_configured_docs_folders() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".tupi/workflows"),
            "deploy",
            "# Deploy\n\n1. suba\n",
        );
        let tool = tool(dir.path()).with_docs_folders(vec![dir.path().join("docs")]);

        let read = call(
            &tool,
            serde_json::json!({"action": "read", "name": "deploy"}),
        )
        .unwrap();
        assert!(read.contains("`docs`"));
        assert!(read.contains("architecture.md"));
        assert!(read.contains("decisions/"));
        assert!(read.contains("docs/rules/"));
        assert!(!read.contains("docs/scribe/"));
    }

    #[test]
    fn scribe_writes_docs_under_the_configured_folders() {
        let (_, body) = NATIVE
            .iter()
            .find(|(name, _)| *name == "scribe")
            .expect("scribe viaja com o binário");
        assert!(!body.contains(".agents/results/scribe"));
        assert!(body.contains("docs_folders"));
        assert!(body.contains("<docs_folder>/architecture.md"));
        assert!(body.contains("<docs_folder>/decisions/"));
        assert!(body.contains("docs(action=\"map\")"));
        assert!(body.contains("docs(action=\"refresh\")"));
        assert!(!body.contains("<docs_folder>/scribe/"));
    }

    #[test]
    fn archivist_writes_rules_under_the_configured_folders() {
        let (_, body) = NATIVE
            .iter()
            .find(|(name, _)| *name == "archivist")
            .expect("archivist viaja com o binário");
        assert!(!body.contains(".agents/rules"));
        assert!(body.contains("docs_folders"));
        assert!(body.contains("<docs_folder>/rules/"));
        assert!(body.contains("docs(action=\"map\")"));
        assert!(body.contains("docs(action=\"write\")"));
        assert!(body.contains("docs(action=\"refresh\")"));
    }
}
