use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Viewport do último desenho do transcript, usado para mapear o mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptView {
    pub rows: usize,
    pub start: usize,
    pub width: u16,
    pub area: ratatui::layout::Rect,
}

/// Seleção em coordenadas absolutas do transcript: linha e coluna em células.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (usize, u16),
    pub cursor: (usize, u16),
}

impl Selection {
    pub fn new(row: usize, column: u16) -> Self {
        Self {
            anchor: (row, column),
            cursor: (row, column),
        }
    }

    pub fn normalized(&self) -> ((usize, u16), (usize, u16)) {
        if (self.anchor.0, self.anchor.1) <= (self.cursor.0, self.cursor.1) {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }
}

/// Recorta `text` pelas colunas de exibição `from..to`.
pub fn slice_display(text: &str, from: u16, to: u16) -> String {
    if from >= to {
        return String::new();
    }

    let mut out = String::new();
    let mut column = 0u16;
    for character in text.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0).max(1) as u16;
        if column >= from && column < to {
            out.push(character);
        }
        column = column.saturating_add(width);
        if column >= to {
            break;
        }
    }
    out
}

/// Texto visível coberto pela seleção, linha a linha.
pub fn selection_text(rows: &[Line<'_>], selection: &Selection) -> String {
    let ((start_row, start_column), (end_row, end_column)) = selection.normalized();
    let mut out = String::new();

    for row in start_row..=end_row {
        let Some(line) = rows.get(row) else {
            break;
        };
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let width = UnicodeWidthStr::width(text.as_str()) as u16;
        let from = if row == start_row { start_column } else { 0 };
        let to = if row == end_row {
            end_column.min(width)
        } else {
            width
        };

        if row > start_row {
            out.push('\n');
        }
        out.push_str(&slice_display(&text, from, to));
    }

    out
}

/// Reverte as células de `from..to` da linha, como a seleção do terminal.
pub fn apply_selection(line: Line<'static>, from: u16, to: u16) -> Line<'static> {
    if from >= to {
        return line;
    }

    let line_style = line.style;
    let mut groups: Vec<(bool, Style, String)> = Vec::new();
    let mut column = 0u16;
    let mut previous_selected = false;

    for span in line.spans {
        for character in span.content.chars() {
            let width = UnicodeWidthChar::width(character).unwrap_or(0) as u16;
            let selected = if width == 0 {
                previous_selected
            } else {
                column >= from && column < to
            };
            previous_selected = selected;

            match groups.last_mut() {
                Some((group_selected, group_style, text))
                    if *group_selected == selected && *group_style == span.style =>
                {
                    text.push(character);
                }
                _ => groups.push((selected, span.style, character.to_string())),
            }

            column = column.saturating_add(width);
        }
    }

    let spans: Vec<Span<'static>> = groups
        .into_iter()
        .map(|(selected, style, text)| {
            let style = if selected {
                style.add_modifier(Modifier::REVERSED)
            } else {
                style
            };
            Span::styled(text, style)
        })
        .collect();
    let mut highlighted = Line::from(spans);
    highlighted.style = line_style;
    highlighted
}
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;
    use ratatui::text::Line;

    #[test]
    fn selection_normalizes_a_reversed_drag() {
        let selection = Selection {
            anchor: (4, 9),
            cursor: (1, 2),
        };
        assert_eq!(selection.normalized(), ((1, 2), (4, 9)));
        assert!(!selection.is_empty());
    }

    #[test]
    fn a_click_without_drag_is_empty() {
        assert!(Selection::new(3, 7).is_empty());
    }

    #[test]
    fn slice_display_uses_columns() {
        assert_eq!(slice_display("abcdef", 1, 4), "bcd");
        assert_eq!(slice_display("abcdef", 0, 99), "abcdef");
        assert_eq!(slice_display("abcdef", 4, 2), "");
    }

    #[test]
    fn slice_display_keeps_wide_characters_whole() {
        assert_eq!(slice_display("a✓b", 1, 2), "✓");
        assert_eq!(slice_display("a✓b", 2, 3), "b");
    }

    #[test]
    fn selection_text_spans_rows_and_columns() {
        let rows = vec![
            Line::from("primeira linha"),
            Line::from("segunda linha"),
            Line::from("terceira"),
        ];
        let selection = Selection {
            anchor: (0, 9),
            cursor: (1, 7),
        };
        assert_eq!(selection_text(&rows, &selection), "linha\nsegunda");
    }

    #[test]
    fn selection_text_includes_full_rows_between_ends() {
        let rows = vec![Line::from("a"), Line::from("meio"), Line::from("z")];
        let selection = Selection {
            anchor: (0, 0),
            cursor: (2, 1),
        };
        assert_eq!(selection_text(&rows, &selection), "a\nmeio\nz");
    }

    #[test]
    fn apply_selection_reverses_only_the_range() {
        let highlighted = apply_selection(Line::from("abcdef"), 2, 4);
        let text: String = highlighted
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let selected: String = highlighted
            .spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::REVERSED))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "abcdef");
        assert_eq!(selected, "cd");
    }

    #[test]
    fn apply_selection_to_the_end_uses_the_line_width() {
        let highlighted = apply_selection(Line::from("abc"), 1, u16::MAX);
        let selected: String = highlighted
            .spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::REVERSED))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(selected, "bc");
    }
}
