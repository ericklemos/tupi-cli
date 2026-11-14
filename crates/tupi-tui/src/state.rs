pub use tupi_domain::WORKFLOWS_PREFIX;
pub const COMMANDS: &[&str] = &[
    "/models",
    "/connect",
    "/enable-show-prompts-messages",
    WORKFLOWS_PREFIX,
];

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
    pub ids: Vec<String>,
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
    Prompt {
        content: String,
    },
    Queued {
        content: String,
    },
    /// Linha discreta de memória: recall injetado ou fato gravado.
    Memory {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsage {
    pub tokens: u64,
    pub budget: u64,
}

#[derive(Debug, Default)]
pub struct ChatState {
    pub lines: Vec<ChatEntry>,
    pub input: String,
    pub status: Status,
    pub mode: tupi_kernel::AgentMode,
    pub scroll: u16,
    pub notice: Option<String>,
    pub overlay: Option<Overlay>,
    pub suggestions: Option<Vec<String>>,
    pub suggestion_index: usize,
    pub selection: Option<crate::selection::Selection>,
    pub(crate) view: std::cell::Cell<Option<crate::selection::TranscriptView>>,
    pub(crate) render_cache: std::cell::RefCell<crate::markdown::Cache>,
    pub context: Option<ContextUsage>,
    /// Nomes dos workflows disponíveis. Só viram sugestão depois de
    /// `/workflows/`; antes disso o namespace não existe.
    pub workflows: Vec<String>,
    pending_memory: Vec<String>,
    streaming_index: Option<usize>,
    streaming_metrics: Option<crate::metrics::ReplyMetrics>,
}

impl ChatState {
    pub fn apply_mode_changed(&mut self, mode: tupi_kernel::AgentMode) {
        // O modo muda por atalho; fechar o SDD é decisão do planejamento
        // (tool `sdd(action="close")`), anunciada pelo app via Event::Notice.
        // Aqui só espelhamos o modo e descartamos aviso preso de outra troca.
        self.mode = mode;
        self.notice = None;
    }

    pub fn push_user(&mut self, text: &str) {
        self.lines.push(ChatEntry::Message {
            role: tupi_domain::Role::User,
            content: text.to_string(),
            metrics: None,
        });
        self.notice = None;
    }

    /// Registra o pacote enviado ao provedor quando o debug está ligado.
    pub fn push_prompt(&mut self, content: &str) {
        self.lines.push(ChatEntry::Prompt {
            content: content.to_string(),
        });
        self.notice = None;
    }

    pub fn begin_assistant(&mut self) {
        self.streaming_index = None;
        for text in self.pending_memory.drain(..) {
            self.lines.push(ChatEntry::Memory { text });
        }
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

    pub fn push_memory(&mut self, text: &str, after: bool) {
        if after {
            self.lines.push(ChatEntry::Memory {
                text: text.to_string(),
            });
            return;
        }
        if self.streaming_index.is_none() && self.status != Status::Streaming {
            self.pending_memory.push(text.to_string());
            return;
        }
        let at = self.streaming_index.unwrap_or(self.lines.len());
        self.lines.insert(
            at,
            ChatEntry::Memory {
                text: text.to_string(),
            },
        );
        if let Some(index) = self.streaming_index.as_mut() {
            *index += 1;
        }
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

    /// Enfileira uma mensagem para o próximo turno, deixando-a visível no
    /// transcript enquanto espera.
    pub fn enqueue(&mut self, text: &str) {
        self.lines.push(ChatEntry::Queued {
            content: text.to_string(),
        });
        self.notice = None;
    }

    /// Remove e devolve a próxima mensagem pendente, na ordem de envio.
    pub fn dequeue(&mut self) -> Option<String> {
        let index = self
            .lines
            .iter()
            .position(|entry| matches!(entry, ChatEntry::Queued { .. }))?;
        match self.lines.remove(index) {
            ChatEntry::Queued { content } => Some(content),
            _ => unreachable!("o índice veio de uma entrada pendente"),
        }
    }

    /// Tempo decorrido do turno em aberto, mesmo antes do primeiro token.
    ///
    /// As métricas migram para o balão a cada tool, então o fallback olha o
    /// último balão cuja resposta ainda não fechou.
    pub fn turn_elapsed(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        if self.status != Status::Streaming {
            return None;
        }

        if let Some(metrics) = self.streaming_metrics.as_ref() {
            return Some(metrics.elapsed(now));
        }

        self.lines.iter().rev().find_map(|entry| match entry {
            ChatEntry::Message {
                metrics: Some(metrics),
                ..
            } if metrics.finished.is_none() => Some(metrics.elapsed(now)),
            _ => None,
        })
    }

    /// Espera a anunciar na linha "processando…": só antes do primeiro token.
    pub fn waiting(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        if !self.processing() {
            return None;
        }

        self.turn_elapsed(now)
    }

    pub fn set_usage(&mut self, prompt: Option<u64>, cached: Option<u64>, completion: Option<u64>) {
        if let Some(index) = self.streaming_index
            && let Some(ChatEntry::Message {
                metrics: Some(metrics),
                ..
            }) = self.lines.get_mut(index)
        {
            metrics.prompt_tokens = prompt;
            metrics.cached_tokens = cached;
            if let Some(tokens) = completion {
                metrics.set_remote(tokens);
            }
            return;
        }

        if let Some(metrics) = self.streaming_metrics.as_mut() {
            metrics.prompt_tokens = prompt;
            metrics.cached_tokens = cached;
            if let Some(tokens) = completion {
                metrics.set_remote(tokens);
            }
        }
    }

    pub fn set_context(&mut self, tokens: u64, budget: u64) {
        self.context = Some(ContextUsage { tokens, budget });
    }

    /// Troca só o teto de contexto, preservando a ocupação já conhecida.
    ///
    /// Sem `ContextUsage` anterior não há o que mostrar: a troca de modelo
    /// fora de um turno não inventa ocupação.
    pub fn set_context_budget(&mut self, budget: u64) {
        if let Some(context) = self.context.as_mut() {
            context.budget = budget;
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
            ids: Vec::new(),
            selected: 0,
        });
    }

    pub fn open_overlay_with_ids(
        &mut self,
        title: impl Into<String>,
        items: Vec<String>,
        ids: Vec<String>,
    ) {
        self.overlay = Some(Overlay {
            title: title.into(),
            items,
            ids,
            selected: 0,
        });
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.overlay
            .as_ref()
            .and_then(|overlay| overlay.ids.get(overlay.selected))
            .map(String::as_str)
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

        // O namespace dos workflows só abre depois da barra final: até lá,
        // `/w` completa para `/workflows/` e nada mais. Antes disso nenhum nome
        // de workflow polui a lista de comandos.
        let items: Vec<String> = if let Some(prefix) = input.strip_prefix(WORKFLOWS_PREFIX) {
            self.workflows
                .iter()
                .map(|name| format!("{WORKFLOWS_PREFIX}{name}"))
                .filter(|command| {
                    command[WORKFLOWS_PREFIX.len()..].starts_with(prefix)
                        && command.as_str() != input
                })
                .collect()
        } else if eligible {
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
    fn content(entry: &ChatEntry) -> &str {
        match entry {
            ChatEntry::Message { content, .. } => content,
            ChatEntry::Queued { content } => content,
            ChatEntry::Prompt { .. } => panic!("esperava mensagem"),
            ChatEntry::Tool { .. } => panic!("esperava mensagem"),
            ChatEntry::Memory { .. } => panic!("esperava mensagem"),
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
    fn workflow_names_only_appear_after_the_trailing_slash() {
        let mut state = ChatState {
            workflows: vec!["feature".into(), "fix-bug".into()],
            input: "/workflows/".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();
        assert_eq!(state.suggestion_selected(), Some("/workflows/feature"));

        state.input = "/workflows/fix".into();
        state.refresh_suggestions();
        assert_eq!(state.suggestion_selected(), Some("/workflows/fix-bug"));

        // Um prefixo que não casa nenhum nome fecha a lista.
        state.input = "/workflows/zzz".into();
        state.refresh_suggestions();
        assert!(!state.suggestions_visible());
    }

    #[test]
    fn no_workflow_name_shows_up_before_the_barra() {
        for input in ["/", "/w", "/workflow", "/workflows"] {
            let mut state = ChatState {
                workflows: vec!["feature".into()],
                input: input.into(),
                ..ChatState::default()
            };
            state.refresh_suggestions();
            let listed = state.suggestions.clone().unwrap_or_default();
            assert!(
                listed.iter().all(|command| !command.contains("feature")),
                "{input} listou um workflow: {listed:?}"
            );
        }
    }

    #[test]
    fn tab_completes_the_namespace_then_the_name() {
        let mut state = ChatState {
            workflows: vec!["review".into()],
            input: "/work".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();
        state.accept_suggestion();
        assert_eq!(state.input, "/workflows/");

        state.refresh_suggestions();
        state.accept_suggestion();
        assert_eq!(state.input, "/workflows/review");
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
        // Estimador compartilhado (tupi_domain::tokens): 1 peça para "olá",
        // 1 para a vírgula e 2 para "mundo".
        assert_eq!(metrics.estimated_tokens, 4);
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
        state.set_usage(Some(120), Some(60), Some(12));

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

    #[test]
    fn memory_lines_land_before_the_streaming_answer() {
        let mut state = ChatState::default();
        state.push_user("oi");
        state.begin_assistant();
        state.append_token("resposta");

        state.push_memory("memória · 1 fato", false);
        assert!(matches!(state.lines[1], ChatEntry::Memory { .. }));
        assert!(matches!(state.lines[2], ChatEntry::Message { .. }));

        state.push_memory("memória · gravado", true);
        assert!(matches!(state.lines.last(), Some(ChatEntry::Memory { .. })));
    }

    #[test]
    fn queued_messages_wait_in_order_and_leave_the_transcript() {
        let mut state = ChatState::default();
        state.push_user("primeira");
        state.begin_assistant();
        state.append_token("ok");
        state.finish_assistant();

        state.enqueue("segunda");
        state.enqueue("terceira");
        assert!(matches!(
            state.lines.last(),
            Some(ChatEntry::Queued { content }) if content == "terceira"
        ));

        assert_eq!(state.dequeue().as_deref(), Some("segunda"));
        assert_eq!(state.dequeue().as_deref(), Some("terceira"));
        assert_eq!(state.dequeue(), None);
        assert_eq!(
            state.lines.len(),
            2,
            "pendentes saem do transcript ao disparar"
        );
    }

    #[test]
    fn set_context_replaces_previous_occupancy() {
        let mut state = ChatState::default();
        assert_eq!(state.context, None);

        state.set_context(1_001, 32_000);
        assert_eq!(
            state.context,
            Some(ContextUsage {
                tokens: 1_001,
                budget: 32_000,
            })
        );

        state.set_context(1_201, 32_000);
        assert_eq!(
            state.context,
            Some(ContextUsage {
                tokens: 1_201,
                budget: 32_000,
            })
        );
    }

    #[test]
    fn context_budget_updates_the_ceiling_without_inventing_occupancy() {
        let mut state = ChatState::default();
        state.set_context_budget(8_000);
        assert_eq!(state.context, None);

        state.set_context(1_001, 32_000);
        state.set_context_budget(8_000);
        assert_eq!(
            state.context,
            Some(ContextUsage {
                tokens: 1_001,
                budget: 8_000,
            })
        );
    }

    #[test]
    fn mode_changed_event_updates_chat_state_without_clearing_transcript() {
        let mut state = ChatState::default();
        assert_eq!(state.mode, tupi_kernel::AgentMode::Execution);
        state.push_user("oi");
        assert_eq!(state.lines.len(), 1);

        state.apply_mode_changed(tupi_kernel::AgentMode::Sdd);
        assert_eq!(state.mode, tupi_kernel::AgentMode::Sdd);
        assert_eq!(state.lines.len(), 1);

        // O atalho alterna o modo; quem fecha o SDD é o app, no planejamento,
        // quando a tool `sdd(action="close")` roda. Trocar de modo não pode
        // anunciar um fechamento que não aconteceu.
        state.apply_mode_changed(tupi_kernel::AgentMode::Execution);
        assert_eq!(state.mode, tupi_kernel::AgentMode::Execution);
        assert_eq!(state.lines.len(), 1);
        assert!(state.notice.is_none(), "troca manual de modo não fecha o SDD");

        // Nem um aviso de fechamento real pode sobreviver à volta para o SDD.
        state.notice = Some("SDD fechado — modo execução".to_string());
        state.apply_mode_changed(tupi_kernel::AgentMode::Sdd);
        assert_eq!(state.mode, tupi_kernel::AgentMode::Sdd);
        assert!(state.notice.is_none(), "aviso de fechamento ficou preso no modo SDD");
    }
}
