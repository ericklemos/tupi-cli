//! Aceite da rota faixa de linhas do `edit`: `start`+`end`+`expect` em qualquer
//! arquivo de texto, com o mesmo contrato de hash e de recusa das outras rotas.

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

fn range(
    dir: &TempDir,
    path: &str,
    start: usize,
    end: usize,
    expect: &str,
    new: &str,
) -> EditParams {
    let content = fs::read(dir.path().join(path)).unwrap();
    EditParams {
        path: path.into(),
        symbol: None,
        old: None,
        new: Some(new.into()),
        start: Some(start),
        end: Some(end),
        expect: Some(expect.into()),
        expected_hash: hash_prefix(&content),
        new_body: None,
        signature: None,
        budget_tokens: None,
    }
}

#[test]
fn a_line_range_is_applied_in_a_python_file() {
    let dir = tempdir().unwrap();
    let before = "import os\n\n\ndef main():\n    return 1\n";
    fs::write(dir.path().join("main.py"), before).unwrap();

    let out = tool_for(dir.path())
        .edit(range(
            &dir,
            "main.py",
            5,
            5,
            "    return 1",
            "    return 2",
        ))
        .unwrap();

    let after = fs::read_to_string(dir.path().join("main.py")).unwrap();
    assert_eq!(after, "import os\n\n\ndef main():\n    return 2\n");
    assert!(out.contains("main.py:5-5"), "{out}");
    assert!(out.contains(&hash_prefix(after.as_bytes())), "{out}");
}

#[test]
fn a_multi_line_range_replaces_only_the_expected_lines() {
    let dir = tempdir().unwrap();
    let before = "a: 1\nkey: old\nkey2: old\nz: 9\n";
    fs::write(dir.path().join("dup.yaml"), before).unwrap();

    tool_for(dir.path())
        .edit(range(
            &dir,
            "dup.yaml",
            2,
            3,
            "key: old\nkey2: old",
            "key: new",
        ))
        .unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("dup.yaml")).unwrap(),
        "a: 1\nkey: new\nz: 9\n"
    );
}

#[test]
fn a_shifted_range_under_a_stale_hash_is_refused() {
    let dir = tempdir().unwrap();
    let before = "a: 1\nkey: old\nz: 9\n";
    fs::write(dir.path().join("dup.yaml"), before).unwrap();
    let mut request = range(&dir, "dup.yaml", 2, 2, "key: old", "key: new");
    request.expected_hash = "deadbeef".into();

    let error = tool_for(dir.path()).edit(request).unwrap_err();

    assert!(matches!(error, ToolError::StaleHash(_)), "{error:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("dup.yaml")).unwrap(),
        before
    );
}

#[test]
fn a_divergent_range_is_refused_and_nothing_is_written() {
    let dir = tempdir().unwrap();
    let before = "a: 1\nkey: old\nz: 9\n";
    fs::write(dir.path().join("dup.yaml"), before).unwrap();

    let error = tool_for(dir.path())
        .edit(range(&dir, "dup.yaml", 1, 1, "key: old", "key: new"))
        .unwrap_err();

    assert!(matches!(error, ToolError::Other(_)), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("não é o texto atual"), "{message}");
    assert!(message.contains("a: 1"), "{message}");
    assert!(message.contains("Nada foi escrito"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.path().join("dup.yaml")).unwrap(),
        before
    );
}

#[test]
fn a_range_past_the_end_is_refused_with_the_line_count() {
    let dir = tempdir().unwrap();
    let before = "a: 1\nkey: old\n";
    fs::write(dir.path().join("dup.yaml"), before).unwrap();

    let error = tool_for(dir.path())
        .edit(range(&dir, "dup.yaml", 7, 8, "nada", "outra"))
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("não existe"), "{message}");
    assert!(message.contains("2 linha(s)"), "{message}");
    assert_eq!(
        fs::read_to_string(dir.path().join("dup.yaml")).unwrap(),
        before
    );
}
