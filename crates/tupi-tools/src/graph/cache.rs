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

    pub fn repair(&self, paths: &[PathBuf]) {
        let current = self
            .graph
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(current) = current else {
            return;
        };

        if !paths.is_empty() {
            let root = self
                .root
                .canonicalize()
                .unwrap_or_else(|_| self.root.clone());
            let unchanged = paths.iter().all(|path| {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    root.join(path)
                };
                match std::fs::read(&resolved) {
                    Ok(bytes) => {
                        let hash = blake3::hash(&bytes);
                        current
                            .file_hashes
                            .get(&resolved)
                            .or_else(|| current.file_hashes.get(path))
                            .is_some_and(|existing| *existing == hash)
                    }
                    Err(_) => {
                        !current.file_hashes.contains_key(&resolved)
                            && !current.file_hashes.contains_key(path)
                    }
                }
            });
            if unchanged {
                return;
            }
        }

        self.invalidate();
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
        let cache = GraphCache {
            root: PathBuf::from("."),
            builder: None,
            graph: RwLock::new(None),
        };
        assert!(cache.get().defs.is_empty());
    }

    #[test]
    fn repair_skips_rebuilding_when_paths_already_match_graph_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "pub fn alpha() {}\n").unwrap();
        let cache = GraphCache::new(dir.path().to_path_buf());
        let first = cache.get();

        // Calling repair again when the file content on disk hasn't changed since the
        // graph was built (e.g. when GraphWatcher fires after RepairingTool already
        // repaired) must keep the existing Arc<ProjectCodeGraph> without rebuilding.
        cache.repair(&[file]);
        let second = cache.get();
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn repair_before_first_get_is_lazy_and_does_not_build_eagerly() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "pub fn alpha() {}\n").unwrap();
        let cache = GraphCache::new(dir.path().to_path_buf());

        cache.repair(&[file]);
        assert!(cache.graph.read().unwrap().is_none());
    }

    #[test]
    fn repairing_tool_extracts_from_to_and_patch_paths() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "pub fn alpha() {}\n").unwrap();
        let cache = Arc::new(GraphCache::new(dir.path().to_path_buf()));
        let first = cache.get();

        let noop = Arc::new(crate::env::EnvTool::new());
        let repairing = crate::graph::RepairingTool::new(noop, cache.clone());

        // Move arguments on unchanged/non-source or matching file should extract paths rather than empty-paths invalidate
        let move_paths = repairing.paths(&serde_json::json!({
            "from": "a.rs",
            "to": "a.rs"
        }));
        assert_eq!(move_paths.len(), 2);

        let patch_paths = repairing.paths(&serde_json::json!({
            "patch": "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-pub fn alpha() {}\n+pub fn alpha() {}\n"
        }));
        assert_eq!(patch_paths, vec![dir.path().join("a.rs")]);
        assert!(Arc::ptr_eq(&first, &cache.get()));
    }
}
