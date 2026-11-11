use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;
use petgraph::stable_graph::NodeIndex;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, QueryCursor};
use tupi_kernel::ToolError;

use crate::ast::{AstChunker, Language};
use crate::graph::model::{ProjectCodeGraph, RelationEdge, RelationKind, SymbolNode};
use crate::graph::parse_cache::ParseCache;
use crate::graph::queries::GraphQueries;
use crate::graph::scope::{
    CallSite, FileScope, ImportTarget, Resolution, ScopeResolver, UseItem, crate_name_for,
    expand_use, pick_target,
};

pub struct GraphBuilder {
    queries: GraphQueries,
    cache: Option<Arc<ParseCache>>,
}

struct ParsedFile {
    relative: PathBuf,
    content: String,
    language: Language,
    tree: Option<Arc<tree_sitter::Tree>>,
}

impl GraphBuilder {
    pub fn new() -> Result<Self, ToolError> {
        Ok(Self {
            queries: GraphQueries::new()?,
            cache: None,
        })
    }

    pub fn with_parse_cache(mut self, cache: Arc<ParseCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    fn parse_tree(
        &self,
        path: &Path,
        content: &str,
        language: Language,
    ) -> Option<Arc<tree_sitter::Tree>> {
        match &self.cache {
            Some(cache) => cache.parse(path, language, content),
            None => parse(content, language).map(Arc::new),
        }
    }

    pub fn build(&self, root: &Path) -> Result<ProjectCodeGraph, ToolError> {
        let files = collect_files(root)?;
        let mut graph = ProjectCodeGraph::new();
        let mut parsed = Vec::new();
        let chunker = match &self.cache {
            Some(cache) => AstChunker::with_cache(cache.clone()),
            None => AstChunker::new(),
        };

        for path in files {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let relative = PathBuf::from(crate::paths::relative(root, &path));
            let Some(language) = Language::from_path(&path) else {
                continue;
            };
            graph
                .file_hashes
                .insert(path.clone(), blake3::hash(content.as_bytes()));
            graph
                .resolver
                .register_file(&relative, &crate_name_for(&relative));

            let snippets = chunker.snippets(&path, &content);
            for snippet in snippets {
                let is_callable =
                    snippet.signature.contains("fn ") || snippet.signature.contains("fun ");
                let is_public = if language == Language::Kotlin {
                    !snippet.signature.contains("private ")
                        && !snippet.signature.contains("internal ")
                } else {
                    snippet.signature.trim_start().starts_with("pub")
                };
                let is_test = relative.to_string_lossy().contains("/tests/")
                    || relative.to_string_lossy().starts_with("tests/")
                    || has_test_attribute(&content, snippet.start_line)
                    || snippet.code.contains("#[cfg(test)]")
                    || snippet.code.contains("@Test");
                graph.graph.add_node(SymbolNode {
                    name: snippet.symbol_name,
                    qualified: snippet.qualified_name,
                    relative: relative.to_string_lossy().to_string(),
                    start_line: snippet.start_line,
                    end_line: snippet.end_line,
                    signature: snippet.signature,
                    is_public,
                    is_callable,
                    is_test,
                    language,
                });
            }

            parsed.push(ParsedFile {
                tree: self.parse_tree(&path, &content, language),
                relative,
                content,
                language,
            });
        }

        for file in &parsed {
            let Some(tree) = &file.tree else {
                continue;
            };
            let scope = collect_imports(
                &self.queries,
                file.language,
                &tree.root_node(),
                &file.content,
                &file.relative,
                &graph.resolver,
            );
            graph.scopes.insert(file.relative.clone(), scope);
        }

        for file in &parsed {
            let Some(tree) = &file.tree else {
                continue;
            };
            let calls = collect_calls(
                &self.queries,
                file.language,
                &tree.root_node(),
                &file.content,
            );
            let bindings = collect_bindings(&file.content);
            let callers = graph
                .graph
                .node_indices()
                .filter(|&index| {
                    graph.graph.node_weight(index).is_some_and(|node| {
                        node.relative == file.relative.to_string_lossy() && node.is_callable
                    })
                })
                .collect::<Vec<_>>();

            for call in calls {
                let Some(caller) = callers.iter().copied().find(|&index| {
                    graph.graph.node_weight(index).is_some_and(|node| {
                        node.start_line <= call.line && node.end_line >= call.line
                    })
                }) else {
                    continue;
                };

                let mut definitions: Vec<NodeIndex> = graph
                    .graph
                    .node_indices()
                    .filter(|&index| {
                        graph.graph.node_weight(index).is_some_and(|node| {
                            node.name == call.name && node.language == file.language
                        })
                    })
                    .collect();
                if let Some(receiver) = &call.receiver
                    && let Some(ty) = bindings.get(receiver)
                {
                    let typed: Vec<NodeIndex> = definitions
                        .iter()
                        .copied()
                        .filter(|&index| {
                            graph.graph.node_weight(index).is_some_and(|node| {
                                node.qualified.starts_with(&format!("{ty}::"))
                                    || node.qualified.contains(&format!("::{ty}::"))
                            })
                        })
                        .collect();
                    if !typed.is_empty() {
                        definitions = typed;
                    }
                }

                let scope = graph
                    .scopes
                    .get(&file.relative)
                    .cloned()
                    .unwrap_or_default();
                let resolution = pick_target(
                    &call.name,
                    &file.relative,
                    &scope,
                    &graph.resolver,
                    call.path.as_deref(),
                    &definitions,
                    &graph,
                );
                match resolution {
                    Resolution::Resolved(target) => {
                        graph.graph.add_edge(
                            caller,
                            target,
                            RelationEdge {
                                line: call.line,
                                kind: RelationKind::Call,
                            },
                        );
                    }
                    Resolution::Ambiguous { .. } => {
                        *graph.dropped.entry(call.name.clone()).or_default() += 1;
                    }
                    Resolution::Unknown => {}
                }
            }
        }

        graph.rebuild_defs();
        Ok(graph)
    }
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>, ToolError> {
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(true)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != "target");

    let mut files = Vec::new();
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if Language::from_path(path).is_some() {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn parse(content: &str, language: Language) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language.tree_sitter_language()).ok()?;
    parser.parse(content, None)
}

fn collect_imports(
    queries: &GraphQueries,
    language: Language,
    root: &Node<'_>,
    content: &str,
    relative: &Path,
    resolver: &ScopeResolver,
) -> FileScope {
    let Some(query) = queries.imports_query(language) else {
        return FileScope::default();
    };
    let Some(module) = resolver.caller_module(relative) else {
        return FileScope::default();
    };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, *root, content.as_bytes());
    let path_index = query.capture_index_for_name("import.path");
    let mut scope = FileScope::default();

