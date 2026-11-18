use super::*;

impl Tool for EditTool {
    fn effect(&self, arguments: &serde_json::Value) -> tupi_kernel::Effect {
        crate::support::effect::writes(arguments)
    }

    fn spec(&self) -> ToolSpec     {
        ToolSpec {
            name: "edit".to_string(),
            description: "Edita uma região do arquivo por âncora literal única: `old`+`new`, texto exato que precisa casar 1 vez em qualquer arquivo UTF-8 (md/yaml/toml/py/ts/go/sql inclusive). Em Rust, `symbol`+`new_body` endereça o corpo por AST. Exige `expected_hash` do read; recusa hash divergente, 0 ou >1 ocorrências de `old`, símbolo ambíguo e sintaxe inválida, sem escrever.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Caminho relativo ao diretório raiz do projeto."
                    },
                    "expected_hash": {
                        "type": "string",
                        "description": "blake3 do arquivo inteiro, como `read` imprime no cabeçalho (prefixo de 12 caracteres aceito). Obrigatório."
                    },
                    "old": {
                        "type": "string",
                        "description": "Rota literal: trecho copiado byte a byte de `read`/`lookup lines`, indentação e quebras de linha incluídas. Precisa casar exatamente 1 vez; 0 ou mais de uma recusa sem escrever."
                    },
                    "new": {
                        "type": "string",
                        "description": "Rota literal: texto que substitui `old`. Vazio apaga o trecho."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Rota AST (só Rust): nome simples (`verify`) ou qualificado (`Auth::verify`, `Default::new`). Alternativa a `old`+`new`, nunca junto."
                    },
                    "new_body": {
                        "type": "string",
                        "description": "Rota AST: novo bloco `{ … }` completo do símbolo `symbol`, chaves incluídas. O splice é de bytes crus: sem reindentação e sem formatação."
                    },
                    "signature": {
                        "type": "string",
                        "description": "Opcional, rota AST: substitui tudo o que antecede o `{` (assinatura, genéricos, where). Sem ele, a assinatura fica como está."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["path", "expected_hash"]
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
