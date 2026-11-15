use tupi_tools::sdd::{SddPlan, SddPlanStatus, render_markdown, write_mirror};

#[test]
fn write_mirror_writes_the_same_markdown_as_the_tool() {
    let dir = tempfile::tempdir().unwrap();
    let plan = SddPlan {
        slug: "auth".into(),
        repo_key: "repo".into(),
        title: "Auth".into(),
        status: SddPlanStatus::Draft,
        goal: "OAuth".into(),
        constraints: vec![],
        nuances: vec![],
        tasks: vec![],
        created_at: 1,
        updated_at: 1,
        closed_at: None,
        ..Default::default()
    };

    let path = write_mirror(dir.path(), &plan).unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        render_markdown(&plan)
    );
}
