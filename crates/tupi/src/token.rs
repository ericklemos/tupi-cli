//! Configurações em `configs/*.ron`, lidas na inicialização.
//!
//! Cada arquivo é opcional: ausência ou erro caem no padrão e geram aviso
//! no log, para que o Tupi nunca deixe de subir por causa de config.

use std::path::{Path, PathBuf};

/// Diretório, relativo à raiz do repositório, onde ficam os `configs/*.ron`.
pub const CONFIG_DIR: &str = "configs";

/// Teto de tokens por resposta usado quando `configs/token.ron` está ausente.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Conteúdo de `configs/token.ron`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TokenConfig {
    /// Teto de tokens por resposta do modelo.
    pub max_tokens: u32,
}

impl Default for TokenConfig {
    fn default() -> Self {
        Self {
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

/// Falhas ao ler ou interpretar um arquivo de `configs/`.
#[derive(Debug, thiserror::Error)]
pub enum FileConfigError {
    #[error("não foi possível ler {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("RON inválido em {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<ron::error::SpannedError>,
    },
    #[error("max_tokens precisa ser maior que zero")]
    ZeroTokens,
    #[error("max_tool_steps precisa ser maior que zero")]
    ZeroToolSteps,
    #[error("system precisa ser não vazio")]
    EmptyPrompt,
}

/// Caminho de `configs/token.ron` sob `root`.
pub fn token_path(root: &Path) -> PathBuf {
    root.join(CONFIG_DIR).join("token.ron")
}

/// Interpreta o texto de `configs/token.ron`.
pub fn parse_token(text: &str, path: &Path) -> Result<TokenConfig, FileConfigError> {
    let parsed: TokenConfig = ron::from_str(text).map_err(|source| FileConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    if parsed.max_tokens == 0 {
        return Err(FileConfigError::ZeroTokens);
    }
    Ok(parsed)
}

/// Lê e interpreta `configs/token.ron` sob `root`.
pub fn read_token(root: &Path) -> Result<TokenConfig, FileConfigError> {
    let path = token_path(root);
    let text = std::fs::read_to_string(&path).map_err(|source| FileConfigError::Read {
        path: path.clone(),
        source,
    })?;
    parse_token(&text, &path)
}

/// Lê o teto de tokens por resposta; ausência ou erro caem no padrão.
pub fn load_token(root: &Path) -> TokenConfig {
    match read_token(root) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "configuração de tokens indisponível; usando o padrão");
            TokenConfig::default()
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
            read_token(&repo_root()).expect("configs/token.ron precisa existir e ser válido");
        assert!(config.max_tokens > 0);
    }

    #[test]
    fn omitted_field_uses_the_default_ceiling() {
        let path = Path::new("token.ron");
        let config = parse_token("()", path).unwrap();
        assert_eq!(config.max_tokens, DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn explicit_ceiling_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(CONFIG_DIR)).unwrap();
        std::fs::write(token_path(dir.path()), "(max_tokens: 1024)").unwrap();
        let config = read_token(dir.path()).unwrap();
        assert_eq!(config.max_tokens, 1024);
    }

    #[test]
    fn zero_ceiling_is_refused() {
        let path = Path::new("configs/token.ron");
        let result = parse_token("(max_tokens: 0)", path);
        assert!(matches!(result, Err(FileConfigError::ZeroTokens)));
    }

    #[test]
    fn a_broken_file_names_its_path() {
        let path = Path::new("configs/token.ron");
        let error = parse_token("(max_tokens: ", path).unwrap_err();
        assert!(error.to_string().contains("configs/token.ron"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let path = Path::new("token.ron");
        assert!(parse_token("(max_tokens: 1, teto: 2)", path).is_err());
    }

    #[test]
    fn a_missing_file_falls_back_to_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_token(dir.path()).max_tokens, DEFAULT_MAX_TOKENS);
    }
}
