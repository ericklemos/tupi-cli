//! Cache compartilhado de árvores tree-sitter.
//!
//! O grafo, a busca e o read parseiam os mesmos arquivos. A chave é o caminho
//! *mais* o digest do conteúdo, então um arquivo editado é miss, nunca uma
//! resposta velha. O limite é por bytes de fonte; passou dele, a entrada mais
//! antiga sai.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use tree_sitter::Tree;

use crate::ast::Language;

const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    digest: blake3::Hash,
    tree: Arc<Tree>,
    bytes: usize,
    generation: u64,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<PathBuf, Entry>,
    order: VecDeque<(PathBuf, u64)>,
    bytes: usize,
    generation: u64,
}

pub struct ParseCache {
    max_bytes: usize,
    inner: RwLock<Inner>,
}

impl ParseCache {
    pub fn new() -> Self {
        Self::with_max_bytes(DEFAULT_MAX_BYTES)
    }

    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            inner: RwLock::new(Inner::default()),
        }
    }

    pub fn get(&self, path: &Path, digest: blake3::Hash) -> Option<Arc<Tree>> {
        let inner = self.inner.read().ok()?;
        let entry = inner.entries.get(path)?;
        (entry.digest == digest).then(|| entry.tree.clone())
    }

    pub fn insert(&self, path: &Path, digest: blake3::Hash, tree: Arc<Tree>) -> Arc<Tree> {
        let Ok(mut inner) = self.inner.write() else {
            return tree;
        };

        inner.generation += 1;
        let generation = inner.generation;
        let bytes = tree.root_node().end_byte();
        if let Some(old) = inner.entries.remove(path) {
            inner.bytes = inner.bytes.saturating_sub(old.bytes);
        }
        inner.bytes += bytes;
        inner.entries.insert(
            path.to_path_buf(),
            Entry {
                digest,
                tree: tree.clone(),
                bytes,
                generation,
            },
        );
        inner.order.push_back((path.to_path_buf(), generation));

        while inner.bytes > self.max_bytes {
            let Some((old_path, old_generation)) = inner.order.pop_front() else {
                break;
            };
            let current = inner
                .entries
                .get(&old_path)
                .is_some_and(|entry| entry.generation == old_generation);
            if current && let Some(removed) = inner.entries.remove(&old_path) {
                inner.bytes = inner.bytes.saturating_sub(removed.bytes);
            }
        }

        tree
    }

    pub fn parse(&self, path: &Path, language: Language, content: &str) -> Option<Arc<Tree>> {
        let digest = blake3::hash(content.as_bytes());
        if let Some(tree) = self.get(path, digest) {
            return Some(tree);
        }
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language.tree_sitter_language()).ok()?;
        let tree = Arc::new(parser.parse(content, None)?);
        Some(self.insert(path, digest, tree))
    }

    pub fn invalidate(&self, path: &Path) {
        let Ok(mut inner) = self.inner.write() else {
            return;
        };
        if let Some(removed) = inner.entries.remove(path) {
            inner.bytes = inner.bytes.saturating_sub(removed.bytes);
        }
    }

    pub fn len(&self) -> usize {
        self.inner
            .read()
            .map(|inner| inner.entries.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ParseCache {
    fn default() -> Self {
        Self::new()
    }
}
