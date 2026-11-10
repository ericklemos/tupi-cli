pub const KEY: &str = "TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY";
pub const MODEL: &str = "TUPI_MODEL";
pub const LOG: &str = "TUPI_LOG";

const DEFAULT_MODEL: &str = "deepseek-flash";
const DEFAULT_LOG: &str = "warn";

#[derive(Debug)]
pub struct Config {
    pub api_key: String,
    pub model: String,
    pub log_filter: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("variável de ambiente {0} não definida")]
    Missing(&'static str),
}

pub fn from_env<F>(get: F) -> Result<Config, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let api_key = get(KEY)
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::Missing(KEY))?;

    let model = get(MODEL)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let log_filter = get(LOG)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOG.to_string());

    Ok(Config {
        api_key,
        model,
        log_filter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_names_the_variable() {
        let error = from_env(|_| None).unwrap_err();
        assert!(error.to_string().contains(KEY));
    }

    #[test]
    fn model_defaults_to_deepseek_flash() {
        let config = from_env(|key| (key == KEY).then(|| "sk-1".to_string())).unwrap();
        assert_eq!(config.model, "deepseek-flash");
    }

    #[test]
    fn model_can_be_overridden() {
        let config = from_env(|key| match key {
            KEY => Some("sk-1".to_string()),
            MODEL => Some("deepseek-v4-pro".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.model, "deepseek-v4-pro");
    }
}
/// Configuração da compactação de contexto, lida de `context.ron`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContextConfig {
    /// Teto de tokens estimados do contexto enviado ao modelo. Zero desliga
    /// a poda e mantém o histórico inteiro.
    pub budget: usize,
    /// Teto alvo depois do corte amortizado. Zero usa 2/5 de `budget`.
    pub target: usize,
    /// Quantos resultados de tool recentes ficam inteiros na poda.
    pub recent_results: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            budget: tupi_domain::context::DEFAULT_CONTEXT_TOKENS,
            target: 0,
            recent_results: tupi_domain::context::DEFAULT_RECENT_RESULTS_KEPT,
        }
    }
}

use std::path::{Path, PathBuf};

pub fn context_path(root: &Path) -> PathBuf {
    crate::token::token_path(root).with_file_name("context.ron")
}

pub fn parse_context(
    text: &str,
    path: &Path,
) -> Result<ContextConfig, crate::token::FileConfigError> {
    let parsed: ContextConfig =
        ron::from_str(text).map_err(|source| crate::token::FileConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
    Ok(parsed)
}

pub fn read_context(root: &Path) -> Result<ContextConfig, crate::token::FileConfigError> {
    let path = context_path(root);
    let text =
        std::fs::read_to_string(&path).map_err(|source| crate::token::FileConfigError::Read {
            path: path.clone(),
            source,
        })?;
    parse_context(&text, &path)
}

pub fn load_context(root: &Path) -> ContextConfig {
    match read_context(root) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "configuração de contexto indisponível; usando o padrão");
            ContextConfig::default()
        }
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("raiz do repositório")
            .to_path_buf()
    }

    #[test]
    fn the_checked_in_file_is_valid() {
        read_context(&repo_root()).expect("configs/context.ron");
    }

    #[test]
    fn omitted_fields_use_the_defaults() {
        let path = Path::new("context.ron");
        let config = parse_context("()", path).unwrap();
        assert_eq!(config, ContextConfig::default());
    }

    #[test]
    fn explicit_values_win() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(context_path(dir.path()).parent().unwrap()).unwrap();
        std::fs::write(
            context_path(dir.path()),
            "(budget: 64000, target: 16000, recent_results: 6)",
        )
        .unwrap();
        let config = read_context(dir.path()).unwrap();
        assert_eq!(config.budget, 64_000);
        assert_eq!(config.target, 16_000);
        assert_eq!(config.recent_results, 6);
    }

    #[test]
    fn a_broken_file_names_its_path() {
        let path = Path::new("configs/context.ron");
        let error = parse_context("(budget: ", path).unwrap_err();
        assert!(error.to_string().contains("configs/context.ron"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let path = Path::new("context.ron");
        assert!(parse_context("(budget: 1, inventado: true)", path).is_err());
    }

    #[test]
    fn a_missing_file_falls_back_to_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_context(dir.path()), ContextConfig::default());
    }
}
