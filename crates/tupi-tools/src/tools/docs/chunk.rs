//! Corte de documentação em chunks com breadcrumb de headings.

/// Limites de tokens de um chunk.
#[derive(Debug, Clone, Copy)]
pub struct ChunkSpec {
    pub max_tokens: usize,
    pub target_tokens: usize,
    pub min_tokens: usize,
}

/// Trecho de documento com a faixa de linhas que o contém.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub heading: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub tokens: usize,
}

struct Section {
    heading: String,
    start: usize,
    end: usize,
}

struct Piece {
    text: String,
    start: usize,
    end: usize,
    explicit: bool,
}

/// Corta o texto em chunks, respeitando headings e o teto de tokens.
pub fn chunk_text(text: &str, spec: ChunkSpec, token_len: &dyn Fn(&str) -> usize) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for section in sections(&lines) {
        let mut chunks = chunk_section(&lines, &section, spec, token_len);
        merge_tiny(&mut chunks, &lines, spec, token_len);
        out.extend(chunks);
    }
    out
}

fn sections(lines: &[&str]) -> Vec<Section> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < lines.len() {
        if let Some((level, title, consumed)) = heading_at(lines, i) {
            if start < i {
                out.push(Section {
                    heading: breadcrumb(&stack),
                    start,
                    end: i,
                });
            }
            stack.truncate(level.saturating_sub(1));
            stack.push((level, title));
            start = i;
            i += consumed;
            continue;
        }
        i += 1;
    }
    if start < lines.len() {
        out.push(Section {
            heading: breadcrumb(&stack),
            start,
            end: lines.len(),
        });
    }
    out
}

fn heading_at(lines: &[&str], i: usize) -> Option<(usize, String, usize)> {
    if let Some((level, title)) = atx(lines[i]) {
        return Some((level, title, 1));
    }
    if !lines[i].trim().is_empty()
        && i + 1 < lines.len()
        && let Some(level) = setext(lines[i + 1])
    {
        return Some((level, lines[i].trim().to_string(), 2));
    }
    None
}

fn atx(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|character| *character == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let title = rest.trim().trim_end_matches('#').trim();
    if title.is_empty() {
        return None;
    }
    Some((level, title.to_string()))
}

fn setext(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return None;
    }
    if trimmed.chars().all(|character| character == '=') {
        Some(1)
    } else if trimmed.chars().all(|character| character == '-') {
        Some(2)
    } else {
        None
    }
}

fn breadcrumb(stack: &[(usize, String)]) -> String {
    stack
        .iter()
        .map(|(_, title)| title.as_str())
        .collect::<Vec<_>>()
        .join(" > ")
}

fn chunk_section(
    lines: &[&str],
    section: &Section,
    spec: ChunkSpec,
    token_len: &dyn Fn(&str) -> usize,
) -> Vec<Chunk> {
    let mut pieces = Vec::new();
    let mut i = section.start;
    while i < section.end {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        while i < section.end && !lines[i].trim().is_empty() {
            i += 1;
        }
        let end = i;
        let paragraph = lines[start..end].join("\n");
        if token_len(&paragraph) <= spec.max_tokens {
            pieces.push(Piece {
                text: paragraph,
                start: start + 1,
                end,
                explicit: false,
            });
            continue;
        }
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        let mut group: Vec<&str> = Vec::new();
        for word in words {
            group.push(word);
            let candidate = group.join(" ");
            if token_len(&candidate) >= spec.max_tokens {
                pieces.push(Piece {
                    text: candidate,
                    start: start + 1,
                    end,
                    explicit: true,
                });
                group.clear();
            }
        }
        if !group.is_empty() {
            pieces.push(Piece {
                text: group.join(" "),
                start: start + 1,
                end,
                explicit: true,
            });
        }
    }

    let mut chunks = Vec::new();
    let mut current: Vec<Piece> = Vec::new();
    for piece in pieces {
        if current.is_empty() {
            current.push(piece);
            continue;
        }
        let candidate_tokens = token_len(&join_text(&current, Some(&piece)));
        if candidate_tokens > spec.max_tokens || candidate_tokens > spec.target_tokens {
            flush_chunk(&mut chunks, &mut current, lines, section, token_len);
        }
        current.push(piece);
    }
    flush_chunk(&mut chunks, &mut current, lines, section, token_len);
    chunks
}

