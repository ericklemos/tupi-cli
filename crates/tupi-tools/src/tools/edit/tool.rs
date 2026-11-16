use super::*;

impl Tool for EditTool {
    fn effect(&self, arguments: &serde_json::Value) -> tupi_kernel::Effect {
        crate::support::effect::writes(arguments)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit".to_string(),
            description: "Edita o corpo de um símbolo por span de AST, com expected_hash do read. Recusa hash divergente, símbolo ambíguo ou sintaxe inválida.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Caminho relativo ao diretório raiz do projeto."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Nome simples (`verify`) ou qualificado (`Auth::verify`, `Default::new`)."
                    },
                    "expected_hash": {
                        "type": "string",
                        "description": "blake3 do arquivo inteiro, como `read` imprime no cabeçalho (prefixo de 12 caracteres aceito)."
                    },
                    "new_body": {
                        "type": "string",
                        "description": "Novo bloco `{ … }` completo, chaves incluídas. O splice é de bytes crus: sem reindentação e sem formatação."
                    },
                    "signature": {
                        "type": "string",
                        "description": "Opcional: substitui tudo o que antecede o `{` (assinatura, genéricos, where). Sem ele, a assinatura fica como está."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["path", "symbol", "expected_hash", "new_body"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: EditParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.edit(params)
    }

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
            parts.push(format!("path={path}"));
        }
        if let Some(symbol) = arguments.get("symbol").and_then(|value| value.as_str()) {
            parts.push(format!("symbol={symbol}"));
        }
        if parts.is_empty() {
            "edit".to_string()
        } else {
            parts.join(" · ")
        }
    }
}
