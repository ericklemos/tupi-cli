use crossterm::event::{
    Event as CrosstermEvent, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
};
use futures_util::StreamExt;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tokio::sync::broadcast::error::RecvError;
use tupi_domain::{Event, RequestId, Role};
use tupi_kernel::{Command, CommandBus, CommandError, CommandOutcome, EventBus, QueryBus};
use unicode_width::UnicodeWidthStr;

use crate::sdd::{SDD_COMMAND, parse_sdd_invocation, sdd_spec_names};
use crate::state::{ChatEntry, ChatState, Status};
use crate::terminal::TerminalGuard;

use crate::workflow::{parse_workflow_invocation, run_workflow, workflow_names};
pub struct BusSet {
    pub commands: CommandBus,
    pub queries: QueryBus,
    pub events: EventBus,
}

const SCROLL_STEP: u16 = 1;
const SCROLL_WHEEL: u16 = 3;
const SCROLL_PAGE: u16 = 10;

/// Sobe (ou reabre) o dashboard web da sessão a partir da TUI.
#[async_trait::async_trait]
pub trait DashboardLauncher: Send + Sync {
    /// Devolve a URL do dashboard, subindo o servidor se necessário.
    async fn open(&self) -> Result<String, String>;
}

/// Mensagem mostrada em `/dashboard` quando o launcher existe ou falta.
pub async fn dashboard_notice(dashboard: Option<&std::sync::Arc<dyn DashboardLauncher>>) -> String {
    match dashboard {
        Some(launcher) => match launcher.open().await {
            Ok(url) => format!("dashboard: {url}"),
            Err(error) => format!("dashboard: {error}"),
        },
        None => "dashboard indisponível nesta sessão; use --web ao iniciar".to_string(),
    }
}

pub async fn run(buses: BusSet, model: String, provider: String) -> anyhow::Result<()> {
    run_with_dashboard(buses, model, provider, None).await
}

