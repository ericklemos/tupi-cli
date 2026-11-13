use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROJECT_FILE: &str = "ai.project.json";
const FALLBACK_NAME: &str = "project";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Nome do projeto.
    pub name: String,
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
}

pub fn project_path(root: &Path) -> PathBuf {
    root.join(PROJECT_FILE)
}

pub fn parse_project(text: &str, path: &Path) -> Result<ProjectConfig, ProjectConfigError> {
    let parsed: ProjectConfig =
        serde_json::from_str(text).map_err(|source| ProjectConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    if parsed.name.trim().is_empty() {
        return Err(ProjectConfigError::EmptyName);
    }
    Ok(parsed)
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
