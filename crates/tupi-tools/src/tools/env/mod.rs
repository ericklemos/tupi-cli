use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::clock::ClockTool;

const ALLOWED: &[&str] = &[
    "TUPI_MODEL",
    "TUPI_LOG",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_SESSION_TYPE",
    "TERM",
    "SHELL",
];

/// Os `kind` aceitos: variáveis da allowlist ou o relógio da sessão.
const KINDS: [&str; 2] = ["vars", "clock"];

#[derive(Debug, Deserialize)]
pub struct EnvParams {
    #[serde(default)]
    pub keys: Option<Vec<String>>,
    /// `vars` (padrão) lê as variáveis da allowlist; `clock` devolve data e
    /// hora correntes (absorveu a tool `clock`).
    #[serde(default)]
    pub kind: Option<String>,
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
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::ReadOnly
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "env".to_string(),
            description:
                "Inspeciona o ambiente de execução: `kind` escolhe vars (allowlist estrita, sem variáveis sensíveis) ou clock (data e hora correntes em UTC e no fuso local)."
                    .to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": KINDS,
                        "description": "vars (padrão) lê variáveis; clock devolve data e hora."
                    },
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
        let kind = params
            .kind
            .as_deref()
            .map(|kind| kind.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "vars".to_string());

        if kind == "clock" {
            return Tool::call(&ClockTool::new(), serde_json::json!({}));
        }
        if kind != "vars" {
            return Err(ToolError::InvalidArguments(format!(
                "env: `kind` desconhecido `{kind}` — use {}",
                KINDS.join("|")
            )));
        }

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
        if arguments
            .get("kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("clock"))
        {
            return "env: clock".to_string();
        }
        let count = arguments
            .get("keys")
            .and_then(Value::as_array)
            .map(|keys| keys.len())
            .unwrap_or(ALLOWED.len());
        format!("env: {count} variável(is)")
    }

    fn mode_mask(&self) -> tupi_kernel::ModeMask {
        tupi_kernel::ModeMask::EXECUTION.union(tupi_kernel::ModeMask::SDD)
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

    #[test]
    fn clock_is_reachable_through_env() {
        let out = Tool::call(&EnvTool::new(), serde_json::json!({"kind": "clock"})).unwrap();
        assert!(out.contains("UTC"), "{out}");
        assert!(out.contains("epoch"), "{out}");
    }

    #[test]
    fn an_unknown_kind_lists_the_valid_ones() {
        let error = Tool::call(&EnvTool::new(), serde_json::json!({"kind": "hora"}))
            .expect_err("kind desconhecido deveria ser recusado");
        assert!(format!("{error:?}").contains("vars|clock"));
    }
}
