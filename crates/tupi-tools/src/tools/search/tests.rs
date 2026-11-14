use super::*;
use std::fs;
use tempfile::{TempDir, tempdir};

use crate::budget::{DEFAULT_BUDGET_TOKENS, PEEK_TOOL_NAME};

fn write(dir: &Path, relative: &str, content: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn tool_for(dir: &Path) -> SearchTool {
    SearchTool::new(
        dir.to_path_buf(),
        Arc::new(ResponseStore::new(
            std::time::Duration::from_secs(60),
            1024 * 1024,
        )),
        Arc::new(DeliveryLedger::new(64, 1 << 20)),
        DEFAULT_BUDGET_TOKENS,
    )
}

fn params(query: &str) -> SearchParams {
    SearchParams {
        query: query.to_string(),
        keywords: Vec::new(),
        path: None,
        max_results: None,
        detail: None,
        budget_tokens: None,
        repeat: None,
    }
}

fn auth_fixture() -> TempDir {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        "src/auth.rs",
        "pub fn verify_token(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
    );
    write(dir.path(), "src/other.rs", "pub fn logout() {}\n");
    dir
}

#[test]
fn a_natural_phrase_finds_the_symbol_and_returns_the_body() {
    let dir = auth_fixture();
    let out = tool_for(dir.path())
        .search(params("onde o token é validado"))
        .unwrap();

    assert!(out.contains("verify_token"), "{out}");
    assert!(out.contains("!raw.is_empty()"), "{out}");
    assert!(out.contains("Todas as"), "{out}");
    assert!(out.contains("lexical"), "{out}");
}

#[test]
fn keywords_are_optional_and_boost_the_name() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "pub fn verify_token_helper() {}\n");
    write(
        dir.path(),
        "b.rs",
        "pub fn other() {\n    let verify_token = 1;\n}\n",
    );
    let tool = tool_for(dir.path());
    let mut request = params("algo");
    request.keywords = vec!["verify_token".to_string()];

    let out = tool.search(request).unwrap();
    let name = out.find("verify_token_helper").unwrap();
    let body = out.find("fn other").unwrap_or(usize::MAX);

    assert!(name < body, "{out}");
}

#[test]
fn an_exact_name_beats_a_body_occurrence() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "pub fn other() {\n    token();\n}\n");
    write(dir.path(), "z.rs", "pub fn token() {}\n");
    let out = tool_for(dir.path()).search(params("token")).unwrap();

    let name = out.find("`token`").unwrap();
    let other = out.find("other").unwrap();
    assert!(name < other, "{out}");
}

#[test]
fn auto_sends_two_bodies_and_signatures_for_the_rest() {
    let dir = tempdir().unwrap();
    for name in ["handler_a", "handler_b", "handler_c"] {
        write(
            dir.path(),
            &format!("{name}.rs"),
            &format!("pub fn {name}() -> u8 {{\n    1\n}}\n"),
        );
    }
    let out = tool_for(dir.path()).search(params("handler")).unwrap();

    assert_eq!(out.matches("```rust").count(), 2, "{out}");
    assert!(out.contains("1 só com assinatura"), "{out}");
}

#[test]
fn signatures_detail_has_no_code_fence() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "pub fn handler() {}\n");
    let mut request = params("handler");
    request.detail = Some(SearchDetail::Signatures);
    let out = tool_for(dir.path()).search(request).unwrap();

    assert!(!out.contains("```rust"), "{out}");
    assert!(out.contains("fn handler"), "{out}");
}

#[test]
fn full_detail_sends_a_body_for_every_result() {
    let dir = tempdir().unwrap();
    for name in ["handler_a", "handler_b", "handler_c"] {
        write(
            dir.path(),
            &format!("{name}.rs"),
            &format!("pub fn {name}() -> u8 {{\n    1\n}}\n"),
        );
    }
    let mut request = params("handler");
    request.detail = Some(SearchDetail::Full);
    let out = tool_for(dir.path()).search(request).unwrap();

    assert_eq!(out.matches("```rust").count(), 3, "{out}");
}

#[test]
fn a_max_results_cut_is_declared() {
    let dir = tempdir().unwrap();
    for name in ["handler_a", "handler_b", "handler_c", "handler_d"] {
        write(
            dir.path(),
            &format!("{name}.rs"),
            &format!("pub fn {name}() {{}}\n"),
        );
    }
    let mut request = params("handler");
    request.max_results = Some(1);
    let out = tool_for(dir.path()).search(request).unwrap();

    assert!(out.contains("max_results=1 cortou o resto"), "{out}");
}

