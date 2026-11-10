pub const COMMANDS: &[&str] = &["/models"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Streaming,
    Busy,
}

#[derive(Debug)]
pub struct Overlay {
    pub title: String,
    pub items: Vec<String>,
    pub selected: usize,
}

#[derive(Debug, Default)]
pub struct ChatState {
    pub lines: Vec<(tupi_domain::Role, String)>,
    pub input: String,
    pub status: Status,
    pub scroll: u16,
    pub notice: Option<String>,
    pub overlay: Option<Overlay>,
    pub suggestions: Option<Vec<String>>,
    pub suggestion_index: usize,
    streaming_index: Option<usize>,
}

impl ChatState {
    pub fn push_user(&mut self, text: &str) {
        self.lines.push((tupi_domain::Role::User, text.to_string()));
        self.notice = None;
    }

    pub fn begin_assistant(&mut self) {
        self.streaming_index = None;
        self.status = Status::Streaming;
        self.notice = None;
    }

    pub fn append_token(&mut self, token: &str) {
        let index = match self.streaming_index {
            Some(index) => index,
            None => {
                self.lines
                    .push((tupi_domain::Role::Assistant, String::new()));
                let index = self.lines.len() - 1;
                self.streaming_index = Some(index);
                index
            }
        };

        if let Some((_, line)) = self.lines.get_mut(index) {
            line.push_str(token);
        }
    }

    pub fn processing(&self) -> bool {
        self.status == Status::Streaming && self.streaming_index.is_none()
    }

    pub fn finish_assistant(&mut self) {
        self.streaming_index = None;
        self.status = Status::Idle;
    }

    pub fn fail(&mut self, message: &str) {
        self.streaming_index = None;
        self.status = Status::Idle;
        self.notice = Some(message.to_string());
    }

    pub fn busy(&mut self) {
        self.status = Status::Busy;
        self.notice = Some("aguarde o fim do stream".to_string());
    }

    pub fn open_overlay(&mut self, title: impl Into<String>, items: Vec<String>) {
        self.overlay = Some(Overlay {
            title: title.into(),
            items,
            selected: 0,
        });
    }

    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    pub fn overlay_open(&self) -> bool {
        self.overlay.is_some()
    }

    pub fn overlay_next(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            if !overlay.items.is_empty() {
                overlay.selected = (overlay.selected + 1) % overlay.items.len();
            }
        }
    }

    pub fn overlay_prev(&mut self) {
        if let Some(overlay) = &mut self.overlay {
            if !overlay.items.is_empty() {
                overlay.selected = overlay
                    .selected
                    .checked_sub(1)
                    .unwrap_or(overlay.items.len() - 1);
            }
        }
    }

    pub fn selected_item(&self) -> Option<&str> {
        self.overlay
            .as_ref()
            .and_then(|overlay| overlay.items.get(overlay.selected))
            .map(String::as_str)
    }

    pub fn refresh_suggestions(&mut self) {
        let input = self.input.trim_start();
        let eligible = input.starts_with('/') && !input.contains(' ');

        let items: Vec<String> = if eligible {
            COMMANDS
                .iter()
                .filter(|command| command.starts_with(input) && **command != input)
                .map(|command| (*command).to_string())
                .collect()
        } else {
            Vec::new()
        };

        if items.is_empty() {
            self.suggestions = None;
            self.suggestion_index = 0;
            return;
        }

        if self.suggestion_index >= items.len() {
            self.suggestion_index = 0;
        }
        self.suggestions = Some(items);
    }

    pub fn suggestions_visible(&self) -> bool {
        self.suggestions
            .as_ref()
            .is_some_and(|suggestions| !suggestions.is_empty())
    }

    pub fn suggestion_selected(&self) -> Option<&str> {
        self.suggestions
            .as_ref()
            .and_then(|suggestions| suggestions.get(self.suggestion_index))
            .map(String::as_str)
    }

    pub fn suggestion_next(&mut self) {
        if let Some(suggestions) = &self.suggestions {
            if !suggestions.is_empty() {
                self.suggestion_index = (self.suggestion_index + 1) % suggestions.len();
            }
        }
    }

    pub fn suggestion_prev(&mut self) {
        if let Some(suggestions) = &self.suggestions {
            if !suggestions.is_empty() {
                self.suggestion_index = self
                    .suggestion_index
                    .checked_sub(1)
                    .unwrap_or(suggestions.len() - 1);
            }
        }
    }

    pub fn accept_suggestion(&mut self) {
        if let Some(selected) = self.suggestion_selected().map(str::to_string) {
            self.input = selected;
        }
        self.refresh_suggestions();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_accumulate_and_finish_sets_idle() {
        let mut state = ChatState::default();

        state.begin_assistant();
        state.append_token("olá");
        state.append_token(", mundo");

        assert_eq!(state.lines.last().unwrap().1, "olá, mundo");

        state.finish_assistant();
        assert_eq!(state.status, Status::Idle);
    }

    #[test]
    fn assistant_bubble_waits_for_the_first_token() {
        let mut state = ChatState::default();

        state.begin_assistant();

        assert!(state.lines.is_empty());
        assert!(state.processing());

        state.append_token("oi");

        assert_eq!(state.lines.len(), 1);
        assert_eq!(state.lines[0].1, "oi");
        assert!(!state.processing());
    }

    #[test]
    fn fail_keeps_partial_text_and_returns_to_idle() {
        let mut state = ChatState::default();

        state.begin_assistant();
        state.append_token("parcial");
        state.fail("conexão caiu");

        assert_eq!(state.status, Status::Idle);
        assert_eq!(state.lines.last().unwrap().1, "parcial");
        assert_eq!(state.notice.as_deref(), Some("conexão caiu"));
    }

    #[test]
    fn overlay_navigation_wraps() {
        let mut state = ChatState::default();
        state.open_overlay("modelos", vec!["a".into(), "b".into()]);

        assert_eq!(state.selected_item(), Some("a"));

        state.overlay_next();
        assert_eq!(state.selected_item(), Some("b"));

        state.overlay_next();
        assert_eq!(state.selected_item(), Some("a"));

        state.overlay_prev();
        assert_eq!(state.selected_item(), Some("b"));
    }

    #[test]
    fn close_overlay_clears_selection() {
        let mut state = ChatState::default();
        state.open_overlay("modelos", vec!["a".into()]);
        state.close_overlay();

        assert!(!state.overlay_open());
        assert_eq!(state.selected_item(), None);
    }

    #[test]
    fn command_autocomplete_filters_by_prefix() {
        let mut state = ChatState {
            input: "/m".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();

        assert!(state.suggestions_visible());
        assert_eq!(state.suggestion_selected(), Some("/models"));
    }

    #[test]
    fn accept_suggestion_completes_input_and_hides_list() {
        let mut state = ChatState {
            input: "/m".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();

        state.accept_suggestion();

        assert_eq!(state.input, "/models");
        assert!(!state.suggestions_visible());
    }

    #[test]
    fn suggestions_hide_for_regular_text() {
        let mut state = ChatState {
            input: "oi".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();

        assert!(!state.suggestions_visible());
    }
}