pub async fn run_with_dashboard(
    buses: BusSet,
    mut model: String,
    mut provider: String,
    dashboard: Option<std::sync::Arc<dyn DashboardLauncher>>,
) -> anyhow::Result<()> {
    let mut terminal = TerminalGuard::enter()?;
    let mut key_events = EventStream::new();
    let mut events = buses.events.subscribe();
    let mut state = ChatState::default();
    let mut request: Option<RequestId> = None;
    state.workflows = workflow_names(&buses.queries).await;
    state.sdd_specs = sdd_spec_names(&buses.queries).await;

    loop {
        terminal.draw(|frame| draw(frame, &state, &model, &provider))?;

        // A TUI só desenha no topo do laço: sem uma fonte de tempo aqui,
        // spinner e cronômetro congelariam justamente enquanto a tool roda.
        // O tick existe apenas enquanto há algo para animar.
        let animating = state.tools_in_flight() > 0 || state.processing();
        let mut tick = std::pin::pin!(tokio::time::sleep(std::time::Duration::from_millis(100)));

        tokio::select! {
            () = &mut tick, if animating => {}
            received = key_events.next() => {
                let Some(Ok(event)) = received else { continue };

                match event {
                    CrosstermEvent::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            match key.code {
                                KeyCode::Char('c' | 'C') => {
                                    if let Some(text) = state
                                        .selection
                                        .and_then(|selection| selected_text(&state, selection))
                                        .filter(|text| !text.is_empty())
                                    {
                                        let route = terminal.copy(&text)?;
                                        state.notice =
                                            Some(crate::clipboard::copy_notice(route).to_string());
                                        continue;
                                    }
                                    break;
                                }
                                KeyCode::Char('v' | 'V') => {
                                    if !state.overlay_open() {
                                        match crate::clipboard::paste_from_system() {
                                            Some(text) if !text.is_empty() => {
                                                state
                                                    .input
                                                    .push_str(&crate::clipboard::normalize_paste(&text));
                                                state.refresh_suggestions();
                                            }
                                            Some(_) => {
                                                state.notice = Some("clipboard vazio".to_string())
                                            }
                                            None => {
                                                state.notice = Some(
                                                    "clipboard: sem acesso (instale wl-clipboard)"
                                                        .to_string(),
                                                )
                                            }
                                        }
                                    }
                                    continue;
                                }
                                KeyCode::Char('q') => break,
                                _ => {}
                            }
                        }

                        if state.overlay_open() {
                            if state
                                .overlay
                                .as_ref()
                                .is_some_and(|overlay| overlay.kind == crate::state::OverlayKind::Sessions)
                            {
                                match key.code {
                                    KeyCode::Esc => {
                                        state.cancel_pending_delete();
                                        state.close_overlay();
                                    }
                                    KeyCode::Up => state.overlay_prev(),
                                    KeyCode::Down => state.overlay_next(),
                                    KeyCode::Char('d') => {
                                        if let Some(id) = state.sessions_delete_pressed() {
                                            match buses
                                                .commands
                                                .dispatch(Command::DeleteSession { id: id.clone() })
                                                .await
                                            {
                                                Ok(CommandOutcome::SessionDeleted { found, .. }) => {
                                                    state.notice = Some(if found {
                                                        format!("sessão apagada: {id}")
                                                    } else {
                                                        format!("sessão {id} não encontrada")
                                                    });
                                                }
                                                Ok(_) => {}
                                                Err(error) => {
                                                    state.notice = Some(error.to_string())
                                                }
                                            }
                                            match buses.commands.dispatch(Command::ListSessions).await {
                                                Ok(CommandOutcome::SessionsList { items, ids })
                                                    if !items.is_empty() =>
                                                {
                                                    state.open_sessions(" sessões ", items, ids);
                                                }
                                                Ok(_) => state.close_overlay(),
                                                Err(error) => {
                                                    state.notice = Some(error.to_string())
                                                }
                                            }
                                        } else {
                                            state.notice =
                                                Some("pressione d de novo para apagar".to_string());
                                        }
                                    }
                                    KeyCode::Enter => {
                                        if let Some(id) = state.selected_id().map(str::to_string) {
                                            match buses
                                                .commands
                                                .dispatch(Command::ResumeSession { id })
                                                .await
                                            {
                                                Ok(CommandOutcome::SessionResumed {
                                                    title,
                                                    mode,
                                                    entries,
                                                    ..
                                                }) => {
                                                    state.close_overlay();
                                                    state.load_transcript(entries);
                                                    state.apply_mode_changed(mode);
                                                    state.sdd_specs =
                                                        sdd_spec_names(&buses.queries).await;
                                                    state.notice =
                                                        Some(format!("sessão retomada: {title}"));
                                                }
                                                Ok(_) => state.close_overlay(),
                                                Err(error) => {
                                                    state.notice = Some(error.to_string())
                                                }
                                            }
                                        } else {
                                            state.close_overlay();
                                        }
                                    }
                                    _ => state.cancel_pending_delete(),
                                }
                                continue;
                            }

                            match key.code {
                                KeyCode::Esc => state.close_overlay(),
                                KeyCode::Up => state.overlay_prev(),
                                KeyCode::Down => state.overlay_next(),
                                KeyCode::Char('d') => {
                                    let id = state.selected_id().map(str::to_string);
                                    let memory = state
                                        .overlay
                                        .as_ref()
                                        .map(|overlay| overlay.title.contains("memória"))
                                        .unwrap_or(false);
                                    if memory
                                        && let Some(id) = id
                                    {
                                        match buses
                                            .commands
                                            .dispatch(Command::Memory {
                                                action: tupi_kernel::MemoryAction::Forget(id.clone()),
                                            })
                                            .await
                                        {
                                            Ok(CommandOutcome::MemoryForgotten { found, .. }) => {
                                                state.close_overlay();
                                                state.notice = Some(if found {
                                                    format!("memória: esquecido {id}")
                                                } else {
                                                    format!("memória: {id} não encontrado")
                                                });
                                            }
                                            Ok(_) => state.close_overlay(),
                                            Err(error) => state.notice = Some(error.to_string()),
                                        }
                                    }
                                }
                                KeyCode::Enter => {
                                    if state.overlay.as_ref().is_some_and(|overlay| {
                                        overlay.title.contains("memória")
                                            || overlay.title.contains("pendências")
                                    }) {
                                        state.close_overlay();
                                        continue;
                                    }
                                    if let Some(selected) = state.selected_item().map(str::to_string) {
                                        match buses
                                            .commands
                                            .dispatch(Command::SetModel { model: selected.clone() })
                                            .await
                                        {
                                            Ok(CommandOutcome::ModelChanged { model: changed }) => {
                                                if let Some((new_provider, new_model)) = changed.split_once('/') {
                                                    provider = new_provider.to_string();
                                                    model = new_model.to_string();
                                                } else {
                                                    model = changed.clone();
                                                }
                                                state.notice = Some(format!("modelo: {changed}"));
                                                state.close_overlay();
                                            }
                                            Ok(_) => state.close_overlay(),
                                            Err(error) => state.notice = Some(error.to_string()),
                                        }
                                    } else {
                                        state.close_overlay();
                                    }
                                }
                                _ => {}
                            }
                            continue;
                        }

                        match should_toggle_mode(&state, key) {
                            ToggleAction::Dispatch => {
                                match buses.commands.dispatch(Command::ToggleMode).await {
                                    Ok(CommandOutcome::ModeChanged { mode }) => {
                                        state.apply_mode_changed(mode);
                                    }
                                    Ok(_) => {}
                                    Err(error) => state.notice = Some(error.to_string()),
                                }
                                continue;
                            }
                            ToggleAction::BusyNotice => {
                                state.notice = Some("aguarde o turno terminar para trocar de modo".to_string());
                                continue;
                            }
                            ToggleAction::Ignore => {}
                        }

                        match key.code {
                            KeyCode::Tab => state.accept_suggestion(),
                            KeyCode::Enter if !state.input.trim().is_empty() => {
                                let input = state.input.trim().to_string();

                                if state.status != Status::Idle {
                                    if input.starts_with('/') {
                                        state.notice = Some(
                                            "comando indisponível enquanto o turno roda".to_string(),
                                        );
                                    } else {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        state.enqueue(&input);
                                    }
                                } else if input.starts_with('/') {
                                    if input == "/connect" || input.starts_with("/connect ") {
                                        let provider_name = input
                                            .strip_prefix("/connect")
                                            .map(str::trim)
                                            .filter(|name| !name.is_empty())
                                            .unwrap_or("antigravity");
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        match buses
                                            .commands
                                            .dispatch(Command::Connect {
                                                provider: provider_name.to_string(),
                                            })
                                            .await
                                        {
                                            Ok(_) => {
                                                state.notice =
                                                    Some(format!("iniciando conexão com {provider_name}…"))
                                            }
                                            Err(error) => state.fail(&error.to_string()),
                                        }
                                    } else if input == "/models" {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        // A listagem vai aos provedores; o comando só dispara a busca.
                                        // O resultado chega por evento, sem travar o desenho da TUI.
                                        match buses.commands.dispatch(Command::RefreshModels).await {
                                            Ok(_) => {
                                                state.notice = Some("carregando modelos…".to_string())
                                            }
                                            Err(error) => state.fail(&error.to_string()),
                                        }
                                    } else if input == "/new" {
                                        // A TUI não guarda o histórico: quem o tem é a sessão do
                                        // app. Limpar aqui é reconstruir o estado local; os catálogos
                                        // locais voltam porque não vêm do app.
                                        match buses.commands.dispatch(Command::NewSession).await {
                                            Ok(CommandOutcome::SessionReset) => {
                                                state.reset_for_new_session();
                                                // Planos podem ter mudado durante a sessão.
                                                state.sdd_specs =
                                                    sdd_spec_names(&buses.queries).await;
                                                state.notice = Some("nova sessão".to_string());
                                            }
                                            Ok(_) => {}
                                            Err(error) => state.fail(&error.to_string()),
                                        }
                                    } else if input == "/sessions" {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        match buses.commands.dispatch(Command::ListSessions).await {
                                            Ok(CommandOutcome::SessionsList { items, ids })
                                                if !items.is_empty() =>
                                            {
                                                state.open_sessions(" sessões ", items, ids);
                                            }
                                            Ok(_) => {
                                                state.notice =
                                                    Some("nenhuma sessão neste repositório".to_string())
                                            }
                                            Err(error) => {
                                                state.notice = Some(error.to_string())
                                            }
                                        }
                                    } else if input == "/enable-show-prompts-messages" {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        match buses
                                            .commands
                                            .dispatch(Command::ToggleShowPrompts)
                                            .await
                                        {
                                            Ok(CommandOutcome::ShowPrompts { enabled }) => {
                                                state.notice = Some(if enabled {
                                                    "prompts dos providers: mostrando".to_string()
                                                } else {
                                                    "prompts dos providers: ocultos".to_string()
                                                });
                                            }
                                            Ok(_) => {}
                                            Err(error) => state.fail(&error.to_string()),
                                        }
                                    } else if input == "/dashboard" {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        state.notice =
                                            Some(dashboard_notice(dashboard.as_ref()).await);
                                    } else if let Some(slug) = parse_sdd_invocation(&input) {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        match slug {
                                            None => {
                                                state.notice = Some(format!(
                                                    "uso: {SDD_COMMAND} <slug> — Tab completa os planos abertos"
                                                ))
                                            }
                                            Some(slug) => match buses
                                                .commands
                                                .dispatch(Command::ResumeSdd {
                                                    slug: slug.to_string(),
                                                })
                                                .await
                                            {
                                                Ok(CommandOutcome::SddResumed {
                                                    mode, ..
                                                }) => {
                                                    // O aviso vem do app como
                                                    // Event::Notice, depois do
                                                    // ModeChanged: assim ele não
                                                    // é apagado pelo rodapé.
                                                    state.apply_mode_changed(mode);
                                                    state.sdd_specs =
                                                        sdd_spec_names(&buses.queries).await;
                                                }
                                                Ok(_) => {}
                                                Err(error) => state.notice = Some(error.to_string()),
                                            },
                                        }
                                    } else if let Some(action) = parse_memory_invocation(&input) {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        match buses
                                            .commands
                                            .dispatch(Command::Memory { action })
                                            .await
                                        {
                                            Ok(CommandOutcome::Memory { title, items, ids }) => {
                                                state.open_overlay_with_ids(title, items, ids)
                                            }
                                            Ok(CommandOutcome::MemoryToggled { enabled }) => {
                                                state.notice = Some(if enabled {
                                                    "memória: ligada".to_string()
                                                } else {
                                                    "memória: desligada".to_string()
                                                });
                                            }
                                            Ok(CommandOutcome::MemoryForgotten { id, found }) => {
                                                state.notice = Some(if found {
                                                    format!("memória: esquecido {id}")
                                                } else {
                                                    format!("memória: {id} não encontrado")
                                                });
                                            }
                                            Ok(_) => {}
                                            Err(error) => state.fail(&error.to_string()),
                                        }
                                    } else {
                                        match parse_workflow_invocation(&input) {
                                            Some((name, extra)) => request = run_workflow(
                                                &buses.commands,
                                                &mut state,
                                                name,
                                                extra,
                                            )
                                            .await,
                                            None => state.fail(&format!("comando desconhecido: {input}")),
                                        }
                                    }
                                } else {
                                    let text = state.input.clone();
                                    if let Some(request_id) =
                                        start_turn(&buses.commands, &mut state, text).await
                                    {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        request = Some(request_id);
                                    }
                                }
                            }
                            KeyCode::Esc => {
                                if state.selection.take().is_some() {
                                    continue;
                                }
                                if let Some(request_id) = request {
                                    // O cancelamento espera as tools em voo
                                    // pararem: avisa e desenha antes para o
                                    // ESC não parecer ignorado.
                                    state.notice = Some("cancelando…".to_string());
                                    terminal.draw(|frame| draw(frame, &state, &model, &provider))?;
                                    let cancelled = matches!(
                                        buses
                                            .commands
                                            .dispatch(Command::CancelStream { request_id })
                                            .await,
                                        Ok(CommandOutcome::Cancelled)
                                    );
                                    state.notice = None;
                                    if cancelled {
                                        state.finish_assistant();
                                        request = drain_queue(&buses.commands, &mut state).await;
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                state.input.pop();
                                state.refresh_suggestions();
                            }
                            KeyCode::Up if state.suggestions_visible() => state.suggestion_prev(),
                            KeyCode::Down if state.suggestions_visible() => state.suggestion_next(),
                            KeyCode::Up => scroll_up(&mut state, SCROLL_STEP),
                            KeyCode::Down => scroll_down(&mut state, SCROLL_STEP),
                            KeyCode::PageUp => scroll_up(&mut state, SCROLL_PAGE),
                            KeyCode::PageDown => scroll_down(&mut state, SCROLL_PAGE),
                            KeyCode::Home => state.scroll = u16::MAX,
                            KeyCode::End => state.scroll = 0,
                            KeyCode::Char(character) => {
                                // Ctrl+letra é atalho, não texto; AltGr chega como
                                // CONTROL+ALT e precisa continuar digitando.
                                if !is_shortcut_char(key) {
                                    state.input.push(character);
                                    state.refresh_suggestions();
                                }
                            }
                            _ => {}
                        }
                    }
                    CrosstermEvent::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                            if let Some((row, column)) =
                                transcript_position(&state, mouse.column, mouse.row)
                            {
                                state.selection =
                                    Some(crate::selection::Selection::new(row, column));
                            } else {
                                state.selection = None;
                            }
                        }
                        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                            let position =
                                clamped_transcript_position(&mut state, mouse.column, mouse.row);
                            if let (Some((row, column)), Some(selection)) =
                                (position, state.selection.as_mut())
                            {
                                selection.cursor = (row, column);
                            }
                        }
                        MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                            if state
                                .selection
                                .map(|selection| selection.is_empty())
                                .unwrap_or(false)
                            {
                                state.selection = None;
                            }
                        }
                        MouseEventKind::ScrollUp => scroll_up(&mut state, SCROLL_WHEEL),
                        MouseEventKind::ScrollDown => scroll_down(&mut state, SCROLL_WHEEL),
                        _ => {}
                    },
                    CrosstermEvent::Paste(text) => {
                        // Bracketed paste: o terminal entrega o texto inteiro de
                        // uma vez, sem virar Enter nem atalho.
                        if !state.overlay_open() {
                            state.input.push_str(&crate::clipboard::normalize_paste(&text));
                            state.refresh_suggestions();
                        }
                    }
                    CrosstermEvent::Resize(_, _) => {
                        state.selection = None;
                    }
                    _ => {}
                }
            }
            received = events.recv() => {
                match received {
                    Ok(Event::Token { request_id, text }) if Some(request_id) == request => {
                        state.append_token(&text);
                    }
                    Ok(Event::StreamFinished {
                        request_id,
                        prompt_tokens,
                        cached_tokens,
                        completion_tokens,
                    }) if Some(request_id) == request => {
                        state.set_usage(prompt_tokens, cached_tokens, completion_tokens);
                        state.finish_assistant();
                        request = drain_queue(&buses.commands, &mut state).await;
                    }
                    Ok(Event::ContextUpdated {
                        request_id,
                        tokens,
                        budget,
                    }) if Some(request_id) == request => {
                        state.set_context(tokens, budget);
                    }
                    Ok(Event::ContextBudget { budget }) => {
                        state.set_context_budget(budget);
                    }
                    Ok(Event::ContextTrimmed {
                        request_id,
                        estimate,
                        omitted_units,
                        shrunk_results,
                    }) if Some(request_id) == request => {
                        state.notice = Some(format!(
                            "contexto podado: ~{estimate} tokens · -{omitted_units} turno(s) · {shrunk_results} ponteiro(s)"
                        ));
                    }
                    Ok(Event::StreamFailed { request_id, message }) if Some(request_id) == request => {
                        state.fail(&message);
                        request = drain_queue(&buses.commands, &mut state).await;
                    }
                    Ok(Event::PromptMessages { request_id, body })
                        if Some(request_id) == request =>
                    {
                        state.push_prompt(&body);
                    }
                    Ok(Event::ToolStarted {
                        request_id,
                        call_id,
                        name,
                        summary,
                        ..
                    }) if Some(request_id) == request => {
                        state.push_tool_started(&call_id, &name, &summary);
                    }
                    Ok(Event::ToolFinished {
                        request_id,
                        call_id,
                        ok,
                        duration_us,
                        ..
                    }) if Some(request_id) == request => {
                        // A duração vem de quem executou a tool: o evento já
                        // atravessou o bus, então medir aqui daria errado.
                        state.finish_tool(
                            &call_id,
                            ok,
                            Some(std::time::Duration::from_micros(duration_us)),
                        );
                    }
                    Ok(Event::Memory { text, after }) => {
                        state.push_memory(&text, after);
                    }
                    Ok(Event::Notice { message }) => {
                        state.notice = Some(message);
                    }
                    Ok(Event::ModelsLoaded { models }) => {
                        state.notice = None;
                        state.open_overlay(" modelos ", models);
                    }
                    Ok(Event::ModelsFailed { message }) => {
                        state.fail(&message);
                    }
                    Ok(Event::ModeChanged { mode }) => {
                        state.apply_mode_changed(mode);
                        // Fechar o SDD muda o plano persistido: recarrega os
                        // slugs retomáveis sem depender do próximo boot.
                        if mode == tupi_kernel::AgentMode::Execution {
                            state.sdd_specs = sdd_spec_names(&buses.queries).await;
                        }
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => {
                        state.notice = Some("eventos perdidos: a interface ficou para trás".to_string());
                    }
                    Err(_) => break,
                }
            }
        }
    }

    Ok(())
}

