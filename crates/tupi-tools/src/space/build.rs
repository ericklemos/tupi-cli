//! Build incremental do índice vetorial a partir dos símbolos do grafo.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::graph::{Def, GraphCache};
use tupi_kernel::embed::{Embedder, EmbedderSlot};

use super::index::{SpaceEntry, SpaceIndex, SpaceSnapshot};
use super::store::{self, SPACE_VERSION, SpaceFile, SpaceManifest, SpaceSymbol};

/// Resultado de uma passada do builder.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BuildReport {
    pub embedded_files: usize,
    pub skipped_files: usize,
    pub symbols: usize,
    pub expected_symbols: usize,
    pub errors: usize,
}

/// Texto que representa um símbolo no índice.
pub fn symbol_text(def: &Def, body: &str) -> String {
    let text = format!(
        "{}\n{}\n{}\n{}",
        def.qualified, def.relative, def.signature, body
    );
    let mut cut = text.len().min(1000);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_string()
}

/// Reconstrói o índice, re-embedando só os arquivos cujo hash mudou.
pub fn rebuild(root: &Path, index: &SpaceIndex, embedder: &dyn Embedder) -> BuildReport {
    let graph = GraphCache::shared(root).get();
    let mut by_file: BTreeMap<String, Vec<&Def>> = BTreeMap::new();
    for def in graph.defs.iter().filter(|def| !def.is_test) {
        by_file.entry(def.relative.clone()).or_default().push(def);
    }
    let expected_symbols = by_file.values().map(Vec::len).sum();
    let mut report = BuildReport {
        expected_symbols,
        ..BuildReport::default()
    };

    let mut manifest = store::load_manifest(root).unwrap_or(SpaceManifest {
        version: SPACE_VERSION,
        model_id: embedder.model_id().to_string(),
        dim: embedder.dim(),
        files: BTreeMap::new(),
    });
    if manifest.version != SPACE_VERSION
        || manifest.model_id != embedder.model_id()
        || manifest.dim != embedder.dim()
    {
        for file in manifest.files.values() {
            let _ = store::remove_vectors(root, &file.hash);
        }
        manifest = SpaceManifest {
            version: SPACE_VERSION,
            model_id: embedder.model_id().to_string(),
            dim: embedder.dim(),
            files: BTreeMap::new(),
        };
    }

    let mut entries = Vec::new();
    let mut vectors = Vec::new();
    let mut contents: HashMap<String, String> = HashMap::new();
    for (relative, defs) in &by_file {
        let content = contents
            .entry(relative.clone())
            .or_insert_with(|| std::fs::read_to_string(root.join(relative)).unwrap_or_default())
            .clone();
        let hash = crate::support::ledger::content_hash(&content)
            .to_hex()
            .to_string();
        let existing = manifest.files.get(relative).cloned();
        let reusable = existing.as_ref().and_then(|file| {
            if file.hash != hash {
                return None;
            }
            match store::load_vectors(root, &file.hash) {
                Ok(Some((dim, blob))) if dim == embedder.dim() && blob.len() == defs.len() => {
                    Some(blob)
                }
                _ => None,
            }
        });
        let file_vectors = match reusable {
            Some(blob) => {
                report.skipped_files += 1;
                blob
            }
            None => {
                let texts: Vec<String> = defs
                    .iter()
                    .map(|def| {
                        let body = crate::tools::docs::store::slice_lines(
                            &content,
                            def.start_line,
                            def.end_line,
                        )
                        .unwrap_or_default();
                        symbol_text(def, &body)
                    })
                    .collect();
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                let mut blob = Vec::with_capacity(refs.len());
                for chunk in refs.chunks(8) {
                    match embedder.embed_documents(chunk) {
                        Ok(mut batch) => blob.append(&mut batch),
                        Err(error) => {
                            tracing::warn!(%error, relative, "space: embedding falhou");
                            report.errors += 1;
                        }
                    }
                }
                if blob.len() != defs.len() {
                    report.errors += 1;
                    continue;
                }
                let file = SpaceFile {
                    hash: hash.clone(),
                    symbols: defs
                        .iter()
                        .map(|def| SpaceSymbol {
                            qualified: def.qualified.clone(),
                            symbol: def.name.clone(),
                            start: def.start_line,
                            end: def.end_line,
                        })
                        .collect(),
                };
                manifest.files.insert(relative.clone(), file);
                if let Err(error) = store::save_vectors(root, &hash, embedder.dim(), &blob) {
                    tracing::warn!(%error, relative, "space: gravar vetores falhou");
                    report.errors += 1;
                }
                if let Some(old) = existing.filter(|file| file.hash != hash) {
                    let _ = store::remove_vectors(root, &old.hash);
                }
                report.embedded_files += 1;
                blob
            }
        };

        for (def, vector) in defs.iter().zip(file_vectors) {
            entries.push(SpaceEntry {
                relative: relative.clone(),
                qualified: def.qualified.clone(),
                symbol: def.name.clone(),
                start_line: def.start_line,
                end_line: def.end_line,
            });
            vectors.push(vector);
        }
        report.symbols += defs.len();
    }

    let stale: Vec<String> = manifest
        .files
        .keys()
        .filter(|relative| !by_file.contains_key(*relative))
        .cloned()
        .collect();
    for relative in stale {
        if let Some(file) = manifest.files.remove(&relative) {
            let _ = store::remove_vectors(root, &file.hash);
        }
    }

    let _ = store::ensure_gitignore(root);
    if let Err(error) = store::save_manifest(root, &manifest) {
        tracing::warn!(%error, "space: gravar manifest falhou");
        report.errors += 1;
    }
    index.set(SpaceSnapshot {
        model_id: embedder.model_id().to_string(),
        dim: embedder.dim(),
        entries,
        vectors,
        expected_symbols,
    });
    report
}

