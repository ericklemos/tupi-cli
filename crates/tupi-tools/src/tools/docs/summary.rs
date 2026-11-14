//! Prompt e sanitização dos resumos de arquivo.

/// Monta o prompt de resumo a partir do caminho, headings e trecho.
pub fn build_prompt(path: &str, headings: &[String], excerpt: &str) -> String {
    format!(
        "Resuma o documento abaixo em UMA linha, em português, com no máximo 160 caracteres. Diga o assunto e a decisão principal. Não use markdown, listas, aspas nem o prefixo \"Resumo:\".\n\nArquivo: {path}\nHeadings: {}\nTrecho:\n{excerpt}\n\nResumo:",
        headings.join(" > ")
    )
}

/// Corta o texto em `max_chars` caracteres, respeitando fronteira de char.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// Limpa a saída do modelo para uma linha curta.
pub fn sanitize(raw: &str, max_chars: usize) -> Option<String> {
    let line = raw.lines().next()?.trim();
    let cleaned = strip_prefixes(line)
        .trim_end_matches(|character: char| character.is_whitespace() || matches!(character, '"' | '\'' | '`'));
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let chars: Vec<char> = collapsed.chars().collect();
    let mut end = max_chars.min(chars.len());
    if end < chars.len() && !chars[end].is_whitespace() {
        while end > 0 && !chars[end - 1].is_whitespace() {
            end -= 1;
        }
    }
    if end == 0 {
        end = max_chars.min(chars.len());
    }
    let out: String = chars[..end].iter().collect();
    let out = out.trim_end();
    if out.is_empty() {
        None
    } else {
        Some(out.to_string())
    }
}

fn strip_prefixes(mut text: &str) -> &str {
    loop {
        let before = text;
        text = text.trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, '#' | '-' | '*' | '"' | '\'' | '`')
        });
        if text
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("resumo:"))
        {
            text = &text[7..];
        }
        if text == before {
            return text;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_one_clean_line() {
        assert_eq!(
            sanitize("# \"Resumo: Cobranca mensal.\nOutra linha\"", 160).as_deref(),
            Some("Cobranca mensal.")
        );
        assert!(sanitize("   ", 160).is_none());
        let long = "palavra ".repeat(40);
        let out = sanitize(&long, 160).unwrap();
        assert!(out.chars().count() <= 160);
        assert!(out.ends_with("palavra"));
    }

    #[test]
    fn the_prompt_carries_path_headings_and_excerpt() {
        let prompt = build_prompt(
            "docs/a.md",
            &["Guia".to_string(), "Guia > Cobranca".to_string()],
            "A cobranca e mensal.",
        );
        assert!(prompt.contains("docs/a.md"));
        assert!(prompt.contains("Guia > Cobranca"));
        assert!(prompt.contains("A cobranca e mensal."));
        assert!(prompt.trim_end().ends_with("Resumo:"));
    }
}
