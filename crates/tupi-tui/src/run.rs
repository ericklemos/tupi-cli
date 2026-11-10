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

use crate::state::{ChatEntry, ChatState, Status};
use crate::terminal::TerminalGuard;

pub struct BusSet {
    pub commands: CommandBus,
    pub queries: QueryBus,
    pub events: EventBus,
}

const SCROLL_STEP: u16 = 1;
const SCROLL_WHEEL: u16 = 3;
const SCROLL_PAGE: u16 = 10;

pub async fn run(buses: BusSet, mut model: String, provider: String) -> anyhow::Result<()> {
    let mut terminal = TerminalGuard::enter()?;
    let mut key_events = EventStream::new();
    let mut events = buses.events.subscribe();
    let mut state = ChatState::default();
    let mut request: Option<RequestId> = None;

    loop {
        terminal.draw(|frame| draw(frame, &state, &model, &provider))?;

        tokio::select! {
            received = key_events.next() => {
                let Some(Ok(event)) = received else { continue };

                match event {
                    CrosstermEvent::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            break;
                        }

                        if state.overlay_open() {
                            match key.code {
                                KeyCode::Esc => state.close_overlay(),
                                KeyCode::Up => state.overlay_prev(),
                                KeyCode::Down => state.overlay_next(),
                                KeyCode::Enter => {
                                    if let Some(selected) = state.selected_item().map(str::to_string) {
                                        match buses
                                            .commands
                                            .dispatch(Command::SetModel { model: selected.clone() })
                                            .await
                                        {
                                            Ok(CommandOutcome::ModelChanged { model: changed }) => {
                                                model = changed.clone();
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

                        match key.code {
                            KeyCode::Tab => state.accept_suggestion(),
                            KeyCode::Enter if state.status == Status::Idle && !state.input.is_empty() => {
                                let input = state.input.trim().to_string();

                                if input.starts_with('/') {
                                    if input == "/models" {
                                        state.input.clear();
                                        state.refresh_suggestions();
                                        // A listagem vai ao provedor; o comando só dispara a busca.
                                        // O resultado chega por evento, sem travar o desenho da TUI.
                                        match buses.commands.dispatch(Command::RefreshModels).await {
                                            Ok(_) => {
                                                state.notice = Some("carregando modelos…".to_string())
                                            }
                                            Err(error) => state.fail(&error.to_string()),
                                        }
                                    } else {
                                        state.fail(&format!("comando desconhecido: {input}"));
                                    }
                                } else {
                                    let text = state.input.clone();
                                    match buses.commands.dispatch(Command::SendMessage { text: text.clone() }).await {
                                        Ok(CommandOutcome::Started { request_id }) => {
                                            state.input.clear();
                                            state.refresh_suggestions();
                                            state.push_user(&text);
                                            state.begin_assistant();
                                            state.scroll = 0;
                                            request = Some(request_id);
                                        }
                                        Ok(_) => {}
                                        Err(CommandError::Busy) => state.fail("aguarde o fim do stream"),
                                        Err(error) => state.fail(&error.to_string()),
                                    }
                                }
                            }
                            KeyCode::Esc => {
                                if let Some(request_id) = request {
                                    let cancelled = matches!(
                                        buses
                                            .commands
                                            .dispatch(Command::CancelStream { request_id })
                                            .await,
                                        Ok(CommandOutcome::Cancelled)
                                    );
                                    if cancelled {
                                        state.finish_assistant();
                                        request = None;
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
                                state.input.push(character);
                                state.refresh_suggestions();
                            }
                            _ => {}
                        }
                    }
                    CrosstermEvent::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => scroll_up(&mut state, SCROLL_WHEEL),
                        MouseEventKind::ScrollDown => scroll_down(&mut state, SCROLL_WHEEL),
                        _ => {}
                    },
                    _ => {}
                }
            }
            received = events.recv() => {
                match received {
                    Ok(Event::Token { request_id, text }) if Some(request_id) == request => {
                        state.append_token(&text);
                    }
                    Ok(Event::StreamFinished { request_id }) if Some(request_id) == request => {
                        state.finish_assistant();
                        request = None;
                    }
                    Ok(Event::StreamFailed { request_id, message }) if Some(request_id) == request => {
                        state.fail(&message);
                        request = None;
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
                        ..
                    }) if Some(request_id) == request => {
                        state.finish_tool(&call_id, ok);
                    }
                    Ok(Event::ModelsLoaded { models }) => {
                        state.notice = None;
                        state.open_overlay(format!(" modelos · {provider} "), models);
                    }
                    Ok(Event::ModelsFailed { message }) => {
                        state.fail(&message);
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => {
                        state.notice = Some("eventos perdidos: a interface ficou para trás".to_string());
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(())
}

fn scroll_up(state: &mut ChatState, rows: u16) {
    state.scroll = state.scroll.saturating_add(rows);
}

fn scroll_down(state: &mut ChatState, rows: u16) {
    state.scroll = state.scroll.saturating_sub(rows);
}

fn draw(frame: &mut ratatui::Frame<'_>, state: &ChatState, model: &str, provider: &str) {
    let area = frame.area();
    let compact = area.height < 16 || area.width < 40;
    let composer_height = if compact { 1 } else { 3 };

    let [history_area, composer_area, details_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_transcript(frame, state, history_area);
    draw_composer(frame, state, composer_area, compact);
    draw_details(frame, state, model, provider, details_area);
    draw_suggestions(frame, state, composer_area);
    draw_overlay(frame, state);
}

fn draw_transcript(frame: &mut ratatui::Frame<'_>, state: &ChatState, area: Rect) {
    let content_area = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };

    let rows = {
        let mut cache = state.render_cache.borrow_mut();
        transcript_rows(
            &state.lines,
            content_area.width,
            state.processing(),
            &mut cache,
        )
    };
    let visible = area.height as usize;
    let (start, end) = visible_range(rows.len(), visible, state.scroll);

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

    let (inner, prompt_x, prompt_y) = if compact {
        (area, area.x, area.y)
    } else {
        let block = Block::bordered()
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " mensagem ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        (inner, inner.x, inner.y)
    };

    let prompt = Span::styled(
        "› ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    let text = if state.input.is_empty() {
        Span::styled(
            "escreva algo…  (/modelos troca o modelo)",
            Style::default().fg(Color::DarkGray),
        )
    } else {
        Span::raw(state.input.clone())
    };

    frame.render_widget(Paragraph::new(Line::from(vec![prompt, text])), inner);

    let cursor_x = prompt_x
        .saturating_add(2)
        .saturating_add(state.input.width() as u16);
    if cursor_x < inner.x.saturating_add(inner.width) {
        frame.set_cursor_position(Position::new(cursor_x, prompt_y));
    }
}

fn draw_details(
    frame: &mut ratatui::Frame<'_>,
    state: &ChatState,
    model: &str,
    provider: &str,
    area: Rect,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if let Some(notice) = &state.notice {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(notice.clone(), Style::default().fg(Color::Yellow)),
            ])),
            area,
        );
        return;
    }

    let state_color = match state.status {
        Status::Idle => Color::DarkGray,
        Status::Streaming => Color::Yellow,
        Status::Busy => Color::Red,
    };

    let left = Line::from(vec![
        Span::styled(" ● ", Style::default().fg(state_color)),
        Span::styled(model.to_string(), Style::default().fg(Color::Gray)),
        Span::styled(
            format!("  ·  {provider}  ·  {}", status_label(state.status)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let hints = "↑↓ rolar · pgup/pgdn · /modelos · ctrl+c sair";
    let hint_width = hints.width() as u16 + 1;

    if area.width > hint_width + 20 {
        let [left_area, right_area] =
            Layout::horizontal([Constraint::Min(10), Constraint::Length(hint_width)]).areas(area);

        frame.render_widget(Paragraph::new(left), left_area);
        frame.render_widget(
            Paragraph::new(hints)
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
    let Some(overlay) = &state.overlay else {
        return;
    };

    let area = centered_rect(frame.area(), 60, 60);
    frame.render_widget(Clear, area);

    let inner_height = area.height.saturating_sub(2).max(1) as usize;
    let start = overlay
        .selected
        .saturating_sub(inner_height.saturating_sub(1));
    let lines: Vec<Line> = overlay
        .items
        .iter()
        .enumerate()
        .skip(start)
        .take(inner_height)
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
        Paragraph::new(" nenhum modelo retornado ")
    } else {
        Paragraph::new(lines)
    };

    frame.render_widget(
        body.block(Block::bordered().title(overlay.title.clone())),
        area,
    );
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

fn transcript_rows(
    lines: &[ChatEntry],
    width: u16,
    processing: bool,
    cache: &mut crate::markdown::Cache,
) -> Vec<Line<'static>> {
    let area_width = width as usize;
    if area_width < 8 {
        return Vec::new();
    }

    let indent = 2usize;
    let content_width = area_width.saturating_sub(indent).max(1);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.push(Line::from(""));

    for (index, entry) in lines.iter().enumerate() {
        if index > 0 {
            rows.push(Line::from(""));
        }

        let (role, text) = match entry {
            ChatEntry::Message { role, content } => (*role, content.as_str()),
            ChatEntry::Tool {
                name,
                summary,
                ok,
                ..
            } => {
                let (mark, color) = match ok {
                    Some(true) => ("✓", Color::Green),
                    Some(false) => ("✗", Color::Red),
                    None => ("…", Color::DarkGray),
                };
                let label = format!("{name} · {summary}");
                let line = wrap_text(&label, content_width)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                rows.push(Line::from(vec![
                    Span::styled(
                        "  ⚙ ",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(line),
                    Span::raw(" "),
                    Span::styled(mark.to_string(), Style::default().fg(color)),
                ]));
                continue;
            }
        };

        let (marker, label, color) = match role {
            Role::User => ("❯", "você", Color::Cyan),
            Role::Assistant => ("●", "tupi", Color::Green),
            Role::System => ("◆", "sistema", Color::Yellow),
            Role::Tool => ("⚙", "tool", Color::DarkGray),
        };

        rows.push(Line::from(vec![
            Span::styled(
                marker.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                label.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));

        let body = match role {
            Role::User | Role::Assistant => cache.rows(index, text, content_width).to_vec(),
            Role::System => markdown_rows(text, content_width),
            Role::Tool => plain_rows(text, content_width),
        };

        for line in body {
            rows.push(indent_line(line, indent));
        }
    }

    if processing {
        if !rows.last().map(|row| row.width() == 0).unwrap_or(true) {
            rows.push(Line::from(""));
        }
        rows.push(Line::from(Span::styled(
            "  processando...",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::DIM),
        )));
    }

    rows.push(Line::from(""));
    rows
}

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
        }
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
        let lines = vec![
            message(Role::User, "leia"),
            ChatEntry::Tool {
                call_id: "c1".into(),
                name: "read".into(),
                summary: "src/a.rs".into(),
                ok: Some(true),
            },
        ];
        let rows = transcript_rows(&lines, 40, false, &mut crate::markdown::Cache::default());
        let text = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("read · src/a.rs"), "{text}");
        assert!(text.contains('✓'), "{text}");
        assert!(text.contains('⚙'), "{text}");
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
}
