//! Tool `ledger`: memória durável por repositório em `.agents/tupi/facts.jsonl`.

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tupi_domain::memory;
use tupi_kernel::{Tool, ToolError, ToolSpec};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FactAction {
    Record,
    List,
    Search,
    Forget,
}

#[derive(Debug, Deserialize)]
pub struct FactParams {
    pub action: FactAction,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct FactsTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl FactsTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for FactsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "ledger".to_string(),
            description:
                "Memória durável do repositório (.agents/tupi/facts.jsonl): record, list, search, forget. Grava o que não deve ser redescoberto."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["record", "list", "search", "forget"],
                        "description": "record grava um fato; list devolve os mais recentes; search filtra por termos; forget esquece por id."
                    },
                    "text": {
                        "type": "string",
                        "description": "O fato a gravar (record) ou os termos da busca (search)."
                    },
                    "id": {
                        "type": "string",
                        "description": "Id do fato a esquecer (forget)."
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
        let root = &self.root_dir;

        let (heading, body) = match params.action {
            FactAction::Record => {
                let text = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("record exige text".to_string()))?;
                let tags = params.tags.unwrap_or_default();
                let saved = memory::record(root, text, tags, "model").map_err(|error| {
                    ToolError::Other(format!("falha ao gravar o ledger: {error}"))
                })?;
                match saved {
                    Some(fact) => {
                        // Quem gravou um fato já devolveu o que a sessão aprendeu.
                        let mut queue = memory::pendings(root);
                        if memory::close_pending(
                            &mut queue,
                            memory::FEEDBACK_ID,
                            "ledger",
                            "recorded by model",
                        ) {
                            let _ = memory::save_pendings(root, &queue);
                        }
                        (
                            "### ledger — gravado".to_string(),
                            format!("\n- {} {}\n", fact.id, fact.text),
                        )
                    }
                    None => (
                        "### ledger — já sabido".to_string(),
                        format!("\n- {text}\n"),
                    ),
                }
            }
            FactAction::List => {
                let facts = memory::facts(root);
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
                let scored = memory::score(root, query, 10);
                if scored.is_empty() {
                    (
                        "### ledger — busca".to_string(),
                        "\nNÃO — nenhum fato corresponde.\n".to_string(),
                    )
                } else {
                    let mut body = String::new();
                    for hit in scored {
                        body.push_str(&render_fact(&hit.fact));
                    }
                    ("### ledger — busca".to_string(), body)
                }
            }
            FactAction::Forget => {
                let id = params
                    .id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("forget exige id".to_string()))?;
                let forgotten = memory::forget(root, id).map_err(|error| {
                    ToolError::Other(format!("falha ao esquecer o fato: {error}"))
                })?;
                if forgotten {
                    ("### ledger — esquecido".to_string(), format!("\n- {id}\n"))
                } else {
                    (
                        "### ledger — não encontrado".to_string(),
                        format!("\n- {id}\n"),
                    )
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
        format!("ledger: {action}")
    }
}

fn render_fact(fact: &memory::Fact) -> String {
    let tags = fact.tags.join(", ");
    if tags.is_empty() {
        format!("- {} {}\n", fact.id, fact.text)
    } else {
        format!("- {} [{}] {}\n", fact.id, tags, fact.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(root: &std::path::Path) -> FactsTool {
        let store = Arc::new(ResponseStore::new(
            std::time::Duration::from_secs(60),
            1024 * 1024,
        ));
        FactsTool::new(root.to_path_buf(), store, 4000)
    }

    #[test]
    fn record_list_and_search_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let tool = tool(root.path());

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
        let tool = tool(root.path());
        assert!(Tool::call(&tool, serde_json::json!({"action": "record"})).is_err());
    }

    #[test]
    fn facts_live_under_the_agents_dir_and_forget_blocks_relearn() {
        let root = tempfile::tempdir().unwrap();
        let tool = tool(root.path());
        let saved = Tool::call(
            &tool,
            serde_json::json!({"action": "record", "text": "o watcher usa notify"}),
        )
        .unwrap();
        assert!(memory::facts_path(root.path()).ends_with(".agents/tupi/facts.jsonl"));
        assert!(memory::facts_path(root.path()).exists());

        let id = memory::facts(root.path())[0].id.clone();
        assert!(saved.contains(&id));

        Tool::call(&tool, serde_json::json!({"action": "forget", "id": id})).unwrap();
        assert!(memory::facts(root.path()).is_empty());
        let again = Tool::call(
            &tool,
            serde_json::json!({"action": "record", "text": "o watcher usa notify"}),
        )
        .unwrap();
        assert!(again.contains("já sabido"));
    }
}
