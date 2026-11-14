use super::*;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};
use tupi_kernel::{Tool, ToolError};

use crate::budget::DEFAULT_BUDGET_TOKENS;

const SOURCE: &str =
    "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n\npub fn logout() {\n}\n";

fn tool_for(dir: &Path) -> ReadTool {
    ReadTool::new(
        dir.to_path_buf(),
        Arc::new(ResponseStore::new(
            std::time::Duration::from_secs(60),
            1024 * 1024,
        )),
        Arc::new(DeliveryLedger::new(64, 1 << 20)),
        DEFAULT_BUDGET_TOKENS,
    )
}

fn fixture() -> TempDir {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("auth.rs"), SOURCE).unwrap();
    dir
}

fn read(
    tool: &ReadTool,
    path: &str,
    symbol: Option<&str>,
    budget: Option<usize>,
    repeat: Option<&str>,
) -> Result<String, ToolError> {
    tool.read(ReadParams {
        path: path.to_string(),
        symbol: symbol.map(str::to_string),
        budget_tokens: budget,
        repeat: repeat.map(str::to_string),
        rev: None,
    })
}

#[test]
fn read_describes_the_parameters_the_model_used() {
    let dir = fixture();
    let tool = tool_for(dir.path());

    assert_eq!(
        tool.describe_call(&serde_json::json!({
            "path": "auth.rs",
            "symbol": "verify_token"
        })),
        "path=auth.rs · symbol=verify_token"
    );
    assert_eq!(
        tool.describe_call(&serde_json::json!({
            "path": "auth.rs",
            "budget_tokens": 300,
            "repeat": "allow"
        })),
        "path=auth.rs · budget=300 · repeat=allow"
    );
}

#[test]
fn outline_lists_every_symbol_without_bodies() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let text = read(&tool, "auth.rs", None, None, None).unwrap();

    assert!(text.contains("### `auth.rs`"), "{text}");
    assert!(text.contains("pub fn verify_token"), "{text}");
    assert!(text.contains("pub fn logout"), "{text}");
    assert!(!text.contains("!raw.is_empty()"), "{text}");
}

#[test]
fn asking_for_a_symbol_returns_only_that_body() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let text = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();

    assert!(text.contains("!raw.is_empty()"), "{text}");
    assert!(!text.contains("pub fn logout"), "{text}");
    assert!(text.contains(":1-3`"), "{text}");
}

#[test]
fn unknown_symbol_is_an_explicit_error() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let error = read(&tool, "auth.rs", Some("nope"), None, None).unwrap_err();
    assert!(matches!(error, ToolError::SymbolNotFound(_)));
}

#[test]
fn path_outside_the_root_is_rejected() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let error = read(&tool, "../../etc/passwd", None, None, None).unwrap_err();
    assert!(matches!(error, ToolError::OutsideRoot(_)));
}

#[test]
fn missing_file_is_an_explicit_error() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let error = read(&tool, "absent.rs", None, None, None).unwrap_err();
    assert!(matches!(error, ToolError::NotFound(_)));
}

#[test]
fn non_utf8_file_is_an_explicit_error() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("bin.rs"), [0xff, 0xfe]).unwrap();
    let tool = tool_for(dir.path());
    let error = read(&tool, "bin.rs", None, None, None).unwrap_err();
    assert!(matches!(error, ToolError::Other(_)), "{error:?}");
}

#[test]
fn the_second_read_of_an_unchanged_symbol_is_a_pointer_with_a_live_handle() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    let first = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    assert!(first.contains("!raw.is_empty()"));

    let second = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    assert!(second.contains("Já entregue nesta sessão"), "{second}");
    assert!(!second.contains("!raw.is_empty()"), "{second}");

    let handle = second
        .split("`peek` `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .unwrap();
    assert!(tool.store.get(handle).unwrap().contains("!raw.is_empty()"));
}

