pub use tupi_domain::WORKFLOWS_PREFIX;

use crate::sdd::SDD_COMMAND;
/// Comando cujo argumento é um id de provedor; o namespace abre depois do espaço.
pub const CONNECT_COMMAND: &str = "/connect";
pub const COMMANDS: &[&str] = &[
    "/dashboard",
    "/models",
    "/sessions",
    "/connect",
    "/enable-show-prompts-messages",
    SDD_COMMAND,
    WORKFLOWS_PREFIX,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Streaming,
    Busy,
}

/// Qual lista o overlay está mostrando; as teclas mudam por tipo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    Generic,
    Models,
    Memory,
    Sessions,
}

#[derive(Debug)]
pub struct Overlay {
    pub title: String,
    pub items: Vec<String>,
    pub ids: Vec<String>,
    pub selected: usize,
    pub kind: OverlayKind,
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
        /// Argumentos da invocação, como chegaram em `Event::ToolStarted`.
        /// É a matéria-prima do detalhe expansível: o resumo compacta o que a
        /// tool achou importante, os argumentos mostram o pedido inteiro.
        arguments: String,
        /// Corpo do resultado, já recortado por quem publica. `None` enquanto
        /// a tool roda (ou quando não houve detalhe): a TUI nunca inventa
        /// conteúdo no render nem vai buscar o corpo por conta própria.
        detail: Option<String>,
        ok: Option<bool>,
        /// Quando a tool começou. Nasce aqui, no estado, e não no render:
        /// um `Instant` criado por frame reiniciaria o cronômetro a cada
        /// desenho e o tempo exibido seria sempre zero.
        started: std::time::Instant,
        /// Duração medida por quem executou a tool, entregue em
        /// `Event::ToolFinished`. `None` enquanto roda ou se não veio.
        duration: Option<std::time::Duration>,
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
    /// Slugs dos planos SDD retomáveis, usados depois de `/sdd `.
    pub sdd_specs: Vec<String>,
    /// Ids dos provedores registrados no registry, usados depois de
    /// `/connect `. Vem do app no boot: lista fixa aqui inventaria provedor
    /// sem credencial nem modelo.
    pub providers: Vec<String>,
    /// Sessão marcada para apagar com o segundo `d`, se houver.
    pub pending_delete: Option<String>,
    /// Índices de `ChatEntry::Tool` com o detalhe aberto. Só estado de UI:
    /// nada daqui volta para o bus nem para o histórico enviado ao provedor,
    /// e `/new` esvazia o conjunto junto com o resto do transcript.
    pub expanded: std::collections::BTreeSet<usize>,
    /// Dono de cada linha do transcript, publicado junto com `view` pelo
    /// último desenho. Compartilhado por `Rc` para o clique não copiar o mapa
    /// a cada frame, e lido só no evento de mouse: o layout nunca consulta
    /// este campo, então expandir não custa leitura de disco nem de store.
    pub(crate) owners: std::cell::RefCell<std::rc::Rc<Vec<Option<usize>>>>,
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
        pub fn push_tool_started(
        &mut self,
        call_id: &str,
        name: &str,
        summary: &str,
        arguments: &str,
    ) {
        // Um turno com tools tem vários balões: as métricas migram para o
        // balão final, e a janela da taxa inclui o tempo de execução das tools.
        if let Some(index) = self.streaming_index
            && let Some(ChatEntry::Message { metrics, .. }) = self.lines.get_mut(index)
        {
            self.streaming_metrics = metrics.take();
        }

        self.streaming_index = None;
        let entry = ChatEntry::Tool {
            call_id: call_id.to_string(),
            name: name.to_string(),
            summary: compact_summary(summary),
            arguments: arguments.to_string(),
            detail: None,
            ok: None,
            started: std::time::Instant::now(),
            duration: None,
        };

        // Sub-chamada (id com `#`) entra no grupo do pai, não no fim da lista:
        // com subagentes paralelos, os eventos chegam intercalados e o grupo
        // precisa continuar contíguo.
        let mut at = self.lines.len();
        if let Some((parent, _)) = call_id.rsplit_once('#')
            && let Some(index) = self.lines.iter().position(
                |entry| matches!(entry, ChatEntry::Tool { call_id: id, .. } if id == parent),
            )
        {
            at = index + 1;
            while at < self.lines.len() {
                let same_parent = match &self.lines[at] {
                    ChatEntry::Tool { call_id: id, .. } => {
                        id.rsplit_once('#').map(|(id_parent, _)| id_parent) == Some(parent)
                    }
                    _ => false,
                };
                if !same_parent {
                    break;
                }
                at += 1;
            }
        }
        self.lines.insert(at, entry);
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

        pub fn finish_tool(
        &mut self,
        call_id: &str,
        ok: bool,
        detail: Option<String>,
        elapsed: Option<std::time::Duration>,
    ) {
        if let Some(ChatEntry::Tool {
            ok: slot,
            detail: detail_slot,
            duration,
            ..
        }) = self
            .lines
            .iter_mut()
            .rev()
            .find(|entry| matches!(entry, ChatEntry::Tool { call_id: id, .. } if id == call_id))
        {
            *slot = Some(ok);
            // O detalhe do resultado vem de quem executou a tool e chega uma
            // vez só: nada de streaming nem de rebuscar conteúdo no render.
            *detail_slot = detail;
            // A duração vem de quem executou a tool e é opcional: um evento
            // sem medição não vira tempo inventado na tela.
            *duration = elapsed;
        }
    }

    /// Quantas tools do turno corrente ainda não reportaram resultado.
    ///
    /// É o gatilho do redesenho periódico: sem tool em voo não há o que
    /// animar, e a TUI volta a bloquear só em teclado e eventos.
    pub fn tools_in_flight(&self) -> usize {
        self.lines
            .iter()
            .filter(|entry| matches!(entry, ChatEntry::Tool { ok: None, .. }))
            .count()
    }

    /// Total e falhas das tools do turno corrente.
    ///
    /// "Turno" são as `ChatEntry::Tool` consecutivas no fim do transcript: o
    /// que o usuário lê como um lote de trabalho. Sub-chamadas de pipeline
    /// (id com `#`) não contam, porque o pai `pipeline` já as representa.
    pub fn turn_tool_stats(&self) -> (usize, usize) {
        let mut total = 0;
        let mut failures = 0;
        for entry in self.lines.iter().rev() {
            let ChatEntry::Tool { call_id, ok, .. } = entry else {
                break;
            };
            if call_id.contains('#') {
                continue;
            }
            total += 1;
            if *ok == Some(false) {
                failures += 1;
            }
        }
        (total, failures)
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
        self.open_overlay_kind(OverlayKind::Generic, title, items, Vec::new());
    }

    pub fn open_overlay_with_ids(
        &mut self,
        title: impl Into<String>,
        items: Vec<String>,
        ids: Vec<String>,
    ) {
        self.open_overlay_kind(OverlayKind::Generic, title, items, ids);
    }

    pub fn selected_id(&self) -> Option<&str> {
        self.overlay
            .as_ref()
            .and_then(|overlay| overlay.ids.get(overlay.selected))
            .map(String::as_str)
    }

    pub fn open_overlay_kind(
        &mut self,
        kind: OverlayKind,
        title: impl Into<String>,
        items: Vec<String>,
        ids: Vec<String>,
    ) {
        self.overlay = Some(Overlay {
            title: title.into(),
            items,
            ids,
            selected: 0,
            kind,
        });
    }

    pub fn open_sessions(
        &mut self,
        title: impl Into<String>,
        items: Vec<String>,
        ids: Vec<String>,
    ) {
        self.open_overlay_kind(OverlayKind::Sessions, title, items, ids);
    }

    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    pub fn overlay_open(&self) -> bool {
        self.overlay.is_some()
    }

    pub fn overlay_next(&mut self) {
        self.cancel_pending_delete();
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if !overlay.items.is_empty() {
            overlay.selected = (overlay.selected + 1) % overlay.items.len();
        }
    }

    pub fn overlay_prev(&mut self) {
        self.cancel_pending_delete();
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

    pub fn sessions_delete_pressed(&mut self) -> Option<String> {
        let id = self.selected_id()?.to_string();
        if self.pending_delete.as_deref() == Some(id.as_str()) {
            self.pending_delete = None;
            Some(id)
        } else {
            self.pending_delete = Some(id);
            None
        }
    }

    pub fn cancel_pending_delete(&mut self) {
        self.pending_delete = None;
    }

    pub fn load_transcript(&mut self, entries: Vec<tupi_kernel::TranscriptEntry>) {
        self.reset_for_new_session();
        for entry in entries {
            match entry {
                tupi_kernel::TranscriptEntry::User { text } => self.push_user(&text),
                tupi_kernel::TranscriptEntry::Assistant { text } => {
                    self.begin_assistant();
                    self.append_token(&text);
                    self.finish_assistant();
                }
                tupi_kernel::TranscriptEntry::Tool {
                    call_id,
                    name,
                    summary,
                    ok,
                    duration_us,
                } => {
                    // O transcript salvo guarda só a linha fechada: sem
                    // argumentos e sem o corpo do resultado, a sessão retomada
                    // reconstrói o cabeçalho e nada expande.
                    self.push_tool_started(&call_id, &name, &summary, "");
                    self.finish_tool(
                        &call_id,
                        ok,
                        None,
                        Some(std::time::Duration::from_micros(duration_us)),
                    );
                }
                tupi_kernel::TranscriptEntry::Notice { text } => self.push_memory(&text, true),
            }
        }
        self.scroll = 0;
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
        } else if let Some(prefix) = input
            .strip_prefix(SDD_COMMAND)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            // `/sdd ` abre o namespace dos planos retomáveis. O item completo
            // inclui o espaço inicial para o Tab só precisar acrescentar o fim.
            self.sdd_specs
                .iter()
                .filter(|slug| slug.starts_with(prefix))
                .map(|slug| format!("{SDD_COMMAND} {slug}"))
                .filter(|command| command.as_str() != input)
                .collect()
        } else if let Some(prefix) = input
            .strip_prefix(CONNECT_COMMAND)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            // `/connect ` abre o catálogo de provedores conhecidos. O item
            // completo não ganha espaço final: com espaço depois do id o
            // prefixo não casa mais nada e a lista fecha em vez de reabrir.
            self.providers
                .iter()
                .filter(|id| id.starts_with(prefix))
                .map(|id| format!("{CONNECT_COMMAND} {id}"))
                .filter(|command| command.as_str() != input)
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
            // `/sdd <slug>` precisa do espaço final: sem ele, Enter no slug
            // nunca chega ao turno seguinte.
            self.input = if selected.starts_with(&format!("{SDD_COMMAND} ")) {
                format!("{selected} ")
            } else {
                selected
            };
        }
        self.refresh_suggestions();
    }

    /// Alterna o detalhe da entrada `index` e devolve `true` quando o bloco
    /// ficou aberto. Não valida o tipo da entrada: quem chama resolve o índice
    /// a partir de um `ChatEntry::Tool` (`hit_test`/seleção), e um índice de
    /// outra variante apenas não tem detalhe para desenhar.
    pub fn toggle_expanded(&mut self, index: usize) -> bool {
        if self.expanded.remove(&index) {
            false
        } else {
            self.expanded.insert(index);
            true
        }
    }

    pub fn is_expanded(&self, index: usize) -> bool {
        self.expanded.contains(&index)
    }

    /// `/new`: limpa o transcript local preservando o modo e os catálogos que
    /// não vêm do app (workflows e planos SDD retomáveis). O app não publica
    /// `ModeChanged` no `/new`, então o modo local precisa continuar igual.
    pub fn reset_for_new_session(&mut self) {
        let workflows = std::mem::take(&mut self.workflows);
        let sdd_specs = std::mem::take(&mut self.sdd_specs);
        let providers = std::mem::take(&mut self.providers);
        let mode = self.mode;
        *self = ChatState::default();
        // O transcript que guardava os blocos abertos foi descartado: sessão
        // nova começa com todo detalhe fechado.
        self.expanded.clear();
        self.workflows = workflows;
        self.sdd_specs = sdd_specs;
        self.providers = providers;
        self.mode = mode;
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
        state.push_tool_started("c1", "read", "path=a.rs · symbol=new", "");

        assert_eq!(state.lines.len(), 2);
        match &state.lines[1] {
            ChatEntry::Tool {
                call_id,
                name,
                summary,
                ok,
                duration,
                ..
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(name, "read");
                assert_eq!(summary, "path=a.rs · symbol=new");
                assert_eq!(*ok, None);
                // O relógio nasce no estado; a duração só existe quando o
                // executor reporta o fim.
                assert_eq!(*duration, None);
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
        state.push_tool_started("c1", "read", "path=a.rs", "");
        state.push_tool_started("c2", "peek", "handle=h_1", "");

        state.finish_tool(
            "c1",
            true,
            None,
            Some(std::time::Duration::from_millis(250)),
        );

        match &state.lines[0] {
            ChatEntry::Tool {
                ok,
                summary,
                duration,
                ..
            } => {
                assert_eq!(*ok, Some(true));
                assert_eq!(summary, "path=a.rs");
                assert_eq!(*duration, Some(std::time::Duration::from_millis(250)));
            }
            other => panic!("esperava tool, veio {other:?}"),
        }
        assert!(matches!(
            &state.lines[1],
            ChatEntry::Tool {
                ok: None,
                duration: None,
                ..
            }
        ));
        assert_eq!(state.tools_in_flight(), 1);
        assert_eq!(state.turn_tool_stats(), (2, 0));
    }

    #[test]
    fn tool_entry_keeps_the_arguments_and_the_finished_detail() {
        let mut state = ChatState::default();
        state.push_tool_started("c1", "read", "path=a.rs", "{\"path\":\"a.rs\"}");
        state.finish_tool("c1", true, Some("linha 1\nlinha 2".to_string()), None);

        match &state.lines[0] {
            ChatEntry::Tool { arguments, detail, .. } => {
                // Os argumentos chegam prontos em `ToolStarted` e ficam no
                // estado: o render não os busca em lugar nenhum.
                assert_eq!(arguments, "{\"path\":\"a.rs\"}");
                assert_eq!(detail.as_deref(), Some("linha 1\nlinha 2"));
            }
            other => panic!("esperava tool, veio {other:?}"),
        }

        // Sem detalhe publicado, o campo fica vazio: a TUI não inventa corpo
        // no render nem pede o resultado por conta própria.
        state.push_tool_started("c2", "peek", "handle=h_1", "{}");
        state.finish_tool("c2", true, None, None);
        assert!(matches!(
            &state.lines[1],
            ChatEntry::Tool { detail: None, .. }
        ));
    }

    #[test]
    fn tool_stats_count_failures_and_ignore_pipeline_children() {
        let mut state = ChatState::default();
        state.push_tool_started("p1", "pipeline", "2 passos", "");
        state.push_tool_started("p1#0.0", "read", "a.rs", "");
        state.push_tool_started("c2", "exec", "cargo test", "");

        state.finish_tool("p1#0.0", true, None, None);
        state.finish_tool("p1", false, None, None);
        state.finish_tool("c2", true, None, None);

        // A sub-chamada de pipeline não infla o placar do turno.
        assert_eq!(state.turn_tool_stats(), (2, 1));
        assert_eq!(state.tools_in_flight(), 0);
    }

    #[test]
    fn subagent_tools_are_inserted_under_their_own_agent() {
        let mut state = ChatState::default();
        state.push_tool_started("sdd#T1.1", "agent", "T1.1 — A", "");
        state.push_tool_started("sdd#T1.2", "agent", "T1.2 — B", "");
        state.push_tool_started("sdd#T1.1#1.0", "read", "a.rs", "");
        state.push_tool_started("sdd#T1.2#1.0", "read", "b.rs", "");

        // Agentes paralelos intercalam eventos; a linha de cada tool precisa
        // ficar no grupo do seu próprio subagente.
        let ids: Vec<&str> = state
            .lines
            .iter()
            .filter_map(|entry| match entry {
                ChatEntry::Tool { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec!["sdd#T1.1", "sdd#T1.1#1.0", "sdd#T1.2", "sdd#T1.2#1.0"]
        );
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
    fn sdd_slugs_appear_only_after_the_space() {
        let mut state = ChatState {
            sdd_specs: vec!["auth".into(), "tui-tabs".into()],
            input: "/sdd ".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();
        assert_eq!(state.suggestion_selected(), Some("/sdd auth"));
        assert_eq!(
            state.suggestions,
            Some(vec!["/sdd auth".into(), "/sdd tui-tabs".into()])
        );

        state.input = "/sdd tu".into();
        state.refresh_suggestions();
        assert_eq!(state.suggestion_selected(), Some("/sdd tui-tabs"));

        // Um prefixo que não casa nenhum slug fecha a lista.
        state.input = "/sdd zzz".into();
        state.refresh_suggestions();
        assert!(!state.suggestions_visible());
    }

    #[test]
    fn sdd_without_space_still_completes_from_the_command_list() {
        let mut state = ChatState {
            sdd_specs: vec!["auth".into()],
            input: "/sd".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();
        assert_eq!(state.suggestion_selected(), Some("/sdd"));

        // Sem o espaço não há namespace de slugs: nada vaza para a lista.
        state.input = "/sdd".into();
        state.refresh_suggestions();
        assert!(!state.suggestions_visible());
    }

    #[test]
    fn tab_completes_an_sdd_slug_with_a_trailing_space() {
        let mut state = ChatState {
            sdd_specs: vec!["auth".into()],
            input: "/sdd au".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();

        state.accept_suggestion();

        assert_eq!(state.input, "/sdd auth ");
        assert!(!state.suggestions_visible());
    }

    #[test]
    fn connect_ids_appear_only_after_the_space() {
        let mut state = ChatState {
            providers: vec!["deepseek".into(), "antigravity".into(), "openai-plan".into()],
            input: "/connect ".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();
        assert_eq!(
            state.suggestions,
            Some(vec![
                "/connect deepseek".into(),
                "/connect antigravity".into(),
                "/connect openai-plan".into(),
            ])
        );

        // O prefixo do id filtra a lista; um id completo não se repete.
        state.input = "/connect anti".into();
        state.refresh_suggestions();
        assert_eq!(state.suggestion_selected(), Some("/connect antigravity"));

        state.input = "/connect zzz".into();
        state.refresh_suggestions();
        assert!(!state.suggestions_visible());

        // Sem o espaço não há namespace de ids: vale só o comando.
        state.input = "/connect".into();
        state.refresh_suggestions();
        assert!(!state.suggestions_visible());
    }

    #[test]
    fn tab_completes_a_provider_id_and_the_space_does_not_reopen_it() {
        let mut state = ChatState {
            providers: vec!["openai-plan".into()],
            input: "/connect op".into(),
            ..ChatState::default()
        };
        state.refresh_suggestions();

        state.accept_suggestion();

        assert_eq!(state.input, "/connect openai-plan");
        assert!(!state.suggestions_visible());

        // Espaço depois do id não reabre a lista.
        state.input.push(' ');
        state.refresh_suggestions();
        assert!(!state.suggestions_visible());
    }

    #[test]
    fn new_session_preserves_the_command_catalogs() {
        let mut state = ChatState {
            input: "/sdd auth".into(),
            workflows: vec!["feature".into()],
            sdd_specs: vec!["auth".into()],
            providers: vec!["deepseek".into()],
            mode: tupi_kernel::AgentMode::Sdd,
            ..ChatState::default()
        };
        state.push_user("oi");

        state.reset_for_new_session();

        assert!(state.lines.is_empty());
        assert!(state.input.is_empty());
        assert_eq!(state.workflows, vec!["feature"]);
        assert_eq!(state.sdd_specs, vec!["auth"]);
        assert_eq!(state.providers, vec!["deepseek"]);
        assert_eq!(state.mode, tupi_kernel::AgentMode::Sdd);
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
        state.push_tool_started("c1", "read", "path=a.rs", "");
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
        assert!(
            state.notice.is_none(),
            "troca manual de modo não fecha o SDD"
        );

        // Nem um aviso de fechamento real pode sobreviver à volta para o SDD.
        state.notice = Some("SDD fechado — modo execução".to_string());
        state.apply_mode_changed(tupi_kernel::AgentMode::Sdd);
        assert_eq!(state.mode, tupi_kernel::AgentMode::Sdd);
        assert!(
            state.notice.is_none(),
            "aviso de fechamento ficou preso no modo SDD"
        );
    }

    #[test]
    fn sessions_overlay_keeps_the_kind() {
        let mut state = ChatState::default();
        state.open_sessions(" sessões ", vec!["a".into()], vec!["1".into()]);
        assert_eq!(
            state.overlay.as_ref().map(|overlay| overlay.kind),
            Some(OverlayKind::Sessions)
        );

        state.open_overlay(" modelos ", Vec::new());
        assert_eq!(
            state.overlay.as_ref().map(|overlay| overlay.kind),
            Some(OverlayKind::Generic)
        );
    }

    #[test]
    fn delete_needs_two_presses_on_the_same_selection() {
        let mut state = ChatState::default();
        state.open_sessions(
            " sessões ",
            vec!["a".into(), "b".into()],
            vec!["1".into(), "2".into()],
        );
        state.overlay_next();

        assert_eq!(state.sessions_delete_pressed(), None);
        assert_eq!(state.sessions_delete_pressed(), Some("2".into()));
    }

    #[test]
    fn choosing_another_row_cancels_the_pending_delete() {
        let mut state = ChatState::default();
        state.open_sessions(
            " sessões ",
            vec!["a".into(), "b".into()],
            vec!["1".into(), "2".into()],
        );
        state.sessions_delete_pressed();
        state.overlay_next();
        assert_eq!(state.pending_delete, None);
        assert_eq!(state.sessions_delete_pressed(), None);
        assert_eq!(state.pending_delete.as_deref(), Some("2"));
    }

    #[test]
    fn load_transcript_rebuilds_every_entry() {
        let mut state = ChatState::default();
        state.load_transcript(vec![
            tupi_kernel::TranscriptEntry::User { text: "oi".into() },
            tupi_kernel::TranscriptEntry::Assistant {
                text: "olá".into()
            },
            tupi_kernel::TranscriptEntry::Tool {
                call_id: "c1".into(),
                name: "read".into(),
                summary: "src/lib.rs".into(),
                ok: true,
                duration_us: 42,
            },
            tupi_kernel::TranscriptEntry::Notice {
                text: "memória".into(),
            },
        ]);

        assert_eq!(state.lines.len(), 4);
        assert!(matches!(
            state.lines[0],
            ChatEntry::Message {
                role: tupi_domain::Role::User,
                ..
            }
        ));
        assert!(matches!(
            state.lines[1],
            ChatEntry::Message {
                role: tupi_domain::Role::Assistant,
                ..
            }
        ));
        assert!(matches!(
            state.lines[2],
            ChatEntry::Tool {
                ok: Some(true),
                duration: Some(duration),
                ..
            } if duration == std::time::Duration::from_micros(42)
        ));
        assert!(matches!(state.lines[3], ChatEntry::Memory { .. }));
        assert_eq!(state.status, Status::Idle);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn load_transcript_resets_the_previous_conversation() {
        let mut state = ChatState::default();
        state.push_user("antiga");
        state.load_transcript(vec![tupi_kernel::TranscriptEntry::User {
            text: "nova".into(),
        }]);

        assert_eq!(state.lines.len(), 1);
        assert!(matches!(
            &state.lines[0],
            ChatEntry::Message { content, .. } if content == "nova"
        ));
    }

    #[test]
    fn toggle_expanded_twice_returns_to_the_initial_state() {
        let mut state = ChatState::default();
        state.push_tool_started("c1", "read", "path=a.rs", "{\"path\":\"a.rs\"}");

        assert!(!state.is_expanded(0));
        assert!(state.toggle_expanded(0));
        assert!(state.is_expanded(0));
        assert!(!state.toggle_expanded(0));
        assert!(!state.is_expanded(0));
        assert!(state.expanded.is_empty());
    }

    #[test]
    fn new_session_clears_the_expanded_blocks() {
        let mut state = ChatState::default();
        state.push_tool_started("c1", "read", "path=a.rs", "{\"path\":\"a.rs\"}");
        state.push_tool_started("c2", "edit", "path=b.rs", "{\"path\":\"b.rs\"}");
        state.toggle_expanded(0);
        state.toggle_expanded(1);

        state.reset_for_new_session();

        assert!(state.expanded.is_empty());
        assert!(!state.is_expanded(0) && !state.is_expanded(1));
    }
}
