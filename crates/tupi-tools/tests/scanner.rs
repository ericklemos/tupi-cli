use tupi_tools::scanner::LexicalScanner;

#[test]
fn search_is_case_insensitive_and_finds_kotlin() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.kt"), "fun Verify_Token() {}\n").unwrap();
    std::fs::write(dir.path().join("b.rs"), "pub fn verify_token() {}\n").unwrap();

    let scanner = LexicalScanner::new(dir.path().to_path_buf());
    let hits = scanner.search(&["verify_token".to_string()], None).unwrap();
    let names: Vec<String> = hits
        .iter()
        .map(|hit| {
            hit.file_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(names.contains(&"a.kt".to_string()));
    assert!(names.contains(&"b.rs".to_string()));
    assert!(
        hits.iter()
            .all(|hit| hit.byte_offset > 0 || hit.line_number == 1)
    );
}
