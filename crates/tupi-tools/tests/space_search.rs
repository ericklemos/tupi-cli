//! Integração densa do search via API pública (registry).

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tupi_kernel::ToolRegistry;
use tupi_kernel::embed::{EmbedError, Embedder, EmbedderSlot};
use tupi_tools::space::{SpaceEntry, SpaceIndex, SpaceSnapshot};
use tupi_tools::{ToolsOptions, register_with};

struct Fake;

impl Embedder for Fake {
    fn model_id(&self) -> &str {
        "fake"
    }
    fn dim(&self) -> usize {
        4
    }
    fn max_doc_tokens(&self) -> usize {
        512
    }
    fn max_query_tokens(&self) -> usize {
        512
    }
    fn token_len(&self, text: &str) -> usize {
        text.split_whitespace().count()
    }
    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|text| vec_of(text)).collect())
    }
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(vec_of(text))
    }
}

fn vec_of(text: &str) -> Vec<f32> {
    let mut buckets = [0f32; 4];
    for word in text
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric() && character != '_')
    {
        if word.is_empty() {
            continue;
        }
        let word = match word {
            "carro" | "automovel" | "veiculo" => "veiculo",
            other => other,
        };
        let index = word.bytes().fold(0usize, |acc, byte| acc + byte as usize) % 4;
        buckets[index] += 1.0;
    }
    buckets.to_vec()
}

fn write(dir: &Path, relative: &str, body: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn snapshot(entries: Vec<(&str, usize, &str)>, expected: usize) -> SpaceSnapshot {
    let mut out = Vec::new();
    let mut vectors = Vec::new();
    for (relative, start, text) in entries {
        out.push(SpaceEntry {
            relative: relative.to_string(),
            qualified: "alpha".to_string(),
            symbol: "alpha".to_string(),
            start_line: start,
            end_line: start + 2,
        });
        vectors.push(vec_of(text));
    }
    SpaceSnapshot {
        model_id: "fake".to_string(),
        dim: 4,
        entries: out,
        vectors,
        expected_symbols: expected,
    }
}

fn registry(dir: &Path, space: Option<Arc<SpaceIndex>>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let embedder = space
        .as_ref()
        .map(|_| Arc::new(EmbedderSlot::new(Arc::new(Fake))));
    let options = ToolsOptions {
        space,
        embedder,
        space_worker: false,
        ..ToolsOptions::default()
    };
    register_with(dir.to_path_buf(), &mut registry, options);
    registry
}

fn call(registry: &ToolRegistry, query: &str, path: Option<&str>) -> String {
    let mut payload = json!({"query": query, "max_results": 5, "detail": "Signatures"});
    if let Some(path) = path {
        payload["path"] = json!(path);
    }
    registry.get("search").unwrap().call(payload).unwrap()
}

fn call_detail(registry: &ToolRegistry, query: &str, path: Option<&str>, detail: &str) -> String {
    let mut payload = json!({"query": query, "max_results": 5, "detail": detail});
    if let Some(path) = path {
        payload["path"] = json!(path);
    }
    registry.get("search").unwrap().call(payload).unwrap()
}

#[test]
fn dense_finds_a_symbol_when_lexical_finds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/a.rs",
        "// veiculo\npub fn alpha() -> u32 {\n    1\n}\n",
    );
    let index = Arc::new(SpaceIndex::new());
    index.set(snapshot(vec![("src/a.rs", 2, "alpha src/a.rs veiculo")], 1));
    let output = call(&registry(dir.path(), Some(index)), "automovel", None);
    assert!(output.contains("alpha"), "{output}");
}

#[test]
fn without_space_a_query_with_no_lexical_match_still_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/a.rs",
        "// veiculo\npub fn alpha() -> u32 {\n    1\n}\n",
    );
    let output = call(&registry(dir.path(), None), "automovel", None);
    assert!(output.contains("Nenhum resultado"), "{output}");
}

#[test]
fn dense_respects_the_path_filter() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/a.rs",
        "// veiculo\npub fn alpha() -> u32 {\n    1\n}\n",
    );
    write(
        dir.path(),
        "src/b.rs",
        "// veiculo\npub fn beta() -> u32 {\n    2\n}\n",
    );
    let index = Arc::new(SpaceIndex::new());
    index.set(snapshot(
        vec![
            ("src/a.rs", 2, "alpha veiculo"),
            ("src/b.rs", 2, "beta veiculo"),
        ],
        2,
    ));
    let output = call(
        &registry(dir.path(), Some(index)),
        "automovel",
        Some("src/b.rs"),
    );
    assert!(output.contains("beta"), "{output}");
    assert!(!output.contains("alpha"), "{output}");
}

#[test]
fn dense_hit_is_delivered_once() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/a.rs",
        "// veiculo\npub fn alpha() -> u32 {\n    1\n}\n",
    );
    let index = Arc::new(SpaceIndex::new());
    index.set(snapshot(vec![("src/a.rs", 2, "alpha veiculo")], 1));
    let registry = registry(dir.path(), Some(index));
    let first = call_detail(&registry, "automovel", None, "Full");
    let second = call_detail(&registry, "automovel", None, "Full");
    assert!(first.contains("alpha"), "{first}");
    assert!(second.contains("Já entregue"), "{second}");
}

#[test]
fn lexical_and_dense_do_not_duplicate_the_same_symbol() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/a.rs",
        "// veiculo\npub fn alpha() -> u32 {\n    1\n}\n",
    );
    let index = Arc::new(SpaceIndex::new());
    index.set(snapshot(vec![("src/a.rs", 2, "alpha veiculo")], 1));
    let output = call(&registry(dir.path(), Some(index)), "alpha", None);
    assert_eq!(
        output.matches("\u{60}alpha\u{60} (\u{60}src/a.rs").count(),
        1,
        "{output}"
    );
}

#[test]
fn partial_index_declares_progress() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "src/a.rs",
        "// veiculo\npub fn alpha() -> u32 {\n    1\n}\n",
    );
    let index = Arc::new(SpaceIndex::new());
    index.set(snapshot(vec![("src/a.rs", 2, "alpha veiculo")], 10));
    let output = call(&registry(dir.path(), Some(index)), "automovel", None);
    assert!(output.contains("símbolos indexados"), "{output}");
}
