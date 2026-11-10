use std::path::Path;

use tree_sitter::Node;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstSnippet {
    pub symbol_name: String,
    pub qualified_name: String,
    pub signature: String,
    pub code: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Default, Clone)]
pub struct AstChunker;

impl AstChunker {
    pub fn new() -> Self {
        Self
    }

    pub fn snippets(&self, path: &Path, content: &str) -> Vec<AstSnippet> {
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            return Vec::new();
        }

        let mut parser = tree_sitter::Parser::new();
        if parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .is_err()
        {
            return Vec::new();
        }
        let Some(tree) = parser.parse(content, None) else {
            return Vec::new();
        };

        let bytes = content.as_bytes();
        let mut snippets = Vec::new();
        collect_snippets(tree.root_node(), bytes, &mut snippets);
        disambiguate(&mut snippets);
        snippets
    }
}

fn is_scope(kind: &str) -> bool {
    matches!(
        kind,
        "function_item" | "impl_item" | "struct_item" | "enum_item" | "trait_item" | "mod_item"
    )
}

fn collect_snippets(node: Node<'_>, source: &[u8], snippets: &mut Vec<AstSnippet>) {
    if is_scope(node.kind()) {
        let code = node.utf8_text(source).unwrap_or("").to_string();
        if !code.is_empty() {
            let symbol_name = symbol_name(node, source);
            let qualified_name = qualified_name(node, source, &symbol_name);
            let start_line = node.start_position().row + 1;
            let end_line = start_line + code.lines().count().saturating_sub(1);
            snippets.push(AstSnippet {
                symbol_name,
                qualified_name,
                signature: compact_signature(&signature(node, source)),
                code,
                start_line,
                end_line,
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_snippets(child, source, snippets);
    }
}

fn symbol_name(node: Node<'_>, source: &[u8]) -> String {
    if node.kind() == "impl_item"
        && let Some(type_node) = node.child_by_field_name("type")
    {
        return format!("impl {}", type_node.utf8_text(source).unwrap_or(""));
    }
    if let Some(name) = node.child_by_field_name("name") {
        let name = name.utf8_text(source).unwrap_or("");
        if !name.is_empty() {
            return name.to_string();
        }
    }
    node.kind().to_string()
}

fn qualified_name(node: Node<'_>, source: &[u8], symbol: &str) -> String {
    let mut containers = Vec::new();
    let mut current = node.parent();
    while let Some(parent) = current {
        let container = match parent.kind() {
            "impl_item" => parent
                .child_by_field_name("type")
                .map(|type_node| type_node.utf8_text(source).unwrap_or("").to_string()),
            "mod_item" => parent
                .child_by_field_name("name")
                .map(|name| name.utf8_text(source).unwrap_or("").to_string()),
            _ => None,
        };
        if let Some(container) = container.filter(|container| !container.is_empty()) {
            containers.push(container);
        }
        current = parent.parent();
    }
    containers.reverse();
    containers.push(symbol.to_string());
    containers.join("::")
}

/// Dois símbolos com o mesmo nome qualificado no mesmo arquivo (dois `impl Foo`
/// com `new`, ou `impl Foo` inerente e `impl Trait for Foo`) recebem a linha no
/// id, para o ledger não confundir os corpos. O caso comum fica sem sufixo.
fn disambiguate(snippets: &mut [AstSnippet]) {
    use std::collections::HashMap;

    let mut counts: HashMap<String, usize> = HashMap::new();
    for snippet in snippets.iter() {
        *counts.entry(snippet.qualified_name.clone()).or_default() += 1;
    }

    for snippet in snippets.iter_mut() {
        if counts
            .get(&snippet.qualified_name)
            .copied()
            .unwrap_or(0)
            > 1
        {
            snippet.qualified_name = format!("{}@{}", snippet.qualified_name, snippet.start_line);
        }
    }
}

fn signature(node: Node<'_>, source: &[u8]) -> String {
    let text = node.utf8_text(source).unwrap_or("");
    if let Some(body) = node.child_by_field_name("body") {
        let end = body.start_byte().saturating_sub(node.start_byte());
        return text[..end].trim().to_string();
    }
    if let Some(index) = text.find('{') {
        return text[..index].trim().to_string();
    }
    if let Some(index) = text.find(';') {
        return text[..=index].trim().to_string();
    }
    text.lines().next().unwrap_or("").trim().to_string()
}

/// Uma assinatura multiline em uma linha. Só quebras e indentação são colapsadas;
/// o espaçamento dentro da linha é preservado.
fn compact_signature(signature: &str) -> String {
    let mut out = String::with_capacity(signature.len());
    for line in signature.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const SOURCE: &str = "pub struct Auth;\n\nimpl Auth {\n    pub fn verify(&self, raw: &str) -> bool {\n        !raw.is_empty()\n    }\n}\n\npub fn logout() {\n}\n";

    #[test]
    fn snippets_include_impl_methods_and_free_functions() {
        let snippets = AstChunker::new().snippets(Path::new("auth.rs"), SOURCE);
        let names: Vec<&str> = snippets.iter().map(|s| s.symbol_name.as_str()).collect();

        assert!(names.contains(&"Auth"), "{names:?}");
        assert!(names.contains(&"verify"), "{names:?}");
        assert!(names.contains(&"logout"), "{names:?}");

        let verify = snippets
            .iter()
            .find(|s| s.symbol_name == "verify")
            .unwrap();
        assert_eq!(verify.qualified_name, "Auth::verify");
        assert_eq!(verify.start_line, 4);
        assert_eq!(verify.end_line, 6);
        assert!(verify.signature.starts_with("pub fn verify"));
        assert!(verify.code.contains("!raw.is_empty()"));
    }

    #[test]
    fn multiline_signatures_are_compacted_to_one_line() {
        let source = "pub fn affected<'a>(\n    graph: &'a u8,\n) -> u8 {\n    1\n}\n";
        let snippets = AstChunker::new().snippets(Path::new("a.rs"), source);

        assert_eq!(
            snippets[0].signature,
            "pub fn affected<'a>( graph: &'a u8, ) -> u8"
        );
    }

    #[test]
    fn duplicate_names_keep_their_qualified_identity() {
        let source = "impl A {\n    fn new() {}\n}\n\nimpl B {\n    fn new() {}\n}\n";
        let snippets = AstChunker::new().snippets(Path::new("a.rs"), source);
        let mut qualified: Vec<&str> = snippets
            .iter()
            .filter(|s| s.symbol_name == "new")
            .map(|s| s.qualified_name.as_str())
            .collect();
        qualified.sort();

        assert_eq!(qualified, vec!["A::new", "B::new"]);
    }

    #[test]
    fn nested_modules_keep_the_full_ancestor_chain() {
        let source = "mod a {\n    impl Foo {\n        fn new() {}\n    }\n}\n\nmod b {\n    impl Foo {\n        fn new() {}\n    }\n}\n";
        let snippets = AstChunker::new().snippets(Path::new("a.rs"), source);
        let mut names: Vec<&str> = snippets
            .iter()
            .filter(|s| s.symbol_name == "new")
            .map(|s| s.qualified_name.as_str())
            .collect();
        names.sort();

        assert_eq!(names, vec!["a::Foo::new", "b::Foo::new"]);
    }

    #[test]
    fn same_container_duplicates_get_a_line_suffix() {
        let source = "impl Foo {\n    fn new() {}\n}\n\nimpl Bar for Foo {\n    fn new() {}\n}\n";
        let snippets = AstChunker::new().snippets(Path::new("a.rs"), source);
        let names: Vec<&str> = snippets
            .iter()
            .filter(|s| s.symbol_name == "new")
            .map(|s| s.qualified_name.as_str())
            .collect();

        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|name| name.starts_with("Foo::new@")), "{names:?}");
        assert_ne!(names[0], names[1]);
    }

    #[test]
    fn unsupported_extension_has_no_snippets() {
        assert!(AstChunker::new()
            .snippets(Path::new("README.md"), "# tupi")
            .is_empty());
    }

    #[test]
    fn broken_source_does_not_panic() {
        let snippets = AstChunker::new().snippets(Path::new("a.rs"), "fn broken(");
        for snippet in snippets {
            assert!(!snippet.code.is_empty());
        }
    }
}
