use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;
use petgraph::stable_graph::NodeIndex;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, QueryCursor};
use tupi_kernel::ToolError;

use crate::ast::{AstChunker, Language};
use crate::graph::model::{
    ProjectCodeGraph, RelationEdge, RelationKind, SectionId, SectionKind, SymbolNode,
};
use crate::graph::parse_cache::ParseCache;
use crate::graph::queries::GraphQueries;
use crate::graph::scope::{
    CallSite, FileScope, ImportBinding, ImportTarget, Resolution, ScopeResolver, UseItem,
    crate_name_for, expand_use, last_segment, pick_target,
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
                let is_container = snippet.signature.contains("struct ")
                    || snippet.signature.contains("class ")
                    || snippet.signature.contains("enum ")
                    || snippet.signature.contains("trait ")
                    || snippet.signature.contains("interface ");
                let kind = if is_container {
                    SectionKind::Container
                } else {
                    SectionKind::Member
                };
                graph.graph.add_node(SymbolNode {
                    name: snippet.symbol_name,
                    qualified: snippet.qualified_name.clone(),
                    relative: relative.to_string_lossy().to_string(),
                    start_line: snippet.start_line,
                    end_line: snippet.end_line,
                    signature: snippet.signature,
                    is_public,
                    is_callable,
                    is_test,
                    language,
                    kind,
                    id: SectionId::new(&relative.to_string_lossy(), &snippet.qualified_name),
                    hash: blake3::hash(snippet.code.as_bytes()),
                });
            }

            parsed.push(ParsedFile {
                tree: self.parse_tree(&path, &content, language),
                relative,
                content,
                language,
            });
        }

        let container_indices: Vec<(NodeIndex, String, String)> = graph
            .graph
            .node_indices()
            .filter_map(|idx| {
                let node = graph.graph.node_weight(idx)?;
                if node.kind == SectionKind::Container {
                    Some((idx, node.name.clone(), node.relative.clone()))
                } else {
                    None
                }
            })
            .collect();

        for member_idx in graph.graph.node_indices().collect::<Vec<_>>() {
            let Some(member) = graph.graph.node_weight(member_idx) else {
                continue;
            };
            if member.kind != SectionKind::Member {
                continue;
            }
            if let Some((container_name, _)) = member.qualified.rsplit_once("::") {
                let target = container_indices
                    .iter()
                    .find(|(_, name, rel)| name == container_name && rel == &member.relative)
                    .or_else(|| {
                        container_indices
                            .iter()
                            .find(|(_, name, _)| name == container_name)
                    });
                if let Some(&(container_idx, _, _)) = target {
                    graph.graph.add_edge(
                        member_idx,
                        container_idx,
                        RelationEdge {
                            line: member.start_line,
                            kind: RelationKind::BelongsTo,
                        },
                    );
                }
            }
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

                if call.receiver.as_deref() == Some("self")
                    && let Some(container) = graph.container_of(caller)
                {
                    if let Some(target) = graph
                        .members_of(container)
                        .into_iter()
                        .find(|&idx| graph.graph[idx].name == call.name)
                    {
                        graph.graph.add_edge(
                            caller,
                            target,
                            RelationEdge {
                                line: call.line,
                                kind: RelationKind::Call,
                            },
                        );
                        continue;
                    }
                }

                let scope = graph
                    .scopes
                    .get(&file.relative)
                    .cloned()
                    .unwrap_or_default();
                let mut definitions: Vec<NodeIndex> = graph
                    .graph
                    .node_indices()
                    .filter(|&index| {
                        graph.graph.node_weight(index).is_some_and(|node| {
                            node.name == call.name && node.language == file.language
                        })
                    })
                    .collect();
                if definitions.is_empty()
                    && let Some(binding) = scope.imports.get(&call.name)
                    && binding.item != call.name
                {
                    let target_file = match &binding.target {
                        ImportTarget::File(file) => Some(file.as_path()),
                        ImportTarget::External => None,
                    };
                    definitions = graph
                        .graph
                        .node_indices()
                        .filter(|&index| {
                            graph.graph.node_weight(index).is_some_and(|node| {
                                node.name == binding.item
                                    && node.language == file.language
                                    && target_file.is_none_or(|target| {
                                        Path::new(node.relative.as_str()) == target
                                    })
                            })
                        })
                        .collect();
                }
                if let Some(receiver) = &call.receiver
                    && let Some(ty) = binding_type_at(&bindings, receiver, call.line)
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
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(true)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker {
        let entry = entry.map_err(|error| ToolError::Other(error.to_string()))?;
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            let path = entry.into_path();
            if Language::from_path(&path).is_some() {
                files.push(path);
            }
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
                        let item = last_segment(&path).unwrap_or(&name).to_string();
                        scope.imports.insert(name, ImportBinding { target, item });
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
    use std::collections::BTreeMap;
    let query = queries.calls_query(language);
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, *root, content.as_bytes());
    let name_index = query.capture_index_for_name("call.name");
    let receiver_index = query.capture_index_for_name("call.receiver");
    let path_index = query.capture_index_for_name("call.path");
    let mut sites: BTreeMap<(usize, usize), CallSite> = BTreeMap::new();

    while let Some(m) = matches.next() {
        let mut name = None;
        let mut name_node = None;
        let mut receiver = None;
        let mut call_path = None;
        for capture in m.captures() {
            let text = capture
                .node
                .utf8_text(content.as_bytes())
                .ok()
                .map(str::to_string);
            if Some(capture.index) == name_index {
                if let Some(text) = text {
                    name = Some(text);
                    name_node = Some(capture.node);
                }
            } else if Some(capture.index) == receiver_index {
                receiver = text;
            } else if Some(capture.index) == path_index {
                call_path = text;
            }
        }
        let (Some(name), Some(name_node)) = (name, name_node) else {
            continue;
        };
        let site = sites
            .entry((name_node.start_byte(), name_node.end_byte()))
            .or_insert_with(|| CallSite {
                name,
                line: name_node.start_position().row + 1,
                receiver: None,
                path: None,
            });
        if site.path.is_none() {
            site.path = call_path;
        }
        if site.receiver.is_none() {
            site.receiver = receiver;
        }
    }
    sites.into_values().collect()
}

