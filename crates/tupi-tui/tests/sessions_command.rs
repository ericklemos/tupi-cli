use tupi_tui::state::{COMMANDS, ChatState};

#[test]
fn the_sessions_command_is_offered_in_autocomplete() {
    assert!(COMMANDS.contains(&"/sessions"));

    let mut state = ChatState::default();
    state.input = "/ses".into();
    state.refresh_suggestions();

    assert_eq!(state.suggestion_selected(), Some("/sessions"));
}
