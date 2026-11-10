use std::ops::Range;
use std::path::Path;

use tree_sitter::{Node, Parser};
use tupi_kernel::ToolError;

const HASH_PREFIX_LEN: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSpan {
    pub name: String,
    pub container: Option<String>,
    pub kind: String,
    pub head_range: Range<usize>,
    pub body_range: Range<usize>,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bodyless {
    pub name: String,
    pub container: Option<String>,
    pub kind: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymbolTable {
    pub spans: Vec<SymbolSpan>,
    pub bodyless: Vec<Bodyless>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    Found(SymbolSpan),
    Ambiguous(Vec<SymbolSpan>),
    NotFound { suggestion: Option<String> },
    NoBody { candidates: Vec<String> },
}

#[derive(Debug, Default)]
pub struct SpanResolver;

impl SpanResolver {
    pub fn new() -> Self {
        Self
    }

    pub fn table(&self, path: &Path, content: &str) -> Result<SymbolTable, ToolError> {
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            return Ok(SymbolTable::default());
        }

        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|error| {
                ToolError::Other(format!("falha ao carregar gramática Rust: {error}"))
            })?;
        let tree = parser
            .parse(content, None)
            .ok_or_else(|| ToolError::Other("falha ao parsear o arquivo".to_string()))?;

        let mut table = SymbolTable::default();
        collect_children(tree.root_node(), content, &mut table, None);
        Ok(table)
    }

    /// Resolve nome simples ou qualificado. Ambiguidade nunca é escolhida.
    pub fn resolve(&self, table: &SymbolTable, symbol: &str) -> ResolveOutcome {
        let (container, name) = split_qualified(symbol);

        let matching: Vec<SymbolSpan> = table
            .spans
            .iter()
            .filter(|span| {
                span.name == name && container_matches(span.container.as_deref(), container)
            })
            .cloned()
            .collect();

        match matching.len() {
            1 => return ResolveOutcome::Found(matching.into_iter().next().unwrap()),
            count if count > 1 => return ResolveOutcome::Ambiguous(matching),
            _ => {}
        }

        let bodyless: Vec<String> = table
            .bodyless
            .iter()
            .filter(|item| {
                item.name == name && container_matches(item.container.as_deref(), container)
            })
            .map(render_bodyless)
            .collect();
        if !bodyless.is_empty() {
            return ResolveOutcome::NoBody {
                candidates: bodyless,
            };
        }

        let suggestion = table
            .spans
            .iter()
            .map(|span| span.name.clone())
            .chain(table.bodyless.iter().map(|item| item.name.clone()))
            .map(|candidate| {
                let distance = levenshtein(&candidate, name);
                (candidate, distance)
            })
            .filter(|(_, distance)| *distance <= 3)
            .min_by_key(|(_, distance)| *distance)
            .map(|(candidate, _)| candidate);

        ResolveOutcome::NotFound { suggestion }
    }
}

/// `impl Foo::new — linha 12`, or `fn verify — linha 3`.
pub fn render_candidate(span: &SymbolSpan) -> String {
    match &span.container {
        Some(container) => format!(
            "impl {container}::{} — linha {}",
            span.name, span.start_line
        ),
        None => format!("{} {} — linha {}", span.kind, span.name, span.start_line),
    }
}

fn render_bodyless(item: &Bodyless) -> String {
    match &item.container {
        Some(container) => format!("{container}::{} — linha {}", item.name, item.line),
        None => format!("{} — linha {}", item.name, item.line),
    }
}

