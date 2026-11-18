//! Verificação pós-escrita declarada por glob em `configs/verify.ron`.
//!
//! O juiz é o verificador do repositório (`cargo fmt --check`, `tsc --noEmit`,
//! `ruff`, `pytest`, `sqlfluff`, `markdownlint`), nunca o parser do tupi: a
//! configuração guarda o comando pronto e o glob de caminho que o dispara.
//! O comando é dado, não código por linguagem — acrescentar um ecossistema é
//! acrescentar uma regra.
//!
//! Formato (`configs/verify.ron`), opcional: ausente ou com `rules: []`
//! significa nenhuma verificação, que é o opt-in do projeto.
//!
//! ```ron
//! (
//!     rules: [
//!         (glob: "**/*.md", program: "markdownlint", args: ["--quiet"]),
//!         (glob: "**/*.ts", program: "tsc", args: ["--noEmit"], timeout_secs: 120),
//!     ],
//! )
//! ```
//!
//! Resolução: as regras casam na ordem de declaração e a primeira que casa
//! vence; glob sem `/` casa o nome do arquivo em qualquer profundidade, glob
//! com `/` casa o caminho relativo à raiz. Caminho sem glob correspondente é
//! no-op explícito (`resolve` devolve `None`), não erro: quem não declarou
//! nada não muda de comportamento.

use std::path::{Path, PathBuf};

pub mod anchors;
mod run;

pub use run::{VerifyOutcome, VerifyReport, run_verification};

/// Nome do arquivo de configuração, dentro de `configs/`.
pub const VERIFY_FILE: &str = "verify.ron";

/// Teto de tempo usado quando a regra não declara `timeout_secs`.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Uma regra: glob de caminho e o comando do verificador que o cobre.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyRule {
    /// Glob de caminho relativo à raiz, como `**/*.ts` ou `*.md`.
    pub glob: String,
    /// Programa do verificador, como `tsc` ou `cargo`.
    pub program: String,
    /// Argumentos do programa, na ordem.
    #[serde(default)]
    pub args: Vec<String>,
    /// Teto de tempo em segundos; ausente usa `DEFAULT_TIMEOUT_SECS`.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl VerifyRule {
    /// Comando como aparece na recusa, no log e na telemetria.
    pub fn command_line(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(self.program.as_str());
        parts.extend(self.args.iter().map(String::as_str));
        parts.join(" ")
    }

    /// Teto de tempo efetivo, em segundos.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS)
    }

    /// Se a regra cobre `rel_path` (já relativo à raiz, separado por `/`).
    fn covers(&self, rel_path: &str) -> bool {
        let pattern = self.glob.trim();
        match pattern.contains('/') {
            true => glob_match(pattern, rel_path),
            false => glob_match(pattern, basename(rel_path)),
        }
    }
}

/// Conteúdo de `configs/verify.ron`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VerifyConfig {
    /// Regras na ordem em que resolvem; a primeira que casa vence.
    pub rules: Vec<VerifyRule>,
}

impl VerifyConfig {
    /// Regra aplicável a `rel_path`, ou `None` quando nenhum glob cobre o
    /// caminho — no-op explícito, não erro.
    pub fn resolve(&self, rel_path: &Path) -> Option<&VerifyRule> {
        let normalized = normalize(rel_path);
        self.rules.iter().find(|rule| rule.covers(&normalized))
    }

    /// Sem regras não há verificação declarada.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Falhas ao ler ou interpretar `configs/verify.ron`.
#[derive(Debug, thiserror::Error)]
pub enum VerifyConfigError {
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
    #[error("glob vazio em {path}: toda regra precisa declarar o glob que cobre")]
    EmptyGlob { path: PathBuf },
    #[error(
        "comando vazio para o glob `{glob}` em {path}: glob sem comando é no-op, declare `program` ou remova a regra"
    )]
    EmptyProgram { path: PathBuf, glob: String },
    #[error("timeout_secs precisa ser maior que zero no glob `{glob}` em {path}")]
    ZeroTimeout { path: PathBuf, glob: String },
}

