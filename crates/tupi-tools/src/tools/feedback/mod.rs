//! Tool `feedback`: o modelo registra o próprio atrito em
//! `.agents/tupi/debug/feedback.jsonl`. É log local de depuração, não memória
//! injetada: não entra no índice de espaço, no grafo nem no prompt.

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
pub enum FeedbackAction {
    Record,
    List,
}

/// O tipo do atrito. Enum fechado: o mantenedor agrega por rótulo, então um
/// rótulo livre viraria sopa de strings.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackKind {
    /// A tool que a tarefa pedia não existe.
    MissingTool,
    /// A tool existe e recusou o que era preciso.
    Refusal,
    /// A resposta veio ruim, truncada ou enganosa.
    BadMessage,
    /// O prompt/tool specs custaram caro demais em contexto.
    PromptCost,
    #[default]
    Other,
}

impl FeedbackKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::MissingTool => "missing-tool",
            Self::Refusal => "refusal",
            Self::BadMessage => "bad-message",
            Self::PromptCost => "prompt-cost",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct FeedbackParams {
    pub action: FeedbackAction,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub kind: Option<FeedbackKind>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub workaround: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct FeedbackTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl FeedbackTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for FeedbackTool {
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::Opaque
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "feedback".to_string(),
            description:
                "Registra o atrito do modelo (.agents/tupi/debug/feedback.jsonl): use quando faltar uma tool, quando ela recusar o que a tarefa pedia, quando a mensagem vier ruim ou quando o prompt custar caro. Um registro encerra a pendência de feedback da sessão."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["record", "list"],
                        "description": "record grava um atrito; list devolve os registros da sessão atual."
                    },
                    "text": {
                        "type": "string",
                        "description": "O atrito em uma ou duas frases (obrigatório no record)."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["missing-tool", "refusal", "bad-message", "prompt-cost", "other"],
                        "description": "Que tipo de atrito. O padrão é other."
                    },
                    "tool": {
                        "type": "string",
                        "description": "Tool envolvida, quando houver."
                    },
                    "workaround": {
                        "type": "string",
                        "description": "O caminho que funcionou apesar do atrito."
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
        let params: FeedbackParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let root = &self.root_dir;

        let (heading, body) = match params.action {
            FeedbackAction::Record => {
                let text = params
                    .text
                    .as_deref()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(|| ToolError::InvalidArguments("record exige text".to_string()))?;
                let kind = params.kind.unwrap_or_default();
                let tool = params.tool.unwrap_or_default().trim().to_string();
                let workaround = params
                    .workaround
                    .map(|hint| hint.trim().to_string())
                    .filter(|hint| !hint.is_empty());

                let mut queue = memory::pendings(root);
                let session = current_session(&queue);
                let id = memory::fact_id(text);

                // O mesmo atrito na mesma sessão já está no sink: repetir só
                // engorda o arquivo. A pendência fecha mesmo assim.
                let known = memory::feedback_records(root).into_iter().any(|record| {
                    record.id == id && (session.is_empty() || record.session == session)
                });
                if close_feedback(&mut queue, &session) {
                    let _ = memory::save_pendings(root, &queue);
                }

                if known {
                    ("### feedback — já registrado".to_string(), format!("\n- {id} {text}\n"))
                } else {
                    let feedback = memory::Feedback {
                        id: id.clone(),
                        ts_ms: memory::now(),
                        session,
                        mode: String::new(),
                        model: String::new(),
                        kind: kind.as_str().to_string(),
                        tool,
                        text: text.to_string(),
                        workaround,
                    };
                    memory::append_feedback(root, &feedback).map_err(|error| {
                        ToolError::Other(format!("falha ao gravar o feedback: {error}"))
                    })?;
                    (
                        "### feedback — registrado".to_string(),
                        format!("\n{}\n", render_feedback(&feedback)),
                    )
                }
            }
            FeedbackAction::List => {
                let session = current_session(&memory::pendings(root));
                let records = memory::feedback_records(root);
                let mut body = String::new();
                let mut shown = 0usize;
                for record in records.iter().rev() {
                    if !session.is_empty() && record.session != session {
                        continue;
                    }
                    if shown == 20 {
                        break;
                    }
                    body.push_str(&render_feedback(record));
                    shown += 1;
                }
                if shown == 0 {
                    body.push_str("\n(nenhum atrito registrado)\n");
                }
                ("### feedback — sessão atual".to_string(), body)
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
        format!("feedback: {action}")
    }

    fn mode_mask(&self) -> tupi_kernel::ModeMask {
        // O atrito é o mesmo no SDD e na execução: a tool vale nos dois.
        tupi_kernel::ModeMask::EXECUTION.union(tupi_kernel::ModeMask::SDD)
    }
}

/// A sessão dona da pendência de feedback aberta pelo `begin_turn`: o id da
/// fila carrega dono (`<sessão>:feedback`), então não é preciso acoplar a tool
/// ao store do app só para saber quem está falando.
fn current_session(queue: &[memory::Pending]) -> String {
    let suffix = format!(":{}", memory::FEEDBACK_ID);
    queue
        .iter()
        .rev()
        .find_map(|pending| {
            let session = pending.id.strip_suffix(&suffix)?;
            (!session.is_empty()).then(|| session.to_string())
        })
        .unwrap_or_default()
}

/// Fecha a pendência de feedback da sessão (e a versão sem dono, para o caso de
/// filas antigas). Gravar o atrito é exatamente o que a pendência pedia.
fn close_feedback(queue: &mut Vec<memory::Pending>, session: &str) -> bool {
    let mut ids = vec![memory::FEEDBACK_ID.to_string()];
    if !session.is_empty() {
        ids.push(format!("{session}:{}", memory::FEEDBACK_ID));
    }
    let mut closed = false;
    for id in ids {
        closed |= memory::close_pending(queue, &id, "feedback", "recorded by model");
    }
    closed
}

fn render_feedback(record: &memory::Feedback) -> String {
    let mut line = format!("- {} [{}] {}", record.id, record.kind, record.text);
    if !record.tool.is_empty() {
        line.push_str(&format!(" (tool: {})", record.tool));
    }
    if let Some(workaround) = record.workaround.as_deref() {
        line.push_str(&format!(" → {workaround}"));
    }
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn tool(root: &std::path::Path) -> FeedbackTool {
        FeedbackTool::new(
            root.to_path_buf(),
            Arc::new(ResponseStore::new(std::time::Duration::from_secs(60), 1024 * 1024)),
            1000,
        )
    }

    #[test]
    fn record_requires_text() {
        let dir = tempfile::tempdir().unwrap();
        let error = tool(dir.path())
            .call(serde_json::json!({ "action": "record" }))
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn record_appends_and_closes_the_feedback_pending() {
        let dir = tempfile::tempdir().unwrap();
        let id = format!("s1:{}", memory::FEEDBACK_ID);
        let mut queue = memory::pendings(dir.path());
        memory::open_pending(&mut queue, &id, memory::FEEDBACK_KIND, "s1");
        memory::save_pendings(dir.path(), &queue).unwrap();

        let out = tool(dir.path())
            .call(serde_json::json!({
                "action": "record",
                "text": "shell recusou grep",
                "kind": "refusal",
                "tool": "shell",
                "workaround": "search"
            }))
            .unwrap();
        assert!(out.contains("feedback — registrado"), "{out}");
        assert!(out.contains("shell recusou grep"), "{out}");
        assert!(out.contains("(tool: shell)"), "{out}");

        let records = memory::feedback_records(dir.path());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session, "s1");
        assert_eq!(records[0].kind, "refusal");
        assert_eq!(records[0].workaround.as_deref(), Some("search"));
        assert!(records[0].ts_ms > 0);
        assert!(!memory::is_open(&memory::pendings(dir.path()), &id));
    }

    #[test]
    fn record_closes_only_the_session_feedback_pending() {
        let dir = tempfile::tempdir().unwrap();
        let read = format!("s1:{}", memory::READ_ID);
        let feedback = format!("s1:{}", memory::FEEDBACK_ID);
        let mut queue = memory::pendings(dir.path());
        memory::open_pending(&mut queue, &read, memory::READ_KIND, "s1");
        memory::open_pending(&mut queue, &feedback, memory::FEEDBACK_KIND, "s1");
        memory::save_pendings(dir.path(), &queue).unwrap();

        tool(dir.path())
            .call(serde_json::json!({ "action": "record", "text": "recusa do shell" }))
            .unwrap();

        // A divida de feedback desta sessao esta paga; a de leitura segue de pe.
        let queue = memory::pendings(dir.path());
        assert!(!memory::is_open(&queue, &feedback));
        assert!(memory::is_open(&queue, &read));
    }

    #[test]
    fn the_same_atrito_is_not_recorded_twice() {
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({ "action": "record", "text": "prompt grande demais" });
        tool(dir.path()).call(args.clone()).unwrap();
        let out = tool(dir.path()).call(args).unwrap();
        assert!(out.contains("já registrado"), "{out}");
        assert_eq!(memory::feedback_records(dir.path()).len(), 1);
    }

    #[test]
    fn list_shows_only_the_current_session() {
        let dir = tempfile::tempdir().unwrap();
        memory::append_feedback(
            dir.path(),
            &memory::Feedback {
                text: "atrito da s1".into(),
                session: "s1".into(),
                ..memory::Feedback::default()
            },
        )
        .unwrap();
        memory::append_feedback(
            dir.path(),
            &memory::Feedback {
                text: "atrito da s2".into(),
                session: "s2".into(),
                ..memory::Feedback::default()
            },
        )
        .unwrap();

        let id = format!("s2:{}", memory::FEEDBACK_ID);
        let mut queue = memory::pendings(dir.path());
        memory::open_pending(&mut queue, &id, memory::FEEDBACK_KIND, "s2");
        memory::save_pendings(dir.path(), &queue).unwrap();

        let out = tool(dir.path())
            .call(serde_json::json!({ "action": "list" }))
            .unwrap();
        assert!(out.contains("atrito da s2"), "{out}");
        assert!(!out.contains("atrito da s1"), "{out}");
        // list não mexe na fila de pendências
        assert!(memory::is_open(&memory::pendings(dir.path()), &id));
    }
}
