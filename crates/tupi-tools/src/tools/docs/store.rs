//! Persistência do índice de documentação.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const SCHEMA: u32 = 1;
const VECTOR_MAGIC: &[u8; 4] = b"TPV1";

/// Manifesto do índice de documentação.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    pub folders: Vec<String>,
    pub embedding: Option<EmbeddingMeta>,
    pub files: Vec<FileEntry>,
    pub skipped: Vec<Skipped>,
}

impl Index {
    pub fn empty(folders: Vec<String>) -> Self {
        Self {
            schema: SCHEMA,
            folders,
            embedding: None,
            files: Vec::new(),
            skipped: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingMeta {
    pub model: String,
    pub dim: usize,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub bytes: u64,
    pub mtime: u64,
    pub summary: Option<String>,
    pub chunks: Vec<ChunkMeta>,
    pub vector_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMeta {
    pub heading: String,
    pub start: usize,
    pub end: usize,
    pub hash: String,
    pub tokens: usize,
    pub terms: BTreeMap<String, u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

pub fn index_dir(root: &Path) -> PathBuf {
    root.join(".agents").join("tupi").join("docs")
}

fn index_path(root: &Path) -> PathBuf {
    index_dir(root).join("index.json")
}

pub fn load_index(root: &Path) -> Option<Index> {
    let bytes = std::fs::read(index_path(root)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save_index(root: &Path, index: &Index) -> std::io::Result<()> {
    let dir = index_dir(root);
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("index.json.tmp");
    let bytes = serde_json::to_vec_pretty(index).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, dir.join("index.json"))?;
    Ok(())
}

fn vector_path(root: &Path, path: &str) -> PathBuf {
    let name = blake3::hash(path.as_bytes()).to_hex().to_string();
    index_dir(root).join("vectors").join(format!("{name}.vec"))
}

pub fn load_vectors(root: &Path, path: &str) -> std::io::Result<Option<(usize, Vec<Vec<f32>>)>> {
    let bytes = match std::fs::read(vector_path(root, path)) {
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

pub fn save_vectors(
    root: &Path,
    path: &str,
    dim: usize,
    vectors: &[Vec<f32>],
) -> std::io::Result<()> {
    let target = vector_path(root, path);
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

pub fn remove_vectors(root: &Path, path: &str) -> std::io::Result<()> {
    match std::fs::remove_file(vector_path(root, path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn ensure_gitignore(root: &Path) -> std::io::Result<()> {
    let dir = index_dir(root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(".gitignore");
    if !path.exists() {
        std::fs::write(path, "*\n")?;
    }
    Ok(())
}

pub fn slice_lines(text: &str, start: usize, end: usize) -> Option<String> {
    if start == 0 {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    if start > lines.len() {
        return None;
    }
    let end = end.min(lines.len());
    if end < start {
        return None;
    }
    Some(lines[start - 1..end].join("\n"))
}

pub fn hash_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = Index::empty(vec!["docs".to_string()]);
        index.files.push(FileEntry {
            path: "docs/a.md".to_string(),
            hash: hash_hex(b"a"),
            bytes: 1,
            mtime: 7,
            summary: None,
            chunks: vec![ChunkMeta {
                heading: "A".to_string(),
                start: 1,
                end: 1,
                hash: hash_hex(b"a"),
                tokens: 1,
                terms: [("a".to_string(), 1)].into_iter().collect(),
            }],
            vector_count: 0,
        });
        save_index(dir.path(), &index).unwrap();
        assert!(!index_dir(dir.path()).join("index.json.tmp").exists());
        let loaded = load_index(dir.path()).expect("indice");
        assert_eq!(loaded.files[0].path, "docs/a.md");
        assert_eq!(loaded.files[0].chunks[0].terms.get("a"), Some(&1));
    }

    #[test]
    fn vector_blob_round_trips_with_dim_and_count() {
        let dir = tempfile::tempdir().unwrap();
        save_vectors(
            dir.path(),
            "docs/a.md",
            3,
            &[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]],
        )
        .unwrap();
        let (dim, vectors) = load_vectors(dir.path(), "docs/a.md")
            .unwrap()
            .expect("blob");
        assert_eq!(dim, 3);
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[1], vec![0.0, 1.0, 0.0]);
    }

    #[test]
    fn gitignore_is_written_once() {
        let dir = tempfile::tempdir().unwrap();
        ensure_gitignore(dir.path()).unwrap();
        ensure_gitignore(dir.path()).unwrap();
        let text = std::fs::read_to_string(index_dir(dir.path()).join(".gitignore")).unwrap();
        assert_eq!(text.trim(), "*");
    }

    #[test]
    fn slice_lines_is_one_based_inclusive() {
        let text = "um\ndois\ntres\nquatro";
        assert_eq!(slice_lines(text, 2, 3).as_deref(), Some("dois\ntres"));
        assert_eq!(slice_lines(text, 4, 9).as_deref(), Some("quatro"));
        assert_eq!(slice_lines(text, 0, 1), None);
    }
}
