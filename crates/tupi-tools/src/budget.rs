/// Um pedaço atômico de resposta de tool.
///
/// A unidade de truncamento é a seção, nunca o caractere: uma resposta que perde
/// metade de uma lista de chamadores parece completa e engana o agente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    pub body: String,
    /// 0 é descartável; 255 nunca é descartado.
    pub priority: u8,
}

/// Caracteres por token estimado. Conservador para código-fonte.
pub const CHARS_PER_TOKEN: f64 = 3.3;

pub const DEFAULT_BUDGET_TOKENS: usize = 2000;

/// Nome da tool que recupera o corpo omitido. Fonte única para o envelope e o
/// ponteiro do ledger não divergirem do registry.
pub const PEEK_TOOL_NAME: &str = "peek";

pub fn estimate_tokens(text: &str) -> usize {
    ((text.chars().count() as f64) / CHARS_PER_TOKEN).ceil() as usize
}

fn section_tokens(section: &Section) -> usize {
    estimate_tokens(&section.heading) + estimate_tokens(&section.body)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    pub included: Vec<Section>,
    pub omitted: Vec<Section>,
    pub omitted_tokens: usize,
}

impl Selection {
    fn needs_handle(&self) -> bool {
        !self.omitted.is_empty()
    }

    /// As seções omitidas como um corpo paginável, cada uma precedida do seu
    /// heading para que todo trecho recuperado diga a que pertence.
    pub fn omitted_body(&self) -> String {
        let mut out = String::new();
        for section in &self.omitted {
            out.push_str(&section.heading);
            out.push('\n');
            out.push_str(&section.body);
            out.push('\n');
        }
        out
    }
}

/// Escolhe seções inteiras por prioridade decrescente até gastar o orçamento.
///
/// Empates mantêm a ordem da tool. A seção de maior prioridade entra mesmo se
/// sozinha ultrapassar o orçamento: perder a resposta principal para respeitar o
/// envelope derrotaria o propósito.
pub fn select(sections: &[Section], budget_tokens: usize) -> Selection {
    let mut order: Vec<usize> = (0..sections.len()).collect();
    order.sort_by(|&a, &b| sections[b].priority.cmp(&sections[a].priority));

    let mut keep = vec![false; sections.len()];
    let mut used = 0usize;
    let mut first = true;

    for idx in order {
        let cost = section_tokens(&sections[idx]);
        if first || used + cost <= budget_tokens {
            keep[idx] = true;
            used += cost;
            first = false;
        }
    }

    let mut selection = Selection::default();
    for (idx, section) in sections.iter().enumerate() {
        if keep[idx] {
            selection.included.push(section.clone());
        } else {
            selection.omitted.push(section.clone());
            selection.omitted_tokens += section_tokens(section);
        }
    }
    selection
}

/// Divide uma seção que sozinha excede o orçamento em partes auto-rotuladas.
pub fn split_section(section: &Section, budget_tokens: usize) -> Vec<Section> {
    if budget_tokens == 0 || section_tokens(section) <= budget_tokens {
        return vec![section.clone()];
    }

    let heading_tokens = estimate_tokens(&section.heading) + 8;
    let body_budget = budget_tokens.saturating_sub(heading_tokens).max(1);
    let body_chars = (body_budget as f64 * CHARS_PER_TOKEN).floor() as usize;

    let (prefix, inner, suffix) = split_fence(&section.body);
    let chunks = chunk_lines(&inner, body_chars);
    let total = chunks.len();
    if total <= 1 {
        return vec![section.clone()];
    }

    chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| Section {
            heading: format!("{} · parte {}/{}", section.heading, index + 1, total),
            body: format!("{prefix}{chunk}{suffix}"),
            priority: section.priority,
        })
        .collect()
}

/// Separa um corpo em (preâmbulo + fence de abertura, texto interno, fence de
/// fechamento). Corpo sem fence volta inteiro.
fn split_fence(body: &str) -> (String, String, String) {
    let trimmed = body.trim_end_matches('\n');
    let lines: Vec<&str> = trimmed.lines().collect();
    let Some(open) = lines.iter().position(|line| line.starts_with("```")) else {
        return (String::new(), body.to_string(), String::new());
    };
    let Some(close) = lines.iter().rposition(|line| line.trim_end() == "```") else {
        return (String::new(), body.to_string(), String::new());
    };
    if close <= open {
        return (String::new(), body.to_string(), String::new());
    }

    (
        format!("{}\n", lines[..=open].join("\n")),
        lines[open + 1..close].join("\n"),
        "```\n".to_string(),
    )
}

