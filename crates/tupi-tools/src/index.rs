use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tupi_domain::index::{Index, IndexFile, IndexSymbol};

use crate::ast::AstChunker;

pub const INDEX_VERSION: u32 = 1;
pub const DEFAULT_PREFIX_TOKENS: usize = 4000;

const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules", "dist", "build"];
const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("falha ao varrer {path}: {message}")]
    Io { path: String, message: String },
}

/// Identidade estável do repo: blake3 do caminho canônico da raiz.
pub fn repo_key(root: &Path) -> String {
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    blake3::hash(canonical.to_string_lossy().as_bytes())
        .to_hex()
        .to_string()
}

/// Varredura determinística: mesmo repo, mesmos bytes.
///
/// Só metadados entram: caminho, hash e símbolos. Conteúdo nunca.
pub fn scan(root: &Path) -> Result<Index, IndexError> {
    let mut paths = collect(root)?;
    paths.sort();

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let is_rust = path.extension().and_then(|ext| ext.to_str()) == Some("rs");
        let symbols = if is_rust {
            AstChunker::new()
                .snippets(&path, &content)
                .into_iter()
                .map(|snippet| IndexSymbol {
                    name: snippet.symbol_name,
                    qualified: snippet.qualified_name,
                    signature: snippet.signature,
                    start_line: snippet.start_line,
                    end_line: snippet.end_line,
                })
                .collect()
        } else {
            Vec::new()
        };

        files.push(IndexFile {
            path: relative,
            hash: blake3::hash(content.as_bytes()).to_hex().to_string(),
            language: if is_rust {
                "rust".into()
            } else {
                String::new()
            },
            symbols,
        });
    }

    Ok(Index {
        version: INDEX_VERSION,
        scanned_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
        files,
    })
}

fn collect(root: &Path) -> Result<Vec<PathBuf>, IndexError> {
    fs::read_dir(root).map_err(|error| IndexError::Io {
        path: root.display().to_string(),
        message: error.to_string(),
    })?;

    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|entry| {
            entry.depth() == 0 || !SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref())
        })
        .build();

    Ok(walker
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(ignore::DirEntry::into_path)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn fixture() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.rs"), "pub fn alpha() {}\n").unwrap();
        fs::write(dir.path().join("src/b.rs"), "pub fn beta() {}\n").unwrap();
        fs::write(dir.path().join("README.md"), "# tupi\n").unwrap();
        fs::create_dir_all(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/gen.rs"), "fn gerado() {}\n").unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dir.path().join("src/blob.bin"), [0xff, 0xfe, 0x00]).unwrap();
        dir
    }

    #[test]
    fn scan_is_deterministic_and_relative() {
        let dir = fixture();
        let first = scan(dir.path()).unwrap();
        let second = scan(dir.path()).unwrap();
        assert_eq!(first.files, second.files);
        assert!(first.files.iter().all(|file| !file.path.starts_with('/')));
        let paths: Vec<&str> = first.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["README.md", "src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn scan_extracts_rust_symbols_and_skips_binary() {
        let dir = fixture();
        let index = scan(dir.path()).unwrap();
        let rust = index
            .files
            .iter()
            .find(|file| file.path == "src/a.rs")
            .unwrap();
        assert_eq!(rust.language, "rust");
        assert!(rust.symbols.iter().any(|symbol| symbol.name == "alpha"));
        assert!(
            !index
                .files
                .iter()
                .any(|file| file.path.contains("blob.bin"))
        );
    }

    #[test]
    fn scan_marks_non_rust_files_path_only() {
        let dir = fixture();
        let index = scan(dir.path()).unwrap();
        let readme = index
            .files
            .iter()
            .find(|file| file.path == "README.md")
            .unwrap();
        assert!(readme.symbols.is_empty());
        assert_eq!(readme.language, "");
    }

    #[test]
    fn repo_key_is_stable_for_the_same_root() {
        let dir = fixture();
        assert_eq!(repo_key(dir.path()), repo_key(dir.path()));
    }
}
