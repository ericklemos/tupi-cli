//! Markdown rendering for chat messages.
//!
//! `tui-markdown` turns Markdown into a `Text`, but a chat transcript needs
//! more than that: every line must fit a fixed width, code fences need a box,
//! and the result should be cached while a message streams.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::{BuiltinCodeTheme, Options, StyleSheet, from_str_with_options};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CODE_BACKGROUND: Color = Color::Indexed(236);
const CODE_FOREGROUND: Color = Color::Indexed(252);

/// Styles used for Markdown inside the chat transcript.
#[derive(Clone, Copy, Debug, Default)]
struct ChatStyleSheet;

impl StyleSheet for ChatStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::new()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            2 => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            3 => Style::new()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
            _ => Style::new()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::ITALIC),
        }
    }

    fn code(&self) -> Style {
        Style::new().fg(CODE_FOREGROUND).bg(CODE_BACKGROUND)
    }

    fn link(&self) -> Style {
        Style::new()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::new()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC)
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn list_marker(&self) -> Style {
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    }

    fn table_header(&self) -> Style {
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    }

    fn table_border(&self) -> Style {
        Style::new().fg(Color::DarkGray)
    }
}

/// Render Markdown into lines that fit `width` columns.
pub fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let options = Options::new(ChatStyleSheet)
        .table_width(width.min(u16::MAX as usize) as u16)
        .code_theme(BuiltinCodeTheme::Base16OceanDark);

    let rendered = from_str_with_options(text, &options);
    let lines = rendered
        .lines
        .into_iter()
        .map(owned_line)
        .collect::<Vec<_>>();

    decorate_code_blocks(lines, width)
}

/// Per-message render cache. Invalidate one message while it streams, and
/// every message when the transcript width changes.
#[derive(Debug, Default)]
pub struct Cache {
    width: usize,
    entries: Vec<Entry>,
}

#[derive(Debug, Default)]
struct Entry {
    text: String,
    lines: Vec<Line<'static>>,
}

impl Cache {
    /// Return the rendered lines for message `index`, rendering it only when
    /// its text or the cache width changed.
    pub fn rows(&mut self, index: usize, text: &str, width: usize) -> &[Line<'static>] {
        if self.width != width {
            self.width = width;
            self.entries.clear();
        }
        if self.entries.len() <= index {
            self.entries.resize_with(index + 1, Entry::default);
        }
        if self.entries[index].text != text {
            self.entries[index] = Entry {
                text: text.to_owned(),
                lines: render(text, width),
            };
        }
        &self.entries[index].lines
    }
}

fn owned_line(line: Line<'_>) -> Line<'static> {
    let style = line.style;
    let alignment = line.alignment;
    let spans = line
        .spans
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect::<Vec<_>>();
    let mut owned = Line::from(spans).style(style);
    if let Some(alignment) = alignment {
        owned = owned.alignment(alignment);
    }
    owned
}

fn decorate_code_blocks(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let border = Style::new().fg(Color::DarkGray);
    let code_style = Style::new().fg(CODE_FOREGROUND).bg(CODE_BACKGROUND);
    let mut rows = Vec::new();
    let mut in_code = false;

    for line in lines {
        let text = line_text(&line);

        if !in_code {
            if let Some(language) = fence_open(&text) {
                if width >= 8 {
                    rows.push(code_top(&language, width, border));
                }
                in_code = true;
                continue;
            }
            rows.extend(wrap_line(&line, width));
            continue;
        }

        if is_fence_close(&text) {
            if width >= 8 {
                rows.push(code_bottom(width, border));
            }
            in_code = false;
            continue;
        }

        if width >= 8 {
            let inner = width.saturating_sub(4).max(1);
            for part in wrap_line(&line, inner) {
                rows.push(code_row(&part, inner, border, code_style));
            }
        } else {
            for part in wrap_line(&line, width) {
                rows.push(part.style(code_style));
            }
        }
    }

    if in_code && width >= 8 {
        rows.push(code_bottom(width, border));
    }

    if rows.is_empty() {
        rows.push(Line::from(""));
    }
    rows
}

fn fence_open(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let fence = "```";
    let rest = trimmed.strip_prefix(fence)?;
    if rest.contains('`') {
        return None;
    }
    Some(rest.trim().to_owned())
}

fn is_fence_close(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '`')
}