fn join_text(current: &[Piece], extra: Option<&Piece>) -> String {
    let mut parts: Vec<&str> = current.iter().map(|piece| piece.text.as_str()).collect();
    if let Some(piece) = extra {
        parts.push(piece.text.as_str());
    }
    parts.join("\n\n")
}

fn flush_chunk(
    chunks: &mut Vec<Chunk>,
    current: &mut Vec<Piece>,
    lines: &[&str],
    section: &Section,
    token_len: &dyn Fn(&str) -> usize,
) {
    if current.is_empty() {
        return;
    }
    let start = current.iter().map(|piece| piece.start).min().unwrap_or(1);
    let end = current
        .iter()
        .map(|piece| piece.end)
        .max()
        .unwrap_or(start);
    let explicit = current.iter().any(|piece| piece.explicit);
    let text = if explicit || end > lines.len() {
        current
            .iter()
            .map(|piece| piece.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    } else {
        lines[start - 1..end].join("\n").trim_end().to_string()
    };
    let tokens = token_len(&text);
    chunks.push(Chunk {
        heading: section.heading.clone(),
        start,
        end,
        text,
        tokens,
    });
    current.clear();
}

fn merge_tiny(
    chunks: &mut Vec<Chunk>,
    lines: &[&str],
    spec: ChunkSpec,
    token_len: &dyn Fn(&str) -> usize,
) {
    let mut i = 0;
    while i < chunks.len() {
        if chunks[i].tokens >= spec.min_tokens || i + 1 >= chunks.len() {
            i += 1;
            continue;
        }
        let start = chunks[i].start;
        let end = chunks[i + 1].end;
        if end > lines.len() {
            i += 1;
            continue;
        }
        let text = lines[start - 1..end].join("\n").trim_end().to_string();
        let tokens = token_len(&text);
        if tokens <= spec.max_tokens {
            let heading = chunks[i].heading.clone();
            chunks[i] = Chunk {
                heading,
                start,
                end,
                text,
                tokens,
            };
            chunks.remove(i + 1);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn spec() -> ChunkSpec {
        ChunkSpec { max_tokens: 50, target_tokens: 30, min_tokens: 5 }
    }

    #[test]
    fn markdown_headings_become_breadcrumbs() {
        let text = "# Guia\n\nintro\n\n## Cobranca\n\nregra\n\n### Recorrencia\n\nmensal\n";
        let chunks = chunk_text(text, spec(), &words);
        let headings: Vec<_> = chunks.iter().map(|chunk| chunk.heading.as_str()).collect();
        assert_eq!(
            headings,
            ["Guia", "Guia > Cobranca", "Guia > Cobranca > Recorrencia"]
        );
    }

    #[test]
    fn line_ranges_cover_exactly_the_chunk_text() {
        let text = "# Guia\n\nlinha um\nlinha dois\n\n## Outra\n\nlinha tres\n";
        let chunks = chunk_text(text, spec(), &words);
        let lines: Vec<_> = text.lines().collect();
        for chunk in &chunks {
            let expected = lines[chunk.start - 1..chunk.end].join("\n");
            assert_eq!(chunk.text, expected);
        }
    }

    #[test]
    fn no_chunk_exceeds_the_hard_cap() {
        let text = format!("# Guia\n\n{}\n", "palavra ".repeat(300));
        let chunks = chunk_text(&text, spec(), &words);
        assert!(chunks.iter().all(|chunk| chunk.tokens <= 50));
    }

    #[test]
    fn tiny_chunks_are_merged_into_the_next() {
        let text = "# Guia\n\ncurto\n\nmais texto aqui para fundir\n";
        let chunks = chunk_text(text, spec(), &words);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("curto"));
        assert!(chunks[0].text.contains("fundir"));
    }

    #[test]
    fn setext_headings_are_recognized() {
        let text = "Guia\n====\n\nintro\n";
        let chunks = chunk_text(text, spec(), &words);
        assert_eq!(chunks[0].heading, "Guia");
    }
}
