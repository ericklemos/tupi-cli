//! Configuração do acervo persistente do modo docs (`ai.project.json` → `archive`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::project_config::ProjectConfig;

fn yes() -> bool {
    true
}

fn default_exclude() -> Vec<String> {
    vec!["**/.tupi/**".to_string()]
}

fn default_language() -> String {
    "pt-BR".to_string()
}

fn default_max_records() -> usize {
    5000
}

fn default_concurrency() -> usize {
    4
}

fn default_batch() -> usize {
    32
}

/// Tipos de mídia que entram no índice.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct MediaConfig {
    #[serde(default = "yes")]
    pub ocr: bool,
    #[serde(default = "yes")]
    pub asr: bool,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            ocr: yes(),
            asr: yes(),
        }
    }
}

/// Tamanho alvo de um shard (um dia, cortado por quantidade).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ShardConfig {
    #[serde(default = "default_max_records")]
    pub max_records: usize,
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            max_records: default_max_records(),
        }
    }
}

/// Limites da fila de ingestão e da adjudicação.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct JobsConfig {
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_batch")]
    pub batch: usize,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            concurrency: default_concurrency(),
            batch: default_batch(),
        }
    }
}

/// Onde o julgamento dos candidatos acontece.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AdjudicationConfig {
    /// true: modelo local (`tupi-infer`); false: provedor configurado.
    #[serde(default = "yes")]
    pub local: bool,
}

impl Default for AdjudicationConfig {
    fn default() -> Self {
        Self { local: yes() }
    }
}

/// Seção `archive` do `ai.project.json`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ArchiveConfig {
    /// Raízes do acervo; `~` é expandido na leitura.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Padrões ignorados pela ingestão.
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub media: MediaConfig,
    #[serde(default)]
    pub shard: ShardConfig,
    #[serde(default)]
    pub jobs: JobsConfig,
    #[serde(default)]
    pub adjudication: AdjudicationConfig,
    /// Idioma do acervo; governa fonética e sinônimos.
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            exclude: default_exclude(),
            media: MediaConfig::default(),
            shard: ShardConfig::default(),
            jobs: JobsConfig::default(),
            adjudication: AdjudicationConfig::default(),
            language: default_language(),
        }
    }
}

impl ProjectConfig {
    /// A seção `archive`, quando declarada.
    pub fn archive_config(&self) -> Option<&ArchiveConfig> {
        self.archive.as_ref()
    }

    /// Raízes resolvidas: `~` vira o home, relativas viram `root/...`.
    pub fn archive_roots(&self, root: &Path) -> Vec<PathBuf> {
        let Some(archive) = self.archive.as_ref() else {
            return Vec::new();
        };
        archive
            .roots
            .iter()
            .map(|entry| resolve_root(root, entry))
            .collect()
    }
}

fn resolve_root(root: &Path, entry: &str) -> PathBuf {
    let trimmed = entry.trim();
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_config::parse_project;

    fn config(text: &str) -> ProjectConfig {
        parse_project(text, Path::new("ai.project.json")).unwrap()
    }

    #[test]
    fn archive_section_defaults_and_expands_the_home_root() {
        let cfg = config(r#"{ "name": "p", "archive": { "roots": ["~/acervo"] } }"#);
        let archive = cfg.archive_config().expect("archive");
        assert_eq!(archive.jobs.concurrency, 4);
        assert_eq!(archive.shard.max_records, 5000);
        assert!(archive.adjudication.local);
        assert!(archive.media.asr);
        assert_eq!(archive.language, "pt-BR");
        assert_eq!(archive.exclude, vec!["**/.tupi/**".to_string()]);
        let roots = cfg.archive_roots(Path::new("/repo"));
        assert!(roots[0].to_string_lossy().ends_with("acervo"));
    }

    #[test]
    fn missing_archive_section_means_the_mode_is_unavailable() {
        assert!(config(r#"{ "name": "p" }"#).archive_config().is_none());
        assert!(
            config(r#"{ "name": "p" }"#)
                .archive_roots(Path::new("/repo"))
                .is_empty()
        );
    }

    #[test]
    fn relative_roots_resolve_against_the_project_root() {
        let cfg = config(r#"{ "name": "p", "archive": { "roots": ["dados"] } }"#);
        assert_eq!(
            cfg.archive_roots(Path::new("/repo")),
            vec![PathBuf::from("/repo/dados")]
        );
    }
}