async fn start_turn(bus: &CommandBus, state: &mut ChatState, text: String) -> Option<RequestId> {
    match bus
        .dispatch(Command::SendMessage { text: text.clone() })
        .await
    {
        Ok(CommandOutcome::Started { request_id }) => {
            state.push_user(&text);
            state.begin_assistant();
            state.scroll = 0;
            Some(request_id)
        }
        Ok(_) => None,
        Err(CommandError::Busy) => {
            state.notice = Some("aguarde o fim do stream".to_string());
            None
        }
        Err(error) => {
            state.notice = Some(error.to_string());
            None
        }
    }
}

fn parse_memory_invocation(input: &str) -> Option<tupi_kernel::MemoryAction> {
    let rest = input
        .strip_prefix("/memory")
        .or_else(|| input.strip_prefix("/memória"))
        .or_else(|| input.strip_prefix("/memoria"))?
        .trim();
    match rest {
        "" => Some(tupi_kernel::MemoryAction::List),
        "pending" | "pendencias" | "pendências" => Some(tupi_kernel::MemoryAction::Pending),
        "off" => Some(tupi_kernel::MemoryAction::Off),
        "on" => Some(tupi_kernel::MemoryAction::On),
        _ => {
            if let Some(query) = rest
                .strip_prefix("search ")
                .or_else(|| rest.strip_prefix("busca "))
            {
                Some(tupi_kernel::MemoryAction::Search(query.trim().to_string()))
            } else {
                rest.strip_prefix("forget ")
                    .or_else(|| rest.strip_prefix("esquece "))
                    .map(|id| tupi_kernel::MemoryAction::Forget(id.trim().to_string()))
            }
        }
    }
}

async fn drain_queue(bus: &CommandBus, state: &mut ChatState) -> Option<RequestId> {
    let text = state.dequeue()?;
    start_turn(bus, state, text).await
}

fn scroll_up(state: &mut ChatState, rows: u16) {
    state.scroll = state.scroll.saturating_add(rows);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToggleAction {
    Dispatch,
    BusyNotice,
    Ignore,
}

fn is_shortcut_char(key: crossterm::event::KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT)
}

#[cfg(test)]
mod key_tests {
    use super::is_shortcut_char;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn ctrl_letter_is_a_shortcut_but_altgr_types() {
        let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
        let ctrl_shift_v = KeyEvent::new(
            KeyCode::Char('V'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let altgr_at = KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        let plain = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);

        assert!(is_shortcut_char(ctrl_v));
        assert!(is_shortcut_char(ctrl_shift_v));
        assert!(!is_shortcut_char(altgr_at));
        assert!(!is_shortcut_char(plain));
    }
}

fn should_toggle_mode(state: &ChatState, key: crossterm::event::KeyEvent) -> ToggleAction {
    if state.overlay_open() {
        return ToggleAction::Ignore;
    }
    let is_shift_tab = key.code == KeyCode::BackTab
        || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));
    if !is_shift_tab {
        return ToggleAction::Ignore;
    }
    if state.processing() {
        ToggleAction::BusyNotice
    } else {
        ToggleAction::Dispatch
    }
}

fn scroll_down(state: &mut ChatState, rows: u16) {
    state.scroll = state.scroll.saturating_sub(rows);
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &ChatState, model: &str, provider: &str) {
    let area = frame.area();
    let compact = area.height < 16 || area.width < 40;

    // O prompt "› " ocupa 2 colunas; o bloco com borda ocupa mais 2.
    let inner_width = if compact {
        area.width
    } else {
        area.width.saturating_sub(2)
    };
    let input_width = inner_width.saturating_sub(2).max(1) as usize;
    let input_rows = wrap_text(&state.input, input_width).len() as u16;
    // Cresce com o texto, mas deixa o transcript respirar.
    let max_rows = (area.height / 3).clamp(1, 6);
    let input_rows = input_rows.min(max_rows).max(1);
    let max_notice_rows = (area.height / 4).clamp(1, 3) as usize;
    let details_height = if let Some(notice) = &state.notice {
        notice_lines(notice, area.width, max_notice_rows)
            .len()
            .clamp(1, max_notice_rows) as u16
    } else {
        1
    };
    let composer_height = if compact {
        input_rows
    } else {
        input_rows.saturating_add(2)
    }
    .min(
        area.height
            .saturating_sub(details_height.saturating_add(1))
            .max(1),
    );

    let [history_area, composer_area, details_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(details_height),
    ])
    .areas(area);

    draw_transcript(frame, state, history_area);
    draw_composer(frame, state, composer_area, compact);
    draw_details(frame, state, model, provider, details_area);
    draw_suggestions(frame, state, composer_area);
    draw_overlay(frame, state);
}

fn selected_text(state: &ChatState, selection: crate::selection::Selection) -> Option<String> {
    let view = state.view.get()?;
    let mut cache = state.render_cache.borrow_mut();
    let rows = transcript_rows(&state.lines, view.width, state.processing(), &mut cache);
    drop(cache);
    Some(crate::selection::selection_text(&rows, &selection))
}

fn transcript_position(state: &ChatState, column: u16, row: u16) -> Option<(usize, u16)> {
    let view = state.view.get()?;
    let area = view.area;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let right = area.x.saturating_add(area.width);
    let bottom = area.y.saturating_add(area.height);
    if column < area.x || column >= right || row < area.y || row >= bottom {
        return None;
    }
    let index = view.start + (row - area.y) as usize;
    if index >= view.rows {
        return None;
    }
    Some((index, column - area.x))
}

fn clamped_transcript_position(
    state: &mut ChatState,
    column: u16,
    row: u16,
) -> Option<(usize, u16)> {
    let view = state.view.get()?;
    let area = view.area;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    if row < area.y {
        scroll_up(state, 1);
    } else if row >= area.y.saturating_add(area.height) {
        scroll_down(state, 1);
    }
    let (start, _) = visible_range(view.rows, area.height as usize, state.scroll);
    let top = area.y;
    let bottom = area.y.saturating_add(area.height - 1);
    let left = area.x;
    let right = area.x.saturating_add(area.width - 1);
    let local_row = row.clamp(top, bottom) - area.y;
    let local_column = column.clamp(left, right) - area.x;
    let index = start + local_row as usize;
    if index >= view.rows {
        return None;
    }
    Some((index, local_column))
}

fn draw_transcript(frame: &mut ratatui::Frame<'_>, state: &ChatState, area: Rect) {
    /// Sessão nova não tem histórico, então o transcript era um retângulo
    /// vazio e a primeira tela não ensinava nada. Estas linhas somem no
    /// primeiro turno.
    fn empty_state(width: u16) -> Vec<Line<'static>> {
        let dim = Style::default().fg(Color::DarkGray);
        let bold_green = Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD);
        let mut lines = vec![Line::from(vec![
            Span::styled("  ● ", bold_green),
            Span::styled("tupi", bold_green),
        ])];
        for hint in [
            "descreva a tarefa ou aponte o caminho de um arquivo",
            "/ abre os comandos  ·  /workflows lista os procedimentos",
        ] {
            for piece in wrap_text(hint, (width as usize).saturating_sub(4).max(1)) {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(piece, dim),
                ]));
            }
        }
        lines
    }

    let content_area = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };

    let rows = {
        let mut cache = state.render_cache.borrow_mut();
        transcript_layout_at(
            &state.lines,
            content_area.width,
            state.processing(),
            &mut cache,
            std::time::Instant::now(),
        )
        .rows
    };
    let visible = area.height as usize;
    let (start, end) = visible_range(rows.len(), visible, state.scroll);

    state.view.set(Some(crate::selection::TranscriptView {
        rows: rows.len(),
        start,
        width: content_area.width,
        area: content_area,
    }));

    if state.lines.is_empty() && !state.processing() {
        frame.render_widget(
            Paragraph::new(empty_state(content_area.width)),
            content_area,
        );
        return;
    }

    let mut rows = rows;
    if let Some(selection) = state.selection {
        let ((start_row, start_column), (end_row, end_column)) = selection.normalized();
        for (index, row) in rows.iter_mut().enumerate().take(end).skip(start) {
            if index < start_row || index > end_row {
                continue;
            }
            let from = if index == start_row { start_column } else { 0 };
            let to = if index == end_row {
                end_column
            } else {
                u16::MAX
            };
            *row =
                crate::selection::apply_selection(std::mem::replace(row, Line::from("")), from, to);
        }
    }

    frame.render_widget(Paragraph::new(rows[start..end].to_vec()), content_area);

    if rows.len() > visible {
        draw_scrollbar(frame, area, rows.len(), visible, state.scroll as usize);
    }
}

fn visible_range(len: usize, height: usize, scroll: u16) -> (usize, usize) {
    if len == 0 || height == 0 {
        return (0, 0);
    }

    let max_offset = len.saturating_sub(height);
    let offset = (scroll as usize).min(max_offset);
    let end = len - offset;
    let start = end.saturating_sub(height);

    (start, end)
}

fn draw_scrollbar(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    len: usize,
    visible: usize,
    scroll: usize,
) {
    if area.width == 0 || area.height < 2 || len <= visible {
        return;
    }

    let track = area.height as usize;
    let max_offset = len - visible;
    let offset = scroll.min(max_offset);

    let thumb_size = ((visible * track) / len).max(1).min(track);
    let progress = if max_offset == 0 {
        1.0
    } else {
        offset as f64 / max_offset as f64
    };
    let travel = track - thumb_size;
    let thumb_top = ((1.0 - progress) * travel as f64).round() as usize;

    let lines: Vec<Line<'static>> = (0..track)
        .map(|row| {
            let is_thumb = row >= thumb_top && row < thumb_top + thumb_size;
            let (glyph, style) = if is_thumb {
                ("┃", Style::default().fg(Color::Cyan))
            } else {
                ("│", Style::default().fg(Color::DarkGray))
            };
            Line::from(Span::styled(glyph, style))
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines),
        Rect {
            x: area.x + area.width - 1,
            y: area.y,
            width: 1,
            height: area.height,
        },
    );
}

