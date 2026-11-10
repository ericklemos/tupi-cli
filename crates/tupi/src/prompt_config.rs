use std::path::{Path, PathBuf};

use crate::token::{CONFIG_DIR, FileConfigError};

/// Conteúdo de `configs/prompts.ron`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PromptConfig {
    /// Texto que configura o agente.
    pub system: String,
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            system: tupi_app::prompt::TUPI_SYSTEM.to_string(),
        }
    }
}

pub fn prompt_path(root: &Path) -> PathBuf {
    root.join(CONFIG_DIR).join("prompts.ron")
}

pub fn parse_prompt(text: &str, path: &Path) -> Result<PromptConfig, FileConfigError> {
    let parsed: PromptConfig = ron::from_str(text).map_err(|source| FileConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    if parsed.system.trim().is_empty() {
        return Err(FileConfigError::EmptyPrompt);
    }
    Ok(parsed)
}

pub fn read_prompt(root: &Path) -> Result<PromptConfig, FileConfigError> {
    let path = prompt_path(root);
    let text = std::fs::read_to_string(&path).map_err(|source| FileConfigError::Read {
        path: path.clone(),
        source,
    })?;
    parse_prompt(&text, &path)
}

pub fn load_prompt(root: &Path) -> PromptConfig {
    match read_prompt(root) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "prompt do sistema indisponível; usando o padrão");
            PromptConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_checked_in_file_is_valid() {
        let config =
            read_prompt(&repo_root()).expect("configs/prompts.ron precisa existir e ser válido");
        assert_eq!(config.system, tupi_app::prompt::TUPI_SYSTEM);
        assert!(config.system.contains("`map(depth=\"dirs\")`"));
    }

    #[test]
    fn omitted_field_uses_the_default_prompt() {
        let path = Path::new("prompts.ron");
        let config = parse_prompt("()", path).unwrap();
        assert_eq!(config.system, tupi_app::prompt::TUPI_SYSTEM);
    }

    #[test]
    fn explicit_prompt_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(CONFIG_DIR)).unwrap();
        std::fs::write(prompt_path(dir.path()), r#"(system: "PERSONA")"#).unwrap();
        let config = read_prompt(dir.path()).unwrap();
        assert_eq!(config.system, "PERSONA");
    }

    #[test]
    fn empty_prompt_is_refused() {
        let path = Path::new("configs/prompts.ron");
        let result = parse_prompt("(system: \"\")", path);
        assert!(matches!(result, Err(FileConfigError::EmptyPrompt)));
        let result = parse_prompt("(system: \"   \")", path);
        assert!(matches!(result, Err(FileConfigError::EmptyPrompt)));
    }

    #[test]
    fn a_broken_file_names_its_path() {
        let path = Path::new("configs/prompts.ron");
        let error = parse_prompt("(system: ", path).unwrap_err();
        assert!(error.to_string().contains("configs/prompts.ron"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let path = Path::new("prompts.ron");
        assert!(parse_prompt("(system: \"x\", extra: 1)", path).is_err());
    }

    #[test]
    fn a_missing_file_falls_back_to_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_prompt(dir.path()).system,
            tupi_app::prompt::TUPI_SYSTEM
        );
    }

    #[test]
    fn an_empty_file_falls_back_to_the_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(CONFIG_DIR)).unwrap();
        std::fs::write(prompt_path(dir.path()), "").unwrap();
        assert_eq!(
            load_prompt(dir.path()).system,
            tupi_app::prompt::TUPI_SYSTEM
        );
    }

    #[test]
    fn a_non_utf8_file_falls_back_to_the_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(CONFIG_DIR)).unwrap();
        std::fs::write(prompt_path(dir.path()), [0xff, 0xfe]).unwrap();
        assert_eq!(
            load_prompt(dir.path()).system,
            tupi_app::prompt::TUPI_SYSTEM
        );
    }

    #[test]
    fn the_loaded_prompt_goes_through_compose() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(CONFIG_DIR)).unwrap();
        std::fs::write(prompt_path(dir.path()), r#"(system: "PERSONA")"#).unwrap();
        let prefix = tupi_app::prompt::compose(&load_prompt(dir.path()).system);
        assert_eq!(prefix, "PERSONA");
    }
}