#[test]
fn a_path_prefix_restricts_the_search() {
    let dir = auth_fixture();
    write(dir.path(), "top.rs", "pub fn verify_top() {}\n");
    let mut request = params("verify");
    request.path = Some("src".to_string());
    let out = tool_for(dir.path()).search(request).unwrap();

    assert!(!out.contains("top.rs"), "{out}");
    assert!(out.contains("auth.rs"), "{out}");
}

#[test]
fn empty_or_stopword_only_queries_are_rejected() {
    let dir = auth_fixture();
    let error = tool_for(dir.path())
        .search(params("onde o é a"))
        .unwrap_err();
    assert!(matches!(error, ToolError::InvalidArguments(_)));
}

#[test]
fn no_match_is_an_explicit_answer() {
    let dir = auth_fixture();
    let out = tool_for(dir.path()).search(params("zzznothing")).unwrap();
    assert!(out.contains("Nenhum resultado"), "{out}");
}

#[test]
fn a_long_hit_is_capped_and_points_to_read() {
    let dir = tempdir().unwrap();
    let body: Vec<String> = (0..120).map(|i| format!("    let v{i} = {i};")).collect();
    write(
        dir.path(),
        "long.rs",
        &format!(
            "pub fn long_handler() -> u32 {{\n{}\n    0\n}}\n",
            body.join("\n")
        ),
    );
    let mut request = params("long_handler");
    request.detail = Some(SearchDetail::Full);
    let out = tool_for(dir.path()).search(request).unwrap();

    assert!(out.contains("peça o corpo inteiro com read"), "{out}");
    assert!(!out.contains("let v119"), "{out}");
}

