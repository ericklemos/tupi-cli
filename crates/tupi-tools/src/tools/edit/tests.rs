use super::*;
use std::fs;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use tupi_kernel::{Tool, ToolError};

use crate::budget::DEFAULT_BUDGET_TOKENS;
use crate::span::hash_prefix;
use crate::store::ResponseStore;

fn fixture() -> TempDir {
    let dir = tempdir().unwrap();
    fs::write(
            dir.path().join("auth.rs"),
            "use std::io;\n\npub fn verify(raw: &str) -> bool {\n    !raw.is_empty()\n}\n\npub fn logout() {\n}\n",
        )
        .unwrap();
    dir
}

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

fn params(dir: &TempDir, symbol: &str, body: &str) -> EditParams {
    let content = fs::read(dir.path().join("auth.rs")).unwrap();
    EditParams {
        path: "auth.rs".into(),
        symbol: Some(symbol.into()),
        old: None,
        new: None,
        expected_hash: hash_prefix(&content),
        new_body: Some(body.into()),
        signature: None,
        budget_tokens: None,
        start: None,
        end: None,
        expect: None,
    }
}

#[test]
fn an_edit_replaces_the_body_and_leaves_the_rest_byte_identical() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let out = tool
        .edit(params(&dir, "verify", "{\n    raw.len() > 3\n}"))
        .unwrap();

    let after = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
    assert_eq!(
        after,
        "use std::io;\n\npub fn verify(raw: &str) -> bool {\n    raw.len() > 3\n}\n\npub fn logout() {\n}\n"
    );
    assert!(out.contains("editado"), "{out}");
    assert!(out.contains(&hash_prefix(after.as_bytes())), "{out}");
}

#[test]
fn a_stale_hash_never_writes() {
    let dir = fixture();
    let target = dir.path().join("auth.rs");
    let before = fs::read_to_string(&target).unwrap();
    let mut request = params(&dir, "verify", "{\n    true\n}");
    request.expected_hash = "deadbeef".into();

    let error = tool_for(dir.path()).edit(request).unwrap_err();

    assert!(matches!(error, ToolError::StaleHash(_)), "{error:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), before);
}

#[test]
fn an_already_applied_body_with_a_stale_hash_is_idempotent() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let old_hash = params(&dir, "verify", "{}").expected_hash;
    tool.edit(params(&dir, "verify", "{\n    raw.len() > 3\n}"))
        .unwrap();
    let after = fs::read_to_string(dir.path().join("auth.rs")).unwrap();

    let mut again = params(&dir, "verify", "{\n    raw.len() > 3\n}");
    again.expected_hash = old_hash;
    let out = tool.edit(again).unwrap();

    assert!(out.contains("já aplicada"), "{out}");
    assert_eq!(
        fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
        after
    );
}

#[test]
fn an_unknown_symbol_is_reported() {
    let dir = fixture();
    let error = tool_for(dir.path())
        .edit(params(&dir, "nope", "{}"))
        .unwrap_err();
    assert!(matches!(error, ToolError::SymbolNotFound(_)), "{error:?}");
}

#[test]
fn an_ambiguous_symbol_is_refused_and_nothing_is_written() {
    let dir = tempdir().unwrap();
    let content = "impl A {\n    fn new() {}\n}\n\nimpl B {\n    fn new() {}\n}\n";
    fs::write(dir.path().join("auth.rs"), content).unwrap();
    let request = params(&dir, "new", "{\n    1\n}");

    let error = tool_for(dir.path()).edit(request).unwrap_err();

    assert!(matches!(error, ToolError::Ambiguous(_)), "{error:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
        content
    );
}

#[test]
fn a_signature_replaces_the_head() {
    let dir = fixture();
    let mut request = params(&dir, "verify", "{\n    1\n}");
    request.signature = Some("pub fn verify(raw: &str) -> u8".into());

    tool_for(dir.path()).edit(request).unwrap();

    let after = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
    assert!(
        after.contains("pub fn verify(raw: &str) -> u8 {\n    1\n}"),
        "{after}"
    );
}

#[test]
fn a_body_without_braces_is_rejected() {
    let dir = fixture();
    let before = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
    let error = tool_for(dir.path())
        .edit(params(&dir, "verify", "return true;"))
        .unwrap_err();

    assert!(matches!(error, ToolError::InvalidArguments(_)), "{error:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
        before
    );
}

#[test]
fn invalid_syntax_does_not_write() {
    let dir = fixture();
    let before = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
    let error = tool_for(dir.path())
        .edit(params(&dir, "verify", "{\n    let ;\n}"))
        .unwrap_err();

    assert!(matches!(error, ToolError::InvalidSyntax(_)), "{error:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
        before
    );
}

#[test]
fn a_struct_body_is_editable() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("point.rs"),
        "struct Point {\n    x: u8,\n}\n",
    )
    .unwrap();
    let content = fs::read(dir.path().join("point.rs")).unwrap();
    let tool = tool_for(dir.path());
    tool.edit(EditParams {
        path: "point.rs".into(),
        symbol: Some("Point".into()),
        old: None,
        new: None,
        expected_hash: hash_prefix(&content),
        new_body: Some("{\n    x: u8,\n    y: u8,\n}".into()),
        signature: None,
        budget_tokens: None,
        start: None,
        end: None,
        expect: None,
    })
    .unwrap();

    let after = fs::read_to_string(dir.path().join("point.rs")).unwrap();
    assert_eq!(after, "struct Point {\n    x: u8,\n    y: u8,\n}\n");
}

