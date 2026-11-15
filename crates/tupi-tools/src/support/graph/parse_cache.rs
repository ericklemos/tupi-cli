//! Cache compartilhado de árvores tree-sitter.
//!
//! O grafo, a busca e o read parseiam os mesmos arquivos. A chave é o caminho
//! *mais* o digest do conteúdo, então um arquivo editado é miss, nunca uma
//! resposta velha. O limite é por bytes de fonte; passou dele, a entrada mais
//! antiga sai.

use std::collections::{BTreeMap, HashMap};
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
    /// Ordem FIFO de despejo, indexada pela `generation` da entrada.
    ///
    /// Uma chave por entrada **viva**: reinserir um `path` remove a chave
    /// antiga, então este índice nunca acumula marcadores mortos. Antes era um
    /// `VecDeque` de marcadores preguiçosos, varridos só sob pressão de bytes:
    /// com folga de orçamento (o caso comum) ele crescia um `PathBuf` por
    /// parse, para sempre.
    order: BTreeMap<u64, PathBuf>,
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
        // Reinserir o mesmo `path` substitui a entrada, então a chave antiga
        // sai do índice junto: `order` e `entries` têm sempre o mesmo tamanho.
        if let Some(old) = inner.entries.remove(path) {
            inner.bytes = inner.bytes.saturating_sub(old.bytes);
            inner.order.remove(&old.generation);
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
        inner.order.insert(generation, path.to_path_buf());

        // Despejo FIFO: a `generation` é estritamente crescente, então a
        // primeira chave do `BTreeMap` é a entrada mais antiga ainda viva.
        // Custo O(log n) por despejo e nenhuma varredura de marcadores mortos.
        while inner.bytes > self.max_bytes {
            let Some((_, old_path)) = inner.order.pop_first() else {
                break;
            };
            if let Some(removed) = inner.entries.remove(&old_path) {
                inner.bytes = inner.bytes.saturating_sub(removed.bytes);
            }
        }

        tree
    }

    pub fn parse(&self, path: &Path, language: Language, content: &str) -> Option<Arc<Tree>> {
        let digest = blake3::hash(content.as_bytes());
        self.parse_with_digest(path, language, content, digest)
    }

    pub fn parse_with_digest(
        &self,
        path: &Path,
        language: Language,
        content: &str,
        digest: blake3::Hash,
    ) -> Option<Arc<Tree>> {
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
            inner.order.remove(&removed.generation);
        }
    }

    pub fn len(&self) -> usize {
        self.inner
            .read()
            .map(|inner| inner.entries.len())
            .unwrap_or(0)
    }

    /// Só para teste: o índice de despejo tem que ter exatamente uma chave por
    /// entrada viva — nunca marcadores mortos acumulando.
    #[cfg(test)]
    fn order_len(&self) -> usize {
        self.inner
            .read()
            .map(|inner| inner.order.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Language;

    /// `parse` só insere quando o digest muda; variar o fonte a cada volta
    /// força uma inserção nova no mesmo `path`, que é o padrão real do graph
    /// cache: cada rebuild invalida e reparseia todos os arquivos.
    #[test]
    fn reparsing_the_same_path_does_not_grow_the_eviction_index() {
        let cache = ParseCache::with_max_bytes(4 * 1024 * 1024);
        let path = Path::new("src/lib.rs");

        for round in 0..64 {
            let source = format!("fn item_{round}() {{}}\n");
            cache
                .parse(path, Language::Rust, &source)
                .expect("o fonte Rust precisa ser parseável");
        }

        assert_eq!(cache.len(), 1, "o cache guarda uma entrada por path");
        assert_eq!(
            cache.order_len(),
            1,
            "o índice de despejo não pode acumular um marcador por parse"
        );
    }

    #[test]
    fn the_oldest_entry_leaves_first_when_the_budget_is_exceeded() {
        let a = "fn a() {}\n";
        let b = "fn b() {}\n";
        let c = "fn c() {}\n";
        // Orçamento de duas árvores e meia: a terceira entrada não cabe.
        let cache = ParseCache::with_max_bytes(a.len() * 2 + a.len() / 2);

        let first = Path::new("a.rs");
        let second = Path::new("b.rs");
        let third = Path::new("c.rs");
        cache.parse(first, Language::Rust, a).expect("parse de a");
        cache.parse(second, Language::Rust, b).expect("parse de b");

        assert_eq!(cache.len(), 2, "as duas primeiras cabem no orçamento");
        assert_eq!(cache.order_len(), 2);

        cache.parse(third, Language::Rust, c).expect("parse de c");

        assert_eq!(cache.len(), 2, "entrou c, saiu a entrada mais antiga");
        assert_eq!(cache.order_len(), 2, "o índice acompanha o cache");
        assert!(
            cache.get(first, blake3::hash(a.as_bytes())).is_none(),
            "a entrada mais antiga é a despejada"
        );
        assert!(cache.get(second, blake3::hash(b.as_bytes())).is_some());
        assert!(cache.get(third, blake3::hash(c.as_bytes())).is_some());
    }

    #[test]
    fn invalidating_a_path_releases_its_index_slot() {
        let source = "fn a() {}\n";
        let cache = ParseCache::with_max_bytes(4 * 1024 * 1024);
        let path = Path::new("a.rs");

        cache.parse(path, Language::Rust, source).expect("parse");
        assert_eq!(cache.order_len(), 1);

        cache.invalidate(path);
        assert_eq!(cache.len(), 0);
        assert_eq!(
            cache.order_len(),
            0,
            "a chave do índice sai junto com a entrada"
        );
    }
}

impl Default for ParseCache {
    fn default() -> Self {
        Self::new()
    }
}