/// Linha 1-based para um offset de byte, segura para `\r\n`.
pub fn offset_to_line(content: &str, offset: usize) -> usize {
    let offset = offset.min(content.len());
    content.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

fn split_qualified(symbol: &str) -> (Option<&str>, &str) {
    match symbol.rsplit_once("::") {
        Some((container, name)) => (Some(container.trim()), name.trim()),
        None => (None, symbol.trim()),
    }
}

fn container_matches(actual: Option<&str>, requested: Option<&str>) -> bool {
    let Some(requested) = requested else {
        return true;
    };
    let requested = requested
        .strip_prefix("impl ")
        .or_else(|| requested.strip_prefix("trait "))
        .or_else(|| requested.strip_prefix("mod "))
        .unwrap_or(requested);
    let Some(actual) = actual else {
        return false;
    };
    actual == requested || format!("impl {actual}") == requested
}

fn collect_children(
    node: Node<'_>,
    content: &str,
    table: &mut SymbolTable,
    container: Option<String>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        let is_symbol = matches!(
            kind,
            "function_item"
                | "function_signature_item"
                | "impl_item"
                | "struct_item"
                | "enum_item"
                | "trait_item"
                | "mod_item"
        );
        let is_container = matches!(kind, "impl_item" | "trait_item" | "mod_item");
        let name = item_name(child, content);

        if is_symbol {
            match body_node(child) {
                Some(body) => {
                    let head_range = child.start_byte()..body.start_byte();
                    let body_range = body.start_byte()..body.end_byte();
                    table.spans.push(SymbolSpan {
                        name: name.clone(),
                        container: container.clone(),
                        kind: display_kind(kind),
                        start_line: offset_to_line(content, body_range.start),
                        end_line: offset_to_line(content, body_range.end.saturating_sub(1)),
                        head_range,
                        body_range,
                    });
                }
                None => table.bodyless.push(Bodyless {
                    name: name.clone(),
                    container: container.clone(),
                    kind: display_kind(kind),
                    line: child.start_position().row + 1,
                }),
            }
        }

        let next_container = if is_container {
            container_name(child, content).or(Some(name))
        } else {
            container.clone()
        };
        // Desce por wrappers de sintaxe e containers, mas não entra em corpo de
        // função: itens aninhados não são o que `symbol` nomeia.
        if is_container || !is_symbol {
            collect_children(child, content, table, next_container);
        }
    }
}

fn body_node(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(body) = node.child_by_field_name("body") {
        return Some(body);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| {
            matches!(
                child.kind(),
                "block" | "declaration_list" | "field_declaration_list" | "enum_variant_list"
            )
        })
        .last()
}

