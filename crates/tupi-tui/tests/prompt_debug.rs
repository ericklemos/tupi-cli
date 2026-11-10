use tupi_tui::state::{ChatState, COMMANDS};

#[test]
fn the_prompt_debug_command_is_offered_in_autocomplete() {
    assert!(COMMANDS.contains(&"/enable-show-prompts-messages"));

    let mut state = ChatState {
        input: "/enable".into(),
        ..ChatState::default()
    };
    state.refresh_suggestions();

    assert_eq!(
        state.suggestion_selected(),
        Some("/enable-show-prompts-messages")
    );
}

#[test]
fn a_prompt_entry_is_kept_in_the_transcript() {
    let mut state = ChatState::default();
    state.push_prompt("modelo: m\n\n[user]\noi");

    assert_eq!(state.lines.len(), 1);
    assert!(matches!(
        &state.lines[0],
        tupi_tui::state::ChatEntry::Prompt { content } if content.contains("[user]")
    ));
}