#[test]
fn a_small_budget_truncates_and_peek_recovers() {
    let dir = tempdir().unwrap();
    let body = |name: &str| {
        format!(
            "pub fn {name}() -> u8 {{\n{}\n    1\n}}\n",
            "    let _ = 1;\n".repeat(80)
        )
    };
    write(dir.path(), "one.rs", &body("handler_one"));
    write(dir.path(), "two.rs", &body("handler_two"));
    let tool = tool_for(dir.path());
    let mut request = params("handler");
    request.detail = Some(SearchDetail::Full);
    request.budget_tokens = Some(120);

    let out = tool.search(request).unwrap();

    assert!(out.contains("Resposta truncada"), "{out}");
    assert!(out.contains(PEEK_TOOL_NAME), "{out}");
    let handle = out
        .split("handle: `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .unwrap();
    assert!(tool.store.get(handle).is_some());
}

#[test]
fn the_second_search_of_the_same_symbol_is_a_pointer() {
    let dir = auth_fixture();
    let tool = tool_for(dir.path());
    let first = tool.search(params("verify_token")).unwrap();
    assert!(first.contains("!raw.is_empty()"), "{first}");

    let second = tool.search(params("verify_token")).unwrap();
    assert!(second.contains("Já entregue nesta sessão"), "{second}");
    assert!(!second.contains("!raw.is_empty()"), "{second}");

    let handle = second
        .split("`peek` `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .unwrap();
    assert!(tool.store.get(handle).is_some());
}

#[test]
fn a_changed_file_turns_the_hit_into_a_diff() {
    let dir = auth_fixture();
    let tool = tool_for(dir.path());
    tool.search(params("verify_token")).unwrap();

    write(
        dir.path(),
        "src/auth.rs",
        "pub fn verify_token(raw: &str) -> bool {\n    !raw.trim().is_empty()\n}\n",
    );
    let second = tool.search(params("verify_token")).unwrap();

    assert!(second.contains("Mudou desde a entrega"), "{second}");
    assert!(second.contains("+    !raw.trim().is_empty()"), "{second}");
}

#[test]
fn repeat_allow_returns_the_body_again() {
    let dir = auth_fixture();
    let tool = tool_for(dir.path());
    tool.search(params("verify_token")).unwrap();

    let mut request = params("verify_token");
    request.repeat = Some("allow".to_string());
    let again = tool.search(request).unwrap();

    assert!(again.contains("!raw.is_empty()"), "{again}");
    assert!(!again.contains("Já entregue"), "{again}");
}

#[test]
fn a_read_delivery_makes_the_search_a_pointer() {
    use crate::read::{ReadParams, ReadTool};

    let dir = auth_fixture();
    let store = Arc::new(ResponseStore::new(
        std::time::Duration::from_secs(60),
        1024 * 1024,
    ));
    let ledger = Arc::new(DeliveryLedger::new(64, 1 << 20));
    ReadTool::new(
        dir.path().to_path_buf(),
        store.clone(),
        ledger.clone(),
        DEFAULT_BUDGET_TOKENS,
    )
    .read(ReadParams {
        path: "src/auth.rs".to_string(),
        symbol: Some("verify_token".to_string()),
        budget_tokens: None,
        repeat: None,
        rev: None,
    })
    .unwrap();

    let search = SearchTool::new(
        dir.path().to_path_buf(),
        store,
        ledger,
        DEFAULT_BUDGET_TOKENS,
    );
    let out = search.search(params("verify_token")).unwrap();

    assert!(out.contains("Já entregue nesta sessão"), "{out}");
}

#[test]
fn a_path_outside_the_root_is_rejected() {
    let dir = auth_fixture();
    let mut request = params("token");
    request.path = Some("../fora".to_string());
    let error = tool_for(dir.path()).search(request).unwrap_err();
    assert!(matches!(error, ToolError::OutsideRoot(_)));
}

#[test]
fn describe_shows_the_query_and_filters() {
    let dir = auth_fixture();
    let tool = tool_for(dir.path());
    let label = tool.describe_call(&serde_json::json!({
        "query": "onde valida token",
        "path": "src",
        "detail": "signatures"
    }));

    assert_eq!(
        label,
        "query=onde valida token · path=src · detail=signatures"
    );
}

#[test]
fn search_deprioritizes_already_delivered_symbols_and_gives_auto_body_to_fresh_hits() {
    let dir = tempdir().unwrap();
    for name in ["handler_a", "handler_b", "handler_c"] {
        write(
            dir.path(),
            &format!("{name}.rs"),
            &format!("pub fn {name}() -> u8 {{\n    99\n}}\n"),
        );
    }
    let tool = tool_for(dir.path());
    let first = tool.search(params("handler")).unwrap();
    assert!(first.contains("1. `handler_a`"), "{first}");
    assert!(first.contains("2. `handler_b`"), "{first}");

    let second = tool.search(params("handler")).unwrap();
    assert!(second.contains("1. `handler_c`"), "{second}");
    assert!(second.contains("99"), "{second}");
    assert!(second.contains("Já entregue nesta sessão"), "{second}");
}

#[test]
fn search_marks_previously_seen_signatures_and_repeated_exhausted_queries() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "pub fn alpha_one() -> u8 {\n    1\n}\n");
    let tool = tool_for(dir.path());

    let first = tool
        .search(SearchParams {
            detail: Some(SearchDetail::Signatures),
            ..params("alpha")
        })
        .unwrap();
    assert!(!first.contains("assinatura já vista"), "{first}");

    let second = tool
        .search(SearchParams {
            detail: Some(SearchDetail::Signatures),
            ..params("alpha")
        })
        .unwrap();
    assert!(second.contains("assinatura já vista"), "{second}");
    assert!(second.contains("> **Backtrack**:"), "{second}");
}

#[test]
fn read_outline_marks_symbols_as_signature_seen_for_subsequent_search() {
    use crate::read::{ReadParams, ReadTool};

    let dir = tempdir().unwrap();
    write(dir.path(), "a.rs", "pub fn alpha_one() -> u8 {\n    1\n}\n");
    write(dir.path(), "b.rs", "pub fn alpha_two() -> u8 {\n    2\n}\n");

    let store = Arc::new(ResponseStore::new(
        std::time::Duration::from_secs(60),
        1024 * 1024,
    ));
    let ledger = Arc::new(DeliveryLedger::new(64, 1 << 20));
    let backtrack = Arc::new(crate::backtrack::BacktrackGraph::default());

    let read = ReadTool::new(
        dir.path().to_path_buf(),
        store.clone(),
        ledger.clone(),
        DEFAULT_BUDGET_TOKENS,
    )
    .with_backtrack(backtrack.clone());
    read.read(ReadParams {
        path: "a.rs".to_string(),
        symbol: None,
        budget_tokens: None,
        repeat: None,
        rev: None,
    })
    .unwrap();

    let search = SearchTool::new(
        dir.path().to_path_buf(),
        store,
        ledger,
        DEFAULT_BUDGET_TOKENS,
    )
    .with_backtrack(backtrack);

    let out = search.search(params("alpha")).unwrap();
    assert!(out.contains("1. `alpha_two`"), "{out}");
    assert!(out.contains("2. `alpha_one`"), "{out}");
}
