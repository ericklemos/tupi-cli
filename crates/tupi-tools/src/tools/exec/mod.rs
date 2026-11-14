mod args;

mod diagnostics;

mod render;

#[cfg(test)]
mod tests;

pub(crate) use args::{build_args, command_line, execute, timeout_for};
pub(crate) use render::{clip, fingerprint, finish, sections_for};

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::{self, Section};
use crate::proc::{self, Output};
use crate::store::ResponseStore;

const DEFAULT_TIMEOUT_SECS: u64 = 900;
const MAX_TIMEOUT_SECS: u64 = 1_800;
const MAX_ERRORS_SHOWN: usize = 8;
const MAX_WARNINGS_SHOWN: usize = 4;
const MAX_FINGERPRINT_FILES: usize = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecKind {
    Build,
    Check,
    Test,
    Clippy,
    Xtask,
}

#[derive(Debug, Deserialize)]
pub struct ExecParams {
    pub kind: ExecKind,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub exact: bool,
    #[serde(default)]
    pub task: Option<String>,
    #[serde(default)]
    pub refresh: bool,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

impl ExecParams {
    pub fn test(
        package: Option<String>,
        target: Option<String>,
        filter: Option<String>,
        exact: bool,
    ) -> Self {
        Self {
            kind: ExecKind::Test,
            package,
            target,
            filter,
            exact,
            task: None,
            refresh: false,
            timeout_secs: None,
            budget_tokens: None,
        }
    }
}

#[derive(Clone)]
struct Cached {
    key: String,
    output: Output,
}

pub struct ExecTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
    cache: Mutex<Option<Cached>>,
}

impl ExecTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
            cache: Mutex::new(None),
        }
    }
}

impl Tool for ExecTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "exec".to_string(),
            description: "Roda cargo check/build/test/clippy/xtask com erros agrupados e cache. Restrinja com package, target e filter para nao compilar o workspace inteiro.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["build", "check", "test", "clippy", "xtask"],
                        "description": "check e o mais rapido para erros de tipo; build liga artefatos; test roda a suite; clippy adiciona lints; xtask roda tarefas do workspace."
                    },
                    "package": {
                        "type": "string",
                        "description": "Restringe ao pacote (cargo -p). Sem ele, o comando vale para o workspace."
                    },
                    "target": {
                        "type": "string",
                        "description": "lib, bins, tests, all-targets, doc (so test) ou o nome de um teste de integracao (so test)."
                    },
                    "filter": {
                        "type": "string",
                        "description": "Filtro de nome de teste (substring). So para kind test."
                    },
                    "exact": {
                        "type": "boolean",
                        "default": false,
                        "description": "Exige casamento exato do filter. So para kind test e exige filter."
                    },
                    "task": {
                        "type": "string",
                        "description": "Comando do xtask. Obrigatorio quando kind e xtask."
                    },
                    "refresh": {
                        "type": "boolean",
                        "default": false,
                        "description": "Ignora o cache e reexecuta o cargo mesmo que nada tenha mudado."
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tempo do processo. Padrao 900s."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["kind"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: ExecParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let args = build_args(&params)?;
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let key =
            fingerprint(&root, MAX_FINGERPRINT_FILES).map(|fp| format!("{}|{fp}", args.join(" ")));

        let cached = if params.refresh {
            None
        } else if let Some(key) = &key {
            self.cache
                .lock()
                .unwrap()
                .clone()
                .filter(|entry| &entry.key == key)
                .map(|entry| entry.output)
        } else {
            None
        };

        let (output, from_cache) = match cached {
            Some(output) => (output, true),
            None => {
                let output = execute(&root, &params, timeout_for(&params))?;
                if let Some(key) = key {
                    *self.cache.lock().unwrap() = Some(Cached {
                        key,
                        output: output.clone(),
                    });
                }
                (output, false)
            }
        };

        let sections = sections_for(&args, &output, from_cache);
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let Ok(params) = serde_json::from_value::<ExecParams>(arguments.clone()) else {
            return "exec".to_string();
        };
        match build_args(&params) {
            Ok(args) => command_line(&args),
            Err(_) => "exec".to_string(),
        }
    }
}
