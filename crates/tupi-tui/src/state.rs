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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEntry {
    Message {
        role: tupi_domain::Role,
        content: String,
        metrics: Option<crate::metrics::ReplyMetrics>,
    },
    Tool {
        call_id: String,
        name: String,
        summary: String,
        ok: Option<bool>,
    },
}

#[derive(Debug, Default)]
pub struct ChatState {
    pub lines: Vec<ChatEntry>,
    pub input: String,
    pub status: Status,
    pub scroll: u16,
    pub notice: Option<String>,
    pub overlay: Option<Overlay>,
    pub suggestions: Option<Vec<String>>,
    pub suggestion_index: usize,
    pub selection: Option<crate::selection::Selection>,
    pub(crate) view: std::cell::Cell<Option<TranscriptView>>,
    pub(crate) render_cache: std::cell::RefCell<crate::markdown::Cache>,
    streaming_index: Option<usize>,
    streaming_metrics: Option<crate::metrics::ReplyMetrics>,
}

impl ChatState {
    pub fn push_user(&mut self, text: &str) {
        self.lines.push(ChatEntry::Message {
            role: tupi_domain::Role::User,
            content: text.to_string(),
            metrics: None,
        });
        self.notice = None;
    }

    pub fn begin_assistant(&mut self) {
        self.streaming_index = None;
        self.streaming_metrics = Some(crate::metrics::ReplyMetrics::start(
            std::time::Instant::now(),
        ));
        self.status = Status::Streaming;
        self.notice = None;
    }

    pub fn append_token(&mut self, token: &str) {
        let index = match self.streaming_index {
            Some(index) => index,
            None => {
                self.lines.push(ChatEntry::Message {
                    role: tupi_domain::Role::Assistant,
                    content: String::new(),
                    metrics: self.streaming_metrics.take(),
                });
                let index = self.lines.len() - 1;
                self.streaming_index = Some(index);
                index
            }
        };

        if let Some(ChatEntry::Message {
            content, metrics, ..
        }) = self.lines.get_mut(index)
        {
            content.push_str(token);
            if let Some(metrics) = metrics {
                metrics.mark_first_token(std::time::Instant::now());
                metrics.add_estimated(crate::metrics::estimate_tokens(token));
            }
        }
    }

    /// Uma tool começa: a bolha em stream termina e a chamada vira uma linha.
    /// `summary` já vem descrito pela tool (ex.: `path=a.rs · symbol=new`).
    pub fn push_tool_started(&mut self, call_id: &str, name: &str, summary: &str) {
        // Um turno com tools tem vários balões: as métricas migram para o
        // balão final, e a janela da taxa inclui o tempo de execução das tools.
        if let Some(index) = self.streaming_index
            && let Some(ChatEntry::Message { metrics, .. }) = self.lines.get_mut(index)
        {
            self.streaming_metrics = metrics.take();
        }

        self.streaming_index = None;
        self.lines.push(ChatEntry::Tool {
            call_id: call_id.to_string(),
            name: name.to_string(),
            summary: compact_summary(summary),
            ok: None,
        });
    }

    pub fn finish_tool(&mut self, call_id: &str, ok: bool) {
        if let Some(ChatEntry::Tool { ok: slot, .. }) = self
            .lines
            .iter_mut()
            .rev()
            .find(|entry| matches!(entry, ChatEntry::Tool { call_id: id, .. } if id == call_id))
        {
            *slot = Some(ok);
        }
    }

    pub fn processing(&self) -> bool {
        self.status == Status::Streaming && self.streaming_index.is_none()
    }

    pub fn set_usage(&mut self, completion_tokens: u64) {
        if let Some(index) = self.streaming_index
            && let Some(ChatEntry::Message {
                metrics: Some(metrics),
                ..
            }) = self.lines.get_mut(index)
        {
            metrics.set_remote(completion_tokens);
            return;
        }

        if let Some(metrics) = self.streaming_metrics.as_mut() {
            metrics.set_remote(completion_tokens);
        }
    }

