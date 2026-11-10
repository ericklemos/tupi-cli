use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use tupi_kernel::ToolError;

/// Mais de 1 MiB é quase sempre gerado/minificado: caro de ler e ruim de exibir.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Teto de linhas casadas por arquivo. O AST deduplica depois; o teto evita
/// varrer um arquivo inteiro quando uma palavra comum casa em tudo.
const MAX_MATCHES_PER_FILE: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMatch {
    pub file_path: PathBuf,
    pub line_number: usize,
}

pub struct LexicalScanner {
    root_dir: PathBuf,
}

impl LexicalScanner {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }

    /// Varre o projeto (ou um prefixo) por linhas que contenham qualquer termo.
    /// A ordem é determinística: caminho e depois linha.
    pub fn search(
        &self,
        terms: &[String],
        path_prefix: Option<&str>,
    ) -> Result<Vec<RawMatch>, ToolError> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let root = self
            .root_dir
            .canonicalize()
            .map_err(|error| ToolError::Other(format!("root inválido: {error}")))?;

        let base = match path_prefix
            .map(str::trim)
            .filter(|prefix| !prefix.is_empty())
        {
            Some(prefix) => {
                let candidate = Path::new(prefix);
                let joined = if candidate.is_absolute() {
                    candidate.to_path_buf()
                } else {
                    root.join(candidate)
                };
                // Normaliza `..` antes de canonicalizar: um prefixo que escapa
                // do root é erro de contenção mesmo quando o alvo não existe.
                let normalized = normalize(&joined);
                if !normalized.starts_with(&root) {
                    return Err(ToolError::OutsideRoot(prefix.to_string()));
                }
                let canonical = normalized
                    .canonicalize()
                    .map_err(|_| ToolError::NotFound(prefix.to_string()))?;
                if !canonical.starts_with(&root) {
                    return Err(ToolError::OutsideRoot(prefix.to_string()));
                }
                canonical
            }
            None => root,
        };

        let mut builder = WalkBuilder::new(&base);
        builder
            .standard_filters(true)
            .hidden(true)
            .require_git(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .filter_entry(|entry| !excluded(entry));

        let mut matches = Vec::new();
        for entry in builder.build() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            if entry
                .metadata()
                .map(|metadata| metadata.len() > MAX_FILE_BYTES)
                .unwrap_or(true)
            {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };

            let mut found = 0;
            for (index, line) in content.lines().enumerate() {
                let lower = line.to_lowercase();
                if terms.iter().any(|term| lower.contains(term)) {
                    matches.push(RawMatch {
                        file_path: path.to_path_buf(),
                        line_number: index + 1,
                    });
                    found += 1;
                    if found >= MAX_MATCHES_PER_FILE {
                        break;
                    }
                }
            }
        }

        matches.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.line_number.cmp(&b.line_number))
        });
        Ok(matches)
    }
}

/// Resolve `.` e `..` sem tocar no disco.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                out.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn excluded(entry: &ignore::DirEntry) -> bool {
    if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    matches!(
        entry.file_name().to_str(),
        Some("target" | "node_modules" | ".git" | "build" | "dist" | ".gradle")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn terms() -> Vec<String> {
        vec!["verify_token".to_string()]
    }

    #[test]
    fn finds_a_term_in_rust_files() {
        let dir = tempdir().unwrap();
        write(
            &dir.path().join("src/auth.rs"),
            "pub fn verify_token() {}\n",
        );
        let scanner = LexicalScanner::new(dir.path().to_path_buf());

        let matches = scanner.search(&terms(), None).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 1);
        assert!(matches[0].file_path.ends_with("src/auth.rs"));
    }

    #[test]
    fn ignores_non_rust_files_and_build_directories() {
        let dir = tempdir().unwrap();
        write(&dir.path().join("notes.md"), "verify_token\n");
        write(&dir.path().join("target/gen.rs"), "verify_token\n");
        write(&dir.path().join("src/auth.rs"), "verify_token\n");
        let scanner = LexicalScanner::new(dir.path().to_path_buf());

        let matches = scanner.search(&terms(), None).unwrap();

        assert_eq!(matches.len(), 1);
        assert!(matches[0].file_path.ends_with("src/auth.rs"));
    }

    #[test]
    fn respects_gitignore_without_a_git_repository() {
        let dir = tempdir().unwrap();
        write(&dir.path().join(".gitignore"), "ignored.rs\n");
        write(&dir.path().join("ignored.rs"), "verify_token\n");
        write(&dir.path().join("kept.rs"), "verify_token\n");
        let scanner = LexicalScanner::new(dir.path().to_path_buf());

        let matches = scanner.search(&terms(), None).unwrap();

        assert_eq!(matches.len(), 1);
        assert!(matches[0].file_path.ends_with("kept.rs"));
    }

    #[test]
    fn a_path_prefix_restricts_the_scan() {
        let dir = tempdir().unwrap();
        write(&dir.path().join("src/auth.rs"), "verify_token\n");
        write(&dir.path().join("top.rs"), "verify_token\n");
        let scanner = LexicalScanner::new(dir.path().to_path_buf());

        let matches = scanner.search(&terms(), Some("src")).unwrap();

        assert_eq!(matches.len(), 1);
        assert!(matches[0].file_path.ends_with("src/auth.rs"));
    }

    #[test]
    fn a_prefix_outside_the_root_is_rejected() {
        let dir = tempdir().unwrap();
        let scanner = LexicalScanner::new(dir.path().to_path_buf());

        let error = scanner.search(&terms(), Some("../fora")).unwrap_err();

        assert!(matches!(error, tupi_kernel::ToolError::OutsideRoot(_)));
    }

    #[test]
    fn non_utf8_files_are_skipped() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("bin.rs"), [0xff, 0xfe]).unwrap();
        let scanner = LexicalScanner::new(dir.path().to_path_buf());

        assert!(scanner.search(&terms(), None).unwrap().is_empty());
    }

    #[test]
    fn results_are_ordered_by_path_then_line() {
        let dir = tempdir().unwrap();
        write(&dir.path().join("b.rs"), "verify_token\n");
        write(&dir.path().join("a.rs"), "verify_token\nverify_token\n");
        let scanner = LexicalScanner::new(dir.path().to_path_buf());

        let matches = scanner.search(&terms(), None).unwrap();

        assert_eq!(matches.len(), 3);
        assert!(matches[0].file_path.ends_with("a.rs"));
        assert_eq!(matches[0].line_number, 1);
        assert_eq!(matches[1].line_number, 2);
        assert!(matches[2].file_path.ends_with("b.rs"));
    }
}
