use super::diagnostics::diagnostics;
use super::*;

fn params(kind: ExecKind) -> ExecParams {
    ExecParams {
        kind,
        package: None,
        target: None,
        filter: None,
        exact: false,
        task: None,
        refresh: false,
        timeout_secs: None,
        budget_tokens: None,
    }
}

#[test]
fn a_check_is_workspace_wide_by_default() {
    assert_eq!(
        build_args(&params(ExecKind::Check)).unwrap(),
        vec!["check", "--workspace"]
    );
}

#[test]
fn a_package_replaces_the_workspace_flag() {
    let mut params = params(ExecKind::Check);
    params.package = Some("tupi-tools".to_string());
    assert_eq!(
        build_args(&params).unwrap(),
        vec!["check", "-p", "tupi-tools"]
    );
}

#[test]
fn a_test_filter_goes_after_the_separator() {
    let mut params = params(ExecKind::Test);
    params.filter = Some("guard".to_string());
    params.exact = true;
    assert_eq!(
        build_args(&params).unwrap(),
        vec!["test", "--workspace", "--", "guard", "--exact"]
    );
}

#[test]
fn exact_without_filter_is_rejected() {
    let mut params = params(ExecKind::Test);
    params.exact = true;
    assert!(build_args(&params).is_err());
}

#[test]
fn a_target_name_is_only_valid_for_tests() {
    let mut params = params(ExecKind::Check);
    params.target = Some("auth".to_string());
    assert!(build_args(&params).is_err());
}

#[test]
fn xtask_requires_a_task() {
    assert!(build_args(&params(ExecKind::Xtask)).is_err());
    let mut params = params(ExecKind::Xtask);
    params.task = Some("docs".to_string());
    assert_eq!(build_args(&params).unwrap(), vec!["xtask", "docs"]);
}

#[test]
fn diagnostics_are_grouped() {
    let stderr = "error[E0308]: mismatched types\n --> src/a.rs:1:1\n  |\n1 | x\n  | ^ expected u8\n\nerror: could not compile x\n\nwarning: unused variable: y\n --> src/a.rs:2:1\n";
    let diag = diagnostics(stderr);
    assert_eq!(diag.errors.len(), 2);
    assert_eq!(diag.warnings.len(), 1);
    assert!(diag.errors[0].contains("mismatched types"));
}

#[test]
fn a_successful_output_has_no_detail_section() {
    let raw = Output {
        stdout: String::new(),
        stderr: String::new(),
        success: true,
        timed_out: false,
        duration: Duration::from_millis(10),
    };
    let args = build_args(&params(ExecKind::Check)).unwrap();
    let sections = sections_for(&args, &raw, false);
    assert_eq!(sections.len(), 1);
    assert!(sections[0].body.contains("ok"));
}

#[test]
fn a_failure_carries_the_detail() {
    let raw = Output {
        stdout: String::new(),
        stderr: "error[E0308]: mismatched types\n".to_string(),
        success: false,
        timed_out: false,
        duration: Duration::from_millis(10),
    };
    let args = build_args(&params(ExecKind::Check)).unwrap();
    let sections = sections_for(&args, &raw, false);
    assert_eq!(sections.len(), 1);
    assert!(sections[0].body.contains("mismatched types"));
}

#[test]
fn describe_shows_the_cargo_command() {
    let tool = ExecTool::new(
        PathBuf::from("."),
        Arc::new(ResponseStore::new(Duration::from_secs(60), 1024)),
        4000,
    );
    assert_eq!(
        tool.describe_call(&serde_json::json!({"kind": "test", "filter": "x"})),
        "cargo test --workspace -- x"
    );
}

#[test]
fn fingerprint_refuses_to_cache_when_file_count_exceeds_limit() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..5 {
        std::fs::write(dir.path().join(format!("f{index}.rs")), "fn a() {}").unwrap();
    }
    assert!(fingerprint(dir.path(), 10).is_some());
    assert!(fingerprint(dir.path(), 3).is_none());
}

#[test]
fn every_compiler_error_reaches_the_model() {
    let mut stderr = String::new();
    for index in 0..40 {
        stderr.push_str(&format!(
            "error[E{index:04}]: falha {index}\n --> src/a.rs:1:1\n\n"
        ));
    }
    let raw = Output {
        stdout: String::new(),
        stderr,
        success: false,
        timed_out: false,
        duration: Duration::from_millis(1),
    };
    let args = build_args(&params(ExecKind::Check)).unwrap();
    let sections = sections_for(&args, &raw, false);
    let important: String = sections
        .iter()
        .filter(|section| section.priority >= 200)
        .map(|section| section.body.as_str())
        .collect();

    for index in 0..40 {
        let code = format!("E{index:04}");
        assert!(
            important.contains(&code),
            "erro {code} não chegou ao modelo"
        );
    }
}
