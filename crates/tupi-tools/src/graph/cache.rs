use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crate::graph::builder::GraphBuilder;
use crate::graph::model::ProjectCodeGraph;
use crate::graph::parse_cache::ParseCache;

/// Cache do grafo por root.
///
/// `repair` reconstrói o mapa a partir das árvores em `ParseCache`: arquivos
/// inalterados são hits de parse, então o custo é a passagem de defs/calls, não
/// o tree-sitter de novo. Reconstruir inteiro evita perder arestas de outros
/// arquivos para um símbolo que mudou de arquivo.
pub struct GraphCache {
    root: PathBuf,
    graph: RwLock<Option<Arc<ProjectCodeGraph>>>,
    builder: Option<GraphBuilder>,
}

impl GraphCache {
    pub fn new(root: PathBuf) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Self {
            root,
            graph: RwLock::new(None),
            builder: GraphBuilder::new().ok(),
        }
    }

    pub fn with_parse_cache(root: PathBuf, cache: Arc<ParseCache>) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Self {
            root,
            graph: RwLock::new(None),
            builder: GraphBuilder::new()
                .ok()
                .map(|builder| builder.with_parse_cache(cache)),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn get(&self) -> Arc<ProjectCodeGraph> {
        if let Ok(graph) = self.graph.read()
            && let Some(graph) = graph.as_ref()
        {
            return graph.clone();
        }

        let mut slot = self
            .graph
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(graph) = slot.as_ref() {
            return graph.clone();
        }
        let built = Arc::new(self.build());
        *slot = Some(built.clone());
        built
    }

    pub fn repair(&self, _paths: &[PathBuf]) {
        let rebuilt = Arc::new(self.build());
        *self
            .graph
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(rebuilt);
    }

    fn build(&self) -> ProjectCodeGraph {
        self.builder
            .as_ref()
            .and_then(|builder| builder.build(&self.root).ok())
            .unwrap_or_default()
    }

    pub fn invalidate(&self) {
        *self
            .graph
            .write()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }
}

static CACHES: OnceLock<Mutex<HashMap<PathBuf, Arc<GraphCache>>>> = OnceLock::new();

fn caches() -> &'static Mutex<HashMap<PathBuf, Arc<GraphCache>>> {
    CACHES.get_or_init(|| Mutex::new(HashMap::new()))
}

impl GraphCache {
    pub fn shared(root: &Path) -> Arc<Self> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut caches = caches().lock().unwrap_or_else(|error| error.into_inner());
        caches
            .entry(root.clone())
            .or_insert_with(|| Arc::new(GraphCache::new(root)))
            .clone()
    }

    pub fn shared_with_parse_cache(root: &Path, cache: Arc<ParseCache>) -> Arc<Self> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut caches = caches().lock().unwrap_or_else(|error| error.into_inner());
        caches
            .entry(root.clone())
            .or_insert_with(|| Arc::new(GraphCache::with_parse_cache(root, cache)))
            .clone()
    }

    #[cfg(test)]
    pub fn invalidate_root(root: &Path) {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let caches = caches().lock().unwrap_or_else(|error| error.into_inner());
        if let Some(cache) = caches.get(&root) {
            cache.invalidate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_without_builder_degrades_to_an_empty_graph() {
        let dir = tempfile::tempdir().unwrap();
        let cache = GraphCache {
            root: dir.path().to_path_buf(),
            graph: RwLock::new(None),
            builder: None,
        };
        assert!(cache.get().matching("qualquer").is_empty());
    }
}
