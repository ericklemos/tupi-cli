use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::store::ResponseStore;
use crate::web::{self, SearchHit};

#[derive(Debug, Deserialize)]
pub struct WebSearchParams {
    pub query: String,
    #[serde(default = "default_results")]
    pub max_results: usize,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

fn default_results() -> usize {
    8
}

pub struct WebSearchTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl WebSearchTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

/// Desfecho de uma busca, já com o tamanho da resposta do provedor: é esse
/// número que separa "o provedor não tem nada" de "o provedor devolveu uma
/// página que não é de resultados".
pub(crate) enum SearchOutcome {
    Results(Vec<SearchHit>),
    NoResults,
    Unrecognized { bytes: usize },
}

/// Texto para quando a busca foi aceita e realmente não há resultado.
pub(crate) fn no_results_note(tool: &str, query: &str) -> String {
    format!(
        "### {tool} — {query}\n\n0 resultados — o provedor respondeu que não há nada para esta consulta.\n"
    )
}

/// Texto para quando o provedor devolveu uma página que não é de resultados.
/// Existe porque "NÃO — nenhum resultado" fazia o modelo concluir que a
/// consulta está vazia quando o caso era bloqueio/anti-bot.
pub(crate) fn unrecognized_note(tool: &str, query: &str, bytes: usize) -> String {
    format!(
        "### {tool} — {query}\n\nNÃO CONFERE — o provedor respondeu {bytes} bytes sem nenhum bloco de resultado reconhecível (página de bloqueio/anti-bot ou markup novo). Isto não é \"sem resultado\": não conclua que a consulta está vazia. Repita com `fetch` numa URL concreta ou em outra fonte.\n"
    )
}

pub(crate) fn search_hits(
    query: &str,
    locale: Option<&str>,
    root: &Path,
) -> Result<SearchOutcome, ToolError> {
    let mut url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        web::url_encode(query)
    );
    if let Some(locale) = locale.map(str::trim).filter(|locale| !locale.is_empty()) {
        url.push_str("&kl=");
        url.push_str(&web::url_encode(locale));
    }
    let html = web::curl(&url, root)?;
    Ok(match web::parse_duckduckgo(&html) {
        web::DuckDuckGoPage::Results(hits) => SearchOutcome::Results(hits),
        web::DuckDuckGoPage::NoResults => SearchOutcome::NoResults,
        web::DuckDuckGoPage::Unrecognized => SearchOutcome::Unrecognized { bytes: html.len() },
    })
}

impl Tool for WebSearchTool {
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::ReadOnly
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "websearch".to_string(),
            description: "Busca na web sem provedor pago e devolve só título, URL, domínio e trecho; use fetch com question para ler os blocos.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Consulta em linguagem natural."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "default": 8,
                        "description": "Resultados após deduplicação."
                    },
                    "locale": {
                        "type": "string",
                        "description": "Região preferida, ex: pt-BR."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: WebSearchParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let query = params.query.trim();
        if query.is_empty() {
            return Err(ToolError::InvalidArguments("query vazia".to_string()));
        }
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let hits = match search_hits(query, params.locale.as_deref(), &root)? {
            SearchOutcome::Results(hits) => hits,
            SearchOutcome::NoResults => return Ok(no_results_note("websearch", query)),
            SearchOutcome::Unrecognized { bytes } => {
                return Ok(unrecognized_note("websearch", query, bytes));
            }
        };
        let limit = params.max_results.clamp(1, 20);

        let mut sections = vec![Section {
            heading: format!("### websearch — {query}"),
            body: format!("\n{} resultado(s).\n", hits.len().min(limit)),
            priority: 255,
        }];

        for (index, hit) in hits.iter().take(limit).enumerate() {
            let domain = hit
                .url
                .split("//")
                .nth(1)
                .unwrap_or(&hit.url)
                .split('/')
                .next()
                .unwrap_or("");
            sections.push(Section {
                heading: format!("#### {}. {}", index + 1, hit.title),
                body: format!(
                    "\n- {}\n- {}\n- {}\n",
                    hit.url,
                    domain,
                    crate::exec::clip(&hit.snippet, 400, 0)
                ),
                priority: (240 - index.min(200)) as u8,
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
        format!("websearch: {query}")
    }

    fn mode_mask(&self) -> tupi_kernel::ModeMask {
        tupi_kernel::ModeMask::EXECUTION.union(tupi_kernel::ModeMask::SDD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_shows_the_query() {
        let tool = WebSearchTool::new(
            PathBuf::from("."),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        );
        assert_eq!(
            tool.describe_call(&serde_json::json!({"query": "rust tree-sitter"})),
            "websearch: rust tree-sitter"
        );
    }

    #[test]
    fn an_empty_query_is_rejected() {
        let tool = WebSearchTool::new(
            PathBuf::from("."),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1 << 20,
            )),
            4000,
        );
        assert!(Tool::call(&tool, serde_json::json!({"query": "  "})).is_err());
    }
}
