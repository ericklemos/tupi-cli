use std::sync::Arc;

use serde::Deserialize;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::{estimate_tokens, CHARS_PER_TOKEN};
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
struct PeekParams {
    handle: String,
    #[serde(default)]
    offset_chars: usize,
    budget_tokens: Option<usize>,
}

/// Recupera o corpo que uma resposta truncada deixou para trás.
///
/// O par do envelope de truncamento: handle inválido é erro explícito, nunca
/// resposta vazia, porque "sem conteúdo" leria como "não há mais nada".
pub struct PeekTool {
    pub(crate) store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl PeekTool {
    pub fn new(store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            store,
            default_budget_tokens,
        }
    }

    pub fn peek(
        &self,
        handle: &str,
        offset_chars: usize,
        budget_tokens: usize,
    ) -> Result<String, ToolError> {
        let body = self
            .store
            .get(handle)
            .ok_or_else(|| ToolError::UnknownHandle(handle.to_string()))?;

        let start = byte_at_char(&body, offset_chars);
        let remaining = &body[start..];

        let page_chars = chars_for_budget(budget_tokens).min(remaining.chars().count());
        let end = byte_at_char(remaining, page_chars);

        let page = &remaining[..end];
        let rest = &remaining[end..];

        if page.is_empty() && rest.is_empty() {
            return Ok("Fim do corpo.".to_string());
        }

        let mut out = page.to_string();
        if !rest.is_empty() {
            let next = offset_chars + page.chars().count();
            out.push_str(&format!(
                "\n\nRestante: {} caracteres (~{} tokens). Próximo offset: {next}.",
                rest.chars().count(),
                estimate_tokens(rest)
            ));
        }

        Ok(out)
    }
}

fn chars_for_budget(budget_tokens: usize) -> usize {
    (budget_tokens as f64 * CHARS_PER_TOKEN) as usize
}

/// Byte onde começa o caractere de índice `chars` (ou o fim do texto).
fn byte_at_char(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(index, _)| index)
        .unwrap_or(text.len())
}

impl Tool for PeekTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "peek".to_string(),
            description: "Recupera o trecho de uma resposta truncada anteriormente, paginando por offset.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "handle": {
                        "type": "string",
                        "description": "Handle devolvido no envelope de truncamento (formato h_xxxxxxxx)."
                    },
                    "offset_chars": {
                        "type": "integer",
                        "default": 0,
                        "minimum": 0,
                        "description": "Posição em caracteres no corpo omitido."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "default": self.default_budget_tokens,
                        "description": "Tamanho da página em tokens estimados."
                    }
                },
                "required": ["handle"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: PeekParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let budget = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        self.peek(&params.handle, params.offset_chars, budget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tupi_kernel::{Tool, ToolError};

    use crate::budget::DEFAULT_BUDGET_TOKENS;

    fn tool() -> PeekTool {
        PeekTool::new(
            Arc::new(ResponseStore::new(Duration::from_secs(60), 1024 * 1024)),
            DEFAULT_BUDGET_TOKENS,
        )
    }

    fn handle_of(tool: &PeekTool, body: String) -> String {
        tool.store.insert(body)
    }

    #[test]
    fn peek_returns_the_whole_body_when_it_fits() {
        let tool = tool();
        let handle = handle_of(&tool, "abc".to_string());
        let out = tool.peek(&handle, 0, DEFAULT_BUDGET_TOKENS).unwrap();

        assert!(out.starts_with("abc"));
        assert!(!out.contains("Restante"));
    }

    #[test]
    fn unknown_handle_is_an_explicit_error() {
        let error = tool().peek("h_missing", 0, DEFAULT_BUDGET_TOKENS).unwrap_err();
        assert!(matches!(error, ToolError::UnknownHandle(_)));
    }

    #[test]
    fn paginating_with_offset_covers_the_body_without_gaps_or_overlap() {
        let tool = tool();
        let body: String = (0..165)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let handle = handle_of(&tool, body.clone());

        let first = tool.peek(&handle, 0, 50).unwrap();
        let first_text = first.split("\n\nRestante:").next().unwrap();
        assert_eq!(first_text.len(), 165);

        let second = tool.peek(&handle, first_text.len(), 50).unwrap();
        assert!(second.contains("Fim do corpo."));
    }

    #[test]
    fn pagination_counts_characters_not_bytes_and_walks_to_the_end() {
        let tool = tool();
        let body = "á".repeat(400);
        let handle = handle_of(&tool, body.clone());

        let mut offset = 0usize;
        let mut rebuilt = String::new();
        loop {
            let page = tool.peek(&handle, offset, 50).unwrap();
            if page == "Fim do corpo." {
                break;
            }
            let text = page.split("\n\nRestante:").next().unwrap();
            rebuilt.push_str(text);
            match page.split("Próximo offset: ").nth(1) {
                Some(next) => {
                    offset = next
                        .trim_end_matches('.')
                        .parse()
                        .expect("offset numérico");
                }
                None => break,
            }
        }

        assert_eq!(rebuilt, body);
    }

    #[test]
    fn offset_past_the_end_returns_nothing_left() {
        let tool = tool();
        let handle = handle_of(&tool, "abc".to_string());
        let out = tool.peek(&handle, 999, DEFAULT_BUDGET_TOKENS).unwrap();

        assert!(out.contains("Fim do corpo."));
    }

    #[test]
    fn missing_handle_argument_is_rejected() {
        let output = tool().call(serde_json::json!({})).unwrap_err();
        assert!(matches!(output, ToolError::InvalidArguments(_)));
    }

    #[test]
    fn plugin_announces_its_name_and_schema() {
        let tool = tool();
        assert_eq!(tool.spec().name, "peek");
    }
}
