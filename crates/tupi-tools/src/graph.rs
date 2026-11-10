use std::collections::{HashMap, HashSet};
use std::path::Path;

use ignore::WalkBuilder;

use crate::ast::AstChunker;

#[derive(Debug, Clone)]
pub struct Def {
    pub name: String,
    pub qualified: String,
    pub relative: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub is_public: bool,
    pub is_callable: bool,
    pub is_test: bool,
    pub calls: Vec<String>,
}

pub struct CallGraph {
    pub defs: Vec<Def>,
}

const KEYWORDS: &[&str] = &[
    "if", "while", "for", "match", "loop", "return", "fn", "let", "impl", "struct", "enum",
    "trait", "mod", "use", "pub", "const", "static", "async", "await", "move", "unsafe", "where",
    "in", "as", "dyn", "ref", "mut", "type", "else", "break", "continue", "self", "Self", "super",
    "crate", "extern", "true", "false",
];

impl CallGraph {
    pub fn build(root: &Path) -> Self {
        let mut builder = WalkBuilder::new(root);
        builder
            .standard_filters(true)
            .hidden(true)
            .require_git(false)
            .filter_entry(|entry| entry.file_name() != "target");

        let mut defs = Vec::new();
        for entry in builder.build().flatten() {
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let relative = crate::paths::relative(root, path);
            for snippet in AstChunker::new().snippets(path, &content) {
                let is_callable = snippet.signature.contains("fn ");
                let is_public = snippet.signature.trim_start().starts_with("pub");
                let is_test = relative.contains("/tests/")
                    || relative.starts_with("tests/")
                    || has_test_attribute(&content, snippet.start_line);
                let calls = if is_callable {
                    calls_in(&snippet.code)
                } else {
                    Vec::new()
                };
                defs.push(Def {
                    name: snippet.symbol_name,
                    qualified: snippet.qualified_name,
                    relative: relative.clone(),
                    start_line: snippet.start_line,
                    end_line: snippet.end_line,
                    signature: snippet.signature,
                    is_public,
                    is_callable,
                    is_test,
                    calls,
                });
            }
        }

        Self { defs }
    }

    pub fn matching(&self, query: &str) -> Vec<&Def> {
        let query = query.trim();
        let query = query.strip_suffix("()").unwrap_or(query).trim();
        self.defs
            .iter()
            .filter(|def| {
                def.name == query
                    || def.qualified == query
                    || def.qualified.ends_with(&format!("::{query}"))
            })
            .collect()
    }

    pub fn callers(&self, name: &str) -> Vec<&Def> {
        self.defs
            .iter()
            .filter(|def| {
                def.is_callable && def.name != name && def.calls.iter().any(|call| call == name)
            })
            .collect()
    }

