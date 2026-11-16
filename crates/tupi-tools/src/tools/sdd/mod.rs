use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tupi_kernel::{AgentMode, Tool, ToolError, ToolSpec};

use crate::budget::{self, Section};
use crate::paths;
use crate::store::ResponseStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddPlanStatus {
    Draft,
    Closed,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddTaskStatus {
    Open,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddSubtaskStatus {
    Open,
    Done,
}

/// Teste cargo que prova o aceite de uma subtask.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceTest {
    #[serde(default)]
    pub package: Option<String>,
    pub filter: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddSubtask {
    pub id: String,
    pub title: String,
    pub acceptance: String,
    #[serde(default)]
    pub touches: Vec<String>,
    #[serde(default)]
    pub acceptance_test: Option<AcceptanceTest>,
    pub status: SddSubtaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddTask {
    pub id: String,
    pub title: String,
    pub details: String,
    #[serde(default)]
    pub nuances: Vec<String>,
    #[serde(default)]
    pub parallel: bool,
    pub status: SddTaskStatus,
    #[serde(default)]
    pub subtasks: Vec<SddSubtask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddPlan {
    pub slug: String,
    pub repo_key: String,
    pub title: String,
    pub status: SddPlanStatus,
    pub goal: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub nuances: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<SddTask>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub closed_at: Option<u64>,
    #[serde(default)]
    pub activities: Vec<SddActivity>,
    #[serde(default)]
    pub notes: Vec<SddNote>,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub docs_feedback: Option<DocsFeedback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddActivity {
    pub seq: u64,
    pub at: u64,
    pub actor: String,
    pub action: String,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub subtask: Option<String>,
    pub status: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SddNote {
    pub at: u64,
    pub actor: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocsFeedbackStatus {
    Skipped,
    Pending,
    Updated,
    Created,
    Noop,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocsFeedback {
    pub status: DocsFeedbackStatus,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub reindexed: bool,
}

impl Default for SddPlanStatus {
    fn default() -> Self {
        Self::Draft
    }
}

impl Default for SddPlan {
    fn default() -> Self {
        Self {
            slug: String::new(),
            repo_key: String::new(),
            title: String::new(),
            status: SddPlanStatus::Draft,
            goal: String::new(),
            constraints: Vec::new(),
            nuances: Vec::new(),
            tasks: Vec::new(),
            created_at: 0,
            updated_at: 0,
            closed_at: None,
            activities: Vec::new(),
            notes: Vec::new(),
            conclusion: None,
            docs_feedback: None,
        }
    }
}

impl SddPlan {
    pub fn record_activity(&mut self, actor: &str, action: &str, status: &str) -> &mut SddActivity {
        let seq = self.activities.len() as u64 + 1;
        self.activities.push(SddActivity {
            seq,
            at: now(),
            actor: actor.to_string(),
            action: action.to_string(),
            task: None,
            subtask: None,
            status: status.to_string(),
            duration_ms: None,
            files: Vec::new(),
            note: None,
        });
        self.activities
            .last_mut()
            .expect("atividade recém-inserida")
    }

    pub fn add_note(&mut self, actor: &str, text: &str) {
        self.notes.push(SddNote {
            at: now(),
            actor: actor.to_string(),
            text: text.to_string(),
        });
    }
}
pub trait SddStore: Send + Sync {
    fn save(&self, plan: &SddPlan) -> Result<(), String>;
    fn load(&self, repo_key: &str, slug: &str) -> Result<Option<SddPlan>, String>;
    fn list(&self, repo_key: &str) -> Result<Vec<SddPlan>, String>;
    fn active_slug(&self, repo_key: &str) -> Result<Option<String>, String>;
    fn set_active_slug(&self, repo_key: &str, slug: Option<&str>) -> Result<(), String>;
}

#[derive(Default)]
pub struct InMemorySddStore {
    plans: Mutex<HashMap<(String, String), SddPlan>>,
    active: Mutex<HashMap<String, String>>,
}

impl SddStore for InMemorySddStore {
    fn save(&self, plan: &SddPlan) -> Result<(), String> {
        self.plans
            .lock()
            .unwrap()
            .insert((plan.repo_key.clone(), plan.slug.clone()), plan.clone());
        Ok(())
    }

    fn load(&self, repo_key: &str, slug: &str) -> Result<Option<SddPlan>, String> {
        Ok(self
            .plans
            .lock()
            .unwrap()
            .get(&(repo_key.to_string(), slug.to_string()))
            .cloned())
    }

    fn list(&self, repo_key: &str) -> Result<Vec<SddPlan>, String> {
        let mut out: Vec<SddPlan> = self
            .plans
            .lock()
            .unwrap()
            .iter()
            .filter(|((rk, _), _)| rk == repo_key)
            .map(|(_, plan)| plan.clone())
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.slug.cmp(&b.slug)));
        Ok(out)
    }

    fn active_slug(&self, repo_key: &str) -> Result<Option<String>, String> {
        Ok(self.active.lock().unwrap().get(repo_key).cloned())
    }

    fn set_active_slug(&self, repo_key: &str, slug: Option<&str>) -> Result<(), String> {
        let mut active = self.active.lock().unwrap();
        match slug {
            Some(s) => {
                active.insert(repo_key.to_string(), s.to_string());
            }
            None => {
                active.remove(repo_key);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SddAction {
    UpsertSpec,
    SetTasks,
    Status,
    List,
    Activate,
    Close,
    Progress,
    Execute,
    Note,
}

#[derive(Debug, Deserialize)]
struct SubtaskInput {
    id: String,
    title: String,
    #[serde(default)]
    acceptance: String,
    #[serde(default)]
    touches: Vec<String>,
    #[serde(default)]
    acceptance_test: Option<AcceptanceTest>,
}

#[derive(Debug, Deserialize)]
struct TaskInput {
    id: String,
    title: String,
    #[serde(default)]
    details: String,
    #[serde(default)]
    nuances: Vec<String>,
    #[serde(default)]
    parallel: bool,
    #[serde(default)]
    subtasks: Vec<SubtaskInput>,
}

#[derive(Debug, Deserialize)]
struct SddParams {
    action: SddAction,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    #[serde(default)]
    constraints: Option<Vec<String>>,
    #[serde(default)]
    nuances: Option<Vec<String>>,
    #[serde(default)]
    tasks: Option<Vec<TaskInput>>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    budget_tokens: Option<usize>,
}

pub struct SddTool {
    root_dir: PathBuf,
    sdd_store: Arc<dyn SddStore>,
    mode_cell: Option<Arc<AtomicU8>>,
    response_store: Arc<ResponseStore>,
    default_budget_tokens: usize,
    docs_enabled: bool,
    pub(crate) executor: Option<Arc<dyn Fn(&SddPlan) -> Result<String, String> + Send + Sync>>,
}

impl SddTool {
    pub fn new(
        root_dir: PathBuf,
        sdd_store: Arc<dyn SddStore>,
        mode_cell: Option<Arc<AtomicU8>>,
        default_budget_tokens: usize,
    ) -> Self {
        Self {
            root_dir,
            sdd_store,
            mode_cell,
            response_store: Arc::new(ResponseStore::new(Duration::from_secs(600), 1024 * 1024)),
            default_budget_tokens,
            docs_enabled: false,
            executor: None,
        }
    }

    pub fn with_response_store(mut self, response_store: Arc<ResponseStore>) -> Self {
        self.response_store = response_store;
        self
    }
    pub fn with_docs(mut self, enabled: bool) -> Self {
        self.docs_enabled = enabled;
        self
    }

    fn current_mode(&self) -> AgentMode {
        self.mode_cell
            .as_ref()
            .map(|cell| AgentMode::from_u8(cell.load(Ordering::SeqCst)))
            .unwrap_or(AgentMode::Execution)
    }

    fn execution_spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sdd".to_string(),
            description: "Consulta e atualiza o progresso de specs SDD no modo execução: status, list, activate, progress e execute.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "list", "activate", "progress", "execute"],
                        "description": "Ação SDD disponível no modo Execution."
                    },
                    "slug": {
                        "type": "string",
                        "description": "Identificador curto da iniciativa ([a-z0-9-_])."
                    },
                    "id": {
                        "type": "string",
                        "description": "Id da task ou subtask para action=progress."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["open", "in_progress", "done"],
                        "description": "Novo status para action=progress."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens da resposta."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn require_sdd_mode(&self, action: &str) -> Result<(), ToolError> {
        if self.mode_cell.is_some() && self.current_mode() != AgentMode::Sdd {
            return Err(ToolError::Other(format!(
                "ação '{action}' exige modo SDD (use Shift+Tab)"
            )));
        }
        Ok(())
    }

    fn require_execution_mode(&self, action: &str) -> Result<(), ToolError> {
        if self.mode_cell.is_some() && self.current_mode() != AgentMode::Execution {
            return Err(ToolError::Other(format!(
                "ação '{action}' exige modo Execution (feche o SDD antes)"
            )));
        }
        Ok(())
    }

    fn repo_key(&self) -> String {
        paths::repo_key(&self.root_dir)
    }

    fn resolve_target_slug(&self, explicit: Option<&str>) -> Result<String, ToolError> {
        if let Some(raw) = explicit {
            return validate_slug(raw);
        }
        let rk = self.repo_key();
        self.sdd_store
            .active_slug(&rk)
            .map_err(ToolError::Other)?
            .ok_or_else(|| {
                ToolError::InvalidArguments("nenhum plano SDD ativo; informe slug".into())
            })
    }

    fn render_budgeted(&self, sections: Vec<Section>, budget_tokens: usize) -> String {
        let selection = budget::select(&sections, budget_tokens);
        let handle = if !selection.omitted.is_empty() {
            Some(self.response_store.insert(selection.omitted_body()))
        } else {
            None
        };
        budget::render(&selection, handle.as_deref())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn validate_slug(raw: &str) -> Result<String, ToolError> {
    let slug = raw.trim();
    if slug.is_empty() || slug.len() > 64 {
        return Err(ToolError::InvalidArguments(
            "slug deve ter entre 1 e 64 caracteres".into(),
        ));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(ToolError::InvalidArguments(format!(
            "slug inválido '{slug}': use apenas [a-z0-9-_]"
        )));
    }
    Ok(slug.to_string())
}

pub(crate) fn has_doc_citation(text: &str) -> bool {
    regex::Regex::new(r"[A-Za-z0-9_./-]+:\d+-\d+")
        .map(|pattern| pattern.is_match(text))
        .unwrap_or(false)
}

fn status_label_plan(status: SddPlanStatus) -> &'static str {
    match status {
        SddPlanStatus::Draft => "draft",
        SddPlanStatus::Closed => "closed",
        SddPlanStatus::Completed => "completed",
        SddPlanStatus::Failed => "failed",
    }
}

fn status_label_task(status: SddTaskStatus) -> &'static str {
    match status {
        SddTaskStatus::Open => "open",
        SddTaskStatus::InProgress => "in_progress",
        SddTaskStatus::Done => "done",
    }
}

/// Grava o espelho markdown do plano em `.agents/tupi/sdd/<slug>.md`.
pub fn write_mirror(root: &std::path::Path, plan: &SddPlan) -> Result<PathBuf, ToolError> {
    let rel = format!(".agents/tupi/sdd/{}.md", plan.slug);
    let target = paths::resolve_for_write(root, &rel)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| ToolError::Other(format!("falha ao criar diretório SDD: {error}")))?;
    }
    let md = render_markdown(plan);
    paths::atomic_write(&target, md.as_bytes()).map_err(|error| {
        ToolError::Other(format!("falha ao gravar espelho SDD em {rel}: {error}"))
    })?;
    Ok(target)
}

pub fn render_markdown(plan: &SddPlan) -> String {
    let mut out = String::new();
    out.push_str("<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->\n\n");
    out.push_str(&format!("# {} (`{}`)\n\n", plan.title, plan.slug));
    out.push_str(&format!(
        "**Status:** {}\n\n",
        status_label_plan(plan.status)
    ));
    out.push_str(&format!("## Objetivo\n\n{}\n\n", plan.goal));

    if !plan.constraints.is_empty() {
        out.push_str("## Restrições\n\n");
        for item in &plan.constraints {
            out.push_str(&format!("- {item}\n"));
        }
        out.push('\n');
    }

    if !plan.nuances.is_empty() {
        out.push_str("## Nuances\n\n");
        for item in &plan.nuances {
            out.push_str(&format!("- {item}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Tasks\n\n");
    for task in &plan.tasks {
        let mark = match task.status {
            SddTaskStatus::Done => "x",
            SddTaskStatus::InProgress => "~",
            SddTaskStatus::Open => " ",
        };
        out.push_str(&format!("- [{mark}] **{}** {}", task.id, task.title));
        if task.parallel {
            out.push_str(" (paralela)");
        }
        out.push('\n');
        if !task.details.trim().is_empty() {
            out.push_str(&format!("  - {}\n", task.details.trim()));
        }
        for nuance in &task.nuances {
            out.push_str(&format!("  - Nuance: {nuance}\n"));
        }
        for sub in &task.subtasks {
            let smark = match sub.status {
                SddSubtaskStatus::Done => "x",
                SddSubtaskStatus::Open => " ",
            };
            out.push_str(&format!("  - [{smark}] **{}** {}", sub.id, sub.title));
            if !sub.touches.is_empty() {
                out.push_str(&format!(" (toca: {})", sub.touches.join(", ")));
            }
            if let Some(test) = &sub.acceptance_test {
                match test.package.as_deref() {
                    Some(package) => {
                        out.push_str(&format!(
                            " (aceite: cargo test -p {package} {})",
                            test.filter
                        ));
                    }
                    None => out.push_str(&format!(" (aceite: cargo test {})", test.filter)),
                }
            }
            if !sub.acceptance.trim().is_empty() {
                out.push_str(&format!(" — {}", sub.acceptance.trim()));
            }
            out.push('\n');
        }
    }
    out
}

fn plan_to_sections(plan: &SddPlan) -> Vec<Section> {
    let mut header = format!(
        "- slug: `{}`\n- status: {}\n- objetivo: {}",
        plan.slug,
        status_label_plan(plan.status),
        plan.goal
    );
    if !plan.constraints.is_empty() {
        header.push_str("\n- restrições:");
        for c in &plan.constraints {
            header.push_str(&format!("\n  - {c}"));
        }
    }
    if !plan.nuances.is_empty() {
        header.push_str("\n- nuances:");
        for n in &plan.nuances {
            header.push_str(&format!("\n  - {n}"));
        }
    }
    let mut sections = vec![Section {
        heading: format!(
            "### sdd — {} ({})",
            plan.title,
            status_label_plan(plan.status)
        ),
        body: header,
        priority: 255,
    }];

    for task in &plan.tasks {
        let mut body = format!("- status: {}", status_label_task(task.status));
        if !task.details.trim().is_empty() {
            body.push_str(&format!("\n- detalhes: {}", task.details.trim()));
        }
        for nuance in &task.nuances {
            body.push_str(&format!("\n- nuance: {nuance}"));
        }
        for sub in &task.subtasks {
            let mark = if sub.status == SddSubtaskStatus::Done {
                "x"
            } else {
                " "
            };
            body.push_str(&format!("\n- [{mark}] {}: {}", sub.id, sub.title));
            if !sub.acceptance.trim().is_empty() {
                body.push_str(&format!(" (aceite: {})", sub.acceptance.trim()));
            }
        }
        sections.push(Section {
            heading: format!("#### {} — {}", task.id, task.title),
            body,
            priority: 200,
        });
    }
    sections
}

impl Tool for SddTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sdd".to_string(),
            description: "Gerencia specs, tasks e subtasks de planejamento SDD (fonte de verdade em .tupi/sdd/<slug>/: meta.json e tasks.json). Antes de upsert_spec, consulte a tool docs e registre as fontes como caminho:inicio-fim em goal, constraints, nuances e details.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["upsert_spec", "set_tasks", "status", "list", "activate", "close", "progress", "note"],
                        "description": "Ação SDD. No modo SDD: upsert_spec, set_tasks, status, list, activate, close. Na execução: status, list, activate, progress."
                    },
                    "slug": {
                        "type": "string",
                        "description": "Identificador curto da iniciativa ([a-z0-9-_])."
                    },
                    "title": {
                        "type": "string",
                        "description": "Título da spec."
                    },
                    "goal": {
                        "type": "string",
                        "description": "Objetivo principal e escopo."
                    },
                    "constraints": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrições e não-objetivos."
                    },
                    "nuances": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Nuances e regras de negócio destiladas da documentação/código."
                    },
                    "tasks": {
                        "type": "array",
                        "description": "Lista completa de tasks e subtasks.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "title": { "type": "string" },
                                "details": { "type": "string" },
                                "nuances": { "type": "array", "items": { "type": "string" } },
                                "subtasks": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "id": { "type": "string" },
                                            "title": { "type": "string" },
                                            "acceptance": { "type": "string" }
                                        },
                                        "required": ["id", "title"]
                                    }
                                }
                            },
                            "required": ["id", "title"]
                        }
                    },
                    "id": {
                        "type": "string",
                        "description": "Id da task ou subtask para action=progress."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["open", "in_progress", "done"],
                        "description": "Novo status para action=progress."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens da resposta."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    fn spec_for_mode(&self, mode: AgentMode) -> ToolSpec {
        match mode {
            AgentMode::Execution => self.execution_spec(),
            AgentMode::Sdd => self.spec(),
            AgentMode::Docs => self.spec(),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: SddParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let budget_tokens = params
            .budget_tokens
            .unwrap_or(self.default_budget_tokens)
            .max(1);
        let rk = self.repo_key();

        let plan_action = matches!(&params.action, SddAction::UpsertSpec | SddAction::Close);
        let sections = match params.action {
            SddAction::UpsertSpec => {
                self.require_sdd_mode("upsert_spec")?;
                let slug = match params.slug.as_deref() {
                    Some(s) => validate_slug(s)?,
                    None => self.resolve_target_slug(None)?,
                };
                let existing = self.sdd_store.load(&rk, &slug).map_err(ToolError::Other)?;
                let ts = now();
                let mut plan = match existing {
                    Some(mut prev) => {
                        if prev.status != SddPlanStatus::Draft {
                            return Err(ToolError::Other(format!(
                                "plano '{slug}' já está {:?}; crie outro slug",
                                prev.status
                            )));
                        }
                        if let Some(t) = params.title {
                            prev.title = t;
                        }
                        if let Some(g) = params.goal {
                            prev.goal = g;
                        }
                        if let Some(c) = params.constraints {
                            prev.constraints = c;
                        }
                        if let Some(n) = params.nuances {
                            prev.nuances = n;
                        }
                        prev.updated_at = ts;
                        prev
                    }
                    None => {
                        let title = params
                            .title
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| {
                                ToolError::InvalidArguments("upsert_spec novo exige title".into())
                            })?;
                        let goal = params
                            .goal
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .ok_or_else(|| {
                                ToolError::InvalidArguments("upsert_spec novo exige goal".into())
                            })?;
                        SddPlan {
                            slug: slug.clone(),
                            repo_key: rk.clone(),
                            title,
                            status: SddPlanStatus::Draft,
                            goal,
                            constraints: params.constraints.unwrap_or_default(),
                            nuances: params.nuances.unwrap_or_default(),
                            tasks: Vec::new(),
                            created_at: ts,
                            updated_at: ts,
                            closed_at: None,
                            ..Default::default()
                        }
                    }
                };
                plan.updated_at = ts;
                plan.record_activity("sdd", "spec_upserted", "ok");
                self.sdd_store.save(&plan).map_err(ToolError::Other)?;
                self.sdd_store
                    .set_active_slug(&rk, Some(&slug))
                    .map_err(ToolError::Other)?;
                plan_to_sections(&plan)
            }
            SddAction::SetTasks => {
                self.require_sdd_mode("set_tasks")?;
                let slug = self.resolve_target_slug(params.slug.as_deref())?;
                let mut plan = self
                    .sdd_store
                    .load(&rk, &slug)
                    .map_err(ToolError::Other)?
                    .ok_or_else(|| {
                        ToolError::InvalidArguments(format!("plano '{slug}' não encontrado"))
                    })?;
                if plan.status != SddPlanStatus::Draft {
                    return Err(ToolError::Other(format!(
                        "plano '{slug}' já está fechado; não pode alterar tasks"
                    )));
                }
                let raw_tasks = params.tasks.ok_or_else(|| {
                    ToolError::InvalidArguments("set_tasks exige lista tasks".into())
                })?;
                if raw_tasks.is_empty() {
                    return Err(ToolError::InvalidArguments(
                        "set_tasks exige ao menos 1 task".into(),
                    ));
                }
                plan.tasks = raw_tasks
                    .into_iter()
                    .map(|t| SddTask {
                        id: t.id,
                        title: t.title,
                        details: t.details,
                        nuances: t.nuances,
                        parallel: t.parallel,
                        status: SddTaskStatus::Open,
                        subtasks: t
                            .subtasks
                            .into_iter()
                            .map(|s| SddSubtask {
                                id: s.id,
                                title: s.title,
                                acceptance: s.acceptance,
                                touches: s.touches,
                                acceptance_test: s.acceptance_test,
                                status: SddSubtaskStatus::Open,
                            })
                            .collect(),
                    })
                    .collect();
                plan.updated_at = now();
                plan.record_activity("sdd", "tasks_set", "ok");
                self.sdd_store.save(&plan).map_err(ToolError::Other)?;
                plan_to_sections(&plan)
            }
            SddAction::Status => {
                let slug = self.resolve_target_slug(params.slug.as_deref())?;
                let plan = self
                    .sdd_store
                    .load(&rk, &slug)
                    .map_err(ToolError::Other)?
                    .ok_or_else(|| {
                        ToolError::InvalidArguments(format!("plano '{slug}' não encontrado"))
                    })?;
                plan_to_sections(&plan)
            }
            SddAction::List => {
                let plans = self.sdd_store.list(&rk).map_err(ToolError::Other)?;
                let active = self.sdd_store.active_slug(&rk).map_err(ToolError::Other)?;
                let body = if plans.is_empty() {
                    "(nenhum plano SDD salvo neste repositório)".to_string()
                } else {
                    plans
                        .iter()
                        .map(|p| {
                            let mark = if active.as_deref() == Some(p.slug.as_str()) {
                                "*"
                            } else {
                                " "
                            };
                            format!(
                                "- [{mark}] `{}` — {} ({}, {} tasks)",
                                p.slug,
                                p.title,
                                status_label_plan(p.status),
                                p.tasks.len()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                vec![Section {
                    heading: "### sdd — lista de planos".to_string(),
                    body,
                    priority: 255,
                }]
            }
            SddAction::Activate => {
                let slug =
                    validate_slug(params.slug.as_deref().ok_or_else(|| {
                        ToolError::InvalidArguments("activate exige slug".into())
                    })?)?;
                let plan = self
                    .sdd_store
                    .load(&rk, &slug)
                    .map_err(ToolError::Other)?
                    .ok_or_else(|| {
                        ToolError::InvalidArguments(format!("plano '{slug}' não encontrado"))
                    })?;
                self.sdd_store
                    .set_active_slug(&rk, Some(&slug))
                    .map_err(ToolError::Other)?;
                plan_to_sections(&plan)
            }
            SddAction::Close => {
                self.require_sdd_mode("close")?;
                let slug = self.resolve_target_slug(params.slug.as_deref())?;
                let mut plan = self
                    .sdd_store
                    .load(&rk, &slug)
                    .map_err(ToolError::Other)?
                    .ok_or_else(|| {
                        ToolError::InvalidArguments(format!("plano '{slug}' não encontrado"))
                    })?;
                if plan.status != SddPlanStatus::Draft {
                    return Err(ToolError::Other(format!(
                        "plano '{slug}' não é um rascunho aberto"
                    )));
                }
                if plan.tasks.is_empty() {
                    return Err(ToolError::InvalidArguments(
                        "não é possível fechar SDD sem ao menos 1 task".into(),
                    ));
                }
                let ts = now();
                plan.status = SddPlanStatus::Closed;
                plan.closed_at = Some(ts);
                plan.updated_at = ts;
                plan.record_activity("sdd", "spec_closed", "ok");
                self.sdd_store.save(&plan).map_err(ToolError::Other)?;
                self.sdd_store
                    .set_active_slug(&rk, Some(&slug))
                    .map_err(ToolError::Other)?;
                let mut sections = vec![Section {
                    heading: format!("### sdd — spec '{}' fechada", plan.slug),
                    body: format!(
                        "- status: closed\n- tasks: {}\n- pasta: .tupi/sdd/{}/",
                        plan.tasks.len(),
                        plan.slug
                    ),
                    priority: 255,
                }];
                if let Some(executor) = &self.executor {
                    let body = match (executor)(&plan) {
                        Ok(report) => report,
                        Err(error) => format!("execução SDD falhou: {error}"),
                    };
                    sections.push(Section {
                        heading: "### sdd — execução".to_string(),
                        body,
                        priority: 255,
                    });
                }
                sections
            }
            SddAction::Note => {
                let slug = self.resolve_target_slug(params.slug.as_deref())?;
                let text = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("note exige text".into()))?;
                let mut plan = self
                    .sdd_store
                    .load(&rk, &slug)
                    .map_err(ToolError::Other)?
                    .ok_or_else(|| {
                        ToolError::InvalidArguments(format!("plano '{slug}' não encontrado"))
                    })?;
                plan.add_note("agent", text);
                plan.updated_at = now();
                self.sdd_store.save(&plan).map_err(ToolError::Other)?;
                plan_to_sections(&plan)
            }
            SddAction::Execute => {
                self.require_execution_mode("execute")?;
                let slug = self.resolve_target_slug(params.slug.as_deref())?;
                let plan = self
                    .sdd_store
                    .load(&rk, &slug)
                    .map_err(ToolError::Other)?
                    .ok_or_else(|| {
                        ToolError::InvalidArguments(format!("plano '{slug}' não encontrado"))
                    })?;
                if plan.status == SddPlanStatus::Draft {
                    return Err(ToolError::Other(
                        "feche o plano SDD antes de executar".into(),
                    ));
                }
                let executor = self.executor.as_ref().ok_or_else(|| {
                    ToolError::Other("execução SDD não configurada neste processo".into())
                })?;
                let body = match (executor)(&plan) {
                    Ok(report) => report,
                    Err(error) => format!("execução SDD falhou: {error}"),
                };
                vec![Section {
                    heading: "### sdd — execução".to_string(),
                    body,
                    priority: 255,
                }]
            }
            SddAction::Progress => {
                self.require_execution_mode("progress")?;
                let slug = self.resolve_target_slug(params.slug.as_deref())?;
                let mut plan = self
                    .sdd_store
                    .load(&rk, &slug)
                    .map_err(ToolError::Other)?
                    .ok_or_else(|| {
                        ToolError::InvalidArguments(format!("plano '{slug}' não encontrado"))
                    })?;
                if plan.status == SddPlanStatus::Draft {
                    return Err(ToolError::Other(
                        "feche o plano SDD antes de atualizar progresso".into(),
                    ));
                }
                let target_id = params
                    .id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("progress exige id".into()))?;
                let status_str = params.status.as_deref().unwrap_or("done");

                let mut matched = false;
                for task in &mut plan.tasks {
                    if task.id == target_id {
                        task.status = match status_str {
                            "open" => SddTaskStatus::Open,
                            "in_progress" => SddTaskStatus::InProgress,
                            "done" => SddTaskStatus::Done,
                            other => {
                                return Err(ToolError::InvalidArguments(format!(
                                    "status inválido: {other}"
                                )));
                            }
                        };
                        if task.status == SddTaskStatus::Done {
                            for sub in &mut task.subtasks {
                                sub.status = SddSubtaskStatus::Done;
                            }
                        }
                        matched = true;
                        break;
                    }
                    for sub in &mut task.subtasks {
                        if sub.id == target_id {
                            sub.status = match status_str {
                                "open" => SddSubtaskStatus::Open,
                                "done" => SddSubtaskStatus::Done,
                                other => {
                                    return Err(ToolError::InvalidArguments(format!(
                                        "status inválido para subtask: {other}"
                                    )));
                                }
                            };
                            matched = true;
                            break;
                        }
                    }
                    if matched {
                        if !task.subtasks.is_empty() {
                            if task
                                .subtasks
                                .iter()
                                .all(|s| s.status == SddSubtaskStatus::Done)
                            {
                                task.status = SddTaskStatus::Done;
                            } else if task.status == SddTaskStatus::Open
                                && task
                                    .subtasks
                                    .iter()
                                    .any(|s| s.status == SddSubtaskStatus::Done)
                            {
                                task.status = SddTaskStatus::InProgress;
                            }
                        }
                        break;
                    }
                }
                if !matched {
                    return Err(ToolError::InvalidArguments(format!(
                        "id '{target_id}' não encontrado no plano '{slug}'"
                    )));
                }
                let all_done = !plan.tasks.is_empty()
                    && plan.tasks.iter().all(|t| t.status == SddTaskStatus::Done);
                if all_done && self.executor.is_none() {
                    plan.status = SddPlanStatus::Completed;
                }
                plan.updated_at = now();
                plan.record_activity("sdd", "progress_updated", "ok");
                self.sdd_store.save(&plan).map_err(ToolError::Other)?;
                let mut sections = plan_to_sections(&plan);
                if all_done {
                    if let Some(executor) = &self.executor {
                        let body = match (executor)(&plan) {
                            Ok(report) => report,
                            Err(error) => format!("fechamento SDD falhou: {error}"),
                        };
                        let latest = self
                            .sdd_store
                            .load(&rk, &slug)
                            .map_err(ToolError::Other)?
                            .unwrap_or(plan);
                        sections = plan_to_sections(&latest);
                        sections.push(Section {
                            heading: "### sdd — fechamento".to_string(),
                            body,
                            priority: 255,
                        });
                    }
                }
                sections
            }
        };

        let mut out = self.render_budgeted(sections, budget_tokens);
        if plan_action && self.docs_enabled && !has_doc_citation(&out) {
            out.push_str("\n> aviso: nenhuma fonte de documentação citada; consulte docs.\n");
        }
        Ok(out)
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let action = arguments
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("?");
        match arguments.get("slug").and_then(Value::as_str) {
            Some(slug) => format!("{action} {slug}"),
            None => action.to_string(),
        }
    }

    fn mutates(&self) -> bool {
        true
    }

    fn mode_mask(&self) -> tupi_kernel::ModeMask {
        tupi_kernel::ModeMask::EXECUTION.union(tupi_kernel::ModeMask::SDD)
    }
}

mod meta;
mod render;
mod store;

pub use store::FsSddStore;

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU8;
    use tempfile::tempdir;
    use tupi_kernel::{AgentMode, Tool};

    use super::*;

    #[test]
    fn upsert_spec_and_set_tasks_append_activities() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let tool = SddTool::new(
            dir.path().to_path_buf(),
            store.clone(),
            Some(Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()))),
            2000,
        );
        tool.call(serde_json::json!({"action":"upsert_spec","slug":"a","title":"A","goal":"G"}))
            .unwrap();
        tool.call(serde_json::json!({"action":"set_tasks","tasks":[{"id":"T1","title":"T"}]}))
            .unwrap();
        let plan = store
            .load(&crate::paths::repo_key(dir.path()), "a")
            .unwrap()
            .unwrap();
        assert_eq!(
            plan.activities
                .iter()
                .map(|activity| activity.action.as_str())
                .collect::<Vec<_>>(),
            ["spec_upserted", "tasks_set"]
        );
    }

    #[test]
    fn a_note_goes_to_notes_and_not_activities() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let tool = SddTool::new(dir.path().to_path_buf(), store.clone(), None, 2000);
        tool.call(serde_json::json!({"action":"upsert_spec","slug":"a","title":"A","goal":"G"}))
            .unwrap();
        tool.call(serde_json::json!({"action":"note","slug":"a","text":"decisão"}))
            .unwrap();
        let plan = store
            .load(&crate::paths::repo_key(dir.path()), "a")
            .unwrap()
            .unwrap();
        assert_eq!(plan.notes.len(), 1);
        assert_eq!(plan.notes[0].text, "decisão");
        assert!(
            !plan
                .activities
                .iter()
                .any(|activity| activity.action == "note")
        );
    }

    #[test]
    fn a_new_plan_records_monotonic_activities_and_notes() {
        let mut plan = SddPlan {
            slug: "auth".into(),
            title: "Auth".into(),
            goal: "OAuth".into(),
            ..Default::default()
        };
        plan.record_activity("sdd", "spec_upserted", "ok");
        plan.record_activity("sdd", "tasks_set", "ok");
        plan.add_note("agent", "lock mantido");
        assert_eq!(plan.activities.len(), 2);
        assert_eq!(plan.activities[0].seq, 1);
        assert_eq!(plan.activities[1].seq, 2);
        assert_eq!(plan.notes[0].text, "lock mantido");
        assert_eq!(status_label_plan(SddPlanStatus::Failed), "failed");
    }

    #[test]
    fn execution_spec_lists_only_progress_actions() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let tool = SddTool::new(dir.path().to_path_buf(), store, None, 2000);

        let exec = tool.spec_for_mode(AgentMode::Execution);
        let actions: Vec<&str> = exec.schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect();
        assert_eq!(
            actions,
            vec!["status", "list", "activate", "progress", "execute"]
        );
        assert!(exec.schema["properties"].get("tasks").is_none());

        let sdd = tool.spec_for_mode(AgentMode::Sdd);
        let actions: Vec<&str> = sdd.schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect();
        assert!(actions.contains(&"upsert_spec"));
        assert!(sdd.schema["properties"].get("tasks").is_some());
    }

    #[test]
    fn sdd_upsert_spec_set_tasks_and_writes_markdown_mirror() {
        let dir = tempdir().unwrap();
        let store = Arc::new(FsSddStore::new(dir.path()));
        let mode_cell = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
        let tool = SddTool::new(dir.path().to_path_buf(), store, Some(mode_cell), 2000);

        let out = tool
            .call(serde_json::json!({
                "action": "upsert_spec",
                "slug": "auth-flow",
                "title": "Fluxo de Autenticação",
                "goal": "Suportar OAuth com refresh",
                "constraints": ["Não quebrar CLI antiga"],
                "nuances": ["Token expira em 55m"]
            }))
            .unwrap();
        assert!(out.contains("auth-flow"), "{out}");

        tool.call(serde_json::json!({
            "action": "set_tasks",
            "tasks": [
                {
                    "id": "T1",
                    "title": "Refresh token",
                    "details": "Renovar antes de 55m",
                    "nuances": ["Evitar race condition"],
                    "subtasks": [
                        { "id": "T1.1", "title": "Lock no store", "acceptance": "Teste concorrente passa" }
                    ]
                }
            ]
        }))
        .unwrap();

        let spec = std::fs::read_to_string(dir.path().join(".tupi/sdd/auth-flow/spec.md")).unwrap();
        assert!(spec.contains("Gerado automaticamente"), "{spec}");
        assert!(spec.contains("Fluxo de Autenticação"), "{spec}");
        assert!(spec.contains("Token expira em 55m"), "{spec}");
        let tasks =
            std::fs::read_to_string(dir.path().join(".tupi/sdd/auth-flow/tasks.json")).unwrap();
        assert!(tasks.contains("T1.1"), "{tasks}");
        assert!(tasks.contains("Lock no store"), "{tasks}");
    }

    #[test]
    fn sdd_close_requires_at_least_one_task_and_active_draft() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mode_cell = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
        let tool = SddTool::new(dir.path().to_path_buf(), store, Some(mode_cell), 2000);

        assert!(tool.call(serde_json::json!({ "action": "close" })).is_err());

        tool.call(serde_json::json!({
            "action": "upsert_spec",
            "slug": "empty-tasks",
            "title": "Vazio",
            "goal": "Testar validação"
        }))
        .unwrap();
        assert!(tool.call(serde_json::json!({ "action": "close" })).is_err());

        tool.call(serde_json::json!({
            "action": "set_tasks",
            "tasks": [{ "id": "T1", "title": "Implementar", "details": "Detalhe", "subtasks": [] }]
        }))
        .unwrap();
        let closed = tool.call(serde_json::json!({ "action": "close" })).unwrap();
        assert!(
            closed.contains("closed") || closed.contains("fechada"),
            "{closed}"
        );
    }

    #[test]
    fn progress_runs_the_executor_when_all_tasks_are_done() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mode_cell = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
        let mut tool = SddTool::new(
            dir.path().to_path_buf(),
            store.clone(),
            Some(mode_cell.clone()),
            2000,
        );
        tool.call(serde_json::json!({
            "action": "upsert_spec", "slug": "rag", "title": "RAG", "goal": "G"
        }))
        .unwrap();
        tool.call(serde_json::json!({
            "action": "set_tasks",
            "tasks": [{
                "id": "T1", "title": "T",
                "subtasks": [{ "id": "T1.1", "title": "S", "acceptance": "A" }]
            }]
        }))
        .unwrap();
        tool.call(serde_json::json!({ "action": "close" })).unwrap();
        mode_cell.store(
            AgentMode::Execution.as_u8(),
            std::sync::atomic::Ordering::SeqCst,
        );

        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        let saved = store.clone();
        tool.executor = Some(Arc::new(move |plan: &SddPlan| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut plan = plan.clone();
            plan.status = SddPlanStatus::Completed;
            saved.save(&plan).unwrap();
            Ok("docs ok".into())
        }));

        let out = tool
            .call(serde_json::json!({
                "action": "progress", "id": "T1.1", "status": "done"
            }))
            .unwrap();

        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        let saved = store
            .load(&crate::paths::repo_key(dir.path()), "rag")
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, SddPlanStatus::Completed);
        assert!(out.contains("fechamento"), "{out}");
    }

    #[test]
    fn sdd_progress_updates_subtask_and_completes_plan() {
        let dir = tempdir().unwrap();
        let store = Arc::new(FsSddStore::new(dir.path()));
        let mode_cell = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
        let tool = SddTool::new(
            dir.path().to_path_buf(),
            store.clone(),
            Some(mode_cell.clone()),
            2000,
        );

        tool.call(serde_json::json!({
            "action": "upsert_spec",
            "slug": "rag",
            "title": "RAG Docs",
            "goal": "Indexar docs"
        }))
        .unwrap();
        tool.call(serde_json::json!({
            "action": "set_tasks",
            "tasks": [{
                "id": "T1",
                "title": "Chunking",
                "details": "Fatiar markdown",
                "subtasks": [{ "id": "T1.1", "title": "Parser", "acceptance": "Passa" }]
            }]
        }))
        .unwrap();
        tool.call(serde_json::json!({ "action": "close" })).unwrap();
        mode_cell.store(
            AgentMode::Execution.as_u8(),
            std::sync::atomic::Ordering::SeqCst,
        );

        tool.call(serde_json::json!({
            "action": "progress",
            "id": "T1.1",
            "status": "done"
        }))
        .unwrap();

        let tasks = std::fs::read_to_string(dir.path().join(".tupi/sdd/rag/tasks.json")).unwrap();
        assert!(tasks.contains("\"id\": \"T1.1\""), "{tasks}");
        assert!(tasks.contains("\"status\": \"done\""), "{tasks}");
        let saved = store
            .load(&crate::paths::repo_key(dir.path()), "rag")
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, SddPlanStatus::Completed);
    }

    #[test]
    fn sdd_close_with_explicit_slug_updates_active_slug() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let mode_cell = Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()));
        let tool = SddTool::new(
            dir.path().to_path_buf(),
            store.clone(),
            Some(mode_cell),
            2000,
        );

        tool.call(serde_json::json!({
            "action": "upsert_spec",
            "slug": "first",
            "title": "First",
            "goal": "One"
        }))
        .unwrap();
        tool.call(serde_json::json!({
            "action": "set_tasks",
            "tasks": [{ "id": "T1", "title": "Task", "details": "" }]
        }))
        .unwrap();
        tool.call(serde_json::json!({
            "action": "upsert_spec",
            "slug": "second",
            "title": "Second",
            "goal": "Two"
        }))
        .unwrap();

        // Active is "second", but we close "first" explicitly
        tool.call(serde_json::json!({ "action": "close", "slug": "first" }))
            .unwrap();
        assert_eq!(
            store
                .active_slug(&crate::paths::repo_key(dir.path()))
                .unwrap()
                .as_deref(),
            Some("first")
        );
    }
    #[test]
    fn plan_without_citation_warns_when_docs_is_enabled() {
        let dir = tempdir().unwrap();
        let store = Arc::new(InMemorySddStore::default());
        let tool = SddTool::new(
            dir.path().to_path_buf(),
            store,
            Some(Arc::new(AtomicU8::new(AgentMode::Sdd.as_u8()))),
            2000,
        )
        .with_docs(true);

        let out = tool
            .call(serde_json::json!({
                "action": "upsert_spec",
                "slug": "cobranca",
                "title": "Cobranca",
                "goal": "Refazer a cobranca"
            }))
            .unwrap();

        assert!(out.contains("aviso"), "{out}");
        assert!(has_doc_citation("docs/cobranca.md:10-20"));
        assert!(!has_doc_citation("sem fonte"));
    }
}