impl VerifyConfigError {
    /// Mensagem no padrão de recusa que ensina: o que falhou e o que fazer.
    pub fn to_message(&self) -> String {
        self.to_string()
    }
}

/// Caminho de `configs/verify.ron` sob `root`.
pub fn verify_path(root: &Path) -> PathBuf {
    root.join("configs").join(VERIFY_FILE)
}

/// Interpreta o texto de `configs/verify.ron`.
pub fn parse_verify(text: &str, path: &Path) -> Result<VerifyConfig, VerifyConfigError> {
    let parsed: VerifyConfig = ron::from_str(text).map_err(|source| VerifyConfigError::Parse {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    for rule in &parsed.rules {
        if rule.glob.trim().is_empty() {
            return Err(VerifyConfigError::EmptyGlob {
                path: path.to_path_buf(),
            });
        }
        if rule.program.trim().is_empty() {
            return Err(VerifyConfigError::EmptyProgram {
                path: path.to_path_buf(),
                glob: rule.glob.clone(),
            });
        }
        if rule.timeout_secs == Some(0) {
            return Err(VerifyConfigError::ZeroTimeout {
                path: path.to_path_buf(),
                glob: rule.glob.clone(),
            });
        }
    }
    Ok(parsed)
}

/// Lê e interpreta `configs/verify.ron` sob `root`.
pub fn read_verify(root: &Path) -> Result<VerifyConfig, VerifyConfigError> {
    let path = verify_path(root);
    let text = std::fs::read_to_string(&path).map_err(|source| VerifyConfigError::Read {
        path: path.clone(),
        source,
    })?;
    parse_verify(&text, &path)
}

/// Lê a verificação declarada; ausência ou erro caem em "nenhuma verificação".
pub fn load_verify(root: &Path) -> VerifyConfig {
    match read_verify(root) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(%error, "verificação pós-escrita indisponível; nenhuma será rodada");
            VerifyConfig::default()
        }
    }
}

/// Caminho normalizado para `/`, sem prefixo `./`.
fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let trimmed = text.trim_start_matches("./");
    trimmed.trim_end_matches('/').to_string()
}

/// Último segmento do caminho normalizado.
fn basename(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

/// Casa `pattern` com `text`: `*` fica no segmento, `**` cruza `/` e `?` é um
/// caractere que não seja `/`.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.trim_start_matches("./").chars().collect();
    let text: Vec<char> = text.chars().collect();
    matches(&pattern, &text)
}

