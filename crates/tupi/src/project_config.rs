use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROJECT_FILE: &str = "ai.project.json";
const FALLBACK_NAME: &str = "project";

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Configuração do índice vetorial local ("space").
pub struct SpaceConfig {
    /// `false` desliga o worker e a fusão densa. Ausente = ligado.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Língua da documentação. Ausente na leitura é preenchido a partir do
    /// locale do sistema por `load_or_create`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<Lang>,
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
    /// Índice vetorial local do repositório. Ausente liga por padrão.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<SpaceConfig>,
}

pub use crate::lang::Lang;
/// Pasta de documentação padrão quando `docs_folders` está ausente ou vazio.
const DEFAULT_DOCS_FOLDER: &str = "docs";
impl ProjectConfig {
    /// Pastas de documentação resolvidas contra `root`, na ordem do arquivo.
    /// O espaço vetorial local está ligado? Ausente = sim.
    pub fn space_enabled(&self) -> bool {
        self.space
            .as_ref()
            .and_then(|space| space.enabled)
            .unwrap_or(true)
    }

    pub fn docs_folders(&self, root: &Path) -> Vec<PathBuf> {
        self.docs_folders
            .iter()
            .map(|folder| root.join(folder))
            .collect()
    }

    /// Chave `provider/modelo` usada em `models`.
    ///
    /// `None` quando provider ou modelo está vazio: `"/modelo"` nunca
    /// resolveria na leitura, então gravá-la seria só sujeira no arquivo.
    pub fn model_key(provider: &str, model: &str) -> Option<String> {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() {
            return None;
        }
        Some(format!("{provider}/{model}"))
    }

