use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;
use crate::web;

#[derive(Debug, Deserialize)]
pub struct ResearchParams {
    pub query: String,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default = "default_sources")]
    pub max_sources: usize,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_sources() -> usize {
    3
}

pub struct ResearchTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl ResearchTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for ResearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "research".to_string(),
            description: "Busca, lê as melhores fontes e devolve a evidência numa chamada, com URL e âncora para citar.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Consulta de busca."
                    },
                    "question": {
                        "type": "string",
                        "description": "O que você quer saber. Ranqueia os blocos de cada fonte; sem ela, a própria query é usada."
                    },
                    "max_sources": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 6,
                        "default": 3,
                        "description": "Quantas fontes baixar e ler."
                    },
                    "locale": {
                        "type": "string",
                        "description": "Região preferida, ex: pt-BR."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: ResearchParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let query = params.query.trim();
        if query.is_empty() {
            return Err(ToolError::InvalidArguments("query vazia".to_string()));
        }
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let hits = crate::websearch::search_hits(query, params.locale.as_deref(), &root)?;
        if hits.is_empty() {
            return Ok(format!(
                "### research — {query}\n\nNÃO — nenhum resultado.\n"
            ));
        }

        let question = params
            .question
            .as_deref()
            .map(str::trim)
            .filter(|question| !question.is_empty())
            .unwrap_or(query);
        let sources = params.max_sources.clamp(1, 6);
        let mut sections = vec![Section {
            heading: format!("### research — {query}"),
            body: format!(
                "\n{} resultado(s); lendo até {sources}.\n",
                hits.len().min(sources)
            ),
            priority: 255,
        }];
        let mut errors = Vec::new();

        for (index, hit) in hits.iter().take(sources).enumerate() {
            let body = match web::curl(&hit.url, &root) {
                Ok(body) => body,
                Err(error) => {
                    errors.push(format!("- {}: {error}", hit.url));
                    continue;
                }
            };
            let lower = body.to_lowercase();
            let is_html = lower.contains("<html")
                || lower.contains("<!doctype html")
                || lower.contains("<body");
            let blocks = if is_html {
                web::html_to_blocks(&body)
            } else {
                web::text_blocks(&body)
            };
            let ranked = web::rank(blocks, Some(question));
            let mut text = format!("\n- {}\n", hit.url);
            if !hit.snippet.is_empty() {
                text.push_str(&format!("- trecho da busca: {}\n\n", hit.snippet));
            }
            let mut shown = 0;
            for block in ranked.into_iter().take(4) {
                text.push_str(&format!(
                    "**[{}]** {}\n\n",
                    block.anchor,
                    crate::exec::clip(&block.text, 1_000, 200)
                ));
                shown += 1;
            }
            if shown == 0 {
                text.push_str("(sem blocos de texto)\n");
            }
            sections.push(Section {
                heading: format!("#### fonte {} — {}", index + 1, hit.title),
                body: text,
                priority: (240 - index as u8 * 5).max(40),
            });
        }

        if !errors.is_empty() {
            sections.push(Section {
                heading: "#### falhas".to_string(),
                body: format!("\n{}\n", errors.join("\n")),
                priority: 60,
            });
        }

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("?");
        format!("research: {query}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_shows_the_query() {
        let tool = ResearchTool::new(
            PathBuf::from("."),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({"query": "atos de fala"})),
            "research: atos de fala"
        );
    }
}
