use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::facts::{default_state_dir, search_facts};
use crate::store::ResponseStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StateAction {
    Resume,
    Status,
    Memory,
    Note,
    Task,
    End,
}

#[derive(Debug, Deserialize)]
pub struct StateParams {
    pub action: StateAction,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    kind: String,
    #[serde(default)]
    id: Option<String>,
    text: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    symbol: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    at: u64,
}

pub struct StateTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    state_dir: PathBuf,
    default_budget_tokens: usize,
}

fn state_path(state_dir: &Path, root: &Path) -> PathBuf {
    state_dir.join(format!("state-{}.jsonl", crate::index::repo_key(root)))
}

fn read_entries(path: &Path) -> Vec<Entry> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .collect()
}

fn write_entries(path: &Path, entries: &[Entry]) -> Result<(), ToolError> {
    let mut body = String::new();
    for entry in entries {
        let line = serde_json::to_string(entry)
            .map_err(|error| ToolError::Other(format!("falha ao serializar estado: {error}")))?;
        body.push_str(&line);
        body.push('\n');
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            ToolError::Other(format!("falha ao criar o diretório de estado: {error}"))
        })?;
    }
    crate::paths::atomic_write(path, body.as_bytes())
        .map_err(|error| ToolError::Other(format!("falha ao gravar estado: {error}")))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

impl StateTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            state_dir: default_state_dir(),
            default_budget_tokens,
        }
    }

    #[cfg(test)]
    fn with_state_dir(root_dir: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            root_dir,
            store: Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            state_dir,
            default_budget_tokens: 4000,
        }
    }

    fn root(&self) -> PathBuf {
        self.root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone())
    }

    fn path(&self) -> PathBuf {
        state_path(&self.state_dir, &self.root())
    }
}

fn render_entries(entries: &[Entry]) -> String {
    let mut body = String::new();
    let open: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.kind == "task" && entry.status != "done")
        .collect();
    body.push_str(&format!("\n- tarefas abertas: {}\n", open.len()));
    for entry in open.iter().take(10) {
        let id = entry.id.as_deref().unwrap_or("-");
        body.push_str(&format!("  - [{id}] {}\n", entry.text));
    }
    let notes: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.kind == "note")
        .collect();
    body.push_str(&format!("- notas: {}\n", notes.len()));
    for entry in notes.iter().rev().take(8) {
        let anchor = entry
            .symbol
            .as_deref()
            .or(entry.path.as_deref())
            .map(|anchor| format!(" ({anchor})"))
            .unwrap_or_default();
        body.push_str(&format!("  - {}{}\n", entry.text, anchor));
    }
    body
}

impl Tool for StateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "state".to_string(),
            description: "Estado local da sessão por repositório: retoma o que foi feito, registra notas e tarefas e consulta a memória durável.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["resume", "status", "memory", "note", "task", "end"],
                        "description": "resume/status mostram o estado; note/task gravam; memory consulta o ledger; end fecha."
                    },
                    "text": {
                        "type": "string",
                        "description": "Texto da nota/tarefa ou, em memory, os termos da busca."
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Rótulos da nota."
                    },
                    "path": {
                        "type": "string",
                        "description": "Arquivo ao qual a nota se ancora."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Símbolo ao qual a nota se ancora."
                    },
                    "task_id": {
                        "type": "string",
                        "description": "Id estável para atualizar a mesma tarefa."
                    },
                    "status": {
                        "type": "string",
                        "enum": ["open", "done"],
                        "description": "Status da tarefa; o padrão é open."
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
        let params: StateParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let path = self.path();
        let mut entries = read_entries(&path);

        let (heading, body) = match params.action {
            StateAction::Note => {
                let text = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("note exige text".to_string()))?;
                entries.push(Entry {
                    kind: "note".to_string(),
                    id: None,
                    text: text.to_string(),
                    path: params.path,
                    symbol: params.symbol,
                    tags: params.tags.unwrap_or_default(),
                    status: String::new(),
                    at: now(),
                });
                write_entries(&path, &entries)?;
                ("### state — nota".to_string(), format!("\n- {text}\n"))
            }
            StateAction::Task => {
                let text = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("task exige text".to_string()))?;
                let status = params.status.unwrap_or_else(|| "open".to_string());
                let id = params.task_id.unwrap_or_else(|| format!("t{}", now()));
                if let Some(existing) = entries
                    .iter_mut()
                    .find(|entry| entry.kind == "task" && entry.id.as_deref() == Some(id.as_str()))
                {
                    existing.text = text.to_string();
                    existing.status = status.clone();
                } else {
                    entries.push(Entry {
                        kind: "task".to_string(),
                        id: Some(id.clone()),
                        text: text.to_string(),
                        path: params.path,
                        symbol: params.symbol,
                        tags: params.tags.unwrap_or_default(),
                        status: status.clone(),
                        at: now(),
                    });
                }
                write_entries(&path, &entries)?;
                (
                    "### state — tarefa".to_string(),
                    format!("\n- [{id}] {text} ({status})\n"),
                )
            }
            StateAction::Status => ("### state — status".to_string(), render_entries(&entries)),
            StateAction::Resume => ("### state — retomada".to_string(), render_entries(&entries)),
            StateAction::Memory => {
                let query = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("memory exige text".to_string()))?;
                let facts = search_facts(&self.state_dir, &self.root(), query);
                let mut body = String::new();
                if facts.is_empty() {
                    body.push_str("\nNÃO — nenhum fato corresponde.\n");
                }
                for fact in facts.iter().rev().take(10) {
                    let text = fact.get("text").and_then(Value::as_str).unwrap_or_default();
                    body.push_str(&format!("- {text}\n"));
                }
                ("### state — memória".to_string(), body)
            }
            StateAction::End => (
                "### state — fim".to_string(),
                "\nSessão encerrada; nada foi apagado.\n".to_string(),
            ),
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
        format!("state: {action}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_appears_in_the_status() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let tool = StateTool::with_state_dir(root.path().to_path_buf(), state.path().to_path_buf());
        Tool::call(
            &tool,
            serde_json::json!({"action": "note", "text": "o edit não grava no ledger", "symbol": "edit"}),
        )
        .unwrap();
        let status = Tool::call(&tool, serde_json::json!({"action": "status"})).unwrap();
        assert!(status.contains("edit não grava"));
        assert!(status.contains("(edit)"));
    }

    #[test]
    fn a_task_can_be_closed_by_id() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let tool = StateTool::with_state_dir(root.path().to_path_buf(), state.path().to_path_buf());
        Tool::call(
            &tool,
            serde_json::json!({"action": "task", "task_id": "t1", "text": "fazer exec"}),
        )
        .unwrap();
        Tool::call(
            &tool,
            serde_json::json!({"action": "task", "task_id": "t1", "text": "fazer exec", "status": "done"}),
        )
        .unwrap();
        let status = Tool::call(&tool, serde_json::json!({"action": "status"})).unwrap();
        assert!(status.contains("tarefas abertas: 0"));
    }

    #[test]
    fn note_requires_text() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let tool = StateTool::with_state_dir(root.path().to_path_buf(), state.path().to_path_buf());
        assert!(Tool::call(&tool, serde_json::json!({"action": "note"})).is_err());
    }
}
