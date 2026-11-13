use tupi_tui::state::{COMMANDS, ChatState};

#[test]
fn the_connect_command_is_offered_in_autocomplete() {
    assert!(COMMANDS.contains(&"/connect"));

    let mut state = ChatState::default();
    state.input = "/con".into();
    state.refresh_suggestions();

    assert_eq!(state.suggestion_selected(), Some("/connect"));
}
