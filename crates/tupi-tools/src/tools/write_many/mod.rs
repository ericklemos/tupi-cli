use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::paths;
use crate::store::ResponseStore;

const MAX_WRITES: usize = 20;

#[derive(Debug, Clone, Deserialize)]
pub struct WriteItem {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub create_only: bool,
    #[serde(default = "crate::write::default_true")]
    pub create_parents: bool,
}

#[derive(Debug, Deserialize)]
pub struct WriteManyParams {
    pub writes: Vec<WriteItem>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct WriteManyTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

struct Planned {
    path: PathBuf,
    item: WriteItem,
    before: Option<Vec<u8>>,
}

impl WriteManyTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn plan(root: &Path, item: &WriteItem) -> Result<Planned, ToolError> {
    let path = paths::resolve_for_write(root, &item.path)?;
    if path.is_dir() {
        return Err(ToolError::InvalidArguments(format!(
            "{} e um diretorio; nada foi escrito",
            item.path
        )));
    }
    if let Ok(metadata) = std::fs::symlink_metadata(&path)
        && metadata.file_type().is_symlink()
    {
        return Err(ToolError::Other(format!(
            "{} e um symlink; nada foi escrito",
            item.path
        )));
    }

    let before =
        if path.exists() {
            Some(std::fs::read(&path).map_err(|error| {
                ToolError::Other(format!("falha ao ler {}: {error}", item.path))
            })?)
        } else {
            None
        };

    match &before {
        Some(current) => {
            if item.create_only {
                return Err(ToolError::Other(format!(
                    "{} ja existe e create_only foi pedido; nada foi escrito",
                    item.path
                )));
            }
            if !item.overwrite {
                let expected = item.expected_hash.as_deref().ok_or_else(|| {
                    ToolError::InvalidArguments(format!(
                        "{}: sobrescrever exige expected_hash ou overwrite=true",
                        item.path
                    ))
                })?;
                if !paths::hash_matches(current, expected) {
                    return Err(ToolError::StaleHash(format!(
                        "{}: hash atual {}",
                        item.path,
                        paths::hash_prefix(current)
                    )));
                }
            }
        }
        None => {
            if item
                .expected_hash
                .as_deref()
                .is_some_and(|hash| !hash.trim().is_empty())
            {
                return Err(ToolError::InvalidArguments(format!(
                    "{}: expected_hash so vale para arquivo existente",
                    item.path
                )));
            }
        }
    }