#[test]
fn a_changed_symbol_returns_only_the_diff() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();

    fs::write(
            dir.path().join("auth.rs"),
            "pub fn verify_token(raw: &str) -> bool {\n    !raw.trim().is_empty()\n}\n\npub fn logout() {\n}\n",
        )
        .unwrap();

    let second = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    assert!(second.contains("Mudou desde a entrega"), "{second}");
    assert!(second.contains("+    !raw.trim().is_empty()"), "{second}");
    assert!(second.contains("-    !raw.is_empty()"), "{second}");
}

#[test]
fn repeat_allow_returns_the_body_and_registers_it_again() {
    let dir = fixture();
    let tool = tool_for(dir.path());
    read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();

    let again = read(&tool, "auth.rs", Some("verify_token"), None, Some("allow")).unwrap();
    assert!(again.contains("!raw.is_empty()"), "{again}");
    assert!(!again.contains("Já entregue"), "{again}");

    let third = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    assert!(third.contains("Já entregue"), "{third}");
}

#[test]
fn an_oversized_symbol_is_split_and_the_tail_is_recoverable() {
    let dir = tempdir().unwrap();
    let body: Vec<String> = (0..300).map(|i| format!("    let v{i} = {i};")).collect();
    fs::write(
        dir.path().join("long.rs"),
        format!(
            "pub fn long_fn() -> u32 {{\n{}\n    0\n}}\n",
            body.join("\n")
        ),
    )
    .unwrap();
    let tool = tool_for(dir.path());

    let text = read(&tool, "long.rs", Some("long_fn"), Some(300), None).unwrap();
    assert!(text.contains("parte 1/"), "{text}");

    let handle = text
        .split("handle: `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .unwrap();
    let tail = tool.store.get(handle).unwrap();
    assert!(tail.contains("let v299"), "{tail}");
    assert!(tail.contains("parte 2/"), "{tail}");
}

#[test]
fn duplicate_names_return_every_matching_body() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("a.rs"),
        "impl A {\n    fn new() {}\n}\n\nimpl B {\n    fn new() {}\n}\n",
    )
    .unwrap();
    let tool = tool_for(dir.path());

    let text = read(&tool, "a.rs", Some("new"), None, None).unwrap();
    assert!(text.contains("`A::new`"), "{text}");
    assert!(text.contains("`B::new`"), "{text}");
}

#[test]
fn a_qualified_symbol_reads_only_the_matching_container() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("a.rs"),
        "impl A {\n    fn new() -> u8 { 111 }\n}\n\nimpl B {\n    fn new() -> u8 { 222 }\n}\n",
    )
    .unwrap();
    let tool = tool_for(dir.path());

    let text = read(&tool, "a.rs", Some("B::new"), None, None).unwrap();
    assert!(text.contains("222"), "{text}");
    assert!(!text.contains("111"), "{text}");
    assert!(text.contains("B::new"), "{text}");
}

#[test]
fn unsupported_files_fall_back_to_the_whole_content() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("README.md"), "# tupi\n\nleitura\n").unwrap();
    let tool = tool_for(dir.path());

    let text = read(&tool, "README.md", None, None, None).unwrap();
    assert!(text.contains("arquivo inteiro"), "{text}");
    assert!(text.contains("# tupi"), "{text}");
}

#[cfg(unix)]
#[test]
fn a_symlink_that_escapes_the_root_is_rejected() {
    use std::os::unix::fs::symlink;

    let dir = fixture();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("secret.rs"), "pub fn secret() {}\n").unwrap();
    symlink(outside.path().join("secret.rs"), dir.path().join("link.rs")).unwrap();
    let tool = tool_for(dir.path());

    let error = read(&tool, "link.rs", None, None, None).unwrap_err();
    assert!(matches!(error, ToolError::OutsideRoot(_)), "{error:?}");
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_rev_read_comes_from_the_commit_not_the_worktree() {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init"]);
    fs::write(dir.path().join("a.rs"), "fn a() {\n    1\n}\n").unwrap();
    git(dir.path(), &["add", "a.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-m",
            "x",
        ],
    );
    fs::write(dir.path().join("a.rs"), "fn a() {\n    2\n}\n").unwrap();
    let tool = tool_for(dir.path());

    let out = tool
        .read(ReadParams {
            path: "a.rs".to_string(),
            symbol: Some("a".to_string()),
            budget_tokens: None,
            repeat: None,
            rev: Some("HEAD".to_string()),
        })
        .unwrap();
    assert!(out.contains("somente leitura"));
    assert!(out.contains("    1"));
}