/// Algum arquivo mudou desde o manifest?
pub fn needs_rebuild(root: &Path, manifest: &SpaceManifest) -> bool {
    let graph = GraphCache::shared(root).get();
    let mut current: BTreeMap<String, String> = BTreeMap::new();
    for def in graph.defs.iter().filter(|def| !def.is_test) {
        if current.contains_key(&def.relative) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(root.join(&def.relative)) else {
            continue;
        };
        current.insert(
            def.relative.clone(),
            crate::support::ledger::content_hash(&content)
                .to_hex()
                .to_string(),
        );
    }
    current.len() != manifest.files.len()
        || current.iter().any(|(relative, hash)| {
            manifest
                .files
                .get(relative)
                .is_none_or(|file| &file.hash != hash)
        })
}

/// Loop de background: espera o embedder e mantém o índice atualizado.
pub fn spawn(root: std::path::PathBuf, index: Arc<SpaceIndex>, embedder: Arc<EmbedderSlot>) {
    std::thread::spawn(move || {
        loop {
            if let Some(current) = embedder.get() {
                let stale = store::load_manifest(&root)
                    .is_none_or(|manifest| index.is_empty() || needs_rebuild(&root, &manifest));
                if stale {
                    let report = rebuild(&root, &index, current.as_ref());
                    tracing::info!(
                        embedded = report.embedded_files,
                        skipped = report.skipped_files,
                        symbols = report.symbols,
                        errors = report.errors,
                        "space: índice atualizado"
                    );
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(10));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::docs::test_support::FakeEmbedder;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/a.rs"),
            "pub fn alpha() -> u32 {\n    1\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/b.rs"),
            "pub fn beta() -> u32 {\n    2\n}\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn first_build_indexes_every_symbol() {
        let dir = fixture();
        let index = SpaceIndex::new();
        let embedder = FakeEmbedder::default();
        let report = rebuild(dir.path(), &index, &embedder);
        assert_eq!(report.embedded_files, 2);
        assert_eq!(report.expected_symbols, report.symbols);
        assert_eq!(index.snapshot().unwrap().entries.len(), report.symbols);
    }

    #[test]
    fn second_build_embeds_nothing() {
        let dir = fixture();
        let index = SpaceIndex::new();
        let embedder = FakeEmbedder::default();
        rebuild(dir.path(), &index, &embedder);
        let report = rebuild(dir.path(), &index, &embedder);
        assert_eq!(report.embedded_files, 0);
        assert_eq!(report.skipped_files, 2);
    }

    #[test]
    fn touching_one_file_reembeds_only_that_file() {
        let dir = fixture();
        let index = SpaceIndex::new();
        let embedder = FakeEmbedder::default();
        rebuild(dir.path(), &index, &embedder);
        std::fs::write(
            dir.path().join("src/a.rs"),
            "pub fn alpha() -> u32 {\n    1\n}\npub fn gamma() -> u32 {\n    3\n}\n",
        )
        .unwrap();
        GraphCache::invalidate_root(dir.path());
        let report = rebuild(dir.path(), &index, &embedder);
        assert_eq!(report.embedded_files, 1);
        assert_eq!(report.skipped_files, 1);
    }

    #[test]
    fn model_change_wipes_the_index() {
        let dir = fixture();
        let index = SpaceIndex::new();
        rebuild(dir.path(), &index, &FakeEmbedder::default());
        let other = FakeEmbedder::named("outro");
        let report = rebuild(dir.path(), &index, &other);
        assert_eq!(report.embedded_files, 2);
        assert_eq!(index.snapshot().unwrap().model_id, "outro");
    }

    #[test]
    fn needs_rebuild_detects_a_touched_file() {
        let dir = fixture();
        let index = SpaceIndex::new();
        rebuild(dir.path(), &index, &FakeEmbedder::default());
        let manifest = store::load_manifest(dir.path()).unwrap();
        assert!(!needs_rebuild(dir.path(), &manifest));
        std::fs::write(
            dir.path().join("src/b.rs"),
            "pub fn beta() -> u32 {\n    2\n}\npub fn delta() -> u32 {\n    4\n}\n",
        )
        .unwrap();
        GraphCache::invalidate_root(dir.path());
        assert!(needs_rebuild(dir.path(), &manifest));
    }

    #[test]
    fn symbol_text_is_truncated_to_one_thousand_chars() {
        let def = Def {
            name: "alpha".to_string(),
            qualified: "a::alpha".to_string(),
            relative: "src/a.rs".to_string(),
            start_line: 1,
            end_line: 2,
            signature: "pub fn alpha()".to_string(),
            is_public: true,
            is_callable: true,
            is_test: false,
            calls: Vec::new(),
            node: petgraph::graph::NodeIndex::new(0),
        };
        let body = "x".repeat(5000);
        assert_eq!(symbol_text(&def, &body).chars().count(), 1000);
    }
}