fn collect_bindings(content: &str) -> HashMap<String, Vec<(usize, String)>> {
    let mut bindings: HashMap<String, Vec<(usize, String)>> = HashMap::new();
    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
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
                        bindings
                            .entry(name.to_string())
                            .or_default()
                            .push((line_number, ty));
                    }
                }
            } else if let Some((name, ty)) = rest.split_once(':') {
                let name = name.trim();
                if !name.is_empty() {
                    bindings
                        .entry(name.to_string())
                        .or_default()
                        .push((line_number, clean_type(ty)));
                }
            }
        }

        if let Some(fn_at) = line.find("fn ")
            && let Some(open) = line[fn_at..].find('(')
        {
            let after_open = fn_at + open + 1;
            if let Some(close) = line[after_open..].find(')') {
                let params = &line[after_open..after_open + close];
                for param in params.split(',') {
                    if let Some((name, ty)) = param.split_once(':') {
                        let name = name.trim().trim_start_matches("mut ").trim();
                        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                        {
                            bindings
                                .entry(name.to_string())
                                .or_default()
                                .push((line_number, clean_type(ty)));
                        }
                    }
                }
            }
        }

        if !line.starts_with("fn ") && !line.starts_with("let ") && !line.starts_with("//") {
            let field_line = line
                .trim_start_matches("pub ")
                .trim_start_matches("pub(crate) ")
                .trim();
            if let Some((name, ty)) = field_line.split_once(':') {
                let name = name.trim();
                let ty = ty.trim().trim_end_matches(',').trim();
                if !name.is_empty()
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !ty.is_empty()
                {
                    let clean = clean_type(ty);
                    bindings
                        .entry(name.to_string())
                        .or_default()
                        .push((line_number, clean.clone()));
                    bindings
                        .entry(format!("self.{name}"))
                        .or_default()
                        .push((line_number, clean));
                }
            }
        }
    }
    bindings
}

fn binding_type_at(
    bindings: &HashMap<String, Vec<(usize, String)>>,
    name: &str,
    line: usize,
) -> Option<String> {
    bindings.get(name).and_then(|entries| {
        entries
            .iter()
            .filter(|(at, _)| *at <= line)
            .max_by_key(|(at, _)| *at)
            .map(|(_, ty)| ty.clone())
    })
}

fn type_from_expr(expr: &str) -> Option<String> {
    let expr = expr.trim().trim_end_matches(';').trim();
    if let Some((type_part, _)) = expr.split_once("::new(") {
        return Some(clean_type(type_part));
    }
    if let Some((type_part, _)) = expr.split_once("::default(") {
        return Some(clean_type(type_part));
    }
    None
}

fn clean_type(ty: &str) -> String {
    let ty = ty.trim();
    let ty = ty.strip_prefix('&').unwrap_or(ty).trim();
    let ty = ty.strip_prefix("mut ").unwrap_or(ty).trim();
    if let Some(inner) = ty.strip_prefix("Arc<").and_then(|t| t.strip_suffix('>')) {
        return clean_type(inner);
    }
    if let Some(inner) = ty.strip_prefix("Box<").and_then(|t| t.strip_suffix('>')) {
        return clean_type(inner);
    }
    ty.split('<').next().unwrap_or(ty).trim().to_string()
}

fn has_test_attribute(content: &str, start_line: usize) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    let check_range = start_line.saturating_sub(5)..start_line;
    for line_idx in check_range {
        if let Some(line) = lines.get(line_idx) {
            let trimmed = line.trim();
            if trimmed == "#[test]" || trimmed == "@Test" || trimmed.starts_with("#[tokio::test") {
                return true;
            }
        }
    }
    false
}