#[test]
fn a_rev_read_does_not_mark_the_worktree_as_delivered() {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init"]);
    fs::write(dir.path().join("a.rs"), "fn a() {\n    1\n}\n").unwrap();
    git(dir.path(), &["add", "a.rs"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-m",
            "x",
        ],
    );
    fs::write(dir.path().join("a.rs"), "fn a() {\n    2\n}\n").unwrap();
    let tool = tool_for(dir.path());

    tool.read(ReadParams {
        path: "a.rs".to_string(),
        symbol: Some("a".to_string()),
        budget_tokens: None,
        repeat: None,
        rev: Some("HEAD".to_string()),
    })
    .unwrap();

    let worktree = read(&tool, "a.rs", Some("a"), None, None).unwrap();
    assert!(worktree.contains("    2"));
    assert!(!worktree.contains("Já entregue"));
}

#[test]
fn an_unknown_rev_is_not_found() {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init"]);
    fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    let tool = tool_for(dir.path());
    let error = tool
        .read(ReadParams {
            path: "a.rs".to_string(),
            symbol: None,
            budget_tokens: None,
            repeat: None,
            rev: Some("nao-existe".to_string()),
        })
        .unwrap_err();
    assert!(matches!(error, ToolError::NotFound(_)), "{error:?}");
}

#[test]
fn a_rev_path_outside_the_root_is_rejected() {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init"]);
    let tool = tool_for(dir.path());
    let error = tool
        .read(ReadParams {
            path: "../fora.rs".to_string(),
            symbol: None,
            budget_tokens: None,
            repeat: None,
            rev: Some("HEAD".to_string()),
        })
        .unwrap_err();
    assert!(matches!(error, ToolError::OutsideRoot(_)), "{error:?}");
}

#[test]
fn a_flag_like_rev_is_rejected_as_invalid_arguments() {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init"]);
    fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
    let tool = tool_for(dir.path());
    let error = tool
        .read(ReadParams {
            path: "a.rs".to_string(),
            symbol: None,
            budget_tokens: None,
            repeat: None,
            rev: Some("--output=/tmp/pwned".to_string()),
        })
        .unwrap_err();
    assert!(matches!(error, ToolError::InvalidArguments(_)), "{error:?}");
}

#[test]
fn symbol_read_includes_resolved_graph_neighbors() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("token.rs"),
        "pub fn decode_jwt(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
    )
    .unwrap();
    fs::write(
            dir.path().join("auth.rs"),
            "use crate::token::decode_jwt;\n\npub fn verify_token(raw: &str) -> bool {\n    decode_jwt(raw)\n}\n",
        )
        .unwrap();
    fs::write(
            dir.path().join("api.rs"),
            "use crate::auth::verify_token;\n\npub fn login(raw: &str) -> bool {\n    verify_token(raw)\n}\n",
        )
        .unwrap();

    let graph = crate::graph::GraphCache::shared(dir.path());
    let tool = tool_for(dir.path()).with_graph_cache(graph);

    let text = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    assert!(
        text.contains("> chama: `decode_jwt` (token.rs:1)"),
        "{text}"
    );
    assert!(text.contains("> chamado por: `login` (api.rs:3)"), "{text}");

    let second = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    assert!(second.contains("Já entregue nesta sessão"), "{second}");
    assert!(!second.contains("> chama:"), "{second}");
    assert!(!second.contains("> chamado por:"), "{second}");
}

