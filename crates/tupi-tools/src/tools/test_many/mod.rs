use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::{self, ExecParams};
use crate::store::ResponseStore;

const MAX_TESTS: usize = 10;
const DEFAULT_TIMEOUT_SECS: u64 = 600;
const MAX_TIMEOUT_SECS: u64 = 3600;

#[derive(Debug, Clone, Deserialize)]
pub struct TestItem {
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub exact: bool,
}

#[derive(Debug, Deserialize)]
pub struct TestManyParams {
    pub tests: Vec<TestItem>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct TestManyTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl TestManyTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn validate(params: &TestManyParams) -> Result<(), ToolError> {
    if params.tests.is_empty() {
        return Err(ToolError::InvalidArguments(
            "tests nao pode estar vazio".to_string(),
        ));
    }
    if params.tests.len() > MAX_TESTS {
        return Err(ToolError::InvalidArguments(format!(
            "no maximo {MAX_TESTS} testes por chamada"
        )));
    }
    for (index, item) in params.tests.iter().enumerate() {
        let has_target = item
            .target
            .as_deref()
            .map(str::trim)
            .is_some_and(|target| !target.is_empty());
        let has_filter = item
            .filter
            .as_deref()
            .map(str::trim)
            .is_some_and(|filter| !filter.is_empty());
        if !has_target && !has_filter {
            return Err(ToolError::InvalidArguments(format!(
                "test {} precisa de target ou filter para nao rodar a suite inteira",
                index + 1
            )));
        }
        if item.exact && !has_filter {
            return Err(ToolError::InvalidArguments(format!(
                "test {}: exact exige filter",
                index + 1
            )));
        }
    }
    Ok(())
}

fn result_lines(stdout: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("test result:") || trimmed.starts_with("running ") {
            out.push_str(&format!("- {trimmed}\n"));
            count += 1;
            if count >= 8 {
                break;
            }
        }
    }
    out
}

fn failure_tail(stdout: &str) -> Option<String> {
    let index = stdout.rfind("failures:")?;
    Some(exec::clip(&stdout[index..], 3_000, 2_000))
}

impl Tool for TestManyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "test_many".to_string(),
            description: "Executa testes especificos em lote. Cada item precisa de target ou filter para nao rodar a suite inteira.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "tests": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 10,
                        "description": "Testes a executar. Cada item precisa de target ou filter.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "package": {
                                    "type": "string",
                                    "description": "Pacote opcional (cargo -p)."
                                },
                                "target": {
                                    "type": "string",
                                    "description": "Alvo de teste de integracao, ou lib, bins, tests, all-targets."
                                },
                                "filter": {
                                    "type": "string",
                                    "description": "Filtro de nome de teste."
                                },
                                "exact": {
                                    "type": "boolean",
                                    "default": false,
                                    "description": "Casamento exato do filter."
                                }
                            }
                        }
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tempo por teste. Padrao 600s."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens da resposta inteira."
                    }
                },
                "required": ["tests"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: TestManyParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        validate(&params)?;

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let timeout = Duration::from_secs(
            params
                .timeout_secs
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .clamp(1, MAX_TIMEOUT_SECS),
        );

        let mut sections = Vec::new();
        let mut summary = String::new();
        let mut passed = 0usize;
        let mut failed = 0usize;

        for item in &params.tests {
            let exec_params = ExecParams::test(
                item.package.clone(),
                item.target.clone(),
                item.filter.clone(),
                item.exact,
            );
            let args = exec::build_args(&exec_params)?;
            let label = exec::command_line(&args);
            let raw = exec::execute(&root, &exec_params, timeout)?;
            let status = if raw.timed_out {
                "timeout"
            } else if raw.success {
                "ok"
            } else {
                "falhou"
            };
            if raw.success {
                passed += 1;
            } else {
                failed += 1;
            }

            summary.push_str(&format!(
                "- {label}: {status} ({:.1}s)\n",
                raw.duration.as_secs_f64()
            ));

            let mut body = String::from("\n");
            let results = result_lines(&raw.stdout);
            if !results.is_empty() {
                body.push_str(&results);
            }
            if !raw.success {
                if let Some(failures) = failure_tail(&raw.stdout) {
                    body.push_str("\n~~~text\n");
                    body.push_str(&failures);
                    body.push_str("\n~~~\n");
                } else if !raw.stderr.trim().is_empty() {
                    body.push_str("\n~~~text\n");
                    body.push_str(&exec::clip(raw.stderr.trim_end(), 1_000, 3_000));
                    body.push_str("\n~~~\n");
                }
            }

            sections.push(Section {
                heading: format!("#### {label} — {status}"),
                body,
                priority: if raw.success { 120 } else { 240 },
            });
        }

        sections.insert(
            0,
            Section {
                heading: "### test_many".to_string(),
                body: format!(
                    "\n{passed} ok · {failed} falhou · {} teste(s)\n{summary}",
                    params.tests.len()
                ),
                priority: 255,
            },
        );

        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(exec::finish(&self.store, sections, budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let Ok(params) = serde_json::from_value::<TestManyParams>(arguments.clone()) else {
            return "test_many".to_string();
        };
        let labels: Vec<String> = params
            .tests
            .iter()
            .filter_map(|item| {
                let exec_params = ExecParams::test(
                    item.package.clone(),
                    item.target.clone(),
                    item.filter.clone(),
                    item.exact,
                );
                exec::build_args(&exec_params)
                    .ok()
                    .map(|args| exec::command_line(&args))
            })
            .collect();
        format!("test_many: {}", labels.join(" · "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> TestItem {
        TestItem {
            package: None,
            target: None,
            filter: None,
            exact: false,
        }
    }

    #[test]
    fn an_empty_batch_is_rejected() {
        assert!(
            validate(&TestManyParams {
                tests: Vec::new(),
                timeout_secs: None,
                budget_tokens: None,
            })
            .is_err()
        );
    }

    #[test]
    fn an_item_without_target_or_filter_is_rejected() {
        assert!(
            validate(&TestManyParams {
                tests: vec![item()],
                timeout_secs: None,
                budget_tokens: None,
            })
            .is_err()
        );
    }

    #[test]
    fn an_item_with_a_filter_is_accepted() {
        let mut item = item();
        item.filter = Some("guard".to_string());
        assert!(
            validate(&TestManyParams {
                tests: vec![item],
                timeout_secs: None,
                budget_tokens: None,
            })
            .is_ok()
        );
    }

    #[test]
    fn exact_without_filter_is_rejected() {
        let mut item = item();
        item.exact = true;
        assert!(
            validate(&TestManyParams {
                tests: vec![item],
                timeout_secs: None,
                budget_tokens: None,
            })
            .is_err()
        );
    }

    #[test]
    fn more_than_ten_tests_are_rejected() {
        let mut item = item();
        item.filter = Some("x".to_string());
        assert!(
            validate(&TestManyParams {
                tests: vec![item; MAX_TESTS + 1],
                timeout_secs: None,
                budget_tokens: None,
            })
            .is_err()
        );
    }
}