/// Divide o texto em blocos alinhados a linhas de no máximo `max_chars`
/// caracteres. Uma linha sozinha maior que o teto é cortada em limite de
/// caractere para o laço sempre progredir.
fn chunk_lines(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();

    for line in text.lines() {
        let line_chars = line.chars().count();
        let separator = usize::from(!current.is_empty());
        if !current.is_empty() && current.chars().count() + separator + line_chars > max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        if line_chars > max_chars {
            for piece in hard_split(line, max_chars) {
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
                chunks.push(piece);
            }
            continue;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

fn hard_split(line: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        current.push(ch);
        if current.chars().count() >= max_chars {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Renderiza as seções incluídas na ordem da tool, precedidas por um envelope de
/// truncamento apenas quando algo ficou de fora.
pub fn render(selection: &Selection, handle: Option<&str>) -> String {
    let mut out = String::new();

    if selection.needs_handle() {
        out.push_str(&format!(
            "> Resposta truncada: {} seções omitidas (~{} tokens estimados).\n",
            selection.omitted.len(),
            selection.omitted_tokens
        ));
        if let Some(handle) = handle {
            out.push_str(&format!(
                "> Recupere com `{PEEK_TOOL_NAME}` — handle: `{handle}`.\n"
            ));
        }
        out.push('\n');
    }

    for section in &selection.included {
        out.push_str(&section.heading);
        out.push('\n');
        out.push_str(&section.body);
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(heading: &str, body_chars: usize, priority: u8) -> Section {
        Section {
            heading: heading.to_string(),
            body: "x".repeat(body_chars),
            priority,
        }
    }

    #[test]
    fn a_section_that_fits_is_not_cut() {
        let sections = vec![section("a", 350, 100)];
        let selected = select(&sections, 200);

        assert_eq!(selected.included.len(), 1);
        assert!(selected.omitted.is_empty());
        assert_eq!(selected.omitted_tokens, 0);
    }

    #[test]
    fn priority_decides_what_is_omitted() {
        let sections = vec![section("low", 350, 10), section("high", 350, 200)];
        let selected = select(&sections, 150);

        assert_eq!(selected.included[0].heading, "high");
        assert_eq!(selected.omitted[0].heading, "low");
    }

    #[test]
    fn highest_priority_section_enters_even_when_it_alone_exceeds_the_budget() {
        let sections = vec![section("huge", 7000, 255)];
        let selected = select(&sections, 100);

        assert_eq!(selected.included.len(), 1);
        assert_eq!(selected.included[0].heading, "huge");
    }

    #[test]
    fn priority_ties_keep_tool_order() {
        let sections = vec![section("one", 350, 100), section("two", 350, 100)];
        let selected = select(&sections, 150);

        assert_eq!(selected.included[0].heading, "one");
        assert_eq!(selected.omitted[0].heading, "two");
    }

    #[test]
    fn rendering_keeps_the_original_order_not_the_priority_order() {
        let sections = vec![section("first", 350, 10), section("second", 350, 200)];
        let selected = select(&sections, 1000);
        let rendered = render(&selected, None);

        assert!(rendered.find("first").unwrap() < rendered.find("second").unwrap());
    }

    #[test]
    fn an_oversized_section_is_split_into_labelled_parts() {
        let body: Vec<String> = (0..200).map(|i| format!("linha {i}")).collect();
        let section = Section {
            heading: "symbol".to_string(),
            body: body.join("\n"),
            priority: 255,
        };

        let parts = split_section(&section, 100);

        assert!(parts.len() > 1);
        assert!(parts[0].heading.contains("parte 1/"));
        let reassembled = parts
            .iter()
            .map(|part| part.body.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(reassembled, body.join("\n"));
    }

    #[test]
    fn splitting_a_code_fence_reopens_it_in_every_part() {
        let body: Vec<String> = (0..200).map(|i| format!("    let x{i} = {i};")).collect();
        let section = Section {
            heading: "fn".to_string(),
            body: format!("```rust\n{}\n```\n", body.join("\n")),
            priority: 255,
        };

        let parts = split_section(&section, 80);

        assert!(parts.len() > 1);
        for part in &parts {
            assert!(part.body.starts_with("```rust\n"));
            assert!(part.body.ends_with("```\n"));
        }
    }

    #[test]
    fn envelope_announces_omitted_sections_and_the_handle() {
        let sections = vec![section("kept", 350, 255), section("dropped", 3500, 10)];
        let selected = select(&sections, 500);
        let rendered = render(&selected, Some("h_1234abcd"));

        assert!(rendered.starts_with("> Resposta truncada: 1 seções omitidas"));
        assert!(rendered.contains("`h_1234abcd`"));
    }

    #[test]
    fn no_omission_means_no_envelope() {
        let sections = vec![section("only", 350, 100)];
        let selected = select(&sections, 1000);
        let rendered = render(&selected, Some("h_1234abcd"));

        assert!(!rendered.contains("truncada"));
        assert!(rendered.starts_with("only"));
    }

    #[test]
    fn omitted_body_concatenates_headings_and_bodies() {
        let sections = vec![
            section("kept", 350, 255),
            section("drop-a", 350, 10),
            section("drop-b", 350, 20),
        ];
        let selected = select(&sections, 200);
        let body = selected.omitted_body();

        assert!(body.starts_with("drop-a\n"));
        assert!(body.contains("drop-b\n"));
        assert!(!body.contains("kept"));
    }

    #[test]
    fn estimate_uses_three_point_three_chars_per_token() {
        assert_eq!(estimate_tokens(&"x".repeat(33)), 10);
        assert_eq!(CHARS_PER_TOKEN, 3.3);
        assert_eq!(DEFAULT_BUDGET_TOKENS, 2000);
    }
}
