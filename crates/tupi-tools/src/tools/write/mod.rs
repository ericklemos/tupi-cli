use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::paths;
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct WriteParams {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub create_only: bool,
    #[serde(default = "default_true")]
    pub create_parents: bool,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub(crate) fn default_true() -> bool {
    true
}

pub struct WriteTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl WriteTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".to_string(),
            description: "Cria ou substitui um arquivo inteiro sob o root. Sobrescrever exige expected_hash ou overwrite=true.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Caminho relativo ao diretorio raiz do projeto."
                    },
                    "content": {
                        "type": "string",
                        "description": "Conteudo completo do arquivo, em UTF-8. Bytes crus: sem formatacao automatica."
                    },
                    "expected_hash": {
                        "type": "string",
                        "description": "Obrigatorio para sobrescrever sem overwrite. blake3 do conteudo atual."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "default": false,
                        "description": "Substitui um arquivo existente sem conferir hash."
                    },
                    "create_only": {
                        "type": "boolean",
                        "default": false,
                        "description": "Falha se o arquivo ja existir."
                    },
                    "create_parents": {
                        "type": "boolean",
                        "default": true,
                        "description": "Cria os diretorios intermediarios dentro do root."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: WriteParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let path = paths::resolve_for_write(&self.root_dir, &params.path)?;

        if path.is_dir() {
            return Err(ToolError::InvalidArguments(format!(
                "{} e um diretorio; nada foi escrito",
                params.path
            )));
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&path)
            && metadata.file_type().is_symlink()
        {
            return Err(ToolError::Other(format!(
                "{} e um symlink; nada foi escrito",
                params.path
            )));
        }

        let existed = path.exists();
        let before = if existed {
            Some(std::fs::read(&path).map_err(|error| {
                ToolError::Other(format!("falha ao ler {}: {error}", params.path))
            })?)
        } else {
            None
        };

        if let Some(current) = &before {
            if params.create_only {
                return Err(ToolError::Other(format!(
                    "{} ja existe e create_only foi pedido; nada foi escrito",
                    params.path
                )));
            }
            if !params.overwrite {
                let expected = params.expected_hash.as_deref().ok_or_else(|| {
                    ToolError::InvalidArguments(
                        "sobrescrever exige expected_hash ou overwrite=true".to_string(),
                    )
                })?;
                if !paths::hash_matches(current, expected) {
                    return Err(ToolError::StaleHash(format!(
                        "hash atual: {}. Reenvie com expected_hash atualizado",
                        paths::hash_prefix(current)
                    )));
                }
            }
        } else if params
            .expected_hash
            .as_deref()
            .is_some_and(|hash| !hash.trim().is_empty())
        {
            return Err(ToolError::InvalidArguments(
                "expected_hash so vale para arquivo existente".to_string(),
            ));
        }

        if params.create_parents
            && let Some(parent) = path.parent()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                ToolError::Other(format!(
                    "falha ao criar diretorios de {}: {error}",
                    params.path
                ))
            })?;
        }

        paths::atomic_write(&path, params.content.as_bytes()).map_err(|error| {
            ToolError::Other(format!("falha ao escrever {}: {error}", params.path))
        })?;

        let on_disk = std::fs::read(&path).map_err(|error| {
            ToolError::Other(format!(
                "a escrita foi feita, mas a verificacao nao releu {}: {error}",
                params.path
            ))
        })?;
        if on_disk != params.content.as_bytes() {
            return Err(ToolError::Other(format!(
                "verificacao pos-escrita falhou: o conteudo em disco difere do enviado ({})",
                params.path
            )));
        }

        let verb = if existed { "substituido" } else { "criado" };
        let before_hash = before
            .as_deref()
            .map(paths::hash_prefix)
            .unwrap_or_else(|| "-".to_string());
        let after_hash = paths::hash_prefix(&on_disk);
        let relative = paths::relative(&self.root_dir, &path);
        let lines = params.content.lines().count();
        let section = Section {
            heading: format!("### {relative} — {verb}"),
            body: format!(
                "\nArquivo: blake3 {after_hash} (antes {before_hash})\nLinhas: {lines} · bytes: {}\n",
                on_disk.len()
            ),
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let path = arguments
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("write");
        let bytes = arguments
            .get("content")
            .and_then(Value::as_str)
            .map(str::len)
            .unwrap_or(0);
        format!("write: {path} ({bytes} bytes)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tupi_kernel::Tool;

    fn tool(dir: &std::path::Path) -> WriteTool {
        WriteTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(Duration::from_secs(60), 1024 * 1024)),
            4000,
        )
    }

    fn call(tool: &WriteTool, value: serde_json::Value) -> Result<String, ToolError> {
        Tool::call(tool, value)
    }

    #[test]
    fn a_new_file_is_created_and_verified() {
        let dir = tempfile::tempdir().unwrap();
        let out = call(
            &tool(dir.path()),
            serde_json::json!({"path": "a/b.txt", "content": "oi\n"}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b.txt")).unwrap(),
            "oi\n"
        );
        assert!(out.contains("criado"));
        assert!(out.contains("blake3"));
    }

    #[test]
    fn overwriting_without_a_hash_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "antes").unwrap();
        let error = call(
            &tool(dir.path()),
            serde_json::json!({"path": "a.txt", "content": "depois"}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("expected_hash"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "antes"
        );
    }

    #[test]
    fn a_stale_hash_never_writes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "antes").unwrap();
        assert!(call(
            &tool(dir.path()),
            serde_json::json!({"path": "a.txt", "content": "depois", "expected_hash": "000000000000"}),
        )
        .is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "antes"
        );
    }

    #[test]
    fn a_current_hash_allows_the_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "antes").unwrap();
        let hash = paths::hash_prefix(b"antes");
        call(
            &tool(dir.path()),
            serde_json::json!({"path": "a.txt", "content": "depois", "expected_hash": hash}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "depois"
        );
    }

    #[test]
    fn create_only_refuses_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "antes").unwrap();
        assert!(call(
            &tool(dir.path()),
            serde_json::json!({"path": "a.txt", "content": "depois", "create_only": true, "overwrite": true}),
        )
        .is_err());
    }

    #[test]
    fn a_path_outside_the_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            call(
                &tool(dir.path()),
                serde_json::json!({"path": "../fora.txt", "content": "x"}),
            )
            .is_err()
        );
    }

    #[test]
    fn create_parents_false_refuses_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            call(
                &tool(dir.path()),
                serde_json::json!({"path": "a/b.txt", "content": "x", "create_parents": false}),
            )
            .is_err()
        );
    }
}
