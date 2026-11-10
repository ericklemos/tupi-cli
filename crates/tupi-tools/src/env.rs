use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

const ALLOWED: &[&str] = &[
    "TUPI_MODEL",
    "TUPI_LOG",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_SESSION_TYPE",
    "TERM",
    "SHELL",
];

#[derive(Debug, Deserialize)]
pub struct EnvParams {
    #[serde(default)]
    pub keys: Option<Vec<String>>,
}

pub struct EnvTool;

impl EnvTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EnvTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for EnvTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "env".to_string(),
            description:
                "Inspeciona o ambiente de execução com allowlist estrita (sem variáveis sensíveis)."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "keys": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Variáveis desejadas. Só as da allowlist são devolvidas."
                    }
                }
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: EnvParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let requested: Vec<String> = match params.keys {
            Some(keys) => keys,
            None => ALLOWED.iter().map(|key| key.to_string()).collect(),
        };

        let mut body = format!(
            "\n- plataforma: {} · arquitetura: {}\n",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        for key in requested {
            if !ALLOWED.contains(&key.as_str()) {
                body.push_str(&format!("- {key}: (fora da allowlist)\n"));
                continue;
            }
            let value = std::env::var(&key).unwrap_or_else(|_| "(não definida)".to_string());
            body.push_str(&format!("- {key}={value}\n"));
        }
        Ok(format!("### env\n{body}"))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let count = arguments
            .get("keys")
            .and_then(Value::as_array)
            .map(|keys| keys.len())
            .unwrap_or(ALLOWED.len());
        format!("env: {count} variável(is)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_is_always_reported() {
        let out = Tool::call(&EnvTool::new(), serde_json::json!({})).unwrap();
        assert!(out.contains("plataforma"));
        assert!(out.contains(std::env::consts::ARCH));
    }

    #[test]
    fn a_secret_is_not_exposed() {
        let out = Tool::call(
            &EnvTool::new(),
            serde_json::json!({"keys": ["TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY"]}),
        )
        .unwrap();
        assert!(out.contains("fora da allowlist"));
        assert!(!out.contains("sk-"));
    }
}
