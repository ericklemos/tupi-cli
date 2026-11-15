//! Persistência do índice vetorial: manifest e blobs de vetores.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const VECTOR_MAGIC: &[u8; 4] = b"TUPV";

/// Versão do formato em disco. Muda quando o layout do manifest quebra.
pub const SPACE_VERSION: u32 = 1;

/// Diretório do índice do espaço, ao lado do índice de docs.
pub fn space_dir(root: &Path) -> PathBuf {
    root.join(".agents").join("tupi").join("space")
}

/// Garante que o índice não seja versionado.
pub fn ensure_gitignore(root: &Path) -> std::io::Result<()> {
    let dir = space_dir(root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(".gitignore");
    if !path.exists() {
        std::fs::write(path, "*\n")?;
    }
    Ok(())
}

/// Manifest do índice: modelo, dimensão e símbolos por arquivo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceManifest {
    pub version: u32,
    pub model_id: String,
    pub dim: usize,
    #[serde(default)]
    pub files: BTreeMap<String, SpaceFile>,
}

/// Um arquivo de código indexado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceFile {
    /// blake3 hex do conteúdo usado para embedar os símbolos.
    pub hash: String,
    #[serde(default)]
    pub symbols: Vec<SpaceSymbol>,
}

/// Um símbolo dentro de um arquivo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceSymbol {
    pub qualified: String,
    pub symbol: String,
    pub start: usize,
    pub end: usize,
}

fn manifest_path(root: &Path) -> PathBuf {
    space_dir(root).join("manifest.json")
}

/// Lê o manifest; qualquer problema devolve None (o índice é cache).
pub fn load_manifest(root: &Path) -> Option<SpaceManifest> {
    let bytes = std::fs::read(manifest_path(root)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Grava o manifest de forma atômica (tmp + rename).
pub fn save_manifest(root: &Path, manifest: &SpaceManifest) -> std::io::Result<()> {
    let dir = space_dir(root);
    std::fs::create_dir_all(&dir)?;
    let target = manifest_path(root);
    let tmp = target.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, target)?;
    Ok(())
}

fn vector_path(root: &Path, file_hash: &str) -> PathBuf {
    let name = blake3::hash(file_hash.as_bytes()).to_hex().to_string();
    space_dir(root).join("vectors").join(format!("{name}.vec"))
}

/// Lê o blob de vetores de um arquivo (None quando não existe).
pub fn load_vectors(
    root: &Path,
    file_hash: &str,
) -> std::io::Result<Option<(usize, Vec<Vec<f32>>)>> {
    let bytes = match std::fs::read(vector_path(root, file_hash)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let invalid =
        || std::io::Error::new(std::io::ErrorKind::InvalidData, "blob de vetores inválido");
    if bytes.len() < 12 || &bytes[0..4] != VECTOR_MAGIC {
        return Err(invalid());
    }
    let dim = u32::from_le_bytes(bytes[4..8].try_into().expect("dim")) as usize;
    let count = u32::from_le_bytes(bytes[8..12].try_into().expect("count")) as usize;
    if dim == 0 || bytes.len() != 12 + count * dim * 4 {
        return Err(invalid());
    }
    let mut vectors = Vec::with_capacity(count);
    for chunk in bytes[12..].chunks_exact(dim * 4) {
        let vector = chunk
            .chunks_exact(4)
            .map(|raw| f32::from_le_bytes(raw.try_into().expect("f32")))
            .collect();
        vectors.push(vector);
    }
    Ok(Some((dim, vectors)))
}

/// Grava o blob de vetores de um arquivo (tmp + rename).
pub fn save_vectors(
    root: &Path,
    file_hash: &str,
    dim: usize,
    vectors: &[Vec<f32>],
) -> std::io::Result<()> {
    let target = vector_path(root, file_hash);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = Vec::with_capacity(12 + vectors.len() * dim * 4);
    bytes.extend_from_slice(VECTOR_MAGIC);
    bytes.extend_from_slice(&(dim as u32).to_le_bytes());
    bytes.extend_from_slice(&(vectors.len() as u32).to_le_bytes());
    for vector in vectors {
        for value in vector {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    let tmp = target.with_extension("vec.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, target)?;
    Ok(())
}

/// Remove o blob de um arquivo que saiu do índice.
pub fn remove_vectors(root: &Path, file_hash: &str) -> std::io::Result<()> {
    match std::fs::remove_file(vector_path(root, file_hash)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> SpaceManifest {
        let mut files = BTreeMap::new();
        files.insert(
            "src/a.rs".to_string(),
            SpaceFile {
                hash: "abc".to_string(),
                symbols: vec![SpaceSymbol {
                    qualified: "a::run".to_string(),
                    symbol: "run".to_string(),
                    start: 3,
                    end: 9,
                }],
            },
        );
        SpaceManifest {
            version: SPACE_VERSION,
            model_id: "fake".to_string(),
            dim: 3,
            files,
        }
    }

    #[test]
    fn manifest_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        save_manifest(dir.path(), &manifest()).unwrap();
        assert_eq!(load_manifest(dir.path()), Some(manifest()));
    }

    #[test]
    fn vector_blob_round_trips_with_dim_and_count() {
        let dir = tempfile::tempdir().unwrap();
        let vectors = vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]];
        save_vectors(dir.path(), "abc", 3, &vectors).unwrap();
        let (dim, loaded) = load_vectors(dir.path(), "abc").unwrap().unwrap();
        assert_eq!(dim, 3);
        assert_eq!(loaded, vectors);
    }

    #[test]
    fn gitignore_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        ensure_gitignore(dir.path()).unwrap();
        ensure_gitignore(dir.path()).unwrap();
        let text = std::fs::read_to_string(space_dir(dir.path()).join(".gitignore")).unwrap();
        assert_eq!(text, "*\n");
    }

    #[test]
    fn manifest_write_keeps_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        save_manifest(dir.path(), &manifest()).unwrap();
        let entries: Vec<_> = std::fs::read_dir(space_dir(dir.path()))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["manifest.json".to_string()]);
    }
}
