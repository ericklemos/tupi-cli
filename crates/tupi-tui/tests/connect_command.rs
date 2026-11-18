use tupi_tui::state::{COMMANDS, ChatState};

#[test]
fn the_connect_command_is_offered_in_autocomplete() {
    assert!(COMMANDS.contains(&"/connect"));

    let mut state = ChatState::default();
    state.input = "/con".into();
    state.refresh_suggestions();

    assert_eq!(state.suggestion_selected(), Some("/connect"));
}

#[test]
fn connect_offers_every_registry_provider_after_the_space() {
    // A lista vem do registry no boot (`crates/tupi-tui/src/run.rs:69`); aqui o
    // teste injeta os ids direto no estado, como o app faz na abertura.
    let mut state = ChatState::default();
    state.providers = vec![
        "deepseek".into(),
        "antigravity".into(),
        "openai-plan".into(),
    ];
    state.input = "/connect ".into();
    state.refresh_suggestions();

    assert_eq!(
        state.suggestions,
        Some(vec![
            "/connect deepseek".into(),
            "/connect antigravity".into(),
            "/connect openai-plan".into(),
        ])
    );
}

#[test]
fn connect_open_filters_and_tab_completes_the_id() {
    let mut state = ChatState::default();
    state.providers = vec!["deepseek".into(), "openai-plan".into()];
    state.input = "/connect open".into();
    state.refresh_suggestions();

    assert_eq!(state.suggestion_selected(), Some("/connect openai-plan"));

    // O laço da TUI mapeia `KeyCode::Tab` para `accept_suggestion`
    // (`crates/tupi-tui/src/run.rs:316`); o teste chama o mesmo método.
    state.accept_suggestion();

    assert_eq!(state.input, "/connect openai-plan");
    assert!(!state.suggestions_visible());
}

#[test]
fn connect_suggestions_stay_silent_for_regular_text() {
    let mut state = ChatState::default();
    state.providers = vec!["openai-plan".into()];

    // Texto comum nunca abre a lista.
    state.input = "oi".into();
    state.refresh_suggestions();
    assert!(!state.suggestions_visible());

    // Sem o espaço não há namespace de ids: `/connect` já está completo.
    state.input = "/connect".into();
    state.refresh_suggestions();
    assert!(!state.suggestions_visible());

    // Prefixo que não casa nenhum id fecha a lista em vez de mostrar tudo.
    state.input = "/connect zzz".into();
    state.refresh_suggestions();
    assert!(!state.suggestions_visible());
}
