//! `lookup` fechando o buraco do `native-search`: busca literal em
//! `caminho:linha:texto` e faixa de linhas com `tail`/`head`, tudo dentro do
//! contrato denso (corte anunciado, erro por item).

use std::path::Path;

use serde_json::json;
use tupi_kernel::ToolRegistry;
use tupi_tools::{ToolsOptions, register_with};

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    std::fs::write(
        dir.path().join("src/x.rs"),
        "pub fn alpha_one() -> u8 {\n    1\n}\n\npub fn alpha_two() -> u8 {\n    2\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("docs/guia.md"),
        "linha 1\nlinha 2\nlinha 3\nlinha 4\nlinha 5\nlinha 6\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("config.ron"),
        "(\n    a: 1,\n    b: 2,\n    c: 3,\n    d: 4,\n)\n",
    )
    .unwrap();
    dir
}

fn registry(root: &Path) -> (ToolRegistry, tempfile::TempDir) {
    let state = tempfile::tempdir().unwrap();
    let mut registry = ToolRegistry::new();
    register_with(
        root.to_path_buf(),
        &mut registry,
        ToolsOptions::hermetic(state.path().to_path_buf()),
    );
    (registry, state)
}

fn call(registry: &ToolRegistry, arguments: serde_json::Value) -> String {
    registry.get("lookup").unwrap().call(arguments).unwrap()
}

#[test]
fn text_groups_matches_as_path_line_text() {
    let dir = fixture();
    let (registry, _state) = registry(dir.path());
    let out = call(
        &registry,
        json!({"ops": [{"kind": "text", "text": "alpha"}]}),
    );
    assert!(out.contains("src/x.rs:1: pub fn alpha_one"), "{out}");
    assert!(out.contains("src/x.rs:5: pub fn alpha_two"), "{out}");
}

#[test]
fn text_declares_the_cut_when_the_cap_trims_the_list() {
    let dir = fixture();
    let (registry, _state) = registry(dir.path());
    let out = call(
        &registry,
        json!({"ops": [{"kind": "text", "text": "alpha", "max_results": 1}]}),
    );
    assert!(out.contains("1 de 2 casamentos"), "{out}");
    assert!(out.contains("peek"), "{out}");
}

#[test]
fn text_filters_by_glob_and_exclude_and_accepts_alternatives() {
    let dir = fixture();
    let (registry, _state) = registry(dir.path());
    let out = call(
        &registry,
        json!({"ops": [{
            "kind": "text",
            "text": "alpha",
            "alternatives": ["linha 2"],
            "glob": "*.rs"
        }]}),
    );
    assert!(out.contains("src/x.rs"), "{out}");
    assert!(!out.contains("docs/guia.md"), "{out}");

    let excluded = call(
        &registry,
        json!({"ops": [{"kind": "text", "text": "linha", "exclude": "docs/*"}]}),
    );
    assert!(!excluded.contains("docs/guia.md"), "{excluded}");
}

#[test]
fn lines_tail_returns_the_last_lines_numbered() {
    let dir = fixture();
    let (registry, _state) = registry(dir.path());
    let out = call(
        &registry,
        json!({"ops": [{"kind": "lines", "path": "docs/guia.md", "tail": 2}]}),
    );
    assert!(out.contains("docs/guia.md:5-6 (de 6 linhas)"), "{out}");
    assert!(out.contains("5  linha 5"), "{out}");
    assert!(out.contains("6  linha 6"), "{out}");
    assert!(!out.contains("linha 1"), "{out}");
}

#[test]
fn lines_range_of_a_ron_file_returns_numbered_lines() {
    let dir = fixture();
    let (registry, _state) = registry(dir.path());
    let out = call(
        &registry,
        json!({"ops": [{"kind": "lines", "path": "config.ron", "start": 2, "end": 4}]}),
    );
    assert!(out.contains("config.ron:2-4 (de 6 linhas)"), "{out}");
    assert!(out.contains("2      a: 1,"), "{out}");
    assert!(out.contains("4      c: 3,"), "{out}");
}

#[test]
fn a_refused_item_does_not_take_the_rest_of_the_batch_down() {
    let dir = fixture();
    let (registry, _state) = registry(dir.path());
    let out = call(
        &registry,
        json!({"ops": [
            {"kind": "lines", "path": "nao-existe.md"},
            {"kind": "symbol", "path": "src/x.rs", "symbol": "alpha_one"}
        ]}),
    );
    assert!(out.contains("recusada"), "{out}");
    assert!(out.contains("alpha_one"), "{out}");
}

#[test]
fn regex_is_refused_with_the_alternative() {
    let dir = fixture();
    let (registry, _state) = registry(dir.path());
    let out = call(
        &registry,
        json!({"ops": [{"kind": "text", "text": "alpha", "regex": true}]}),
    );
    assert!(out.contains("regex: true"), "{out}");
    assert!(out.contains("alternatives"), "{out}");
}
