use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::budget::Section;
use crate::exec::finish;
use crate::paths;
use crate::store::ResponseStore;

#[derive(Debug, Deserialize)]
pub struct MoveParams {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub copy: bool,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub create_parents: bool,
    #[serde(default)]
    pub expected_hash: Option<String>,
    #[serde(default)]
    pub budget_tokens: Option<usize>,
}

pub struct MoveTool {
    root_dir: PathBuf,
    store: Arc<ResponseStore>,
    default_budget_tokens: usize,
}

impl MoveTool {
    pub fn new(root_dir: PathBuf, store: Arc<ResponseStore>, default_budget_tokens: usize) -> Self {
        Self {
            root_dir,
            store,
            default_budget_tokens,
        }
    }
}

fn copy_dir(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn importers(root: &Path, source: &Path, dest: &Path) -> Vec<String> {
    let Some(old_stem) = source.file_stem().and_then(|stem| stem.to_str()) else {
        return Vec::new();
    };
    let new_stem = dest
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("");
    if old_stem == new_stem || source.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        return Vec::new();
    }

    let needle_mod = format!("mod {old_stem};");
    let needle_use = format!("::{old_stem}::");
    let mut hits = Vec::new();
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(true)
        .require_git(false);

    for entry in builder.build().flatten() {
        if hits.len() >= 8 {
            break;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if path == source || path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if text.contains(&needle_mod) || text.contains(&needle_use) {
            hits.push(paths::relative(root, path));
        }
    }
    hits
}

impl Tool for MoveTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "move".to_string(),
            description: "Move ou copia arquivo/diretorio dentro do root; recusa destino existente e hash velho, e lista quem importava o nome antigo.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "string",
                        "description": "Origem, relativa ao root."
                    },
                    "to": {
                        "type": "string",
                        "description": "Destino, relativo ao root. O pai precisa existir, salvo create_parents."
                    },
                    "copy": {
                        "type": "boolean",
                        "default": false,
                        "description": "Copia em vez de mover; a origem permanece."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "default": false,
                        "description": "Substitui um destino existente. Sem isso, destino existente e recusado."
                    },
                    "create_parents": {
                        "type": "boolean",
                        "default": false,
                        "description": "Cria os diretorios intermediarios do destino."
                    },
                    "expected_hash": {
                        "type": "string",
                        "description": "blake3 atual da origem; recusa se o arquivo mudou desde a leitura."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    }
                },
                "required": ["from", "to"]
            }),
        }
    }

    fn call(&self, arguments: Value) -> Result<String, ToolError> {
        let params: MoveParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        let root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let source = paths::resolve_existing(&root, &params.from)?;
        let dest = paths::resolve_for_write(&root, &params.to)?;

        if source == dest {
            return Err(ToolError::InvalidArguments(
                "origem e destino sao o mesmo caminho".to_string(),
            ));
        }
        let source_meta = std::fs::symlink_metadata(&source)
            .map_err(|error| ToolError::Other(format!("falha ao ler {}: {error}", params.from)))?;
        if source_meta.file_type().is_symlink() {
            return Err(ToolError::Other(format!(
                "{} e um symlink; nada foi feito",
                params.from
            )));
        }
        let is_dir = source_meta.is_dir();
        if is_dir && dest.starts_with(&source) {
            return Err(ToolError::InvalidArguments(
                "o destino nao pode ficar dentro da origem".to_string(),
            ));
        }

        let bytes = if !is_dir {
            Some(std::fs::read(&source).map_err(|error| {
                ToolError::Other(format!("falha ao ler {}: {error}", params.from))
            })?)
        } else {
            None
        };
        if let Some(expected) = params.expected_hash.as_deref() {
            let Some(bytes) = &bytes else {
                return Err(ToolError::InvalidArguments(
                    "expected_hash so vale para arquivo".to_string(),
                ));
            };
            if !paths::hash_matches(bytes, expected) {
                return Err(ToolError::StaleHash(format!(
                    "hash atual: {}",
                    paths::hash_prefix(bytes)
                )));
            }
        }

        let dest_exists = dest.exists();
        if dest_exists && !params.overwrite {
            return Err(ToolError::Other(format!(
                "{} ja existe; use overwrite=true",
                params.to
            )));
        }
        if dest_exists && is_dir {
            return Err(ToolError::Other(
                "sobrescrever diretorio nao e suportado".to_string(),
            ));
        }

        if params.create_parents
            && let Some(parent) = dest.parent()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                ToolError::Other(format!(
                    "falha ao criar diretorios de {}: {error}",
                    params.to
                ))
            })?;
        } else if let Some(parent) = dest.parent()
            && !parent.exists()
        {
            return Err(ToolError::NotFound(format!(
                "o diretorio pai de {} nao existe",
                params.to
            )));
        }

        if params.copy {
            if is_dir {
                copy_dir(&source, &dest).map_err(|error| {
                    ToolError::Other(format!("falha ao copiar {}: {error}", params.from))
                })?;
            } else {
                paths::atomic_write(&dest, bytes.as_deref().unwrap_or_default()).map_err(
                    |error| ToolError::Other(format!("falha ao copiar {}: {error}", params.from)),
                )?;
            }
        } else {
            match std::fs::rename(&source, &dest) {
                Ok(()) => {}
                Err(_) if is_dir => {
                    copy_dir(&source, &dest).map_err(|error| {
                        ToolError::Other(format!("falha ao mover {}: {error}", params.from))
                    })?;
                    std::fs::remove_dir_all(&source).map_err(|error| {
                        ToolError::Other(format!(
                            "a copia foi feita, mas a origem nao foi removida: {error}"
                        ))
                    })?;
                }
                Err(_) => {
                    paths::atomic_write(&dest, bytes.as_deref().unwrap_or_default()).map_err(
                        |error| {
                            ToolError::Other(format!("falha ao mover {}: {error}", params.from))
                        },
                    )?;
                    std::fs::remove_file(&source).map_err(|error| {
                        ToolError::Other(format!(
                            "a copia foi feita, mas a origem nao foi removida: {error}"
                        ))
                    })?;
                }
            }
        }

        if !is_dir {
            let written = std::fs::read(&dest).map_err(|error| {
                ToolError::Other(format!(
                    "a operacao foi feita, mas nao releu {}: {error}",
                    params.to
                ))
            })?;
            if written != bytes.as_deref().unwrap_or_default() {
                return Err(ToolError::Other(format!(
                    "verificacao pos-operacao falhou em {}",
                    params.to
                )));
            }
        }

        let references = if !params.copy {
            importers(&root, &source, &dest)
        } else {
            Vec::new()
        };
        let mode = if params.copy { "copiado" } else { "movido" };
        let mut body = String::new();
        body.push_str(&format!(
            "\n- origem: {}\n- destino: {}\n",
            params.from, params.to
        ));
        if let Some(bytes) = &bytes {
            body.push_str(&format!(
                "- blake3: {} · {} bytes\n",
                paths::hash_prefix(bytes),
                bytes.len()
            ));
        } else {
            body.push_str("- diretorio\n");
        }
        if !references.is_empty() {
            body.push_str("- ainda referenciam o nome antigo:\n");
            for reference in &references {
                body.push_str(&format!("  - {reference}\n"));
            }
        }

        let section = Section {
            heading: format!("### {} — {mode}", params.to),
            body,
            priority: 255,
        };
        let budget_tokens = params.budget_tokens.unwrap_or(self.default_budget_tokens);
        Ok(finish(&self.store, vec![section], budget_tokens))
    }

    fn describe_call(&self, arguments: &Value) -> String {
        let from = arguments.get("from").and_then(Value::as_str).unwrap_or("?");
        let to = arguments.get("to").and_then(Value::as_str).unwrap_or("?");
        let copy = arguments
            .get("copy")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let verb = if copy { "cp" } else { "mv" };
        format!("{verb}: {from} -> {to}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(dir: &Path) -> MoveTool {
        MoveTool::new(
            dir.to_path_buf(),
            Arc::new(ResponseStore::new(
                std::time::Duration::from_secs(60),
                1024 * 1024,
            )),
            4000,
        )
    }

    #[test]
    fn a_move_removes_the_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        Tool::call(
            &tool(dir.path()),
            serde_json::json!({"from": "a.txt", "to": "b.txt"}),
        )
        .unwrap();
        assert!(!dir.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "x"
        );
    }

    #[test]
    fn a_copy_keeps_the_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        Tool::call(
            &tool(dir.path()),
            serde_json::json!({"from": "a.txt", "to": "b.txt", "copy": true}),
        )
        .unwrap();
        assert!(dir.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "x"
        );
    }

    #[test]
    fn an_existing_destination_needs_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        std::fs::write(dir.path().join("b.txt"), "y").unwrap();
        assert!(
            Tool::call(
                &tool(dir.path()),
                serde_json::json!({"from": "a.txt", "to": "b.txt"}),
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "y"
        );
    }

    #[test]
    fn a_stale_hash_never_moves() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x").unwrap();
        assert!(Tool::call(
            &tool(dir.path()),
            serde_json::json!({"from": "a.txt", "to": "b.txt", "expected_hash": "000000000000"}),
        )
        .is_err());
        assert!(dir.path().join("a.txt").exists());
    }

    #[test]
    fn a_directory_is_moved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/x.txt"), "x").unwrap();
        Tool::call(
            &tool(dir.path()),
            serde_json::json!({"from": "a", "to": "b"}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b/x.txt")).unwrap(),
            "x"
        );
    }

    #[test]
    fn a_path_outside_the_root_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Tool::call(
                &tool(dir.path()),
                serde_json::json!({"from": "../fora.txt", "to": "b.txt"}),
            )
            .is_err()
        );
    }
}
