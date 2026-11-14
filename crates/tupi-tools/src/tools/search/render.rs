use super::*;

pub(super) fn capped_body(snippet: &AstSnippet) -> (String, bool) {
    let lines: Vec<&str> = snippet.code.lines().collect();
    if lines.len() <= MAX_SEARCH_BODY_LINES {
        return (snippet.code.clone(), false);
    }

    let remaining = lines.len() - MAX_SEARCH_BODY_LINES;
    (
        format!(
            "{}\n// ... (+{remaining} linhas; peça o corpo inteiro com read)",
            lines[..MAX_SEARCH_BODY_LINES].join("\n")
        ),
        true,
    )
}

pub(super) fn completeness_line(
    total: usize,
    shown: usize,
    max_results: usize,
    signature_only: usize,
) -> String {
    let mut line = if shown < total {
        format!("#### {shown} de {total} resultados — max_results={max_results} cortou o resto.")
    } else {
        format!("#### Todas as {shown} candidatas casaram com a query.")
    };
    if signature_only > 0 {
        line.push_str(&format!(" {signature_only} só com assinatura."));
    }
    line.push_str(" lexical.");
    line
}
