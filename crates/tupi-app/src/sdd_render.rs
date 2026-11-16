//! Renderização dos templates do modo SDD (fonte: `configs/modes/sdd.ron`).

/// Substitui placeholders `{nome}` de um template do modo SDD.
///
/// `{{` e `}}` escapam chaves literais. Chave desconhecida é mantida no texto
/// (visível em vez de sumir), e o teste de contrato falha se algum template da
/// config usar uma chave que o código não fornece.
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(index) = rest.find(['{', '}']) {
        out.push_str(&rest[..index]);
        let tail = &rest[index..];
        if tail.starts_with("{{") {
            out.push('{');
            rest = &tail[2..];
        } else if tail.starts_with("}}") {
            out.push('}');
            rest = &tail[2..];
        } else if tail.starts_with('}') {
            out.push('}');
            rest = &tail[1..];
        } else if let Some(end) = tail.find('}') {
            let name = &tail[1..end];
            match values.iter().find(|(key, _)| *key == name) {
                Some((_, value)) => out.push_str(value),
                None => out.push_str(&tail[..=end]),
            }
            rest = &tail[end + 1..];
        } else {
            out.push_str(tail);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Bloco de lista do plano (`\nTítulo:\n- item\n...`), vazio sem itens.
pub fn list_block(title: &str, items: &[String]) -> String {
    let mut out = String::new();
    if items.is_empty() {
        return out;
    }
    out.push_str(&format!("\n{title}:\n"));
    for item in items {
        out.push_str(&format!("- {item}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_repeated_and_distinct_placeholders() {
        let out = render("{a}-{b}-{a}", &[("a", "1"), ("b", "2")]);
        assert_eq!(out, "1-2-1");
    }

    #[test]
    fn render_escapes_doubled_braces() {
        assert_eq!(render("{{a}} {a}", &[("a", "x")]), "{a} x");
    }

    #[test]
    fn render_keeps_unknown_placeholders_visible() {
        assert_eq!(render("{desconhecido}", &[("a", "x")]), "{desconhecido}");
        assert_eq!(render("sem chave {", &[]), "sem chave {");
    }

    #[test]
    fn list_block_is_empty_without_items() {
        assert!(list_block("Restrições", &[]).is_empty());
        assert_eq!(
            list_block("Nuances", &["a".to_string(), "b".to_string()]),
            "\nNuances:\n- a\n- b\n"
        );
    }
}
