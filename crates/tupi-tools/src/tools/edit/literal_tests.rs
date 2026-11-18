//! Aceite da rota literal do `edit`: âncora exata e única em qualquer texto,
//! com o mesmo contrato de hash e de recusa da rota por símbolo.

use super::*;
use std::fs;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use tupi_kernel::ToolError;

use crate::budget::DEFAULT_BUDGET_TOKENS;
use crate::span::hash_prefix;
use crate::store::ResponseStore;

fn tool_for(dir: &std::path::Path) -> EditTool {
    EditTool::new(
        dir.to_path_buf(),
        Arc::new(ResponseStore::new(
            std::time::Duration::from_secs(60),
            1024 * 1024,
        )),
        DEFAULT_BUDGET_TOKENS,
    )
}

fn literal(dir: &TempDir, path: &str, old: &str, new: &str) -> EditParams {
    let content = fs::read(dir.path().join(path)).unwrap();
    EditParams {
        path: path.into(),
        symbol: None,
        old: Some(old.into()),
        new: Some(new.into()),
        expected_hash: hash_prefix(&content),
        new_body: None,
        signature: None,
        budget_tokens: None,
    }
}

#[test]
fn a_markdown_anchor_is_edited_by_literal_text() {
    let dir = tempdir().unwrap();
    let before = "# Guia\n\nInstale com `cargo install tupi`.\n\nDepois rode `tupi`.\n";
    fs::write(dir.path().join("guia.md"), before).unwrap();

    let out = tool_for(dir.path())
        .edit(literal(
            &dir,
            "guia.md",
            "Instale com `cargo install tupi`.",
            "Instale com `cargo binstall tupi`.",
        ))
        .unwrap();

    let after = fs::read_to_string(dir.path().join("guia.md")).unwrap();
    assert_eq!(
        after,
        "# Guia\n\nInstale com `cargo binstall tupi`.\n\nDepois rode `tupi`.\n"
    );
    assert!(out.contains("guia.md:3"), "{out}");
    assert!(out.contains(&hash_prefix(after.as_bytes())), "{out}");
}

#[test]
fn a_yaml_anchor_is_edited_and_the_rest_is_byte_identical() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("config.yaml"),
        "model: gpt\nmax_tokens: 4096\nprovider: local\n",
    )
    .unwrap();

    tool_for(dir.path())
        .edit(literal(
            &dir,
            "config.yaml",
            "max_tokens: 4096",
            "max_tokens: 2048",
        ))
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
        "model: gpt\nmax_tokens: 2048\nprovider: local\n"
    );
}

#[test]
fn a_missing_anchor_is_refused_and_nothing_is_written() {
    let dir = tempdir().unwrap();
    let before = "# Guia\n\nInstale com `cargo install tupi`.\n";
    fs::write(dir.path().join("guia.md"), before).unwrap();

    let error = tool_for(dir.path())
        .edit(literal(
            &dir,
            "guia.md",
            "Rode `tupi init` primeiro.",
            "Rode `tupi setup` primeiro.",
        ))
        .unwrap_err();

    assert!(matches!(error, ToolError::Other(_)), "{error:?}");
    assert!(error.to_string().contains("não existe"), "{error}");
    assert_eq!(
        fs::read_to_string(dir.path().join("guia.md")).unwrap(),
        before
    );
}

#[test]
fn an_ambiguous_anchor_is_refused_with_the_count_and_the_lines() {
    let dir = tempdir().unwrap();
    let before = "a: 1\n-\na: 1\n";
    fs::write(dir.path().join("dup.yaml"), before).unwrap();

    let error = tool_for(dir.path())
        .edit(literal(&dir, "dup.yaml", "a: 1", "a: 2"))
        .unwrap_err();

    let message = match error {
        ToolError::Ambiguous(message) => message,
        other => panic!("esperava Ambiguous, veio {other:?}"),
    };
    assert!(message.contains("casa 2 vezes"), "{message}");
    assert!(message.contains("linhas 1, 3"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.path().join("dup.yaml")).unwrap(),
        before
    );
}

#[test]
fn a_stale_hash_never_writes_through_the_literal_route() {
    let dir = tempdir().unwrap();
    let before = "model: gpt\nmax_tokens: 4096\n";
    fs::write(dir.path().join("config.yaml"), before).unwrap();
    let mut request = literal(&dir, "config.yaml", "max_tokens: 4096", "max_tokens: 2048");
    request.expected_hash = "deadbeef".into();

    let error = tool_for(dir.path()).edit(request).unwrap_err();

    assert!(matches!(error, ToolError::StaleHash(_)), "{error:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
        before
    );
}