fn code_top(language: &str, width: usize, border: Style) -> Line<'static> {
    let label_budget = width.saturating_sub(4);
    let label = if language.is_empty() {
        String::new()
    } else {
        let language = language.chars().take(label_budget).collect::<String>();
        format!(" {language} ")
    };
    let left = format!("╭─{label}");
    let fill = width.saturating_sub(left.width() + 1);
    Line::from(vec![
        Span::styled(left, border),
        Span::styled(format!("{}╮", "─".repeat(fill)), border),
    ])
}

fn code_bottom(width: usize, border: Style) -> Line<'static> {
    Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(width.saturating_sub(2))),
        border,
    ))
}

fn code_row(line: &Line<'static>, inner: usize, border: Style, code_style: Style) -> Line<'static> {
    let mut spans = vec![Span::styled("│ ", border)];
    spans.extend(line.spans.iter().cloned());
    let padding = inner.saturating_sub(line.width());
    if padding > 0 {
        spans.push(Span::styled(" ".repeat(padding), code_style));
    }
    spans.push(Span::styled(" │", border));
    Line::from(spans).style(code_style)
}

#[derive(Clone, Copy, Debug)]
struct StyledChar {
    ch: char,
    style: Style,
}

fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let characters = flatten(line);
    if characters.is_empty() {
        return vec![Line::from("").style(line.style)];
    }

    let hang = hanging_width(line).min(width.saturating_sub(1));
    let mut rows = Vec::new();
    let mut start = 0;
    let mut first = true;

    while start < characters.len() {
        let available = if first {
            width
        } else {
            width.saturating_sub(hang).max(1)
        };
        let (end, stop) = next_chunk(&characters, start, available);
        let mut spans = Vec::new();
        if !first && hang > 0 {
            spans.push(Span::raw(" ".repeat(hang)));
        }
        spans.extend(spans_from_chars(&characters[start..stop]));

        if spans.is_empty() {
            spans.push(Span::raw(String::new()));
        }
        rows.push(Line::from(spans).style(line.style));

        start = end;
        while start < characters.len() && characters[start].ch == ' ' {
            start += 1;
        }
        first = false;
    }

    rows
}

fn next_chunk(characters: &[StyledChar], start: usize, width: usize) -> (usize, usize) {
    let mut end = start;
    let mut used = 0usize;
    let mut last_space = None;

    while end < characters.len() {
        let character_width = characters[end].ch.width().unwrap_or(0);
        if used + character_width > width && end > start {
            if let Some(space) = last_space {
                end = space;
            }
            break;
        }
        used += character_width;
        if characters[end].ch == ' ' {
            last_space = Some(end);
        }
        end += 1;
    }

    if end == start {
        end += 1;
    }

    let mut stop = end;
    while stop > start && characters[stop - 1].ch == ' ' {
        stop -= 1;
    }
    if stop == start {
        stop = end;
    }

    (end, stop)
}

fn flatten(line: &Line<'_>) -> Vec<StyledChar> {
    line.spans
        .iter()
        .flat_map(|span| {
            span.content.chars().map(|ch| StyledChar {
                ch,
                style: span.style,
            })
        })
        .collect()
}