fn draw_composer(frame: &mut ratatui::Frame<'_>, state: &ChatState, area: Rect, compact: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Fora de Idle o Enter não abre turno: enfileira (e comando `/` é
    // recusado). Cor e legenda saem do MESMO `status` que `run_with_dashboard`
    // consulta, então avisam antes da tecla em vez de depois do aviso.
    let busy = state.status != Status::Idle;
    let accent = match state.status {
        Status::Idle => Color::Cyan,
        Status::Streaming => Color::Yellow,
        Status::Busy => Color::Red,
    };

    let (inner, prompt_x, prompt_y) = if compact {
        (area, area.x, area.y)
    } else {
        let mut block = Block::bordered().border_style(Style::default().fg(if busy {
            accent
        } else {
            Color::DarkGray
        }));
        let title = " enter enfileira ";
        // Em terminal estreito o título comeria a borda inteira; aí ele some.
        if busy && title.len() + 2 <= area.width as usize {
            block = block.title(Line::from(Span::styled(title, Style::default().fg(accent))));
        }
        let inner = block.inner(area);
        frame.render_widget(block, area);
        (inner, inner.x, inner.y)
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let prompt = Span::styled(
        "› ",
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    );

    // Quebra o texto na largura útil; as continuações recuam 2 colunas para
    // alinhar com o texto depois do prompt.
    let input_width = inner.width.saturating_sub(2).max(1) as usize;
    // `wrap_input`, não `wrap_text`: o composer mostra o rascunho como foi
    // digitado, inclusive o espaço final que anda o cursor.
    let wrapped = wrap_input(&state.input, input_width);
    let start = wrapped.len().saturating_sub(inner.height as usize);
    let visible = &wrapped[start..];

    let mut lines: Vec<Line> = Vec::with_capacity(visible.len());
    if state.input.is_empty() {
        // Enquanto o turno roda `/models` seria recusado, então o placeholder
        // troca a dica de comando pelo que o Enter faz agora.
        let placeholder = if busy {
            "escreva algo…  (enter enfileira)"
        } else {
            "escreva algo…  (/models modelo · /sdd retoma)"
        };
        lines.push(Line::from(vec![
            prompt.clone(),
            Span::styled(placeholder, Style::default().fg(Color::DarkGray)),
        ]));
    } else {
        for (index, chunk) in visible.iter().enumerate() {
            let prefix = if start + index == 0 {
                prompt.clone()
            } else {
                Span::raw("  ")
            };
            lines.push(Line::from(vec![prefix, Span::raw(chunk.clone())]));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);

    let cursor_row = if state.input.is_empty() {
        0
    } else {
        wrapped.len().saturating_sub(1).saturating_sub(start) as u16
    };
    let cursor_column = if state.input.is_empty() {
        0
    } else {
        wrapped.last().map_or(0, |line| line.width() as u16)
    };
    // No limite da linha o cursor cairia fora da área; encosta na última coluna.
    let max_x = inner.x.saturating_add(inner.width.saturating_sub(1));
    let cursor_x = prompt_x
        .saturating_add(2)
        .saturating_add(cursor_column)
        .min(max_x);
    let cursor_y = prompt_y.saturating_add(cursor_row);
    if cursor_x < inner.x.saturating_add(inner.width)
        && cursor_y < inner.y.saturating_add(inner.height)
    {
        frame.set_cursor_position(Position::new(cursor_x, cursor_y));
    }
}

fn sanitize_notice(notice: &str) -> String {
    let mut out = String::with_capacity(notice.len());
    let mut last_space = false;
    for ch in notice.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch.is_control() {
            if !last_space && !out.is_empty() {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = ch.is_whitespace();
        }
    }
    out.trim().to_string()
}

fn notice_lines(notice: &str, width: u16, max_rows: usize) -> Vec<Line<'static>> {
    let cleaned = sanitize_notice(notice);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let mut wrapped = wrap_text(&cleaned, inner_width);
    let max_rows = max_rows.max(1);
    if wrapped.len() > max_rows {
        wrapped.truncate(max_rows);
        if let Some(last) = wrapped.last_mut() {
            let mut trimmed = String::new();
            let limit = inner_width.saturating_sub(1);
            let mut used = 0usize;
            for ch in last.chars() {
                let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + w > limit {
                    break;
                }
                trimmed.push(ch);
                used += w;
            }
            trimmed.push('…');
            *last = trimmed;
        }
    }
    wrapped
        .into_iter()
        .map(|line| {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(line, Style::default().fg(Color::Yellow)),
            ])
        })
        .collect()
}

fn draw_details(
    frame: &mut ratatui::Frame<'_>,
    state: &ChatState,
    model: &str,
    provider: &str,
    area: Rect,
) {
    fn format_compact_tokens(tokens: u64) -> String {
        if tokens < 1_000 {
            return tokens.to_string();
        }
        let tenths = tokens / 100;
        if tenths.is_multiple_of(10) {
            format!("{}k", tenths / 10)
        } else {
            format!("{}.{}k", tenths / 10, tenths % 10)
        }
    }

    /// Placar das tools do turno, ao lado do contexto. `None` quando o turno
    /// não chamou nenhuma tool: rodapé sem tool continua idêntico.
    fn placar_text(state: &ChatState) -> Option<String> {
        let (total, failures) = state.turn_tool_stats();
        if total == 0 {
            return None;
        }
        Some(if failures > 0 {
            format!("⚙ {total} {failures}✗")
        } else {
            format!("⚙ {total}")
        })
    }

    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(Clear, area);

    if let Some(notice) = &state.notice {
        let lines = notice_lines(notice, area.width, area.height as usize);
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }

    let state_color = match state.status {
        Status::Idle => Color::DarkGray,
        Status::Streaming => Color::Yellow,
        Status::Busy => Color::Red,
    };

    let (mode_badge, mode_color) = match state.mode {
        tupi_kernel::AgentMode::Sdd => ("[SDD]", Color::Cyan),
        tupi_kernel::AgentMode::Execution => ("[EXEC]", Color::DarkGray),
        tupi_kernel::AgentMode::Docs => ("[DOCS]", Color::Magenta),
    };

    let left = Line::from(vec![
        Span::styled(" ● ", Style::default().fg(state_color)),
        Span::styled(
            format!("{mode_badge} "),
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(model.to_string(), Style::default().fg(Color::Gray)),
        Span::styled(
            format!("  ·  {provider}  ·  {}", status_label(state.status)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let full_hints = "shift+tab: modo · arraste para selecionar · ctrl+c copia · ctrl+q sai";
    let short_hints = "ctrl+c copia · ctrl+q sai";

    if let Some(ctx) = state.context {
        let percent = (ctx.budget > 0)
            .then(|| ((ctx.tokens as f64 / ctx.budget as f64) * 100.0).round() as u64);
        let ctx_color = match percent {
            Some(p) if p >= 90 => Color::Red,
            Some(p) if p >= 75 => Color::Yellow,
            _ => Color::Gray,
        };
        let value = match percent {
            Some(p) => format!(
                "{}/{} ({p}%)",
                format_compact_tokens(ctx.tokens),
                format_compact_tokens(ctx.budget)
            ),
            None => format!("{} tokens", format_compact_tokens(ctx.tokens)),
        };

        let placar = placar_text(state);
        let placar_width = placar
            .as_ref()
            .map(|text| text.chars().count() + 5)
            .unwrap_or(0) as u16;

        let left_width = left.width() as u16;
        let ctx_width = ("ctx ".width() + value.width()) as u16;
        let full_width = full_hints.width() as u16 + 5 + placar_width + ctx_width;
        let short_width = short_hints.width() as u16 + 5 + placar_width + ctx_width;

        // O placar entra antes do contexto e é a primeira coisa a cair quando
        // a largura aperta: o contexto nunca sai por causa dele.
        let right_line = |hints: &str| {
            let mut spans = vec![Span::styled(
                format!("{hints}  ·  "),
                Style::default().fg(Color::DarkGray),
            )];
            if let Some(text) = placar.clone() {
                spans.push(Span::styled(text, Style::default().fg(Color::DarkGray)));
                spans.push(Span::raw("  ·  "));
            }
            spans.push(Span::styled("ctx ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(value.clone(), Style::default().fg(ctx_color)));
            Line::from(spans)
        };

        let (right_line, right_width) = if area.width >= left_width + 2 + full_width {
            (right_line(full_hints), full_width + 1)
        } else if area.width >= left_width + 2 + short_width {
            (right_line(short_hints), short_width + 1)
        } else if area.width > ctx_width + 10 {
            (
                Line::from(vec![
                    Span::styled("ctx ", Style::default().fg(Color::DarkGray)),
                    Span::styled(value, Style::default().fg(ctx_color)),
                ]),
                ctx_width + 1,
            )
        } else {
            frame.render_widget(Paragraph::new(left), area);
            return;
        };

        let [left_area, right_area] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(right_width)]).areas(area);

        frame.render_widget(Paragraph::new(left), left_area);
        frame.render_widget(
            Paragraph::new(right_line).alignment(Alignment::Right),
            right_area,
        );
        return;
    }

    let hint_width = full_hints.width() as u16 + 1;

    if area.width > hint_width + 20 {
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(hint_width)]).areas(area);

        frame.render_widget(Paragraph::new(left), left_area);
        frame.render_widget(
            Paragraph::new(full_hints)
                .alignment(Alignment::Right)
                .style(Style::default().fg(Color::DarkGray)),
            right_area,
        );
    } else {
        frame.render_widget(Paragraph::new(left), area);
    }
}

fn draw_suggestions(frame: &mut ratatui::Frame<'_>, state: &ChatState, composer_area: Rect) {
    let Some(suggestions) = &state.suggestions else {
        return;
    };
    if suggestions.is_empty() || composer_area.y == 0 {
        return;
    }

    let height = (suggestions.len() as u16 + 2).min(composer_area.y);
    let width = composer_area.width.clamp(1, 34);
    let area = Rect {
        x: composer_area.x,
        y: composer_area.y.saturating_sub(height),
        width,
        height,
    };
    frame.render_widget(Clear, area);

    let lines: Vec<Line> = suggestions
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let style = if index == state.suggestion_index {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(format!(" {item} ")).style(style)
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" comandos ")),
        area,
    );
}

fn draw_overlay(frame: &mut ratatui::Frame<'_>, state: &ChatState) {
    /// Dicas de teclado do modal, no mesmo estilo do rodapé principal: com o
    /// overlay aberto todos os atalhos de baixo ficam inertes, então estas são
    /// as únicas teclas que o usuário tem para descobrir.
    fn hints(overlay: &crate::state::Overlay, visible: usize) -> String {
        let memory = overlay.title.contains("memória");
        let pendings = overlay.title.contains("pendências");
        let mut keys = vec!["↑↓ navegar"];
        if memory {
            keys.push("d esquece");
        }
        if !memory && !pendings {
            keys.push("enter aplica");
        }
        keys.push("esc fecha");
        let keys = keys.join(" · ");
        if overlay.items.len() > visible {
            format!(
                " {}/{} · {keys} ",
                overlay.selected + 1,
                overlay.items.len()
            )
        } else {
            format!(" {keys} ")
        }
    }

    let Some(overlay) = &state.overlay else {
        return;
    };

    let area = centered_rect(frame.area(), 60, 60);
    frame.render_widget(Clear, area);

    let block = Block::bordered().title(overlay.title.clone());
    let inner = block.inner(area);
    // A última linha interna fica reservada às dicas de teclado.
    let list_height = (inner.height as usize).saturating_sub(1).max(1);
    let start = overlay
        .selected
        .saturating_sub(list_height.saturating_sub(1));
    let lines: Vec<Line> = overlay
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(list_height)
        .map(|(index, item)| {
            let style = if index == overlay.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(format!(" {item} ")).style(style)
        })
        .collect();

    let body = if lines.is_empty() {
        Paragraph::new(" nada para mostrar ")
    } else {
        Paragraph::new(lines)
    };

    frame.render_widget(block, area);

    let [list_area, hints_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(body, list_area);
    frame.render_widget(
        Paragraph::new(hints(overlay, list_height)).style(Style::default().fg(Color::DarkGray)),
        hints_area,
    );
}

#[cfg(test)]
mod overlay_hint_tests {
    use super::*;

    fn overlay_text(state: &ChatState, width: u16, height: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw_overlay(frame, state)).unwrap();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn overlay_shows_its_keyboard_hints_and_position() {
        let mut state = ChatState::default();
        state.open_overlay(
            " modelos ",
            (0..40).map(|n| format!("modelo-{n}")).collect(),
        );
        let text = overlay_text(&state, 80, 24);
        assert!(text.contains("↑↓ navegar"), "{text}");
        assert!(text.contains("enter aplica"), "{text}");
        assert!(text.contains("esc fecha"), "{text}");
        assert!(text.contains("1/40"), "{text}");
    }

    #[test]
    fn overlay_hints_follow_what_enter_and_d_actually_do() {
        let mut state = ChatState::default();
        state.open_overlay_with_ids(
            " memória ",
            vec!["fato um".to_string()],
            vec!["id-1".to_string()],
        );
        let text = overlay_text(&state, 80, 24);
        assert!(text.contains("d esquece"), "{text}");
        assert!(!text.contains("enter aplica"), "{text}");
        assert!(!text.contains("1/1"), "{text}");
    }
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let width = area.width * percent_x / 100;
    let height = area.height * percent_y / 100;

    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Idle => "pronto",
        Status::Streaming => "gerando…",
        Status::Busy => "ocupado",
    }
}

#[allow(dead_code)]
fn transcript_rows(
    lines: &[ChatEntry],
    width: u16,
    processing: bool,
    cache: &mut crate::markdown::Cache,
) -> Vec<Line<'static>> {
    transcript_rows_at(lines, width, processing, cache, std::time::Instant::now())
}