    Ok(Planned {
        path,
        item: item.clone(),
        before,
    })
}

fn rollback(planned: &[Planned]) -> Vec<String> {
    let mut failures = Vec::new();
    for plan in planned.iter().rev() {
        let result = match &plan.before {
            Some(bytes) => paths::atomic_write(&plan.path, bytes),
            None => std::fs::remove_file(&plan.path),
        };
        if let Err(error) = result {
            failures.push(format!("{}: {error}", plan.item.path));
        }
    }
    failures
}

impl Tool for WriteManyTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_many".to_string(),
            description: "Grava varios arquivos como um lote: valida todos antes de escrever, aborta sem tocar no disco se um item for invalido e desfaz o que ja gravou se uma escrita falhar no meio.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "writes": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 20,
                        "description": "Arquivos a gravar, na ordem do relatorio. Caminhos repetidos sao recusados.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "Caminho relativo ao root." },
                                "content": { "type": "string", "description": "Conteudo completo, em UTF-8." },
                                "expected_hash": { "type": "string", "description": "Obrigatorio para sobrescrever sem overwrite." },
                                "overwrite": { "type": "boolean", "default": false },
                                "create_only": { "type": "boolean", "default": false },
                                "create_parents": { "type": "boolean", "default": true }
                            },
                            "required": ["path", "content"]
                        }
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["writes"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: WriteManyParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;

        if params.writes.is_empty() {
            return Err(ToolError::InvalidArguments(
                "writes nao pode estar vazio".to_string(),
            ));
        }
        if params.writes.len() > MAX_WRITES {
            return Err(ToolError::InvalidArguments(format!(
                "no maximo {MAX_WRITES} arquivos por chamada"
            )));
        }

        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let mut seen = HashSet::new();
        let mut planned = Vec::new();
        for item in &params.writes {
            let entry = plan(&root, item)?;
            if !seen.insert(entry.path.clone()) {
                return Err(ToolError::InvalidArguments(format!(
                    "caminho repetido: {}",
                    item.path
                )));
            }
            planned.push(entry);
        }

        for (written, plan) in planned.iter().enumerate() {
            if plan.item.create_parents
                && let Some(parent) = plan.path.parent()
            {
                std::fs::create_dir_all(parent).map_err(|error| {
                    ToolError::Other(format!(
                        "falha ao criar diretorios de {}: {error}",
                        plan.item.path
                    ))
                })?;
            }
            if let Err(error) = paths::atomic_write(&plan.path, plan.item.content.as_bytes()) {
                let failures = rollback(&planned[..written]);
                let rollback_note = if failures.is_empty() {
                    "rollback completo".to_string()
                } else {
                    format!("rollback incompleto: {}", failures.join("; "))
                };
                return Err(ToolError::Other(format!(
                    "falha ao gravar {}: {error}; {rollback_note}",
                    plan.item.path
                )));
            }
        }

        let mut summary = String::new();
        for plan in &planned {
            let on_disk = std::fs::read(&plan.path).map_err(|error| {
                ToolError::Other(format!("verificacao nao releu {}: {error}", plan.item.path))
            })?;
            if on_disk != plan.item.content.as_bytes() {
                return Err(ToolError::Other(format!(
                    "verificacao pos-escrita falhou em {}",
                    plan.item.path
                )));
            }
            let verb = if plan.before.is_some() {
                "substituido"
            } else {
                "criado"
            };
            let before_hash = plan
                .before
                .as_deref()
                .map(paths::hash_prefix)
                .unwrap_or_else(|| "-".to_string());
            summary.push_str(&format!(
                "- {}: {verb} · blake3 {} (antes {before_hash}) · {} bytes\n",
                plan.item.path,
                paths::hash_prefix(&on_disk),
                on_disk.len()
            ));
        }

        let section = Section {
            heading: format!("### write_many — {} arquivo(s)", planned.len()),
            body: format!("\n{summary}"),
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let count = arguments
            .get("writes")
            .and_then(Value::as_array)
            .map(|writes| writes.len())
            .unwrap_or(0);
        format!("write_many: {count} arquivo(s)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(dir: &Path) -> WriteManyTool {
        WriteManyTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1024 * 1024,
            )),
            4000,
        )
    }

    #[test]
    fn a_batch_writes_every_file() {
        let dir = tempfile::tempdir().unwrap();
        Tool::call(
            &tool(dir.path()),
            serde_json::json!({
                "writes": [
                    {"path": "a.txt", "content": "a"},
                    {"path": "b/c.txt", "content": "c"}
                ]
            }),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "a"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b/c.txt")).unwrap(),
            "c"
        );
    }

    #[test]
    fn one_invalid_item_aborts_the_whole_batch_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.txt"), "antes").unwrap();
        let error = Tool::call(
            &tool(dir.path()),
            serde_json::json!({
                "writes": [
                    {"path": "a.txt", "content": "a"},
                    {"path": "b.txt", "content": "depois"}
                ]
            }),
        )
        .unwrap_err();
        assert!(error.to_string().contains("expected_hash"));
        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "antes"
        );
    }

    #[test]
    fn a_duplicated_path_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Tool::call(
                &tool(dir.path()),
                serde_json::json!({
                    "writes": [
                        {"path": "a.txt", "content": "a"},
                        {"path": "a.txt", "content": "b"}
                    ]
                }),
            )
            .is_err()
        );
    }

    #[test]
    fn an_empty_batch_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Tool::call(&tool(dir.path()), serde_json::json!({"writes": []})).is_err());
    }
}