fn spans_from_chars(characters: &[StyledChar]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut style = None;

    for character in characters {
        if style != Some(character.style) {
            if !text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut text),
                    style.unwrap_or_default(),
                ));
            }
            style = Some(character.style);
        }
        text.push(character.ch);
    }

    if !text.is_empty() {
        spans.push(Span::styled(text, style.unwrap_or_default()));
    }
    spans
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn hanging_width(line: &Line<'_>) -> usize {
    let text = line_text(line);
    let indent = text.len() - text.trim_start_matches(' ').len();
    let rest = &text[indent..];

    if rest.starts_with("- [ ] ")
        || rest.starts_with("- [x] ")
        || rest.starts_with("* [ ] ")
        || rest.starts_with("* [x] ")
        || rest.starts_with("+ [ ] ")
        || rest.starts_with("+ [x] ")
    {
        return indent + 6;
    }

    if rest.starts_with("- ") || rest.starts_with("* ") || rest.starts_with("+ ") {
        return indent + 2;
    }

    let digits = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digits > 0 {
        let after = &rest[digits..];
        if let Some(after) = after.strip_prefix(". ") {
            if after.starts_with("[ ] ") || after.starts_with("[x] ") {
                return indent + digits + 6;
            }
            return indent + digits + 2;
        }
    }

    let quotes = rest
        .chars()
        .take_while(|character| *character == '>')
        .count();
    if quotes > 0 && rest[quotes..].starts_with(' ') {
        return indent + quotes + 1;
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_text(row: &Line<'_>) -> String {
        row.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn headings_are_styled_without_markers() {
        let rows = render("# Título", 30);

        assert!(rows.iter().any(|row| row_text(row).contains("Título")));
        assert!(rows.iter().all(|row| !row_text(row).contains('#')));
        let heading = rows
            .iter()
            .find(|row| row_text(row).contains("Título"))
            .expect("título renderizado");
        assert_eq!(heading.style.fg, Some(Color::LightCyan));
    }

    #[test]
    fn inline_formatting_keeps_its_styles() {
        let rows = render("isso é **forte** e `código`", 50);

        let bold = rows
            .iter()
            .flat_map(|row| row.spans.iter())
            .find(|span| span.content == "forte")
            .expect("trecho em negrito");
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));

        let code = rows
            .iter()
            .flat_map(|row| row.spans.iter())
            .find(|span| span.content == "código")
            .expect("código inline");
        assert_eq!(code.style.bg, Some(CODE_BACKGROUND));
    }

    #[test]
    fn fenced_code_gets_a_language_label_and_background() {
        let rows = render("```rust\nfn main() {}\n```", 30);

        assert!(rows.iter().any(|row| row_text(row).contains("rust")));
        let code = rows
            .iter()
            .find(|row| row_text(row).contains("fn main()"))
            .expect("linha de código");
        assert_eq!(code.style.bg, Some(CODE_BACKGROUND));
        assert!(rows.iter().all(|row| row.width() <= 30));
    }

    #[test]
    fn tables_fit_the_requested_width() {
        let rows = render("| Nome | Estado |\n|:-----|-------:|\n| API | Pronto |", 32);

        assert!(rows.iter().any(|row| row_text(row).contains('┌')));
        assert!(rows.iter().all(|row| row.width() <= 32));
    }

    #[test]
    fn lists_hang_their_continuation_lines() {
        let rows = render(
            "- um item bem longo que precisa quebrar em várias linhas",
            20,
        );

        assert!(rows.iter().all(|row| row.width() <= 20));
        assert!(rows.len() > 1);
        assert!(row_text(&rows[1]).starts_with("  "));
    }

    #[test]
    fn realistic_assistant_message_is_formatted() {
        let text = "Oi! Vamos lá — para melhorar desempenho em Rust, a regra de ouro é: **meça antes de otimizar**.\n\n## 1. Meça e profile primeiro\n\n- Benchmarks: `criterion`\n- Profiling de CPU: `cargo flamegraph`\n- Alocações: `heaptrack`\n\n```toml\n[profile.release]\nopt-level = 3\nlto = \"fat\"\n```\n\n```bash\nRUSTFLAGS=\"-C target-cpu=native\" cargo build --release\n```";
        let rows = render(text, 60);
        let joined = rows.iter().map(row_text).collect::<Vec<_>>().join("\n");

        assert!(rows.iter().all(|row| row.width() <= 60));
        assert!(joined.contains("Meça e profile primeiro"));
        assert!(!joined.contains("##"));
        assert!(joined.contains("criterion"));
        assert!(joined.contains("toml"));
        assert!(joined.contains("bash"));
        assert!(rows.iter().flat_map(|row| row.spans.iter()).any(|span| {
            span.content.contains("meça antes de otimizar")
                && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        assert!(rows.iter().any(|row| row.style.bg == Some(CODE_BACKGROUND)));
    }

    #[test]
    fn cache_invalidates_only_the_changed_message() {
        let mut cache = Cache::default();
        let first = cache.rows(0, "# A", 20).to_vec();
        let cached = cache.rows(0, "# A", 20).to_vec();
        assert_eq!(first, cached);

        let changed = cache.rows(0, "**B**", 20).to_vec();
        assert!(changed
            .iter()
            .flat_map(|row| row.spans.iter())
            .any(|span| span.content == "B" && span.style.add_modifier.contains(Modifier::BOLD)));
    }
}
