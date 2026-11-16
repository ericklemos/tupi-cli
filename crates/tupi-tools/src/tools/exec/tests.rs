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
    let sections = sections_for(&args, &raw, CacheNote::default());
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
    let sections = sections_for(&args, &raw, CacheNote::default());
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
    assert!(fingerprint(dir.path(), 10).is_ok());
    assert_eq!(
        fingerprint(dir.path(), 3).unwrap_err(),
        super::render::CacheOff::Files(3)
    );
}

#[test]
fn fingerprint_follows_content_when_length_and_mtime_are_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn a() {}").unwrap();
    let before = fingerprint(dir.path(), 10).unwrap();
    let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();

    std::fs::write(&path, "fn b() {}").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();

    assert_eq!(std::fs::metadata(&path).unwrap().len(), 9);
    assert_ne!(fingerprint(dir.path(), 10).unwrap(), before);
}

#[test]
fn fingerprint_covers_hidden_build_configuration() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".cargo")).unwrap();
    let config = dir.path().join(".cargo/config.toml");
    std::fs::write(&config, "[build]\nrustflags = [\"--cfg\", \"a\"]\n").unwrap();
    let before = fingerprint(dir.path(), 10).unwrap();

    std::fs::write(&config, "[build]\nrustflags = [\"--cfg\", \"b\"]\n").unwrap();

    assert_ne!(fingerprint(dir.path(), 10).unwrap(), before);
}

#[test]
fn fingerprint_covers_fixtures_outside_the_source_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("case.json");
    std::fs::write(&fixture, "{\"ok\":true}").unwrap();
    let before = fingerprint(dir.path(), 10).unwrap();

    std::fs::write(&fixture, "{\"ok\":false}").unwrap();

    assert_ne!(fingerprint(dir.path(), 10).unwrap(), before);
}

#[test]
fn a_cache_hit_reports_its_age_instead_of_the_original_runtime() {
    let raw = Output {
        stdout: String::new(),
        stderr: String::new(),
        success: true,
        timed_out: false,
        duration: Duration::from_millis(3_400),
    };
    let args = build_args(&params(ExecKind::Check)).unwrap();
    let note = CacheNote {
        age: Some(Duration::from_secs(125)),
        key_cost: Some(Duration::from_millis(5)),
        off: None,
    };
    let sections = sections_for(&args, &raw, note);

    let body = &sections[0].body;
    assert!(body.contains("em cache há 2m5s"), "{body}");
    assert!(body.contains("execução original: 3.4s"), "{body}");
    assert!(body.contains("cache: hit · chave 5.0ms"), "{body}");
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
    let sections = sections_for(&args, &raw, CacheNote::default());
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

#[test]
fn fingerprint_covers_files_the_gitignore_hides() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".gitignore"), "*.json\n").unwrap();
    let fixture = dir.path().join("case.json");
    std::fs::write(&fixture, "{\"ok\":true}").unwrap();
    let before = fingerprint(dir.path(), 10).unwrap();

    std::fs::write(&fixture, "{\"ok\":false}").unwrap();

    assert_ne!(fingerprint(dir.path(), 10).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn fingerprint_follows_a_directory_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = outside.path().join("lib.rs");
    std::fs::write(&source, "fn a() {}").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("vendored")).unwrap();
    let before = fingerprint(dir.path(), 10).unwrap();

    std::fs::write(&source, "fn b() {}").unwrap();

    assert_ne!(fingerprint(dir.path(), 10).unwrap(), before);
}

#[test]
fn fingerprint_ignores_the_agents_own_bookkeeping() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();
    let state = dir.path().join(".agents").join("tupi");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("facts.jsonl"), "{\"id\":1}\n").unwrap();
    let before = fingerprint(dir.path(), 10).unwrap();

    std::fs::write(state.join("facts.jsonl"), "{\"id\":2}\n").unwrap();

    assert_eq!(fingerprint(dir.path(), 10).unwrap(), before);
}

#[test]
fn fingerprint_changes_when_the_build_environment_changes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn a() {}").unwrap();

    let plain = super::render::environment_block_from(
        std::env::vars().filter(|(name, _)| name != "CARGO_NET_OFFLINE"),
        "rustc 1.90.0",
        &[],
    );
    let offline = super::render::environment_block_from(
        std::iter::once(("CARGO_NET_OFFLINE".to_string(), "1".to_string())),
        "rustc 1.90.0",
        &[],
    );
    let noisy = super::render::environment_block_from(
        std::iter::once(("RUST_LOG".to_string(), "debug".to_string())),
        "rustc 1.90.0",
        &[],
    );

    // `CARGO_NET_OFFLINE` muda o que o cargo faz; `RUST_LOG` muda só o log.
    assert_ne!(offline, plain);
    assert_eq!(noisy, super::render::environment_block_from(
        std::iter::empty(),
        "rustc 1.90.0",
        &[],
    ));
}
