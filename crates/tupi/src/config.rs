use std::path::{Path, PathBuf};

const KEY: &str = "DEEPSEEK_API_KEY";
const MODEL: &str = "DEEPSEEK_MODEL";
const PROVIDER: &str = "TUPI_PROVIDER";
const TUPI_MODEL: &str = "TUPI_MODEL";
const LOG: &str = "TUPI_LOG";
const DEFAULT_MODEL: &str = "deepseek-flash";
const DEFAULT_ANTIGRAVITY_MODEL: &str = "antigravity-gemini-3.8-flash";
const DEFAULT_LOG: &str = "info";
const DEFAULT_PROVIDER: &str = "deepseek";

#[derive(Debug)]
pub struct Config {
    pub api_key: String,
    pub model: String,
    pub log_filter: String,
    pub provider: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("variável de ambiente '{0}' ausente ou vazia")]
    Missing(&'static str),
}

pub fn from_env<F>(get: F) -> Result<Config, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let provider = get(PROVIDER)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PROVIDER.to_string());

    let (api_key, default_model) = if provider == "antigravity" {
        (get(KEY).unwrap_or_default(), DEFAULT_ANTIGRAVITY_MODEL)
    } else {
        let key = get(KEY)
            .filter(|value| !value.trim().is_empty())
            .ok_or(ConfigError::Missing(KEY))?;
        (key, DEFAULT_MODEL)
    };

    let model = get(TUPI_MODEL)
        .or_else(|| get(MODEL))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_model.to_string());

    let log_filter = get(LOG)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOG.to_string());

    Ok(Config {
        api_key,
        model,
        log_filter,
        provider,
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
        assert_eq!(config.provider, "deepseek");
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

    #[test]
    fn provider_can_be_set_to_antigravity_without_deepseek_key() {
        let config = from_env(|key| match key {
            "TUPI_PROVIDER" => Some("antigravity".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.provider, "antigravity");
        assert_eq!(config.model, "antigravity-gemini-3.8-flash");
    }

    #[test]
    fn antigravity_model_can_be_overridden() {
        let config = from_env(|key| match key {
            "TUPI_PROVIDER" => Some("antigravity".to_string()),
            "TUPI_MODEL" => Some("antigravity-claude-sonnet-4-6".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(config.model, "antigravity-claude-sonnet-4-6");
    }
    #[test]
    fn embedding_is_enabled_only_with_folders_and_without_off() {
        let folders = vec![std::path::PathBuf::from("docs")];
        assert!(super::docs_embedding_enabled(&folders, None));
        assert!(super::docs_embedding_enabled(&folders, Some("auto")));
        assert!(!super::docs_embedding_enabled(&folders, Some("off")));
        assert!(!super::docs_embedding_enabled(&[], None));
    }
    #[cfg(feature = "local-models")]
    #[test]
    fn real_embedding_model_loads_with_1024_dims() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let path = home.join(
            ".tupi/models/LiquidAI/LFM2.5-Embedding-350M-GGUF/LFM2.5-Embedding-350M-Q8_0.gguf",
        );
        if !path.exists() {
            return;
        }
        let embedder = tupi_infer::LlamaEmbedder::load(&path).expect("modelo carrega");
        let vectors = tupi_kernel::embed::Embedder::embed_documents(&embedder, &["um texto"])
            .expect("embedding");
        assert_eq!(vectors[0].len(), 1024);
    }
    #[test]
    fn summaries_are_enabled_unless_explicitly_off() {
        assert!(super::summaries_enabled(None));
        assert!(super::summaries_enabled(Some("auto")));
        assert!(!super::summaries_enabled(Some("off")));
        assert!(!super::summaries_enabled(Some("0")));
    }

    #[cfg(feature = "local-models")]
    #[test]
    #[ignore = "requer o GGUF LFM3 verificado"]
    fn real_generation_model_produces_a_line() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };
        let path = home.join(".tupi/models/LiquidAI/LFM3-1.7B-GGUF/LFM3-1.7B-Q4_K_M.gguf");
        if !path.exists() {
            return;
        }
        let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        if bytes != tupi_infer::LFM3_BYTES {
            return;
        }
        let generator = tupi_infer::LlamaGenerator::load(&path).expect("modelo carrega");
        let text = tupi_kernel::generate::Generator::generate(
            &generator,
            "Resuma em uma linha: A cobranca e mensal.",
            16,
        )
        .expect("geracao");
        assert!(!text.trim().is_empty());
    }
}

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
            budget: 32_000,
            target: 12_800,
            recent_results: 4,
        }
    }
}

pub fn context_path(root: &Path) -> PathBuf {
    root.join("configs").join("context.ron")
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

#[test]
fn local_embedding_is_needed_for_space_even_without_docs() {
    assert!(local_embedding_enabled(true, &[], None));
    assert!(!local_embedding_enabled(false, &[], None));
    assert!(!local_embedding_enabled(true, &[], Some("off")));
}

#[test]
fn download_follows_the_enabled_consumers() {
    assert!(should_download(true, false));
    assert!(should_download(false, true));
    assert!(!should_download(false, false));
}

/// Autoriza baixar o GGUF: docs mantêm o legado; space exige a flag.
/// A IA local so existe quando a feature `local-models` entrou no build.
pub const LOCAL_MODELS: bool = cfg!(feature = "local-models");

pub fn should_download(docs: bool, space: bool) -> bool {
    // O consentimento é a feature de build (`local-models`), não uma variável
    // de ambiente: se o consumidor está ligado, o modelo pode ser baixado.
    docs || space
}

/// O embedder local é necessário quando há docs declarados ou `space` ligado.
pub fn local_embedding_enabled(
    space: bool,
    folders: &[std::path::PathBuf],
    value: Option<&str>,
) -> bool {
    let disabled = matches!(value.map(str::trim), Some("off" | "0" | "false"));
    // Sem `local-models` não há embedder no binário: nada de download nem de
    // worker vetorial, mesmo com `space` ligado no projeto.
    LOCAL_MODELS && !disabled && (space || !folders.is_empty())
}

pub fn docs_embedding_enabled(folders: &[std::path::PathBuf], value: Option<&str>) -> bool {
    !folders.is_empty() && !matches!(value.map(str::trim), Some("off" | "0" | "false"))
}

pub fn summaries_enabled(value: Option<&str>) -> bool {
    // A pasta de documentação deixou de ser condição: quem liga o gerador é a
    // feature `local-models`, e o download só acontece quando existe consumidor
    // (a tool `docs`, que por sua vez precisa de pasta declarada).
    LOCAL_MODELS && !matches!(value.map(str::trim), Some("off" | "0" | "false"))
}
