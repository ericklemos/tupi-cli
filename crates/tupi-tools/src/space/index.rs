//! Snapshot em memória do índice vetorial.

use std::path::Path;
use std::sync::{Arc, RwLock};

use super::store::{self, SpaceManifest};

/// Um símbolo pronto para consulta densa.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceEntry {
    pub relative: String,
    pub qualified: String,
    pub symbol: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// Vetores e metadados publicados pelo worker.
#[derive(Debug, Clone)]
pub struct SpaceSnapshot {
    pub model_id: String,
    pub dim: usize,
    pub entries: Vec<SpaceEntry>,
    pub vectors: Vec<Vec<f32>>,
    /// Quantos símbolos existem no repo; maior que entries durante o build.
    pub expected_symbols: usize,
}

/// Índice compartilhado entre o worker e o search.
#[derive(Default)]
pub struct SpaceIndex {
    inner: RwLock<Option<Arc<SpaceSnapshot>>>,
}

impl SpaceIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconstrói o snapshot a partir do manifest e dos blobs em disco.
    pub fn load(root: &Path) -> Self {
        let index = Self::new();
        if let Some(manifest) = store::load_manifest(root)
            && let Some(snapshot) = snapshot_from_manifest(root, &manifest)
        {
            index.set(snapshot);
        }
        index
    }

    pub fn set(&self, snapshot: SpaceSnapshot) {
        if let Ok(mut guard) = self.inner.write() {
            *guard = Some(Arc::new(snapshot));
        }
    }

    pub fn snapshot(&self) -> Option<Arc<SpaceSnapshot>> {
        self.inner.read().ok().and_then(|guard| guard.clone())
    }

    pub fn is_empty(&self) -> bool {
        self.snapshot().is_none()
    }
}

fn snapshot_from_manifest(root: &Path, manifest: &SpaceManifest) -> Option<SpaceSnapshot> {
    let mut entries = Vec::new();
    let mut vectors = Vec::new();
    for (relative, file) in &manifest.files {
        let (dim, blob) = store::load_vectors(root, &file.hash).ok()??;
        if dim != manifest.dim || blob.len() != file.symbols.len() {
            return None;
        }
        for (symbol, vector) in file.symbols.iter().zip(blob) {
            entries.push(SpaceEntry {
                relative: relative.clone(),
                qualified: symbol.qualified.clone(),
                symbol: symbol.symbol.clone(),
                start_line: symbol.start,
                end_line: symbol.end,
            });
            vectors.push(vector);
        }
    }
    Some(SpaceSnapshot {
        model_id: manifest.model_id.clone(),
        dim: manifest.dim,
        expected_symbols: entries.len(),
        entries,
        vectors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space::store::{
        SPACE_VERSION, SpaceFile, SpaceManifest, SpaceSymbol, save_manifest, save_vectors,
    };
    use std::collections::BTreeMap;

    fn snapshot() -> SpaceSnapshot {
        SpaceSnapshot {
            model_id: "fake".to_string(),
            dim: 2,
            entries: vec![SpaceEntry {
                relative: "src/a.rs".to_string(),
                qualified: "a::run".to_string(),
                symbol: "run".to_string(),
                start_line: 3,
                end_line: 9,
            }],
            vectors: vec![vec![1.0, 2.0]],
            expected_symbols: 1,
        }
    }

    #[test]
    fn set_then_snapshot_returns_the_same_entries() {
        let index = SpaceIndex::new();
        index.set(snapshot());
        let got = index.snapshot().unwrap();
        assert_eq!(got.entries, snapshot().entries);
        assert_eq!(got.vectors, snapshot().vectors);
        assert!(!index.is_empty());
    }

    #[test]
    fn load_without_manifest_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SpaceIndex::load(dir.path()).is_empty());
    }

    #[test]
    fn load_round_trips_a_saved_index() {
        let dir = tempfile::tempdir().unwrap();
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
        let manifest = SpaceManifest {
            version: SPACE_VERSION,
            model_id: "fake".to_string(),
            dim: 2,
            files,
        };
        save_manifest(dir.path(), &manifest).unwrap();
        save_vectors(dir.path(), "abc", 2, &[vec![1.0, 2.0]]).unwrap();

        let index = SpaceIndex::load(dir.path());
        let got = index.snapshot().unwrap();
        assert_eq!(got.model_id, "fake");
        assert_eq!(got.entries.len(), 1);
        assert_eq!(got.entries[0].relative, "src/a.rs");
        assert_eq!(got.vectors, vec![vec![1.0, 2.0]]);
        assert_eq!(got.expected_symbols, 1);
    }
}