fn matches(pattern: &[char], text: &[char]) -> bool {
    let Some(first) = pattern.first().copied() else {
        return text.is_empty();
    };
    match first {
        '*' if pattern.get(1) == Some(&'*') => {
            if pattern.get(2) == Some(&'/') && matches(&pattern[3..], text) {
                // `**/` também casa zero diretórios.
                return true;
            }
            (0..=text.len()).any(|i| matches(&pattern[2..], &text[i..]))
        }
        '*' => (0..=text.len())
            .take_while(|&i| i == 0 || text[i - 1] != '/')
            .any(|i| matches(&pattern[1..], &text[i..])),
        '?' => text.first().is_some_and(|&c| c != '/') && matches(&pattern[1..], &text[1..]),
        literal => {
            text.first() == Some(&literal) && matches(&pattern[1..], &text[1..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"(
        rules: [
            (glob: "**/*.md", program: "markdownlint", args: ["--quiet"]),
            (glob: "**/*.ts", program: "tsc", args: ["--noEmit"], timeout_secs: 120),
        ],
    )"#;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn markdown_and_typescript_resolve_to_distinct_commands() {
        let path = Path::new("configs/verify.ron");
        let config = parse_verify(EXAMPLE, path).unwrap();
        let md = config.resolve(Path::new("docs/index.md")).unwrap();
        let ts = config.resolve(Path::new("src/app.ts")).unwrap();
        assert_eq!(md.command_line(), "markdownlint --quiet");
        assert_eq!(ts.command_line(), "tsc --noEmit");
        assert_ne!(md.command_line(), ts.command_line());
        assert_eq!(md.timeout_secs(), DEFAULT_TIMEOUT_SECS);
        assert_eq!(ts.timeout_secs(), 120);
    }

    #[test]
    fn a_path_without_a_matching_glob_is_an_explicit_no_op() {
        let path = Path::new("configs/verify.ron");
        let config = parse_verify(EXAMPLE, path).unwrap();
        assert!(config.resolve(Path::new("src/main.rs")).is_none());
        assert!(VerifyConfig::default().is_empty());
        assert!(VerifyConfig::default()
            .resolve(Path::new("docs/index.md"))
            .is_none());
    }

    #[test]
    fn the_first_matching_rule_wins() {
        let path = Path::new("verify.ron");
        let config = parse_verify(
            r#"(
                rules: [
                    (glob: "docs/**/*.md", program: "markdownlint"),
                    (glob: "**/*.md", program: "md-check"),
                ],
            )"#,
            path,
        )
        .unwrap();
        let rule = config.resolve(Path::new("docs/rules/model-control.md")).unwrap();
        assert_eq!(rule.program, "markdownlint");
        let rule = config.resolve(Path::new("README.md")).unwrap();
        assert_eq!(rule.program, "md-check");
    }

    #[test]
    fn a_bare_glob_covers_any_depth_and_a_slash_glob_covers_the_path() {
        let path = Path::new("verify.ron");
        let config = parse_verify(
            r#"(
                rules: [
                    (glob: "*.rs", program: "rustfmt", args: ["--check"]),
                    (glob: "src/**", program: "tsc", args: ["--noEmit"]),
                ],
            )"#,
            path,
        )
        .unwrap();
        assert_eq!(
            config.resolve(Path::new("crates/tupi/src/lib.rs")).unwrap().program,
            "rustfmt"
        );
        assert_eq!(config.resolve(Path::new("./src/app.ts")).unwrap().program, "tsc");
        assert!(config.resolve(Path::new("docs/index.md")).is_none());
    }

    #[test]
    fn a_rule_without_a_command_is_refused() {
        let path = Path::new("verify.ron");
        let error = parse_verify(r#"(rules: [(glob: "**/*.md", program: "  ")])"#, path).unwrap_err();
        assert!(error.to_string().contains("no-op"), "{error}");
        assert!(error.to_string().contains("**/*.md"), "{error}");
    }

    #[test]
    fn an_empty_glob_or_a_zero_timeout_is_refused() {
        let path = Path::new("verify.ron");
        let error = parse_verify(r#"(rules: [(glob: "", program: "ruff")])"#, path).unwrap_err();
        assert!(matches!(error, VerifyConfigError::EmptyGlob { .. }));
        let error = parse_verify(
            r#"(rules: [(glob: "*.py", program: "ruff", timeout_secs: 0)])"#,
            path,
        )
        .unwrap_err();
        assert!(matches!(error, VerifyConfigError::ZeroTimeout { .. }));
    }

    #[test]
    fn a_broken_file_names_its_path() {
        let path = Path::new("configs/verify.ron");
        let error = parse_verify("(rules: ", path).unwrap_err();
        assert!(error.to_string().contains("configs/verify.ron"), "{error}");
        assert!(parse_verify("(rules: [], extra: 1)", path).is_err());
    }

    #[test]
    fn a_missing_file_means_no_verification() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_verify(dir.path()).is_empty());
    }

    #[test]
    fn the_checked_in_file_is_empty_or_valid() {
        let config = read_verify(&repo_root()).expect("configs/verify.ron precisa ser válido");
        // Este repositório não declara verificador: toda escrita segue no-op.
        assert!(config.is_empty(), "{} regra(s) declarada(s)", config.rules.len());
    }
}