fn container_name(node: Node<'_>, content: &str) -> Option<String> {
    let field = match node.kind() {
        "impl_item" if node.child_by_field_name("trait").is_some() => "trait",
        "impl_item" => "type",
        _ => "name",
    };
    node.child_by_field_name(field)
        .and_then(|name| name.utf8_text(content.as_bytes()).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn item_name(node: Node<'_>, content: &str) -> String {
    if let Some(name) = node
        .child_by_field_name("name")
        .and_then(|name| name.utf8_text(content.as_bytes()).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
    {
        return name;
    }
    if node.kind() == "impl_item"
        && let Some(ty) = node
            .child_by_field_name("type")
            .and_then(|ty| ty.utf8_text(content.as_bytes()).ok())
    {
        return format!("impl {}", ty.trim());
    }
    display_kind(node.kind())
}

fn display_kind(kind: &str) -> String {
    match kind {
        "function_item" | "function_signature_item" => "fn".to_string(),
        "impl_item" => "impl".to_string(),
        "struct_item" => "struct".to_string(),
        "enum_item" => "enum".to_string(),
        "trait_item" => "trait".to_string(),
        "mod_item" => "mod".to_string(),
        other => other.to_string(),
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[b.len()]
}

/// 12 hex chars: identifica a versão do arquivo sem custar contexto.
pub fn hash_prefix(content: &[u8]) -> String {
    blake3::hash(content).to_hex()[..HASH_PREFIX_LEN].to_string()
}

/// Aceita o prefixo de 12 que `read` imprime ou o digest completo.
pub fn hash_matches(content: &[u8], expected: &str) -> bool {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.len() < 8 || expected.len() > 64 {
        return false;
    }
    blake3::hash(content)
        .to_hex()
        .to_string()
        .starts_with(&expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn symbols_of(content: &str) -> SymbolTable {
        SpanResolver::new()
            .table(Path::new("x.rs"), content)
            .unwrap()
    }

    fn span<'a>(table: &'a SymbolTable, name: &str) -> &'a SymbolSpan {
        table
            .spans
            .iter()
            .find(|span| span.name == name)
            .unwrap_or_else(|| panic!("faltou {name}: {:?}", table.spans))
    }

    #[test]
    fn a_function_body_range_covers_exactly_the_braces() {
        let content =
            "use std::io;\n\npub fn verify(raw: &str) -> bool {\n    !raw.is_empty()\n}\n";
        let table = symbols_of(content);
        let span = span(&table, "verify");

        assert_eq!(
            &content[span.body_range.clone()],
            "{\n    !raw.is_empty()\n}"
        );
        assert!(content[span.head_range.clone()].ends_with("bool "));
        assert_eq!(span.start_line, 3);
        assert_eq!(span.end_line, 5);
        assert_eq!(span.container, None);
    }

    #[test]
    fn a_method_resolves_to_the_method_not_the_impl() {
        let content = "impl Auth {\n    pub fn verify(&self) -> bool {\n        true\n    }\n}\n";
        let table = symbols_of(content);
        let span = span(&table, "verify");

        assert_eq!(&content[span.body_range.clone()], "{\n        true\n    }");
        assert_eq!(span.container.as_deref(), Some("Auth"));
    }

    #[test]
    fn struct_and_enum_bodies_are_editable() {
        let content = "struct Point {\n    x: u8,\n}\n\nenum Color {\n    Red,\n}\n";
        let table = symbols_of(content);

        assert_eq!(
            &content[span(&table, "Point").body_range.clone()],
            "{\n    x: u8,\n}"
        );
        assert_eq!(
            &content[span(&table, "Color").body_range.clone()],
            "{\n    Red,\n}"
        );
    }

    #[test]
    fn a_trait_impl_is_qualified_by_the_trait() {
        let content = "impl Foo {\n    fn new() -> Self { Foo }\n}\n\nimpl Default for Foo {\n    fn new() -> Self { Foo }\n}\n";
        let table = symbols_of(content);

        match SpanResolver::new().resolve(&table, "Default::new") {
            ResolveOutcome::Found(span) => assert_eq!(span.start_line, 6),
            other => panic!("esperava Found, veio {other:?}"),
        }
        match SpanResolver::new().resolve(&table, "new") {
            ResolveOutcome::Ambiguous(candidates) => assert_eq!(candidates.len(), 2),
            other => panic!("esperava Ambiguous, veio {other:?}"),
        }
    }

    #[test]
    fn a_bodyless_item_is_reported_as_such() {
        let table = symbols_of("trait T {\n    fn required(&self);\n}\n");
        match SpanResolver::new().resolve(&table, "required") {
            ResolveOutcome::NoBody { candidates } => {
                assert!(candidates[0].contains("required"), "{candidates:?}");
            }
            other => panic!("esperava NoBody, veio {other:?}"),
        }
    }

    #[test]
    fn a_missing_symbol_suggests_the_closest_name() {
        let table = symbols_of("pub fn verify_token() {}\n");
        match SpanResolver::new().resolve(&table, "verify_tokn") {
            ResolveOutcome::NotFound {
                suggestion: Some(name),
            } => assert_eq!(name, "verify_token"),
            other => panic!("esperava sugestão, veio {other:?}"),
        }
    }

    #[test]
    fn offset_to_line_is_one_based_and_crlf_safe() {
        let content = "a\r\nbb\r\nccc";
        assert_eq!(offset_to_line(content, 0), 1);
        assert_eq!(offset_to_line(content, 3), 2);
        assert_eq!(offset_to_line(content, 7), 3);
    }

    #[test]
    fn hash_prefix_is_twelve_hex_and_matching_accepts_a_prefix() {
        let content = b"pub fn x() {}\n";
        let prefix = hash_prefix(content);
        assert_eq!(prefix.len(), 12);
        assert!(hash_matches(content, &prefix));
        assert!(hash_matches(content, &prefix[..8]));
        assert!(!hash_matches(content, "deadbeef"));
        assert!(!hash_matches(content, "abc"));
    }
}
