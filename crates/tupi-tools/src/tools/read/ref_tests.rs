use super::*;
use std::fs;
use std::sync::Arc;
use tempfile::tempdir;

fn tool_for(dir: &std::path::Path) -> ReadTool {
    ReadTool::new(
        dir.to_path_buf(),
        Arc::new(ResponseStore::new(
            std::time::Duration::from_secs(60),
            1024 * 1024,
        )),
        Arc::new(DeliveryLedger::new(64, 1 << 20)),
        crate::budget::DEFAULT_BUDGET_TOKENS,
    )
}

fn read(tool: &ReadTool, path: &str, symbol: Option<&str>) -> Result<String, ToolError> {
    tool.read(ReadParams {
        path: path.to_string(),
        symbol: symbol.map(str::to_string),
        budget_tokens: None,
        repeat: None,
        rev: None,
    })
}

#[test]
fn read_starts_with_a_reference_line() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("auth.rs"), "pub fn alpha() {}\n").unwrap();
    let tool = tool_for(dir.path());
    let out = read(&tool, "auth.rs", None).unwrap();
    let first = out.lines().next().unwrap();
    assert!(first.starts_with("[ref] auth.rs @"), "{first}");
}

#[test]
fn symbol_read_names_the_symbol_in_the_reference() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("auth.rs"), "pub fn alpha() {}\n").unwrap();
    let tool = tool_for(dir.path());
    let out = read(&tool, "auth.rs", Some("alpha")).unwrap();
    assert!(out.lines().next().unwrap().ends_with(" :: alpha"));
}

#[test]
fn missing_file_is_not_found_even_if_the_index_lists_it() {
    let dir = tempdir().unwrap();
    let tool = tool_for(dir.path());
    assert!(matches!(
        read(&tool, "absent.rs", None),
        Err(ToolError::NotFound(_))
    ));
}
