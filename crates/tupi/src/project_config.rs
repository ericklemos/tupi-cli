use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROJECT_FILE: &str = "ai.project.json";
const FALLBACK_NAME: &str = "project";

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Janela de contexto do modelo, em tokens. Ausente ou zero mantém o
    /// default de `configs/context.ron`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
}

/// Configuração do projeto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Nome do projeto.
    pub name: String,
    /// Pastas com documentação do projeto, relativas ao root.
    ///
    /// Entradas vazias, absolutas ou com `..` são recusadas na carga.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub docs_folders: Vec<String>,
    /// Janelas de contexto por modelo. A chave é `provider/modelo`, com
    /// fallback para apenas o nome do modelo.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, ModelConfig>,
    /// Porta local do dashboard web. Ausente ou zero deixa o SO escolher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_web_port: Option<u16>,
}

impl ProjectConfig {
    /// Pastas de documentação do projeto, ancoradas no root.
    pub fn docs_folders(&self, root: &Path) -> Vec<PathBuf> {
        self.docs_folders
            .iter()
            .map(|folder| root.join(folder))
            .collect()
    }
}


#[derive(Debug, Error)]
pub enum ProjectConfigError {
    #[error("não foi possível ler {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("não foi possível escrever {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("não foi possível serializar {path}: {source}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("JSON inválido em {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("name precisa ser não vazio")]
    EmptyName,
    #[error(
        "docs_folders inválido em {path}: {entry:?} precisa ser um caminho relativo ao root, não vazio e sem `..`"
    )]
    InvalidDocsFolder { path: PathBuf, entry: String },
}

pub fn project_path(root: &Path) -> PathBuf {
    root.join(PROJECT_FILE)
}

pub fn parse_project(text: &str, path: &Path) -> Result<ProjectConfig, ProjectConfigError> {
    fn normalize_docs_folders(
        path: &Path,
        entries: &[String],
    ) -> Result<Vec<String>, ProjectConfigError> {
        use std::path::Component;

        let invalid = |entry: &String| ProjectConfigError::InvalidDocsFolder {
            path: path.to_path_buf(),
            entry: entry.clone(),
        };
        let mut folders: Vec<String> = Vec::with_capacity(entries.len());
        for entry in entries {
            let candidate = Path::new(entry.trim());
            if candidate.as_os_str().is_empty() || candidate.is_absolute() {
                return Err(invalid(entry));
            }
            if candidate.components().any(|component| {
                matches!(
                    component,
                    Component::Prefix(_) | Component::RootDir | Component::ParentDir
                )
            }) {
                return Err(invalid(entry));
            }
            let normalized = candidate
                .components()
                .filter(|component| !matches!(component, Component::CurDir))
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if normalized.is_empty() {
                return Err(invalid(entry));
            }
            if !folders.contains(&normalized) {
                folders.push(normalized);
            }
        }
        Ok(folders)
    }

    let parsed: ProjectConfig =
        serde_json::from_str(text).map_err(|source| ProjectConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if parsed.name.trim().is_empty() {
        return Err(ProjectConfigError::EmptyName);
    }
    let docs_folders = normalize_docs_folders(path, &parsed.docs_folders)?;
    Ok(ProjectConfig {
        docs_folders,
        ..parsed
    })
}

pub fn read_project(root: &Path) -> Result<ProjectConfig, ProjectConfigError> {
    let path = project_path(root);
    let text = std::fs::read_to_string(&path).map_err(|source| ProjectConfigError::Read {
        path: path.clone(),
        source,
    })?;
    parse_project(&text, &path)
}

pub fn load_or_create(root: &Path) -> ProjectConfig {
    let path = project_path(root);
    match read_project(root) {
        Ok(config) => config,
        Err(ProjectConfigError::Read { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            let config = fallback(root);
            if let Err(error) = write_project(&path, &config) {
                tracing::warn!(%error, "não foi possível criar a configuração do projeto");
            }
            config
        }
        Err(error) => {
            tracing::warn!(
                %error,
                path = %path.display(),
                "configuração do projeto indisponível; usando o nome da pasta"
            );
            fallback(root)
        }
    }
}

fn fallback(root: &Path) -> ProjectConfig {
    ProjectConfig {
        name: fallback_name(root),
        docs_folders: Vec::new(),
        models: Default::default(),
        dashboard_web_port: None,
    }
}


fn fallback_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| FALLBACK_NAME.to_string())
}

fn write_project(path: &Path, config: &ProjectConfig) -> Result<(), ProjectConfigError> {
    let mut text =
        serde_json::to_string_pretty(config).map_err(|source| ProjectConfigError::Serialize {
            path: path.to_path_buf(),
            source,
        })?;
    text.push('\n');
    std::fs::write(path, text).map_err(|source| ProjectConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    fn folder(dir: &Path) -> String {
        dir.file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn a_missing_file_is_created_with_the_folder_name() {
        let dir = tempdir().unwrap();
        let config = load_or_create(dir.path());
        assert_eq!(config.name, folder(dir.path()));
        let text = std::fs::read_to_string(project_path(dir.path())).unwrap();
        assert_eq!(
            text,
            format!("{}\n", serde_json::to_string_pretty(&config).unwrap())
        );
    }

    #[test]
    fn an_existing_file_wins() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());
        std::fs::write(&path, r#"{"name":"outro"}"#).unwrap();
        assert_eq!(load_or_create(dir.path()).name, "outro");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"name":"outro"}"#
        );
    }

    #[test]
    fn an_invalid_file_falls_back_without_overwriting() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());
        std::fs::write(&path, "{").unwrap();
        assert_eq!(load_or_create(dir.path()).name, folder(dir.path()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{");
    }

    #[test]
    fn an_empty_name_falls_back() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());
        std::fs::write(&path, r#"{"name":"   "}"#).unwrap();
        assert_eq!(load_or_create(dir.path()).name, folder(dir.path()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"name":"   "}"#);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let dir = tempdir().unwrap();
        std::fs::write(project_path(dir.path()), r#"{"name":"x","future":true}"#).unwrap();
        assert_eq!(load_or_create(dir.path()).name, "x");
    }

    #[test]
    fn a_root_without_a_file_name_uses_the_fallback() {
        assert_eq!(fallback_name(Path::new("/")), "project");
    }
}

#[cfg(test)]
mod dashboard_port_tests {
    use super::*;

    fn config(text: &str) -> ProjectConfig {
        parse_project(text, Path::new("ai.project.json")).unwrap()
    }

    #[test]
    fn dashboard_web_port_is_read_when_present() {
        let parsed = config(r#"{"name":"tupi-cli","dashboard_web_port":7788}"#);
        assert_eq!(parsed.dashboard_web_port, Some(7788));
    }

    #[test]
    fn dashboard_web_port_is_absent_by_default() {
        let parsed = config(r#"{"name":"tupi-cli"}"#);
        assert_eq!(parsed.dashboard_web_port, None);
    }

    #[test]
    fn a_port_out_of_range_is_refused() {
        assert!(
            parse_project(
                r#"{"name":"tupi-cli","dashboard_web_port":70000}"#,
                Path::new("ai.project.json")
            )
            .is_err()
        );
    }

    #[test]
    fn an_absent_port_is_not_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai.project.json");
        let parsed = config(r#"{"name":"tupi-cli"}"#);
        write_project(&path, &parsed).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("dashboard_web_port"), "texto: {text}");
    }
}
