use std::io::Write;
use std::path::{Component, Path, PathBuf};

use tupi_kernel::ToolError;

pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn resolve(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let root = root
        .canonicalize()
        .map_err(|error| ToolError::Other(format!("root invalido: {error}")))?;
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(ToolError::InvalidArguments("caminho vazio".to_string()));
    }
    let candidate = Path::new(requested);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let normalized = normalize(&joined);
    if !normalized.starts_with(&root) {
        return Err(ToolError::OutsideRoot(requested.to_string()));
    }
    Ok(normalized)
}

pub fn resolve_existing(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let path = resolve(root, requested)?;
    let canonical = path
        .canonicalize()
        .map_err(|_| ToolError::NotFound(requested.to_string()))?;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !canonical.starts_with(&root) {
        return Err(ToolError::OutsideRoot(requested.to_string()));
    }
    Ok(canonical)
}

pub fn resolve_for_write(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let root = root
        .canonicalize()
        .map_err(|error| ToolError::Other(format!("root invalido: {error}")))?;
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(ToolError::InvalidArguments("caminho vazio".to_string()));
    }
    let candidate = Path::new(requested);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let normalized = normalize(&joined);
    if !normalized.starts_with(&root) {
        return Err(ToolError::OutsideRoot(requested.to_string()));
    }
    if normalized == root {
        return Err(ToolError::InvalidArguments(
            "o caminho precisa nomear um arquivo, nao o root".to_string(),
        ));
    }

    let mut existing = normalized.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return Err(ToolError::OutsideRoot(requested.to_string()));
        };
        suffix.push(name.to_os_string());
        let Some(parent) = existing.parent() else {
            return Err(ToolError::OutsideRoot(requested.to_string()));
        };
        existing = parent;
    }

    let canonical = existing
        .canonicalize()
        .map_err(|error| ToolError::Other(format!("falha ao resolver {requested}: {error}")))?;
    if !canonical.starts_with(&root) {
        return Err(ToolError::OutsideRoot(requested.to_string()));
    }
    let mut out = canonical;
    for name in suffix.iter().rev() {
        out.push(name);
    }
    Ok(out)
}

pub fn relative(root: &Path, path: &Path) -> String {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    path.strip_prefix(&root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn repo_key(root: &Path) -> String {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    blake3::hash(canonical.to_string_lossy().as_bytes())
        .to_hex()
        .to_string()
}

pub fn hash_prefix(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()[..12].to_string()
}

pub fn hash_matches(bytes: &[u8], expected: &str) -> bool {
    let expected = expected
        .trim()
        .trim_matches('\u{0060}')
        .to_ascii_lowercase();
    if expected.len() < 8 || expected.len() > 64 {
        return false;
    }
    if !expected
        .chars()
        .all(|character| character.is_ascii_hexdigit())
    {
        return false;
    }
    blake3::hash(bytes)
        .to_hex()
        .to_string()
        .starts_with(&expected)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "sem diretorio pai")
    })?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    if let Ok(metadata) = std::fs::metadata(path) {
        std::fs::set_permissions(temp.path(), metadata.permissions())?;
    }
    temp.persist(path).map(|_| ()).map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parent_component_cannot_escape_the_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_for_write(dir.path(), "../fora.txt").is_err());
    }

    #[test]
    fn a_nested_new_file_resolves_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let path = resolve_for_write(dir.path(), "a/b/c.txt").unwrap();
        assert!(path.starts_with(dir.path().canonicalize().unwrap()));
        assert!(path.ends_with("c.txt"));
    }

    #[test]
    fn a_dot_dot_inside_the_root_is_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let path = resolve_for_write(dir.path(), "a/../b.txt").unwrap();
        assert_eq!(path.file_name().unwrap(), "b.txt");
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn the_hash_guard_accepts_a_prefix_and_refuses_a_stale_one() {
        let bytes = b"conteudo";
        let full = blake3::hash(bytes).to_hex().to_string();
        assert!(hash_matches(bytes, &full[..12]));
        assert!(!hash_matches(bytes, "000000000000"));
        assert!(!hash_matches(bytes, "xyz"));
    }

    #[test]
    fn an_atomic_write_replaces_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "antes").unwrap();
        atomic_write(&path, b"depois").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "depois");
    }
}