    /// Grava `models[key].context_window` em `ai.project.json`.
    ///
    /// O merge acontece sobre `serde_json::Value` porque a config ignora
    /// campos desconhecidos: reserializar `ProjectConfig` apagaria o que o
    /// usuário escreveu à mão. `Ok(false)` quando nada foi escrito — entrada
    /// já presente com `context_window`, `key` vazia ou janela zero (zero
    /// desliga a poda e só poluiria o arquivo). Erro de leitura ou parse não
    /// toca no arquivo.
    pub fn record_model_window(
        root: &Path,
        key: &str,
        context_window: usize,
    ) -> Result<bool, ProjectConfigError> {
        let path = project_path(root);
        if key.trim().is_empty() || context_window == 0 {
            return Ok(false);
        }

        let value = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<serde_json::Value>(&text).map_err(|source| {
                ProjectConfigError::Parse {
                    path: path.clone(),
                    source,
                }
            })?,
            // Arquivo ausente nasce com o mesmo fallback de `load_or_create`.
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                serde_json::to_value(fallback(root)).map_err(|source| {
                    ProjectConfigError::Serialize {
                        path: path.clone(),
                        source,
                    }
                })?
            }
            Err(source) => {
                return Err(ProjectConfigError::Read {
                    path: path.clone(),
                    source,
                });
            }
        };

        let serde_json::Value::Object(mut object) = value else {
            return Err(ProjectConfigError::NotAnObject { path });
        };

        if !matches!(object.get("models"), Some(serde_json::Value::Object(_))) {
            object.insert(
                "models".to_string(),
                serde_json::Value::Object(serde_json::Map::new()),
            );
        }
        let models = object
            .get_mut("models")
            .and_then(|models| models.as_object_mut())
            .expect("models garantido como objeto acima");

        if !matches!(models.get(key), Some(serde_json::Value::Object(_))) {
            models.insert(
                key.to_string(),
                serde_json::Value::Object(serde_json::Map::new()),
            );
        }
        let entry = models
            .get_mut(key)
            .and_then(|entry| entry.as_object_mut())
            .expect("models[key] garantido como objeto acima");

        // Valor presente (inclusive escrito à mão) é intenção preservada.
        if entry
            .get("context_window")
            .is_some_and(|window| !window.is_null())
        {
            return Ok(false);
        }
        entry.insert(
            "context_window".to_string(),
            serde_json::json!(context_window),
        );

        let mut text =
            serde_json::to_string_pretty(&serde_json::Value::Object(object)).map_err(|source| {
                ProjectConfigError::Serialize {
                    path: path.clone(),
                    source,
                }
            })?;
        text.push('\n');
        std::fs::write(&path, text).map_err(|source| ProjectConfigError::Write { path, source })?;
        Ok(true)
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
    #[error("{path} não tem a forma de um objeto JSON de configuração")]
    NotAnObject { path: PathBuf },
    #[error("name precisa ser não vazio")]
    EmptyName,
    #[error(
        "docs_folders inválido em {path}: {entry:?} precisa ser um caminho relativo ao root, não vazio e sem `..`"
    )]
    InvalidDocsFolder { path: PathBuf, entry: String },
    #[error("lang inválido em {path}: {value:?} precisa ser `pt-BR` ou `en`")]
    InvalidLang { path: PathBuf, value: String },
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
        // Lista ausente ou vazia usa o default do projeto: `docs/`.
        if folders.is_empty() {
            return Ok(vec![DEFAULT_DOCS_FOLDER.to_string()]);
        }
        Ok(folders)
    }

    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|source| ProjectConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
    // `lang` é validado aqui, e não pelo serde, para o erro carregar o caminho
    // do arquivo e o valor recusado, como em `docs_folders`.
    if let Some(raw) = value.get("lang").filter(|raw| !raw.is_null()) {
        let raw = match raw {
            serde_json::Value::String(raw) => raw.clone(),
            other => other.to_string(),
        };
        if Lang::parse(&raw).is_none() {
            return Err(ProjectConfigError::InvalidLang {
                path: path.to_path_buf(),
                value: raw,
            });
        }
    }
    let parsed: ProjectConfig =
        serde_json::from_value(value).map_err(|source| ProjectConfigError::Parse {
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
        Ok(config) => crate::lang::record(&path, config),
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
        lang: Some(Lang::from_system()),
        docs_folders: vec![DEFAULT_DOCS_FOLDER.to_string()],
        models: Default::default(),
        dashboard_web_port: None,
        space: None,
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
        assert_eq!(config.docs_folders, vec!["docs".to_string()]);
        assert_eq!(config.lang, Some(Lang::from_system()));
        let text = std::fs::read_to_string(project_path(dir.path())).unwrap();
        assert_eq!(
            text,
            format!("{}\n", serde_json::to_string_pretty(&config).unwrap())
        );
    }

    #[test]
    fn a_missing_file_gains_the_model_window_of_the_key() {
        let dir = tempdir().unwrap();
        let key = "anthropic/claude";
        assert!(ProjectConfig::record_model_window(dir.path(), key, 4096).unwrap());

        let text = std::fs::read_to_string(project_path(dir.path())).unwrap();
        assert!(text.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["models"][key]["context_window"],
            serde_json::json!(4096)
        );

        let config = read_project(dir.path()).unwrap();
        assert_eq!(config.name, folder(dir.path()));
        assert_eq!(config.models[key].context_window, Some(4096));
    }

    #[test]
    fn recording_a_window_keeps_unknown_fields_and_other_entries() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());
        std::fs::write(
            &path,
            r#"{
  "name": "projeto",
  "future": 1,
  "docs_folders": ["docs"],
  "models": {"openai/gpt": {"context_window": 8192}}
}"#,
        )
        .unwrap();

        assert!(ProjectConfig::record_model_window(dir.path(), "anthropic/claude", 4096).unwrap());

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["name"], serde_json::json!("projeto"));
        assert_eq!(value["future"], serde_json::json!(1));
        assert_eq!(value["docs_folders"], serde_json::json!(["docs"]));
        assert_eq!(
            value["models"]["openai/gpt"]["context_window"],
            serde_json::json!(8192)
        );
        assert_eq!(
            value["models"]["anthropic/claude"]["context_window"],
            serde_json::json!(4096)
        );

        let config = read_project(dir.path()).unwrap();
        assert_eq!(config.name, "projeto");
        assert_eq!(
            config.docs_folders(dir.path()),
            vec![dir.path().join("docs")]
        );
        assert_eq!(config.models["openai/gpt"].context_window, Some(8192));
        assert_eq!(config.models["anthropic/claude"].context_window, Some(4096));
    }

    #[test]
    fn an_existing_window_and_a_zero_default_are_left_untouched() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());
        let original = "{\n  \"models\": {\n    \"anthropic/claude\": {\n      \"context_window\": 1024\n    }\n  }\n}\n";
        std::fs::write(&path, original).unwrap();

        // Valor já preenchido pelo usuário nunca é sobrescrito.
        assert!(!ProjectConfig::record_model_window(dir.path(), "anthropic/claude", 4096).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        // Janela zero desliga a poda: não cria entrada nenhuma.
        assert!(!ProjectConfig::record_model_window(dir.path(), "openai/gpt", 0).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn an_empty_provider_or_model_is_refused() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());

        for (provider, model) in [("", "model_2"), ("prov_b", ""), ("   ", "model_2")] {
            assert_eq!(ProjectConfig::model_key(provider, model), None);
            let key = ProjectConfig::model_key(provider, model).unwrap_or_default();
            assert!(!ProjectConfig::record_model_window(dir.path(), &key, 4096).unwrap());
            assert!(
                !path.exists(),
                "chave vazia não pode criar nem tocar o arquivo"
            );
        }

        assert_eq!(
            ProjectConfig::model_key("prov_b", "model_2").as_deref(),
            Some("prov_b/model_2")
        );
    }

    #[test]
    fn an_existing_file_wins() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());
        std::fs::write(&path, r#"{"name":"outro","lang":"en"}"#).unwrap();
        assert_eq!(load_or_create(dir.path()).name, "outro");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"name":"outro","lang":"en"}"#
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

    #[test]
    fn an_invalid_file_is_not_rewritten_by_a_recorded_window() {
        let dir = tempdir().unwrap();
        let path = project_path(dir.path());
        let original = "{\n  \"models\": {\n";
        std::fs::write(&path, original).unwrap();

        let result = ProjectConfig::record_model_window(dir.path(), "anthropic/claude", 4096);
        assert!(
            result.is_err(),
            "JSON inválido precisa voltar como erro, não como sucesso: {result:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
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

#[test]
fn space_is_enabled_by_default() {
    assert!(config(r#"{"name":"x"}"#).space_enabled());
}

#[test]
fn space_can_be_disabled_in_the_project_file() {
    assert!(!config(r#"{"name":"x","space":{"enabled":false}}"#).space_enabled());
}

#[cfg(test)]
fn config(text: &str) -> ProjectConfig {
    parse_project(text, Path::new("ai.project.json")).unwrap()
}
