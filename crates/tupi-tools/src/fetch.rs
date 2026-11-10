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
pub struct FetchParams {
    pub url: String,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default = "default_blocks")]
    pub max_blocks: usize,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_blocks() -> usize {
    80
}

pub struct FetchTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl FetchTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for FetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fetch".to_string(),
            description: "Baixa uma URL e devolve só os blocos relevantes, verbatim, com âncora blk_xxxxxxxx; o excedente fica em peek.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL http(s) absoluta a ler."
                    },
                    "question": {
                        "type": "string",
                        "description": "O que você quer saber. Os blocos são ranqueados contra isso; sem pergunta, a ordem do documento é a resposta."
                    },
                    "max_blocks": {
                        "type": "integer",
                        "default": 80,
                        "description": "Máximo de blocos considerados antes do orçamento."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["url"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: FetchParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let body = web::curl(params.url.trim(), &root)?;
        let looks_html = {
            let lower = body.to_lowercase();
            lower.contains("<html") || lower.contains("<!doctype html") || lower.contains("<body")
        };
        let (page_title, blocks) = if looks_html {
            (web::title(&body), web::html_to_blocks(&body))
        } else {
            (None, web::text_blocks(&body))
        };
        if blocks.is_empty() {
            return Err(ToolError::Other(format!(
                "a página não rendeu blocos de texto: {}",
                params.url
            )));
        }
        let blocks = web::rank(blocks, params.question.as_deref());
        let total = blocks.len();
        let shown: Vec<web::Block> = blocks.into_iter().take(params.max_blocks.max(1)).collect();

        let mut sections = vec![Section {
            heading: format!(
                "### fetch — {}{}",
                params.url,
                page_title
                    .as_deref()
                    .map(|title| format!(" · {title}"))
                    .unwrap_or_default()
            ),
            body: format!(
                "\n- blocos: {total} (mostrando {})\n- âncoras: use o blk_xxxxxxxx nas citações\n",
                shown.len()
            ),
            priority: 255,
        }];
        for block in &shown {
            sections.push(Section {
                heading: format!("#### {}", block.anchor),
                body: format!("\n{}\n", crate::exec::clip(&block.text, 1_200, 200)),
                priority: 200,
            });
        }

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let url = arguments.get("url").and_then(Value::as_str).unwrap_or("?");
        format!("fetch: {url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_shows_the_url() {
        let tool = FetchTool::new(
            PathBuf::from("."),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({"url": "https://example.com"})),
            "fetch: https://example.com"
        );
    }

    #[test]
    fn a_non_http_url_is_rejected() {
        let tool = FetchTool::new(
            PathBuf::from("."),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        );
        assert!(Tool::call(&tool, serde_json::json!({"url": "file:///etc/passwd"})).is_err());
    }
}
