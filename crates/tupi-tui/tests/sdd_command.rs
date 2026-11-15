use tupi_tui::sdd::parse_sdd_invocation;
use tupi_tui::state::{COMMANDS, ChatState};

#[test]
fn the_sdd_command_is_offered_in_autocomplete() {
    assert!(COMMANDS.contains(&"/sdd"));

    let mut state = ChatState::default();
    state.input = "/sd".into();
    state.refresh_suggestions();

    assert_eq!(state.suggestion_selected(), Some("/sdd"));
}

#[test]
fn sdd_with_a_slug_is_recognized_as_the_resume_command() {
    assert_eq!(parse_sdd_invocation("/sdd"), Some(None));
    assert_eq!(parse_sdd_invocation("  /sdd  "), Some(None));
    assert_eq!(parse_sdd_invocation("/sdd auth"), Some(Some("auth")));
    assert_eq!(parse_sdd_invocation("/sdd  auth  "), Some(Some("auth")));
    // O Tab deixa um espaço final; texto extra na mesma linha não vira slug.
    assert_eq!(
        parse_sdd_invocation("/sdd auth continue"),
        Some(Some("auth"))
    );
    assert_eq!(parse_sdd_invocation("/sddx"), None);
    assert_eq!(parse_sdd_invocation("/models"), None);
}