    fn finish_metrics(&mut self, now: std::time::Instant) {
        if let Some(index) = self.streaming_index
            && let Some(ChatEntry::Message {
                metrics: Some(metrics),
                ..
            }) = self.lines.get_mut(index)
        {
            metrics.finish(now);
        }

        if let Some(metrics) = self.streaming_metrics.as_mut() {
            metrics.finish(now);
        }
    }

    pub fn finish_assistant(&mut self) {
        self.finish_metrics(std::time::Instant::now());
        self.streaming_index = None;
        self.status = Status::Idle;
    }

    pub fn fail(&mut self, message: &str) {
        self.finish_metrics(std::time::Instant::now());
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
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if !overlay.items.is_empty() {
            overlay.selected = (overlay.selected + 1) % overlay.items.len();
        }
    }

    pub fn overlay_prev(&mut self) {
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if !overlay.items.is_empty() {
            overlay.selected = overlay
                .selected
                .checked_sub(1)
                .unwrap_or(overlay.items.len() - 1);
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
        let Some(suggestions) = &self.suggestions else {
            return;
        };
        if !suggestions.is_empty() {
            self.suggestion_index = (self.suggestion_index + 1) % suggestions.len();
        }
    }

    pub fn suggestion_prev(&mut self) {
        let Some(suggestions) = &self.suggestions else {
            return;
        };
        if !suggestions.is_empty() {
            self.suggestion_index = self
                .suggestion_index
                .checked_sub(1)
                .unwrap_or(suggestions.len() - 1);
        }
    }

    pub fn accept_suggestion(&mut self) {
        if let Some(selected) = self.suggestion_selected().map(str::to_string) {
            self.input = selected;
        }
        self.refresh_suggestions();
    }

    pub fn hovered_entry(&self) -> Option<usize> {
        self.hovered.get()
    }

    pub fn hovered_text(&self) -> Option<String> {
        self.hovered
            .get()
            .and_then(|index| self.lines.get(index))
            .map(entry_text)
    }
}

fn entry_text(entry: &ChatEntry) -> String {
    match entry {
        ChatEntry::Message { content, .. } => content.clone(),
        ChatEntry::Tool { name, summary, .. } => format!("{name} · {summary}"),
    }
}

fn compact_summary(summary: &str) -> String {
    let flat = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(60).collect();
    if flat.chars().count() > 60 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_domain::Role;

    fn content(entry: &ChatEntry) -> &str {
        match entry {
            ChatEntry::Message { content, .. } => content,
            ChatEntry::Tool { .. } => panic!("esperava mensagem"),
        }
    }

    #[test]
    fn tokens_accumulate_and_finish_sets_idle() {
        let mut state = ChatState::default();

        state.begin_assistant();
        state.append_token("olá");
        state.append_token(", mundo");

        assert_eq!(content(state.lines.last().unwrap()), "olá, mundo");

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
        assert_eq!(content(&state.lines[0]), "oi");
        assert!(!state.processing());
    }

    #[test]
    fn fail_keeps_partial_text_and_returns_to_idle() {
        let mut state = ChatState::default();

        state.begin_assistant();
        state.append_token("parcial");
        state.fail("conexão caiu");

        assert_eq!(state.status, Status::Idle);
        assert_eq!(content(state.lines.last().unwrap()), "parcial");
        assert_eq!(state.notice.as_deref(), Some("conexão caiu"));
    }

    #[test]
    fn tool_start_closes_the_streaming_bubble_and_adds_a_line() {
        let mut state = ChatState::default();
        state.begin_assistant();
        state.append_token("vou ler");
        state.push_tool_started("c1", "read", "path=a.rs · symbol=new");

        assert_eq!(state.lines.len(), 2);
        match &state.lines[1] {
            ChatEntry::Tool {
                call_id,
                name,
                summary,
                ok,
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(name, "read");
                assert_eq!(summary, "path=a.rs · symbol=new");
                assert_eq!(*ok, None);
            }
            other => panic!("esperava tool, veio {other:?}"),
        }

        state.append_token("pronto");
        assert_eq!(state.lines.len(), 3);
        assert_eq!(content(state.lines.last().unwrap()), "pronto");
    }

    #[test]
    fn finish_tool_updates_only_the_matching_call() {
        let mut state = ChatState::default();
        state.push_tool_started("c1", "read", "path=a.rs");
        state.push_tool_started("c2", "peek", "handle=h_1");

        state.finish_tool("c1", true);

        match &state.lines[0] {
            ChatEntry::Tool { ok, summary, .. } => {
                assert_eq!(*ok, Some(true));
                assert_eq!(summary, "path=a.rs");
            }
            other => panic!("esperava tool, veio {other:?}"),
        }
        assert!(matches!(&state.lines[1], ChatEntry::Tool { ok: None, .. }));
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
    #[test]
    fn assistant_metrics_track_first_token_and_estimate() {
        let mut state = ChatState::default();
        state.begin_assistant();
        state.append_token("olá, mundo");
        state.finish_assistant();

        let ChatEntry::Message {
            metrics: Some(metrics),
            ..
        } = &state.lines[0]
        else {
            panic!("esperava métricas");
        };
        assert_eq!(metrics.estimated_tokens, 3);
        assert!(metrics.first_token.is_some());
        assert!(metrics.finished.is_some());
    }

    #[test]
    fn metrics_follow_the_answer_across_tool_steps() {
        let mut state = ChatState::default();
        state.begin_assistant();
        state.append_token("vou ler");
        state.push_tool_started("c1", "read", "path=a.rs");
        state.append_token("pronto");

        let ChatEntry::Message { metrics: None, .. } = &state.lines[0] else {
            panic!("o balão intermediário não deve ficar com métricas");
        };
        let ChatEntry::Message {
            metrics: Some(metrics),
            ..
        } = &state.lines[2]
        else {
            panic!("a resposta final deve herdar as métricas");
        };
        assert_eq!(
            metrics.estimated_tokens,
            crate::metrics::estimate_tokens("vou ler") + crate::metrics::estimate_tokens("pronto")
        );
    }

    #[test]
    fn set_usage_records_remote_tokens() {
        let mut state = ChatState::default();
        state.begin_assistant();
        state.append_token("oi");
        state.set_usage(12);

        let ChatEntry::Message {
            metrics: Some(metrics),
            ..
        } = &state.lines[0]
        else {
            panic!("esperava métricas");
        };
        assert_eq!(metrics.remote_tokens, Some(12));
    }

    #[test]
    fn fail_closes_metrics() {
        let mut state = ChatState::default();
        state.begin_assistant();
        state.append_token("parcial");
        state.fail("caiu");

        let ChatEntry::Message {
            metrics: Some(metrics),
            ..
        } = &state.lines[0]
        else {
            panic!("esperava métricas");
        };
        assert!(metrics.finished.is_some());
    }

    fn message(role: Role, text: &str) -> ChatEntry {
        ChatEntry::Message {
            role,
            content: text.to_string(),
            metrics: None,
        }
    }

    #[test]
    fn entry_text_keeps_markdown_verbatim() {
        let entry = message(Role::Assistant, "**forte** e `código`");
        assert_eq!(entry_text(&entry), "**forte** e `código`");
    }

    #[test]
    fn entry_text_formats_tool_as_name_and_summary() {
        let entry = ChatEntry::Tool {
            call_id: "c1".into(),
            name: "read".into(),
            summary: "src/lib.rs".into(),
            ok: Some(true),
        };
        assert_eq!(entry_text(&entry), "read · src/lib.rs");
    }

    #[test]
    fn hovered_text_is_none_without_hover() {
        let state = ChatState::default();
        assert_eq!(state.hovered_entry(), None);
        assert_eq!(state.hovered_text(), None);
    }

    #[test]
    fn hovered_text_returns_the_hovered_entry() {
        let mut state = ChatState::default();
        state.lines.push(message(Role::Assistant, "resposta"));
        state.hovered.set(Some(0));
        assert_eq!(state.hovered_entry(), Some(0));
        assert_eq!(state.hovered_text().as_deref(), Some("resposta"));
    }

    #[test]
    fn hovered_entry_out_of_range_is_ignored() {
        let state = ChatState::default();
        state.hovered.set(Some(3));
        assert_eq!(state.hovered_text(), None);
    }
}