#[test]
fn a_second_outline_read_of_an_unchanged_file_is_a_pointer_with_backtrack_summary() {
    let dir = fixture();
    let tool = tool_for(dir.path());

    let first = read(&tool, "auth.rs", None, None, None).unwrap();
    assert!(first.contains("pub fn verify_token"), "{first}");
    assert!(first.contains("pub fn logout"), "{first}");

    read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();

    let second = read(&tool, "auth.rs", None, None, None).unwrap();
    assert!(
        second.contains("Esqueleto já entregue nesta sessão"),
        "{second}"
    );
    assert!(second.contains("já abertos: `verify_token`"), "{second}");
    assert!(second.contains("ainda não abertos: `logout`"), "{second}");
    assert!(!second.contains("pub fn logout"), "{second}");

    let repeated = read(&tool, "auth.rs", None, None, Some("allow")).unwrap();
    assert!(repeated.contains("pub fn verify_token"), "{repeated}");
    assert!(repeated.contains("pub fn logout"), "{repeated}");
}

#[test]
fn revisiting_a_symbol_with_graph_neighbors_shows_backtrack_frontier() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("token.rs"),
        "pub fn decode_jwt(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
    )
    .unwrap();
    fs::write(
            dir.path().join("auth.rs"),
            "use crate::token::decode_jwt;\n\npub fn verify_token(raw: &str) -> bool {\n    decode_jwt(raw)\n}\n",
        )
        .unwrap();
    fs::write(
            dir.path().join("api.rs"),
            "use crate::auth::verify_token;\n\npub fn login(raw: &str) -> bool {\n    verify_token(raw)\n}\n",
        )
        .unwrap();

    let graph = crate::graph::GraphCache::shared(dir.path());
    let tool = tool_for(dir.path()).with_graph_cache(graph);

    read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    read(&tool, "token.rs", Some("decode_jwt"), None, None).unwrap();

    let revisit = read(&tool, "auth.rs", Some("verify_token"), None, None).unwrap();
    assert!(revisit.contains("Já entregue nesta sessão"), "{revisit}");
    assert!(revisit.contains("> **Backtrack**:"), "{revisit}");
    assert!(revisit.contains("já abertos:"), "{revisit}");
    assert!(revisit.contains("decode_jwt"), "{revisit}");
    assert!(revisit.contains("fronteira ainda não aberta:"), "{revisit}");
    assert!(revisit.contains("login"), "{revisit}");
    assert!(!revisit.contains("> chama:"), "{revisit}");
    assert!(!revisit.contains("> chamado por:"), "{revisit}");
}

#[test]
fn a_line_suffix_selects_one_of_two_identical_impls() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("cache.rs"),
        "impl Cache {\n    fn first(&self) {}\n}\n\nimpl Cache {\n    fn second(&self) {}\n}\n",
    )
    .unwrap();
    let tool = tool_for(dir.path());

    let text = read(&tool, "cache.rs", Some("impl Cache@5"), None, None).unwrap();

    assert!(text.contains("fn second"), "{text}");
    assert!(!text.contains("fn first"), "{text}");
}

#[test]
fn a_budget_truncated_symbol_is_a_pointer_on_the_next_read() {
    let dir = tempdir().unwrap();
    let mut source = String::from("pub fn big() {\n");
    for index in 0..120 {
        source.push_str(&format!("    let v{index} = {index};\n"));
    }
    source.push_str("}\n");
    fs::write(dir.path().join("big.rs"), source).unwrap();
    let tool = tool_for(dir.path());

    let first = read(&tool, "big.rs", Some("big"), Some(60), None).unwrap();
    assert!(first.contains("Resposta truncada"), "{first}");

    let second = read(&tool, "big.rs", Some("big"), None, None).unwrap();
    assert!(second.contains("Já entregue nesta sessão"), "{second}");
    assert!(!second.contains("let v0"), "{second}");
}