    while let Some(m) = matches.next() {
        for capture in m.captures() {
            if Some(capture.index) != path_index {
                continue;
            }
            let Ok(text) = capture.node.utf8_text(content.as_bytes()) else {
                continue;
            };
            for item in expand_use(text) {
                match item {
                    UseItem::Named { name, path } => {
                        let target = resolver
                            .resolve_import_in(relative, &module, &path)
                            .unwrap_or(ImportTarget::External);
                        scope.imports.insert(name, target);
                    }
                    UseItem::Glob { path } => {
                        if let Some(ImportTarget::File(file)) =
                            resolver.resolve_import_in(relative, &module, &path)
                        {
                            scope.globs.push(file);
                        }
                    }
                }
            }
        }
    }
    scope
}

fn collect_calls(
    queries: &GraphQueries,
    language: Language,
    root: &Node<'_>,
    content: &str,
) -> Vec<CallSite> {
    let query = queries.calls_query(language);
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, *root, content.as_bytes());
    let name_index = query.capture_index_for_name("call.name");
    let receiver_index = query.capture_index_for_name("call.receiver");
    let path_index = query.capture_index_for_name("call.path");
    let mut calls = Vec::new();

    while let Some(m) = matches.next() {
        let mut name = None;
        let mut receiver = None;
        let mut call_path = None;
        for capture in m.captures() {
            let text = capture
                .node
                .utf8_text(content.as_bytes())
                .ok()
                .map(str::to_string);
            if Some(capture.index) == name_index {
                name = text;
            } else if Some(capture.index) == receiver_index {
                receiver = text;
            } else if Some(capture.index) == path_index {
                call_path = text;
            }
        }
        if let Some(name) = name {
            calls.push(CallSite {
                name,
                line: m.captures()[0].node.start_position().row + 1,
                receiver,
                path: call_path,
            });
        }
    }
    calls
}

fn collect_bindings(content: &str) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if let Some((_, after)) = line.split_once("let ") {
            let rest = after.trim_start_matches("mut ").trim();
            if let Some((left, right)) = rest.split_once('=') {
                let name = left.split(':').next().unwrap_or("").trim();
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    let ty = type_from_expr(right).or_else(|| {
                        left.split_once(':')
                            .map(|(_, ty)| clean_type(ty))
                            .filter(|ty| !ty.is_empty())
                    });
                    if let Some(ty) = ty {
                        bindings.insert(name.to_string(), ty);
                    }
                }
            } else if let Some((name, ty)) = rest.split_once(':') {
                let name = name.trim();
                if !name.is_empty() {
                    bindings.insert(name.to_string(), clean_type(ty));
                }
            }
        }
    }
    bindings
}

fn type_from_expr(expr: &str) -> Option<String> {
    let expr = expr.trim().trim_start_matches('&').trim();
    let head = expr.split(['(', '{', ';']).next()?;
    let (ty, rest) = head.split_once("::")?;
    if rest.trim().is_empty() {
        return None;
    }
    let ty = clean_type(ty);
    (!ty.is_empty()).then_some(ty)
}

fn clean_type(ty: &str) -> String {
    ty.trim()
        .trim_start_matches('&')
        .trim()
        .trim_start_matches("mut ")
        .trim()
        .trim_end_matches('<')
        .split('<')
        .next()
        .unwrap_or(ty)
        .trim()
        .to_string()
}

fn has_test_attribute(content: &str, start_line: usize) -> bool {
    let from = start_line.saturating_sub(4);
    content
        .lines()
        .skip(from)
        .take(start_line.saturating_sub(from))
        .any(|line| line.contains("#[test]") || line.contains("::test]"))
}