#[allow(dead_code)]
fn transcript_rows_at(
    lines: &[ChatEntry],
    width: u16,
    processing: bool,
    cache: &mut crate::markdown::Cache,
    now: std::time::Instant,
) -> Vec<Line<'static>> {
    // Fonte única: a seleção por arraste precisa das MESMAS linhas que o
    // desenho produz. Manter aqui uma segunda cópia do render já fez o texto
    // copiado divergir do exibido quando o formato das tools mudou.
    transcript_layout_at(lines, width, processing, cache, now).rows
}

pub(crate) struct Transcript {
    pub rows: Vec<Line<'static>>,
    #[allow(dead_code)]
    pub owners: Vec<Option<usize>>,
}

#[allow(dead_code)]
fn transcript_layout_at(
    lines: &[ChatEntry],
    width: u16,
    processing: bool,
    cache: &mut crate::markdown::Cache,
    now: std::time::Instant,
) -> Transcript {
    /// Glifo e cor por categoria de tool: leitura por tipo sem gastar
    /// colunas. Só caracteres de largura 1, porque a TUI depende do terminal.
    fn tool_glyph(name: &str) -> (&'static str, Color) {
        match name {
            "read" | "read_many" | "search" | "map" | "trace" | "impact" | "why" | "peek"
            | "docs" | "context_graph" | "facts" | "diff" | "assert" | "websearch" | "fetch"
            | "research" | "env" => ("◆", Color::Cyan),
            "edit" | "write" | "write_many" | "patch" | "relocate" => ("▪", Color::Yellow),
            "exec" | "shell" | "test_many" | "setup" => ("▸", Color::Magenta),
            "agent" => ("◇", Color::Cyan),
            _ => ("⚙", Color::DarkGray),
        }
    }

    /// Spinner de tool em voo: braille é largura 1 e não pisca como `…`.
    fn spinner(now: std::time::Instant, started: std::time::Instant) -> &'static str {
        const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
        let tick = now.saturating_duration_since(started).as_millis() / 120;
        FRAMES[(tick as usize) % FRAMES.len()]
    }

    let area_width = width as usize;
    if area_width < 8 {
        return Transcript {
            rows: Vec::new(),
            owners: Vec::new(),
        };
    }

    let indent = 2usize;
    let content_width = area_width.saturating_sub(indent).max(1);
    let compact = area_width < 60;
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<usize>> = Vec::new();
    rows.push(Line::from(""));
    owners.push(None);

    // Uma "run" são as `ChatEntry::Tool` consecutivas: o lote de trabalho que
    // o usuário lê junto. Runs de 3+ ganham cabeçalho com placar.
    let mut previous_was_tool = false;
    let mut group_open = false;

    for (index, entry) in lines.iter().enumerate() {
        if index > 0 {
            rows.push(Line::from(""));
            owners.push(None);
        }

        let is_tool = matches!(entry, ChatEntry::Tool { .. });
        let continuing_run = previous_was_tool && is_tool;
        previous_was_tool = is_tool;
        if !is_tool {
            group_open = false;
        }

        let (role, text, metrics, pending) = match entry {
            ChatEntry::Message {
                role,
                content,
                metrics,
            } => (*role, content.as_str(), metrics.as_ref(), false),
            ChatEntry::Queued { content } => (Role::User, content.as_str(), None, true),
            ChatEntry::Tool {
                call_id,
                name,
                summary,
                ok,
                started,
                duration,
            } => {
                if !continuing_run {
                    let run_len = lines[index..]
                        .iter()
                        .take_while(|entry| matches!(entry, ChatEntry::Tool { .. }))
                        .count();
                    if run_len >= 3 {
                        let run = &lines[index..index + run_len];
                        let top: Vec<&ChatEntry> = run
                            .iter()
                            .filter(|entry| {
                                matches!(entry, ChatEntry::Tool { call_id, .. } if !call_id.contains('#'))
                            })
                            .collect();
                        let total = top.len();
                        let done = top
                            .iter()
                            .filter(|entry| matches!(entry, ChatEntry::Tool { ok: Some(_), .. }))
                            .count();
                        let failures = top
                            .iter()
                            .filter(|entry| {
                                matches!(
                                    entry,
                                    ChatEntry::Tool {
                                        ok: Some(false),
                                        ..
                                    }
                                )
                            })
                            .count();
                        // O total conta só chamadas de topo: as de pipeline
                        // (id com `#`) já estão representadas pelo pai.
                        let placar = if done < total {
                            format!("{done} de {total}")
                        } else if failures > 0 {
                            format!("{}✓ {failures}✗", total - failures)
                        } else {
                            format!("{total}✓")
                        };
                        rows.push(Line::from(vec![
                            Span::styled(
                                "  ⚙ ",
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("{total} tools · "),
                                Style::default().fg(Color::Gray),
                            ),
                            Span::styled(placar, Style::default().fg(Color::DarkGray)),
                        ]));
                        owners.push(Some(index));
                        group_open = true;
                    }
                }

                let depth = call_id.matches('#').count();
                let extra = if group_open { 2 } else { 0 };
                let lead = format!(
                    "  {}{}{}",
                    " ".repeat(extra),
                    " ".repeat(2 * depth),
                    if depth > 0 { "└ " } else { "" }
                );
                let (glyph, glyph_color) = tool_glyph(name);
                let (mark, mark_color) = match ok {
                    Some(true) => ("✓", Color::Green),
                    Some(false) => ("✗", Color::Red),
                    None => (spinner(now, *started), Color::Yellow),
                };
                // Em voo o tempo é vivo; concluída, é a duração medida por
                // quem executou — nunca reinventada aqui.
                let elapsed = duration.unwrap_or_else(|| now.saturating_duration_since(*started));
                let time = if compact {
                    String::new()
                } else {
                    crate::metrics::format_duration(elapsed)
                };

                let lead_width = lead.chars().count();
                let time_width = time.chars().count();
                // Sobra para o resumo: prefixo + glifo + espaço + tempo +
                // espaço + marcador, com 1 coluna de respiro.
                let reserved = lead_width + 1 + 1 + time_width + 1 + 1 + 1;
                let label_width = content_width.saturating_sub(reserved).max(8);
                let label = format!("{name} · {summary}");
                let line = wrap_text(&label, label_width)
                    .into_iter()
                    .next()
                    .unwrap_or_default();

                let used = lead_width + 1 + 1 + line.chars().count() + 1 + 1;
                let slack = content_width.saturating_sub(used);
                let show_time = !time.is_empty() && slack > time_width;
                let mut spans = vec![
                    Span::styled(lead, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        glyph,
                        Style::default()
                            .fg(glyph_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::raw(line),
                ];
                if show_time {
                    spans.push(Span::raw(" ".repeat(slack - time_width)));
                    spans.push(Span::styled(time, Style::default().fg(Color::DarkGray)));
                }
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    mark.to_string(),
                    Style::default().fg(mark_color),
                ));
                rows.push(Line::from(spans));
                owners.push(Some(index));
                continue;
            }
            ChatEntry::Memory { text } => {
                let mut first = true;
                for line in wrap_text(text, content_width.saturating_sub(4)) {
                    let prefix = if first { "  ◆ " } else { "    " };
                    rows.push(Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            line,
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::DIM),
                        ),
                    ]));
                    owners.push(Some(index));
                    first = false;
                }
                continue;
            }
            ChatEntry::Prompt { content } => {
                rows.push(Line::from(vec![
                    Span::styled(
                        "  ◆ ",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "prompt enviado",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                owners.push(Some(index));
                for line in plain_rows(content, content_width) {
                    rows.push(indent_line(line, indent));
                    owners.push(Some(index));
                }
                continue;
            }
        };

        let (marker, label, color) = match role {
            Role::User if pending => ("❯", "você · pendente", Color::DarkGray),
            Role::User => ("❯", "você", Color::Cyan),
            Role::Assistant => ("●", "tupi", Color::Green),
            Role::System => ("◆", "sistema", Color::Yellow),
            Role::Tool => ("⚙", "tool", Color::DarkGray),
        };

        let mut header = vec![
            Span::styled(
                marker.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                label.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ];
        if role == Role::Assistant
            && let Some(metrics) = metrics
        {
            header.push(Span::styled(
                format!("  {}", metrics.header(now, compact)),
                Style::default().fg(Color::DarkGray),
            ));
        }
        rows.push(Line::from(header));
        owners.push(Some(index));

        let body = match role {
            Role::User | Role::Assistant => cache.rows(index, text, content_width).to_vec(),
            Role::System => markdown_rows(text, content_width),
            Role::Tool => plain_rows(text, content_width),
        };

        for line in body {
            rows.push(indent_line(line, indent));
            owners.push(Some(index));
        }
    }

    if processing {
        if !rows.last().map(|row| row.width() == 0).unwrap_or(true) {
            rows.push(Line::from(""));
            owners.push(None);
        }
        rows.push(Line::from(Span::styled(
            "  processando...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        )));
        owners.push(None);
    }

    rows.push(Line::from(""));
    owners.push(None);

    Transcript { rows, owners }
}

#[allow(dead_code)]
fn hit_test(
    owners: &[Option<usize>],
    start: usize,
    area: Rect,
    pointer: Option<(u16, u16)>,
) -> Option<usize> {
    let (x, y) = pointer?;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    if x < area.x
        || x >= area.x.saturating_add(area.width)
        || y < area.y
        || y >= area.y.saturating_add(area.height)
    {
        return None;
    }

    let index = start + (y - area.y) as usize;
    owners.get(index).copied().flatten()
}

#[allow(dead_code)]
fn indent_line(mut line: Line<'static>, indent: usize) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(indent))];
    spans.append(&mut line.spans);
    Line::from(spans).style(line.style)
}

fn plain_rows(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut rows = Vec::new();

    for raw in text.lines() {
        if raw.is_empty() {
            rows.push(Line::from(""));
            continue;
        }
        for part in wrap_text(raw, width) {
            rows.push(Line::from(part));
        }
    }

    if rows.is_empty() {
        rows.push(Line::from(""));
    }

    rows
}

fn markdown_rows(text: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();

    for raw in text.lines() {
        let trimmed = raw.trim_end();
        let trimmed_start = trimmed.trim_start();

        if let Some(language) = trimmed_start.strip_prefix("```") {
            flush_paragraph(&mut rows, &mut paragraph, width);
            if in_code {
                in_code = false;
                code_lang.clear();
            } else {
                in_code = true;
                code_lang = language.trim().to_string();
                if !code_lang.is_empty() {
                    rows.push(Line::from(Span::styled(
                        format!("  {code_lang}"),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
            continue;
        }

        if in_code {
            rows.extend(code_rows(trimmed, width));
            continue;
        }

        if trimmed_start.is_empty() {
            flush_paragraph(&mut rows, &mut paragraph, width);
            push_blank(&mut rows);
            continue;
        }

        if let Some((prefix, rest)) = list_prefix(trimmed_start) {
            flush_paragraph(&mut rows, &mut paragraph, width);
            rows.extend(prefixed_rows(&prefix, rest, width));
            continue;
        }

        if let Some(rest) = trimmed_start.strip_prefix("> ") {
            flush_paragraph(&mut rows, &mut paragraph, width);
            rows.extend(quote_rows(rest, width));
            continue;
        }

        if is_rule(trimmed_start) {
            flush_paragraph(&mut rows, &mut paragraph, width);
            rows.push(Line::from(Span::styled(
                "─".repeat(width),
                Style::default().fg(Color::DarkGray),
            )));
            continue;
        }

        if let Some((level, rest)) = heading(trimmed_start) {
            flush_paragraph(&mut rows, &mut paragraph, width);
            let color = match level {
                1 | 2 => Color::Cyan,
                3 => Color::Blue,
                _ => Color::Magenta,
            };
            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            for part in wrap_text(rest, width) {
                rows.push(Line::from(inline_spans(&part)).style(style));
            }
            continue;
        }

        paragraph.push(trimmed_start);
    }

    flush_paragraph(&mut rows, &mut paragraph, width);

    if rows.is_empty() {
        rows.push(Line::from(""));
    }

    rows
}

fn flush_paragraph(rows: &mut Vec<Line<'static>>, paragraph: &mut Vec<&str>, width: usize) {
    if paragraph.is_empty() {
        return;
    }

    let joined = paragraph.join(" ");
    paragraph.clear();

    for part in wrap_text(&joined, width) {
        rows.push(Line::from(inline_spans(&part)));
    }
}

fn push_blank(rows: &mut Vec<Line<'static>>) {
    if rows.last().map(|row| row.width() == 0).unwrap_or(false) {
        return;
    }
    rows.push(Line::from(""));
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(1..=6).contains(&level) {
        return None;
    }

    let rest = line[level..].strip_prefix(' ')?;
    let title = rest.trim();
    if title.is_empty() {
        return None;
    }

    Some((level, title))
}

fn is_rule(line: &str) -> bool {
    line.len() >= 3
        && line
            .chars()
            .all(|character| matches!(character, '-' | '*' | '_'))
}

fn list_prefix(line: &str) -> Option<(String, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(("• ".to_string(), rest));
        }
    }

    let digits = line
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    let ordered = line
        .get(digits..)
        .and_then(|rest| rest.strip_prefix(". "))
        .filter(|_| digits > 0);
    if let Some(rest) = ordered {
        return Some((format!("{}. ", &line[..digits]), rest));
    }

    None
}

fn prefixed_rows(prefix: &str, content: &str, width: usize) -> Vec<Line<'static>> {
    let prefix_width = prefix.width();
    let available = width.saturating_sub(prefix_width).max(1);
    let wrapped = wrap_text(content, available);
    let continuation = " ".repeat(prefix_width);
    let marker_style = Style::default().fg(Color::Cyan);
    let mut rows = Vec::new();

    for (index, part) in wrapped.iter().enumerate() {
        let lead = if index == 0 {
            prefix.to_string()
        } else {
            continuation.clone()
        };
        let mut spans = vec![Span::styled(lead, marker_style)];
        spans.extend(inline_spans(part));
        rows.push(Line::from(spans));
    }

    if rows.is_empty() {
        rows.push(Line::from(Span::styled(prefix.to_string(), marker_style)));
    }

    rows
}

fn quote_rows(content: &str, width: usize) -> Vec<Line<'static>> {
    let available = width.saturating_sub(2).max(1);
    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);

    wrap_text(content, available)
        .into_iter()
        .map(|part| Line::from(vec![Span::styled("│ ", style), Span::styled(part, style)]))
        .collect()
}

fn code_rows(line: &str, width: usize) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(Color::Indexed(252))
        .bg(Color::Indexed(236));

    if width < 6 {
        let clipped: String = line.chars().take(width).collect();
        return vec![Line::from(Span::styled(clipped, style))];
    }

    let inner = width.saturating_sub(4).max(1);

    if line.is_empty() {
        return vec![Line::from(Span::styled(" ".repeat(width), style))];
    }

    split_word(line, inner)
        .into_iter()
        .map(|chunk| {
            let padding = " ".repeat(inner.saturating_sub(chunk.width()));
            Line::from(Span::styled(format!("  {chunk}{padding}  "), style))
        })
        .collect()
}

fn inline_spans(text: &str) -> Vec<Span<'static>> {
    let characters: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];

        let code_end = if character == '`' {
            find_char(&characters, index + 1, '`')
        } else {
            None
        };
        if let Some(end) = code_end {
            push_plain(&mut spans, &mut plain);
            let code: String = characters[index + 1..end].iter().collect();
            spans.push(Span::styled(code, Style::default().fg(Color::Yellow)));
            index = end + 1;
            continue;
        }

        let bold_end =
            if character == '*' && index + 1 < characters.len() && characters[index + 1] == '*' {
                find_double_star(&characters, index + 2)
            } else {
                None
            };
        if let Some(end) = bold_end {
            push_plain(&mut spans, &mut plain);
            let bold: String = characters[index + 2..end].iter().collect();
            spans.push(Span::styled(
                bold,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            index = end + 2;
            continue;
        }

        let italic_end = if character == '*' {
            find_single_star(&characters, index + 1)
        } else {
            None
        };
        if let Some(end) = italic_end {
            push_plain(&mut spans, &mut plain);
            let italic: String = characters[index + 1..end].iter().collect();
            spans.push(Span::styled(
                italic,
                Style::default().add_modifier(Modifier::ITALIC),
            ));
            index = end + 1;
            continue;
        }

        plain.push(character);
        index += 1;
    }

    push_plain(&mut spans, &mut plain);

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }

    spans
}

fn push_plain(spans: &mut Vec<Span<'static>>, plain: &mut String) {
    if !plain.is_empty() {
        spans.push(Span::raw(std::mem::take(plain)));
    }
}

fn find_char(characters: &[char], from: usize, target: char) -> Option<usize> {
    (from..characters.len()).find(|index| characters[*index] == target)
}

fn find_double_star(characters: &[char], from: usize) -> Option<usize> {
    let mut index = from;
    while index + 1 < characters.len() {
        if characters[index] == '*' && characters[index + 1] == '*' {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn find_single_star(characters: &[char], from: usize) -> Option<usize> {
    let mut index = from;
    while index < characters.len() {
        if characters[index] == '*'
            && (index + 1 >= characters.len() || characters[index + 1] != '*')
            && (index == 0 || characters[index - 1] != '*')
        {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Quebra o rascunho do composer preservando cada espaço digitado.
///
/// `wrap_text` colapsa espaços (`split_whitespace`), o que serve ao
/// transcript mas não a um editor: com `"oi "` a última linha virava `"oi"`,
/// o cursor não andava e a tecla espaço parecia não ter efeito até a letra
/// seguinte. Aqui a quebra continua preferindo fim de palavra (o espaço fica
/// no fim da linha anterior), mas todo espaço conta coluna.
fn wrap_input(text: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;

    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for character in text.chars() {
        // Colagem multilinha não tem linha própria no composer de uma linha
        // só: vira espaço, como já acontecia na quebra por palavras.
        let character = match character {
            '\n' | '\r' => ' ',
            other => other,
        };
        let character_width = character.width().unwrap_or(0);

        if current_width > 0 && current_width + character_width > width {
            // Quebra no último espaço para não cortar palavra ao meio; sem
            // espaço, corta no limite mesmo.
            let last_space = current
                .char_indices()
                .rev()
                .find(|(index, candidate)| *index > 0 && *candidate == ' ')
                .map(|(index, _)| index);
            match last_space {
                Some(index) => {
                    let rest = current.split_off(index + 1);
                    lines.push(std::mem::take(&mut current));
                    current = rest;
                }
                None => lines.push(std::mem::take(&mut current)),
            }
            current_width = current.width();
        }

        current.push(character);
        current_width += character_width;
    }

    lines.push(current);
    lines
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = word.width();

        if current_width > 0 && current_width + 1 + word_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }

        if word_width > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            for chunk in split_word(word, width) {
                let chunk_width = chunk.width();
                if current_width > 0 && current_width + 1 + chunk_width > width {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                if current_width == 0 && chunk_width == width {
                    lines.push(chunk);
                } else {
                    if current_width > 0 {
                        current.push(' ');
                        current_width += 1;
                    }
                    current.push_str(&chunk);
                    current_width += chunk_width;
                }
            }
            continue;
        }

        if current_width > 0 {
            current.push(' ');
            current_width += 1;
        }
        current.push_str(word);
        current_width += word_width;
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

fn split_word(word: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;

    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for character in word.chars() {
        let character_width = character.width().unwrap_or(0);
        if current_width > 0 && current_width + character_width > width {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(character);
        current_width += character_width;
    }

    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(row: &Line<'_>) -> String {
        row.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn message(role: Role, text: &str) -> ChatEntry {
        ChatEntry::Message {
            role,
            content: text.to_string(),
            metrics: None,
        }
    }

    fn transcript_text(state: &ChatState) -> String {
        let backend = ratatui::backend::TestBackend::new(60, 8);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_transcript(frame, state, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..8)
            .map(|y| {
                (0..60u16)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_transcript_teaches_how_to_start_and_steps_aside() {
        let text = transcript_text(&ChatState::default());
        assert!(text.contains("tupi"), "{text}");
        assert!(text.contains("/workflows lista os procedimentos"), "{text}");

        let mut state = ChatState::default();
        state.push_user("oi");
        let text = transcript_text(&state);
        assert!(!text.contains("aponte o caminho de um arquivo"), "{text}");
    }

    #[test]
    fn transcript_marks_user_and_assistant_messages() {
        let lines = vec![message(Role::User, "oi"), message(Role::Assistant, "olá")];
        let rows = transcript_rows(&lines, 40, false, &mut crate::markdown::Cache::default());
        let texts = rows.iter().map(row_text).collect::<Vec<_>>();

        assert!(texts.iter().any(|text| text.starts_with("❯")));
        assert!(texts.iter().any(|text| text.starts_with("●")));
        assert!(texts.iter().any(|text| text.contains("oi")));
        assert!(texts.iter().any(|text| text.contains("olá")));
    }

    #[test]
    fn user_messages_render_markdown() {
        let lines = vec![message(Role::User, "# Título\n\nisso é **forte**")];
        let rows = transcript_rows(&lines, 40, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("Título"));
        assert!(!text.contains('#'));
        assert!(rows.iter().flat_map(|row| row.spans.iter()).any(|span| {
            span.content == "forte" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
    }

    #[test]
    fn wide_characters_stay_within_width() {
        let lines = vec![
            message(Role::User, "😀😀😀😀😀"),
            message(Role::Assistant, "resposta ✅ com emoji 🚀"),
        ];
        let rows = transcript_rows(&lines, 40, false, &mut crate::markdown::Cache::default());

        for row in &rows {
            assert!(row.width() <= 40, "linha larga: {}", row_text(row));
        }
    }

    #[test]
    fn processing_line_appears_while_waiting() {
        let lines = vec![message(Role::User, "oi")];
        let rows = transcript_rows(&lines, 40, true, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("processando..."));
    }

    #[test]
    fn tool_entries_render_as_a_compact_line() {
        let now = std::time::Instant::now();
        let lines = vec![
            message(Role::User, "leia"),
            ChatEntry::Tool {
                call_id: "c1".into(),
                name: "read".into(),
                summary: "src/a.rs".into(),
                ok: Some(true),
                started: now,
                duration: Some(std::time::Duration::from_millis(250)),
            },
        ];
        let rows = transcript_rows(&lines, 80, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("read · src/a.rs"), "{text}");
        assert!(text.contains('✓'), "{text}");
        // Categoria da tool: leitura usa ◆, não o ⚙ genérico.
        assert!(text.contains('◆'), "{text}");
        // A duração vem do evento e aparece alinhada à direita.
        assert!(text.contains("0,2s"), "{text}");
    }

    #[test]
    fn subagent_tools_render_indented_under_the_agent_line() {
        let now = std::time::Instant::now();
        let lines = vec![
            ChatEntry::Tool {
                call_id: "sdd#T1.1".into(),
                name: "agent".into(),
                summary: "T1.1 — Parser".into(),
                ok: None,
                started: now,
                duration: None,
            },
            ChatEntry::Tool {
                call_id: "sdd#T1.1#1.0".into(),
                name: "read".into(),
                summary: "src/a.rs".into(),
                ok: Some(true),
                started: now,
                duration: Some(std::time::Duration::from_millis(300)),
            },
        ];
        let rows = transcript_layout_at(
            &lines,
            100,
            false,
            &mut crate::markdown::Cache::default(),
            now,
        );
        let texts: Vec<String> = rows.rows.iter().map(row_text).collect();
        let agent = texts
            .iter()
            .find(|row| row.contains("agent · T1.1"))
            .unwrap();
        let tool = texts
            .iter()
            .find(|row| row.contains("read · src/a.rs"))
            .unwrap();
        let indent = |row: &str| row.chars().take_while(|ch| *ch == ' ').count();

        // O subagente tem glifo próprio e a tool dele desce um nível.
        assert!(agent.contains('◇'), "{agent}");
        assert!(indent(tool) > indent(agent), "{agent:?} vs {tool:?}");
    }

    #[test]
    fn tool_run_of_three_gains_a_header_with_live_placar() {
        let now = std::time::Instant::now();
        let mut lines = vec![message(Role::User, "vai")];
        let tool = |id: &str, name: &str, ok: Option<bool>| ChatEntry::Tool {
            call_id: id.into(),
            name: name.into(),
            summary: "x".into(),
            ok,
            started: now,
            duration: ok.map(|_| std::time::Duration::from_millis(100)),
        };
        lines.push(tool("p1", "pipeline", Some(false)));
        lines.push(tool("p1#0.0", "read", Some(true)));
        lines.push(tool("c2", "exec", Some(true)));

        let rows = transcript_rows(&lines, 80, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        // O placar conta só o pai: a sub-chamada p1#0.0 não infla o total.
        assert!(text.contains("2 tools"), "{text}");
        assert!(text.contains("1✓ 1✗"), "{text}");
        // A sub-chamada aparece indentada sob o pai.
        assert!(text.contains("└ "), "{text}");
    }

    #[test]
    fn short_tool_run_has_no_group_header() {
        let now = std::time::Instant::now();
        let lines = vec![
            message(Role::User, "leia"),
            ChatEntry::Tool {
                call_id: "c1".into(),
                name: "read".into(),
                summary: "a.rs".into(),
                ok: Some(true),
                started: now,
                duration: Some(std::time::Duration::from_millis(100)),
            },
            ChatEntry::Tool {
                call_id: "c2".into(),
                name: "read".into(),
                summary: "b.rs".into(),
                ok: Some(true),
                started: now,
                duration: Some(std::time::Duration::from_millis(100)),
            },
        ];
        let rows = transcript_rows(&lines, 80, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(!text.contains("tools ·"), "{text}");
    }

    #[test]
    fn tool_in_flight_shows_spinner_and_elapsed_time() {
        let started = std::time::Instant::now();
        let lines = vec![ChatEntry::Tool {
            call_id: "c1".into(),
            name: "exec".into(),
            summary: "cargo test".into(),
            ok: None,
            started,
            duration: None,
        }];
        let early = transcript_layout_at(
            &lines,
            100,
            false,
            &mut crate::markdown::Cache::default(),
            started,
        );
        let later = transcript_layout_at(
            &lines,
            100,
            false,
            &mut crate::markdown::Cache::default(),
            started + std::time::Duration::from_secs(2),
        );
        let early_text = early
            .rows
            .iter()
            .map(row_text)
            .collect::<Vec<_>>()
            .join("\n");
        let later_text = later
            .rows
            .iter()
            .map(row_text)
            .collect::<Vec<_>>()
            .join("\n");

        // O tempo cresce sem evento novo: quem muda é o now do tick.
        assert!(early_text.contains("0,0s"), "{early_text}");
        assert!(later_text.contains("2,0s"), "{later_text}");
        assert_ne!(early_text, later_text);
    }

    #[test]
    fn narrow_width_drops_the_time_but_keeps_the_tool_name() {
        let now = std::time::Instant::now();
        let lines = vec![ChatEntry::Tool {
            call_id: "c1".into(),
            name: "read".into(),
            summary: "src/a.rs".into(),
            ok: Some(true),
            started: now,
            duration: Some(std::time::Duration::from_secs(3)),
        }];
        let rows = transcript_rows(&lines, 40, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("read · src/a.rs"), "{text}");
        assert!(!text.contains("3,0s"), "tempo não deveria aparecer: {text}");
    }

    #[test]
    fn draw_details_shows_tool_placar_beside_context() {
        let mut state = ChatState::default();
        state.set_context(28_800, 32_000);
        for id in ["c1", "c2", "c3"] {
            state.push_tool_started(id, "read", "a.rs");
            state.finish_tool(id, id != "c2", None);
        }

        let backend = ratatui::backend::TestBackend::new(120, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_details(frame, &state, "modelo", "prov", frame.area());
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..120).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(row.contains("⚙ 3 1✗"), "{row}");
        assert!(row.contains("ctx 28.8k/32k"), "{row}");
    }

    #[test]
    fn wrapping_keeps_words_within_width() {
        assert_eq!(split_word("abcdefgh", 4), vec!["abcd", "efgh"]);
        assert_eq!(
            wrap_text("uma frase um pouco maior", 10),
            vec!["uma frase", "um pouco", "maior"]
        );
        assert_eq!(wrap_text("😀😀😀", 5), vec!["😀😀", "😀"]);
    }

    #[test]
    fn markdown_headings_and_code_are_styled() {
        let text = "# Título\n\n```rust\nfn main() {}\n```\n";
        let rows = markdown_rows(text, 30);
        let joined = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(joined.contains("Título"));
        assert!(!joined.contains('#'));
        assert!(joined.contains("fn main()"));

        let code_row = rows
            .iter()
            .find(|row| row_text(row).contains("fn main()"))
            .expect("linha de código");
        assert_eq!(code_row.spans[0].style.bg, Some(Color::Indexed(236)));
    }

    #[test]
    fn inline_markdown_styles_bold_and_code() {
        let spans = inline_spans("isso é **forte** e `código`");

        assert!(spans.iter().any(|span| {
            span.content == "forte" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        assert!(
            spans
                .iter()
                .any(|span| { span.content == "código" && span.style.fg == Some(Color::Yellow) })
        );
    }

    #[test]
    fn visible_range_clamps_scroll() {
        assert_eq!(visible_range(100, 10, 0), (90, 100));
        assert_eq!(visible_range(100, 10, 5), (85, 95));
        assert_eq!(visible_range(100, 10, 1000), (0, 10));
        assert_eq!(visible_range(3, 10, 0), (0, 3));
    }

    fn assistant_with_metrics(text: &str, t0: std::time::Instant) -> ChatEntry {
        let mut metrics = crate::metrics::ReplyMetrics::start(t0);
        metrics.add_estimated(340);
        metrics.mark_first_token(t0 + std::time::Duration::from_secs(1));
        metrics.finish(t0 + std::time::Duration::from_secs(11));
        metrics.set_remote(320);
        ChatEntry::Message {
            role: Role::Assistant,
            content: text.to_string(),
            metrics: Some(metrics),
        }
    }

    #[test]
    fn assistant_header_shows_elapsed_and_rates() {
        let t0 = std::time::Instant::now();
        let lines = vec![assistant_with_metrics("oi", t0)];
        let rows = transcript_rows_at(
            &lines,
            80,
            false,
            &mut crate::markdown::Cache::default(),
            t0,
        );
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("11,0s · 1º 1,0s · ≈34 → 32 tok/s"), "{text}");
    }

    #[test]
    fn assistant_header_is_compact_on_narrow_width() {
        let t0 = std::time::Instant::now();
        let lines = vec![assistant_with_metrics("oi", t0)];
        let rows = transcript_rows_at(
            &lines,
            40,
            false,
            &mut crate::markdown::Cache::default(),
            t0,
        );
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("≈34→32 t/s"), "{text}");
    }

    #[test]
    fn header_without_usage_uses_estimate() {
        let t0 = std::time::Instant::now();
        let mut metrics = crate::metrics::ReplyMetrics::start(t0);
        metrics.add_estimated(340);
        metrics.mark_first_token(t0 + std::time::Duration::from_secs(1));
        metrics.finish(t0 + std::time::Duration::from_secs(11));
        let lines = vec![ChatEntry::Message {
            role: Role::Assistant,
            content: "oi".into(),
            metrics: Some(metrics),
        }];
        let rows = transcript_rows_at(
            &lines,
            80,
            false,
            &mut crate::markdown::Cache::default(),
            t0,
        );
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("≈34 tok/s (est.)"), "{text}");
    }

    #[test]
    fn prompt_entries_render_as_a_debug_block() {
        let lines = vec![ChatEntry::Prompt {
            content: "modelo: m\n\n[user]\noi".into(),
        }];
        let rows = transcript_rows(&lines, 60, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("prompt enviado"), "{text}");
        assert!(text.contains("modelo: m"), "{text}");
        assert!(text.contains("[user]"), "{text}");
    }

    #[test]
    fn long_prompt_wraps_inside_the_composer() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(24, 5)).unwrap();
        let mut state = ChatState::default();
        state.input = "uma mensagem longa que nao cabe".into();

        terminal
            .draw(|frame| draw_composer(frame, &state, Rect::new(0, 0, 24, 5), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rows = (0..5)
            .map(|y| (0..24).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>();

        assert!(rows[1].contains("uma mensagem longa"), "{rows:?}");
        assert!(rows[2].contains("que nao cabe"), "{rows:?}");
        terminal.backend_mut().assert_cursor_position((15, 2));
    }

    /// O espaço digitado no fim do rascunho precisa andar o cursor na hora. A
    /// quebra por palavras descartava o espaço final, então `"oi "` desenhava
    /// `"oi"` e a tecla só surtia efeito quando a próxima letra chegava.
    #[test]
    fn trailing_space_moves_the_composer_cursor() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(24, 3)).unwrap();
        let mut state = ChatState::default();
        state.input = "oi ".into();

        terminal
            .draw(|frame| draw_composer(frame, &state, Rect::new(0, 0, 24, 3), false))
            .unwrap();

        // borda + "› " + "oi" e a coluna do espaço recém-digitado.
        terminal.backend_mut().assert_cursor_position((6, 1));
    }

    #[test]
    fn repeated_spaces_are_rendered_in_the_composer() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(24, 3)).unwrap();
        let mut state = ChatState::default();
        state.input = "a  b".into();

        terminal
            .draw(|frame| draw_composer(frame, &state, Rect::new(0, 0, 24, 3), false))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row = (0..24).map(|x| buffer[(x, 1)].symbol()).collect::<String>();
        assert!(row.contains("a  b"), "{row:?}");
        // borda + "› " + "a  b": o segundo espaço também anda o cursor.
        terminal.backend_mut().assert_cursor_position((7, 1));
    }

    #[test]
    fn queued_messages_render_as_pending_user_entries() {
        let lines = vec![ChatEntry::Queued {
            content: "depois".into(),
        }];
        let rows = transcript_rows(&lines, 40, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("pendente"), "{text}");
        assert!(text.contains("depois"), "{text}");
    }

    #[tokio::test]
    async fn draining_sends_the_queued_message_and_starts_the_turn() {
        struct Recorder(std::sync::Mutex<Vec<Command>>);

        #[async_trait::async_trait]
        impl tupi_kernel::CommandHandler for Recorder {
            async fn handle(&self, command: Command) -> Result<CommandOutcome, CommandError> {
                self.0.lock().unwrap().push(command);
                Ok(CommandOutcome::Started {
                    request_id: Default::default(),
                })
            }
        }

        let handler = std::sync::Arc::new(Recorder(std::sync::Mutex::new(Vec::new())));
        let bus = tupi_kernel::CommandBus::new(handler.clone());
        let mut state = ChatState::default();
        state.enqueue("próxima");

        let request = drain_queue(&bus, &mut state).await;

        assert!(request.is_some());
        assert!(matches!(state.status, Status::Streaming));
        assert!(matches!(
            state.lines.last(),
            Some(ChatEntry::Message { role: Role::User, content, .. }) if content == "próxima"
        ));
        assert!(
            !state
                .lines
                .iter()
                .any(|entry| matches!(entry, ChatEntry::Queued { .. }))
        );
        assert!(matches!(
            handler.0.lock().unwrap().as_slice(),
            [Command::SendMessage { text }] if text == "próxima"
        ));
    }

    #[test]
    fn notice_lines_sanitizes_control_chars_and_wraps_cleanly() {
        let raw = "erro HTTP 400:\n{\n  \"error\": {\n    \"message\": \"role function is not supported neste endpoint\"\n  }\n}";
        let lines = notice_lines(raw, 40, 2);
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let text = row_text(line);
            assert!(!text.contains('\n'));
            assert!(!text.contains('\r'));
            assert!(text.width() <= 40);
        }
        assert!(row_text(lines.last().unwrap()).ends_with('…'));
    }

    #[test]
    fn draw_details_shows_context_in_bottom_right() {
        let mut state = ChatState::default();
        state.set_context(12_450, 32_000);

        let backend = ratatui::backend::TestBackend::new(100, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_details(frame, &state, "gemini-4", "antigravity", frame.area());
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..100).map(|x| buffer[(x, 0)].symbol()).collect();
        let trimmed = row.trim_end();
        assert!(
            trimmed.ends_with("ctx 12.4k/32k (39%)"),
            "esperava contexto alinhado à direita, veio: {trimmed:?}"
        );
        assert!(row.contains("gemini-4"));
    }

    #[test]
    fn draw_details_prioritizes_context_over_hints_when_narrow() {
        let mut state = ChatState::default();
        state.set_context(28_800, 32_000);

        let backend = ratatui::backend::TestBackend::new(65, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_details(frame, &state, "modelo", "prov", frame.area());
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..65).map(|x| buffer[(x, 0)].symbol()).collect();
        let trimmed = row.trim_end();
        assert!(
            trimmed.ends_with("ctx 28.8k/32k (90%)"),
            "esperava contexto visível mesmo em tela estreita, veio: {trimmed:?}"
        );
        let col = (0..65u16)
            .find(|&x| buffer[(x, 0)].symbol() == "9" && buffer[(x + 1, 0)].symbol() == "0")
            .unwrap();
        assert_eq!(buffer[(col, 0)].fg, Color::Red);
    }

    #[test]
    fn draw_details_formats_unbounded_context_and_yellow_threshold() {
        let mut state = ChatState::default();
        state.set_context(12_450, 0);

        let backend = ratatui::backend::TestBackend::new(80, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_details(frame, &state, "modelo", "prov", frame.area());
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let row: String = (0..80).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(row.trim_end().ends_with("ctx 12.4k tokens"), "{row:?}");

        state.set_context(24_000, 32_000);
        terminal
            .draw(|frame| {
                draw_details(frame, &state, "modelo", "prov", frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let col = (0..80u16)
            .find(|&x| buffer[(x, 0)].symbol() == "7" && buffer[(x + 1, 0)].symbol() == "5")
            .unwrap();
        assert_eq!(buffer[(col, 0)].fg, Color::Yellow);
    }

    #[test]
    fn draw_details_renders_sdd_badge_when_in_sdd_mode() {
        let mut state = ChatState::default();
        let backend = ratatui::backend::TestBackend::new(100, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_details(frame, &state, "modelo", "prov", frame.area()))
            .unwrap();
        let exec_row: String = (0..100)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(exec_row.contains("[EXEC]"), "{exec_row:?}");

        state.apply_mode_changed(tupi_kernel::AgentMode::Sdd);
        terminal
            .draw(|frame| draw_details(frame, &state, "modelo", "prov", frame.area()))
            .unwrap();
        let sdd_row: String = (0..100)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(sdd_row.contains("[SDD]"), "{sdd_row:?}");

        state.apply_mode_changed(tupi_kernel::AgentMode::Docs);
        terminal
            .draw(|frame| draw_details(frame, &state, "modelo", "prov", frame.area()))
            .unwrap();
        let docs_row: String = (0..100)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(docs_row.contains("[DOCS]"), "{docs_row:?}");
    }

    #[test]
    fn shift_tab_helper_blocks_toggle_when_processing_or_overlay_open() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut state = ChatState::default();
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        let plain_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        assert_eq!(should_toggle_mode(&state, backtab), ToggleAction::Dispatch);
        assert_eq!(
            should_toggle_mode(&state, shift_tab),
            ToggleAction::Dispatch
        );
        assert_eq!(should_toggle_mode(&state, plain_tab), ToggleAction::Ignore);

        state.begin_assistant();
        assert_eq!(
            should_toggle_mode(&state, backtab),
            ToggleAction::BusyNotice
        );

        state.finish_assistant();
        state.open_overlay("modelos", vec!["a".into()]);
        assert_eq!(should_toggle_mode(&state, backtab), ToggleAction::Ignore);
    }
}

#[cfg(test)]
mod composer_hint_tests {
    use super::*;

    fn rows(state: &ChatState, width: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, 5)).unwrap();
        terminal
            .draw(|frame| draw_composer(frame, state, Rect::new(0, 0, width, 5), false))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..5)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    }

    /// O composer tem que dizer o que o Enter faz ANTES da tecla: fora de Idle
    /// a mensagem entra na fila e comando `/` é recusado.
    #[test]
    fn composer_announces_queueing_while_a_turn_runs() {
        let idle = rows(&ChatState::default(), 48);
        assert!(
            idle.iter()
                .any(|row| row.contains("/models modelo") && row.contains("/sdd retoma")),
            "{idle:?}"
        );
        assert!(
            !idle.iter().any(|row| row.contains("enfileira")),
            "{idle:?}"
        );

        // `begin_assistant` é o caminho público que leva o estado a Streaming.
        let mut streaming = ChatState::default();
        streaming.begin_assistant();
        let busy = rows(&streaming, 48);
        assert!(
            busy.iter().any(|row| row.contains("enter enfileira")),
            "{busy:?}"
        );
        assert!(!busy.iter().any(|row| row.contains("/models")), "{busy:?}");

        // Mesmo com texto já digitado a legenda continua valendo.
        streaming.input = "segunda pergunta".into();
        let typed = rows(&streaming, 48);
        assert!(
            typed.iter().any(|row| row.contains("enter enfileira")),
            "{typed:?}"
        );
    }
}