#[test]
fn a_path_outside_the_root_is_rejected() {
    let dir = fixture();
    let mut request = params(&dir, "verify", "{}");
    request.path = "../fora.rs".into();
    let error = tool_for(dir.path()).edit(request).unwrap_err();
    assert!(matches!(error, ToolError::OutsideRoot(_)), "{error:?}");
}

#[test]
fn a_non_utf8_file_is_rejected() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("bin.rs"), [0xff, 0xfe]).unwrap();
    let error = tool_for(dir.path())
        .edit(EditParams {
            path: "bin.rs".into(),
            symbol: Some("x".into()),
            old: None,
            new: None,
            expected_hash: "deadbeef".into(),
            new_body: Some("{}".into()),
            signature: None,
            budget_tokens: None,
            start: None,
            end: None,
            expect: None,
        })
        .unwrap_err();
    assert!(matches!(error, ToolError::Other(_)), "{error:?}");
}

#[test]
fn describe_shows_path_and_symbol() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    assert_eq!(
        tool.describe_call(&serde_json::json!({
            "path": "auth.rs",
            "symbol": "Auth::verify"
        })),
        "path=auth.rs · symbol=Auth::verify"
    );
}

#[test]
fn a_signature_that_would_change_the_item_kind_is_refused() {
    let dir = fixture();
    let before = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
    let mut request = params(&dir, "verify", "{\n    1\n}");
    request.signature = Some("impl verify".into());

    let error = tool_for(dir.path()).edit(request).unwrap_err();

    assert!(matches!(error, ToolError::InvalidArguments(_)), "{error:?}");
    assert_eq!(
        fs::read_to_string(dir.path().join("auth.rs")).unwrap(),
        before
    );
}

#[test]
fn a_body_that_smuggles_a_second_item_is_refused() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("cache.rs"),
        "struct Entry {\n    text: String,\n}\n",
    )
    .unwrap();
    let content = fs::read(dir.path().join("cache.rs")).unwrap();

    let error = tool_for(dir.path())
        .edit(EditParams {
            path: "cache.rs".into(),
            symbol: Some("Entry".into()),
            old: None,
            new: None,
            expected_hash: hash_prefix(&content),
            new_body: Some("{\n    text: String,\n}\n\npub struct Cache {\n    width: usize,\n}".into()),
            signature: None,
            budget_tokens: None,
            start: None,
            end: None,
            expect: None,
        })
        .unwrap_err();

    assert!(matches!(error, ToolError::InvalidArguments(_)), "{error:?}");
    assert_eq!(fs::read(dir.path().join("cache.rs")).unwrap(), content);
}

#[test]
fn a_request_without_symbol_or_old_is_refused_naming_both_routes() {
    let dir = fixture();
    let content = fs::read(dir.path().join("auth.rs")).unwrap();
    let error = tool_for(dir.path())
        .edit(EditParams {
            path: "auth.rs".into(),
            symbol: None,
            old: None,
            new: None,
            expected_hash: hash_prefix(&content),
            new_body: None,
            signature: None,
            budget_tokens: None,
            start: None,
            end: None,
            expect: None,
        })
        .unwrap_err();

    let message = match error {
        ToolError::InvalidArguments(message) => message,
        other => panic!("esperava InvalidArguments, veio {other:?}"),
    };
    assert!(
        message.contains("`symbol`") && message.contains("`old`"),
        "{message}"
    );
    assert!(
        message.contains("`new_body`") && message.contains("`new`"),
        "{message}"
    );
    assert_eq!(fs::read(dir.path().join("auth.rs")).unwrap(), content);
}

#[test]
fn symbol_and_old_together_are_refused_as_alternatives() {
    let dir = fixture();
    let error = tool_for(dir.path())
        .edit(EditParams {
            path: "auth.rs".into(),
            symbol: Some("verify".into()),
            old: Some("pub fn logout() {\n}".into()),
            new: Some("pub fn logout() { }".into()),
            expected_hash: hash_prefix(&fs::read(dir.path().join("auth.rs")).unwrap()),
            new_body: None,
            signature: None,
            budget_tokens: None,
            start: None,
            end: None,
            expect: None,
        })
        .unwrap_err();

    let message = match error {
        ToolError::InvalidArguments(message) => message,
        other => panic!("esperava InvalidArguments, veio {other:?}"),
    };
    assert!(message.contains("alternativas"), "{message}");
    assert!(
        message.contains("`symbol`") && message.contains("`old`"),
        "{message}"
    );
}

#[test]
fn a_signature_can_rename_the_symbol() {
    let dir = fixture();
    let mut request = params(&dir, "verify", "{\n    1\n}");
    request.signature = Some("fn check".into());

    tool_for(dir.path()).edit(request).unwrap();

    let after = fs::read_to_string(dir.path().join("auth.rs")).unwrap();
    assert!(after.contains("fn check"), "{after}");
}
