use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FactAction {
    Record,
    List,
    Search,
}

#[derive(Debug, Deserialize)]
pub struct FactParams {
    pub action: FactAction,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct FactsTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    state_dir: PathBuf,
    default_budget_tokens: usize,
}

pub fn default_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(dir).join("tupi");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/tupi");
    }
    std::env::temp_dir().join("tupi")
}

pub(crate) fn facts_path(state_dir: &Path, root: &Path) -> PathBuf {
    state_dir.join(format!("facts-{}.jsonl", crate::index::repo_key(root)))
}

pub(crate) fn read_facts(state_dir: &Path, root: &Path) -> Vec<Value> {
    let path = facts_path(state_dir, root);
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .collect()
}

pub(crate) fn search_facts(state_dir: &Path, root: &Path, query: &str) -> Vec<Value> {
    let terms: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| term.len() >= 3)
        .map(str::to_string)
        .collect();
    read_facts(state_dir, root)
        .into_iter()
        .filter(|fact| {
            let haystack = format!(
                "{}\n{}",
                fact.get("text").and_then(Value::as_str).unwrap_or_default(),
                fact.get("tags")
                    .and_then(Value::as_array)
                    .map(|tags| tags
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" "))
                    .unwrap_or_default()
            )
            .to_lowercase();
            terms.iter().any(|term| haystack.contains(term))
        })
        .collect()
}

impl FactsTool {
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

    fn path(&self) -> PathBuf {
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        facts_path(&self.state_dir, &root)
    }
}

impl Tool for FactsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ledger".to_string(),
            description:
                "Fatos e decisões duráveis por repositório (record/list/search), sem redescobrir."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["record", "list", "search"],
                        "description": "record grava um fato; list devolve os mais recentes; search filtra por termos."
                    },
                    "text": {
                        "type": "string",
                        "description": "O fato a gravar (record) ou os termos da busca (search)."
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Rótulos opcionais do fato gravado."
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
        let params: FactParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let path = self.path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                ToolError::Other(format!("falha ao criar o diretório do ledger: {error}"))
            })?;
        }

        let (heading, body) = match params.action {
            FactAction::Record => {
                let text = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("record exige text".to_string()))?;
                let fact = json!({
                    "text": text,
                    "tags": params.tags.unwrap_or_default(),
                    "at": now(),
                });
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(|error| {
                        ToolError::Other(format!("falha ao abrir o ledger: {error}"))
                    })?;
                writeln!(file, "{fact}").map_err(|error| {
                    ToolError::Other(format!("falha ao gravar o ledger: {error}"))
                })?;
                ("### ledger — gravado".to_string(), format!("\n- {text}\n"))
            }
            FactAction::List => {
                let facts = read_facts(&self.state_dir, &self.root_dir);
                let mut body = String::new();
                if facts.is_empty() {
                    body.push_str("\n(nenhum fato gravado)\n");
                }
                for fact in facts.iter().rev().take(20) {
                    body.push_str(&render_fact(fact));
                }
                ("### ledger — recentes".to_string(), body)
            }
            FactAction::Search => {
                let query = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("search exige text".to_string()))?;
                let facts = search_facts(&self.state_dir, &self.root_dir, query);
                let mut body = String::new();
                if facts.is_empty() {
                    body.push_str("\nNÃO — nenhum fato corresponde.\n");
                }
                for fact in facts.iter().rev().take(20) {
                    body.push_str(&render_fact(fact));
                }
                ("### ledger — busca".to_string(), body)
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
        format!("ledger: {action}")
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn render_fact(fact: &Value) -> String {
    let text = fact.get("text").and_then(Value::as_str).unwrap_or_default();
    let tags = fact
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if tags.is_empty() {
        format!("- {text}\n")
    } else {
        format!("- [{tags}] {text}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_list_and_search_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let tool = FactsTool::with_state_dir(root.path().to_path_buf(), state.path().to_path_buf());

        Tool::call(
            &tool,
            serde_json::json!({"action": "record", "text": "o orçamento é 3.3 chars/token", "tags": ["token"]}),
        )
        .unwrap();
        let list = Tool::call(&tool, serde_json::json!({"action": "list"})).unwrap();
        assert!(list.contains("3.3 chars/token"));
        assert!(list.contains("[token]"));

        let found = Tool::call(
            &tool,
            serde_json::json!({"action": "search", "text": "orçamento"}),
        )
        .unwrap();
        assert!(found.contains("3.3"));
        let missing = Tool::call(
            &tool,
            serde_json::json!({"action": "search", "text": "inexistente"}),
        )
        .unwrap();
        assert!(missing.contains("NÃO"));
    }

    #[test]
    fn record_requires_text() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let tool = FactsTool::with_state_dir(root.path().to_path_buf(), state.path().to_path_buf());
        assert!(Tool::call(&tool, serde_json::json!({"action": "record"})).is_err());
    }
}