    pub fn callees(&self, def: &Def) -> Vec<&Def> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for call in &def.calls {
            for target in self.matching(call) {
                let key = (&target.qualified, &target.relative);
                if seen.insert(key) {
                    out.push(target);
                }
            }
        }
        out
    }

    pub fn trace(&self, from: &str, to: &str, max_depth: usize) -> Option<Vec<String>> {
        let target_matches = |name: &str| {
            self.matching(name)
                .iter()
                .any(|def| def.name == to || def.qualified == to)
                || name == to
        };

        let starts: Vec<String> = self
            .matching(from)
            .iter()
            .map(|def| def.name.clone())
            .collect();
        if starts.is_empty() {
            return None;
        }
        if starts.iter().any(|name| target_matches(name)) {
            return Some(starts);
        }

        let mut parents: HashMap<String, String> = HashMap::new();
        let mut frontier = starts.clone();
        let mut visited: HashSet<String> = starts.into_iter().collect();

        for _ in 0..max_depth {
            let mut next = Vec::new();
            for name in &frontier {
                for def in self
                    .defs
                    .iter()
                    .filter(|def| &def.name == name && def.is_callable)
                {
                    for call in &def.calls {
                        if !visited.insert(call.clone()) {
                            continue;
                        }
                        parents.insert(call.clone(), name.clone());
                        if target_matches(call) {
                            let mut path = vec![call.clone()];
                            let mut current = call.clone();
                            while let Some(parent) = parents.get(&current) {
                                path.push(parent.clone());
                                current = parent.clone();
                            }
                            path.reverse();
                            return Some(path);
                        }
                        next.push(call.clone());
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        None
    }
}

fn has_test_attribute(content: &str, start_line: usize) -> bool {
    let from = start_line.saturating_sub(4);
    content
        .lines()
        .skip(from)
        .take(start_line.saturating_sub(from))
        .any(|line| line.contains("#[test]") || line.contains("::test]"))
}

pub fn calls_in(code: &str) -> Vec<String> {
    let code = match code.find('{') {
        Some(index) => &code[index..],
        None => code,
    };
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut ident = String::new();
    let mut line_comment = false;
    let mut block_depth = 0usize;
    let mut string = false;
    let mut escaped = false;
    let mut index = 0;

    while index < chars.len() {
        let current = chars[index];
        let next = chars.get(index + 1).copied();

        if line_comment {
            if current == '\n' {
                line_comment = false;
            }
            index += 1;
            continue;
        }
        if block_depth > 0 {
            if current == '/' && next == Some('*') {
                block_depth += 1;
                index += 2;
                continue;
            }
            if current == '*' && next == Some('/') {
                block_depth -= 1;
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if string {
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                string = false;
            }
            index += 1;
            continue;
        }
        if current == '/' && next == Some('/') {
            line_comment = true;
            index += 2;
            continue;
        }
        if current == '/' && next == Some('*') {
            block_depth = 1;
            index += 2;
            continue;
        }
        if current == '"' {
            string = true;
            ident.clear();
            index += 1;
            continue;
        }
        if current == '\'' {
            if next == Some('\\') {
                index += 2;
                while index < chars.len() {
                    if chars[index] == '\\' {
                        index += 2;
                        continue;
                    }
                    if chars[index] == '\'' {
                        index += 1;
                        break;
                    }
                    index += 1;
                }
                ident.clear();
                continue;
            }
            if chars.get(index + 2) == Some(&'\'') {
                index += 3;
                ident.clear();
                continue;
            }
            ident.clear();
            index += 1;
            continue;
        }

        if current.is_alphanumeric() || current == '_' {
            ident.push(current);
        } else if current == '(' {
            if !ident.is_empty()
                && !KEYWORDS.contains(&ident.as_str())
                && seen.insert(ident.clone())
            {
                out.push(ident.clone());
            }
            ident.clear();
        } else {
            ident.clear();
        }
        index += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn calls_ignore_comments_strings_and_macros() {
        let calls =
            calls_in("fn a() { b(); println!(\"c()\"); // d()\n let x = 'e'; /* f() */ g(h()); }");
        assert_eq!(calls, vec!["b", "g", "h"]);
    }

    #[test]
    fn lifetimes_do_not_swallow_the_body() {
        let calls = calls_in("fn a<'x>(v: &'x str) { b(v); }");
        assert_eq!(calls, vec!["b"]);
    }

    #[test]
    fn the_graph_finds_callers_and_callees_across_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "pub fn helper() {}\n");
        write(dir.path(), "src/b.rs", "pub fn caller() { helper(); }\n");
        let graph = CallGraph::build(dir.path());
        let callers = graph.callers("helper");
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "caller");
        let helper = graph.matching("helper").remove(0);
        assert_eq!(helper.relative, "src/a.rs");
        assert!(helper.is_public);
        assert!(
            graph
                .callees(callers[0])
                .iter()
                .any(|def| def.name == "helper")
        );
    }

    #[test]
    fn a_trace_walks_multiple_hops() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "src/a.rs",
            "pub fn a() { b(); }\nfn b() { c(); }\nfn c() {}\n",
        );
        let graph = CallGraph::build(dir.path());
        assert_eq!(
            graph.trace("a", "c", 6),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
        assert_eq!(graph.trace("c", "a", 6), None);
    }

    #[test]
    fn a_test_is_marked_by_attribute() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "src/a.rs", "#[test]\nfn it_works() {}\n");
        let graph = CallGraph::build(dir.path());
        assert!(graph.matching("it_works").remove(0).is_test);
    }
}
