//! Limite de tool calls por turno em `configs/tools.ron`.
//!
//! O arquivo é opcional: ausência ou erro caem no padrão e geram aviso
//! no log, para que o Tupi nunca deixe de subir por causa de config.

use std::path::{Path, PathBuf};

use crate::token::{CONFIG_DIR, FileConfigError};

/// Limite de passos de tool call usado quando `configs/tools.ron` está ausente.
pub const DEFAULT_MAX_TOOL_STEPS: usize = 30;

/// Conteúdo de `configs/tools.ron`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolConfig {
    /// Quantos passos com tool calls o modelo pode dar por turno.
    pub max_tool_steps: usize,
    /// Nomes das tools expostas ao modelo. Vazio deixa só o conjunto núcleo.
    #[serde(default)]
    pub expose: Vec<String>,
    /// Expõe todas as tools registradas, ignorando `expose`.
    #[serde(default)]
    pub expose_all: bool,
}

impl ToolConfig {
    /// Perfil de exposição de tools derivado desta configuração.
    pub fn exposure(&self) -> tupi_tools::Exposure<'_> {
        if self.expose_all {
            tupi_tools::Exposure::All
        } else if self.expose.is_empty() {
            tupi_tools::Exposure::Core
        } else {
            tupi_tools::Exposure::Only(&self.expose)
        }
    }
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            max_tool_steps: DEFAULT_MAX_TOOL_STEPS,
            expose: Vec::new(),
            expose_all: false,
        }
    }
}

/// Caminho de `configs/tools.ron` sob `root`.
pub fn tools_path(root: &Path) -> PathBuf {
    root.join(CONFIG_DIR).join("tools.ron")
}

/// Interpreta o texto de `configs/tools.ron`.
pub fn parse_tools(text: &str, path: &Path) -> Result<ToolConfig, FileConfigError> {
    let parsed: ToolConfig = ron::from_str(text).map_err(|source| FileConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    if parsed.max_tool_steps == 0 {
        return Err(FileConfigError::ZeroToolSteps);
    }
    Ok(parsed)
}

/// Lê e interpreta `configs/tools.ron` sob `root`.
pub fn read_tools(root: &Path) -> Result<ToolConfig, FileConfigError> {
    let path = tools_path(root);
    let text = std::fs::read_to_string(&path).map_err(|source| FileConfigError::Read {
        path: path.clone(),
        source,
    })?;
    parse_tools(&text, &path)
}

/// Lê o limite de passos; ausência ou erro caem no padrão.
pub fn load_tools(root: &Path) -> ToolConfig {
    match read_tools(root) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "configuração de tool calls indisponível; usando o padrão");
            ToolConfig::default()
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
            read_tools(&repo_root()).expect("configs/tools.ron precisa existir e ser válido");
        assert!(config.max_tool_steps > 0);
    }

    #[test]
    fn omitted_field_uses_the_default_limit() {
        let path = Path::new("tools.ron");
        let config = parse_tools("()", path).unwrap();
        assert_eq!(config.max_tool_steps, DEFAULT_MAX_TOOL_STEPS);
    }

    #[test]
    fn explicit_limit_wins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(CONFIG_DIR)).unwrap();
        std::fs::write(tools_path(dir.path()), "(max_tool_steps: 12)").unwrap();
        let config = read_tools(dir.path()).unwrap();
        assert_eq!(config.max_tool_steps, 12);
    }

    #[test]
    fn zero_limit_is_refused() {
        let path = Path::new("configs/tools.ron");
        let result = parse_tools("(max_tool_steps: 0)", path);
        assert!(matches!(result, Err(FileConfigError::ZeroToolSteps)));
    }

    #[test]
    fn a_broken_file_names_its_path() {
        let path = Path::new("configs/tools.ron");
        let error = parse_tools("(max_tool_steps: ", path).unwrap_err();
        assert!(error.to_string().contains("configs/tools.ron"), "{error}");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let path = Path::new("tools.ron");
        assert!(parse_tools("(max_tool_steps: 1, passos: 2)", path).is_err());
    }

    #[test]
    fn a_missing_file_falls_back_to_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_tools(dir.path()).max_tool_steps,
            DEFAULT_MAX_TOOL_STEPS
        );
    }

    #[test]
    fn default_config_registers_only_core_tools_when_wired_to_register_exposed() {
        let cfg = ToolConfig::default();
        let mut exposed = tupi_kernel::ToolRegistry::new();
        tupi_tools::register_exposed(repo_root(), &mut exposed, cfg.exposure());

        let mut all = tupi_kernel::ToolRegistry::new();
        tupi_tools::register(repo_root(), &mut all);

        assert!(exposed.specs().len() < all.specs().len());
        assert!(exposed.has("read"));
        assert!(exposed.has("edit"));
        assert!(exposed.has("search"));
        assert!(exposed.has("map"));
    }
}
