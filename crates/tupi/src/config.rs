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
