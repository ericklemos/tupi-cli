use super::*;

pub(super) fn qualified_span(span: &SymbolSpan) -> String {
    match &span.container {
        Some(container) => format!("{container}::{}", span.name),
        None => span.name.clone(),
    }
}

pub(super) fn splice(
    content: &str,
    span: &SymbolSpan,
    new_body: &str,
    signature: Option<&str>,
) -> Result<String, ToolError> {
    if span.body_range.end > content.len()
        || span.body_range.start > span.body_range.end
        || span.head_range.end > span.body_range.start
    {
        return Err(ToolError::Other(
            "intervalo do símbolo inválido; nada foi escrito".to_string(),
        ));
    }

    let resolver = SpanResolver::new();
    let signature = signature.map(str::trim_end);
    if let Some(signature) = signature {
        if signature.is_empty() {
            return Err(ToolError::InvalidArguments(
                "'signature' vazia; nada foi escrito".to_string(),
            ));
        }
        // O cabeçalho novo tem que descrever o mesmo item, no mesmo lugar:
        // trocar o tipo do símbolo (struct -> impl) produz um arquivo que
        // parseia mas deixa de conter o alvo, e o erro só apareceria depois da
        // escrita. O nome pode mudar — renomear pelo `signature` é justamente
        // o que o parâmetro existe para cobrir; o container não pode, senão o
        // alvo sai do dono em que foi encontrado.
        let head = format!("{} {{}}\n", signature.trim_end_matches('{').trim_end());
        // A cabeça de um membro não expressa o dono: um `fn check` solto parseia
        // como função de topo, com outro `container` e às vezes outro `kind`, e o
        // rename legítimo seria recusado. Com container, o probe é montado dentro
        // de um `impl` de mesmo nome, para comparar maçã com maçã.
        let probe = match span.container.as_deref() {
            Some(container) => format!("impl {container} {{\n{head}}}\n"),
            None => head,
        };
        let probe_table = resolver.table(Path::new("signature.rs"), &probe)?;
        // O tipo e o dono têm que ser os mesmos: `struct` -> `impl` produz um
        // arquivo que parseia mas deixa de conter o alvo, e o erro só apareceria
        // depois da escrita. O nome pode mudar — renomear pelo `signature` é
        // justamente o que o parâmetro cobre.
        // `impl Auth { fn check {} }` gera dois spans: o `impl` vem primeiro e
        // não é o item. O alvo do probe é o span com o mesmo container do alvo.
        let same_item = probe_table
            .spans
            .iter()
            .find(|probe| probe.container == span.container)
            .is_some_and(|probe| probe.kind == span.kind);
        if !same_item {
            return Err(ToolError::InvalidArguments(format!(
                "'signature' mudaria o tipo ou o dono do item; o alvo é '{}'. Para renomear, mantenha tipo e container e troque só o nome. Nada foi escrito.",
                render_candidate(span)
            )));
        }
    }

    let mut spliced = String::with_capacity(
        content.len() - span.body_range.len() + new_body.len() + signature.map_or(0, str::len) + 1,
    );
    spliced.push_str(&content[..span.body_range.start]);
    spliced.push_str(new_body);
    spliced.push_str(&content[span.body_range.end..]);

    let mut body_start = span.body_range.start;
    if let Some(signature) = signature {
        let mut with_head = String::with_capacity(spliced.len() + signature.len() + 1);
        with_head.push_str(&spliced[..span.head_range.start]);
        with_head.push_str(signature);
        with_head.push(' ');
        with_head.push_str(&spliced[span.body_range.start..]);
        spliced = with_head;
        body_start = span.head_range.start + signature.len() + 1;
    }

    // Um 'new_body' que fecha o item e deixa outro código fora dele parseia
    // sem erro e mudaria o arquivo inteiro. O bloco gravado tem que ser
    // exatamente o corpo do item que começa em 'body_start'.
    let spliced_table = resolver.table(Path::new("splice.rs"), &spliced)?;
    let exact = spliced_table.spans.iter().any(|candidate| {
        candidate.body_range.start >= body_start
            && candidate.body_range.end <= body_start + new_body.len()
            && spliced[candidate.body_range.clone()].trim() == new_body.trim()
    });
    if !exact {
        return Err(ToolError::InvalidArguments(
            "'new_body' precisa ser exatamente o bloco do símbolo; ele fecharia o item e deixaria outro código fora. Nada foi escrito."
                .to_string(),
        ));
    }

    Ok(spliced)
}

pub(super) fn first_parse_error(content: &str) -> Option<usize> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(content, None)?;
    let root = tree.root_node();
    if !root.has_error() {
        return None;
    }
    Some(first_error_node(root).map_or(1, |node| node.start_position().row + 1))
}

pub(super) fn first_error_node<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_error_node(child) {
            return Some(found);
        }
    }
    None
}
