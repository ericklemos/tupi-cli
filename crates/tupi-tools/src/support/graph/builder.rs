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
    path: PathBuf,
    relative: PathBuf,
    language: Language,
    file_hash: blake3::Hash,
    nodes: Vec<SymbolNode>,
    scope: Option<FileScope>,
    calls: Vec<CallSite>,
    bindings: Option<HashMap<String, Vec<(usize, String)>>>,
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

    pub fn build(&self, root: &Path) -> Result<ProjectCodeGraph, ToolError> {
        let files = collect_files(root)?;
        let mut graph = ProjectCodeGraph::new();

        // Passo 1 (serial e barato): registra todos os arquivos no resolvedor de
        // escopo. `collect_imports` só lê o resolvedor, então registrá-lo antes
        // permite ler/parsear em paralelo logo depois.
        let mut targets: Vec<(PathBuf, PathBuf, Language)> = Vec::with_capacity(files.len());
        for path in files {
            let Some(language) = Language::from_path(&path) else {
                continue;
            };
            let relative = PathBuf::from(crate::paths::relative(root, &path));
            graph
                .resolver
                .register_file(&relative, &crate_name_for(&relative));
            targets.push((path, relative, language));
        }

        // Passo 2 (paralelo): lê, faz o hash, parseia e extrai símbolos, escopo e
        // chamadas. Conteúdo e árvore morrem no worker; só os dados derivados
        // sobrevivem, o que reduz o pico de RSS do grafo inteiro.
        let parsed = parse_files(&self.queries, self.cache.as_ref(), &graph.resolver, targets);

        // Passo 3 (serial): insere nós e escopos na ordem dos arquivos, mantendo o
        // resultado idêntico ao build serial.
        let mut pending: Vec<(PathBuf, Language, Vec<CallSite>, Option<Bindings>)> =
            Vec::with_capacity(parsed.len());
        for file in parsed {
            graph.file_hashes.insert(file.path, file.file_hash);
            for node in file.nodes {
                graph.graph.add_node(node);
            }
            if let Some(scope) = file.scope {
                graph.scopes.insert(file.relative.clone(), scope);
            }
            pending.push((file.relative, file.language, file.calls, file.bindings));
        }

        // O(N) container index by name (previously O(M * K) linear search per member).
        let belongs_to_edges: Vec<(NodeIndex, NodeIndex, usize)> = {
            let mut containers_by_name: HashMap<&str, Vec<NodeIndex>> = HashMap::new();
            for idx in graph.graph.node_indices() {
                if let Some(node) = graph.graph.node_weight(idx)
                    && node.kind == SectionKind::Container
                {
                    containers_by_name
                        .entry(node.name.as_str())
                        .or_default()
                        .push(idx);
                }
            }

            let mut edges = Vec::new();
            for member_idx in graph.graph.node_indices() {
                let Some(member) = graph.graph.node_weight(member_idx) else {
                    continue;
                };
                if member.kind != SectionKind::Member {
                    continue;
                }
                if let Some((container_name, _)) = member.qualified.rsplit_once("::")
                    && let Some(candidates) = containers_by_name.get(container_name)
                {
                    let target = candidates
                        .iter()
                        .copied()
                        .find(|&idx| {
                            graph
                                .graph
                                .node_weight(idx)
                                .is_some_and(|c| c.relative == member.relative)
                        })
                        .or_else(|| candidates.first().copied());
                    if let Some(container_idx) = target {
                        edges.push((member_idx, container_idx, member.start_line));
                    }
                }
            }
            edges
        };
        for (member_idx, container_idx, line) in belongs_to_edges {
            graph.graph.add_edge(
                member_idx,
                container_idx,
                RelationEdge {
                    line,
                    kind: RelationKind::BelongsTo,
                },
            );
        }

        // O(N) indices replace O(F * N) caller scans and O(C * N) full-graph definition scans,
        // reducing call-resolution complexity from O(C * N) to O(C + N).
        let (call_edges, dropped_names) = {
            use petgraph::visit::EdgeRef;

            let lang_slot = |language: Language| match language {
                Language::Rust => 0usize,
                Language::Kotlin => 1usize,
            };
            let mut callers_by_file: HashMap<&str, Vec<NodeIndex>> = HashMap::new();
            let mut defs_by_name: [HashMap<&str, Vec<NodeIndex>>; 2] =
                [HashMap::new(), HashMap::new()];
            for index in graph.graph.node_indices() {
                let Some(node) = graph.graph.node_weight(index) else {
                    continue;
                };
                if node.is_callable {
                    callers_by_file
                        .entry(node.relative.as_str())
                        .or_default()
                        .push(index);
                }
                defs_by_name[lang_slot(node.language)]
                    .entry(node.name.as_str())
                    .or_default()
                    .push(index);
            }

            let default_scope = FileScope::default();
            let mut call_edges: Vec<(NodeIndex, NodeIndex, usize)> = Vec::new();
            let mut dropped_names: Vec<String> = Vec::new();

            for (relative, language, calls, bindings) in &pending {
                if calls.is_empty() {
                    continue;
                }
                let file_rel = relative.to_string_lossy();
                let callers = callers_by_file
                    .get(file_rel.as_ref())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if callers.is_empty() {
                    continue;
                }

                let scope = graph.scopes.get(relative).unwrap_or(&default_scope);
                let lang_defs = &defs_by_name[lang_slot(*language)];

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
                        && let Some(target) = graph
                            .graph
                            .edges_directed(container, petgraph::Direction::Incoming)
                            .filter(|edge| edge.weight().kind == RelationKind::BelongsTo)
                            .map(|edge| edge.source())
                            .find(|&idx| graph.graph[idx].name == call.name)
                    {
                        call_edges.push((caller, target, call.line));
                        continue;
                    }

                    let mut definitions: std::borrow::Cow<'_, [NodeIndex]> =
                        std::borrow::Cow::Borrowed(
                            lang_defs
                                .get(call.name.as_str())
                                .map(Vec::as_slice)
                                .unwrap_or(&[]),
                        );
                    if definitions.is_empty()
                        && let Some(binding) = scope.imports.get(&call.name)
                        && binding.item != call.name
                    {
                        let target_file = match &binding.target {
                            ImportTarget::File(file) => Some(file.as_path()),
                            ImportTarget::External => None,
                        };
                        if let Some(candidates) = lang_defs.get(binding.item.as_str()) {
                            if let Some(target) = target_file {
                                definitions = std::borrow::Cow::Owned(
                                    candidates
                                        .iter()
                                        .copied()
                                        .filter(|&index| {
                                            graph.graph.node_weight(index).is_some_and(|node| {
                                                Path::new(node.relative.as_str()) == target
                                            })
                                        })
                                        .collect(),
                                );
                            } else {
                                definitions = std::borrow::Cow::Borrowed(candidates.as_slice());
                            }
                        }
                    }
                    if let Some(receiver) = &call.receiver
                        && let Some(bindings) = bindings.as_ref()
                        && let Some(ty) = binding_type_at(bindings, receiver, call.line)
                    {
                        let prefix = format!("{ty}::");
                        let infix = format!("::{ty}::");
                        let typed: Vec<NodeIndex> = definitions
                            .iter()
                            .copied()
                            .filter(|&index| {
                                graph.graph.node_weight(index).is_some_and(|node| {
                                    node.qualified.starts_with(&prefix)
                                        || node.qualified.contains(&infix)
                                })
                            })
                            .collect();
                        if !typed.is_empty() {
                            definitions = std::borrow::Cow::Owned(typed);
                        }
                    }

                    let resolution = pick_target(
                        &call.name,
                        relative,
                        scope,
                        &graph.resolver,
                        call.path.as_deref(),
                        &definitions,
                        &graph,
                    );
                    match resolution {
                        Resolution::Resolved(target) => {
                            call_edges.push((caller, target, call.line));
                        }
                        Resolution::Ambiguous { .. } => {
                            dropped_names.push(call.name.clone());
                        }
                        Resolution::Unknown => {}
                    }
                }
            }

            (call_edges, dropped_names)
        };

        for (caller, target, line) in call_edges {
            graph.graph.add_edge(
                caller,
                target,
                RelationEdge {
                    line,
                    kind: RelationKind::Call,
                },
            );
        }
        for name in dropped_names {
            *graph.dropped.entry(name).or_default() += 1;
        }

        graph.rebuild_defs();
        Ok(graph)
    }
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>, ToolError> {
    // Walk paralelo: o diretório do walker é reaproveitado por várias threads e
    // cada arquivo encontrado é acumulado num Mutex. A ordenação final mantém o
    // resultado idêntico ao walk serial.
    let files = std::sync::Mutex::new(Vec::new());
    let failure = std::sync::Mutex::new(None::<String>);

    WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(true)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build_parallel()
        .run(|| {
            Box::new(|entry| match entry {
                Ok(entry) => {
                    if entry.file_type().is_some_and(|ft| ft.is_file()) {
                        let path = entry.into_path();
                        if Language::from_path(&path).is_some()
                            && let Ok(mut files) = files.lock()
                        {
                            files.push(path);
                        }
                    }
                    ignore::WalkState::Continue
                }
                Err(error) => {
                    if let Ok(mut slot) = failure.lock()
                        && slot.is_none()
                    {
                        *slot = Some(error.to_string());
                    }
                    ignore::WalkState::Continue
                }
            })
        });

    if let Some(message) = failure
        .into_inner()
        .unwrap_or_else(|error| error.into_inner())
    {
        return Err(ToolError::Other(message));
    }

    let mut files = files
        .into_inner()
        .unwrap_or_else(|error| error.into_inner());
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

fn has_test_attribute(lines: &[&str], start_line: usize) -> bool {
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

/// Lê, parseia e extrai os dados derivados de um arquivo. O conteúdo e a árvore
/// morrem no fim da chamada: quem chama recebe só o que o grafo consome.
#[allow(clippy::too_many_arguments)]
fn parse_file(
    queries: &GraphQueries,
    cache: Option<&Arc<ParseCache>>,
    resolver: &ScopeResolver,
    path: PathBuf,
    relative: PathBuf,
    language: Language,
) -> Option<ParsedFile> {
    let content = std::fs::read_to_string(&path).ok()?;
    let file_hash = blake3::hash(content.as_bytes());
    let tree = match cache {
        Some(cache) => cache.parse_with_digest(&path, language, &content, file_hash),
        None => parse(&content, language).map(Arc::new),
    };

    let relative_str = relative.to_string_lossy();
    let is_test_path = relative_str.contains("/tests/") || relative_str.starts_with("tests/");
    let mut lines_cache: Option<Vec<&str>> = None;
    let mut nodes = Vec::new();
    let snippets = tree
        .as_deref()
        .map(|t| AstChunker::snippets_from_tree(t, &content, language))
        .unwrap_or_default();

    for snippet in snippets {
        let is_callable = snippet.signature.contains("fn ") || snippet.signature.contains("fun ");
        let is_public = if language == Language::Kotlin {
            !snippet.signature.contains("private ") && !snippet.signature.contains("internal ")
        } else {
            snippet.signature.trim_start().starts_with("pub")
        };
        let is_test = is_test_path
            || snippet.code.contains("#[cfg(test)]")
            || snippet.code.contains("@Test")
            || {
                let lines = lines_cache.get_or_insert_with(|| content.lines().collect());
                has_test_attribute(lines, snippet.start_line)
            };
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
        nodes.push(SymbolNode {
            name: snippet.symbol_name,
            qualified: snippet.qualified_name.clone(),
            relative: relative_str.to_string(),
            start_line: snippet.start_line,
            end_line: snippet.end_line,
            signature: snippet.signature,
            is_public,
            is_callable,
            is_test,
            language,
            kind,
            id: SectionId::new(relative_str.as_ref(), &snippet.qualified_name),
            hash: blake3::hash(snippet.code.as_bytes()),
        });
    }

    let (scope, calls) = match &tree {
        Some(tree) => {
            let root_node = tree.root_node();
            (
                Some(collect_imports(
                    queries, language, &root_node, &content, &relative, resolver,
                )),
                collect_calls(queries, language, &root_node, &content),
            )
        }
        None => (None, Vec::new()),
    };
    let bindings = if calls.iter().any(|call| call.receiver.is_some()) {
        Some(collect_bindings(&content))
    } else {
        None
    };

    Some(ParsedFile {
        path,
        relative,
        language,
        file_hash,
        nodes,
        scope,
        calls,
        bindings,
    })
}

/// Parse paralelo com resultado determinístico: cada worker pega o próximo
/// índice e o vetor final é reordenado pelo índice do arquivo.
fn parse_files(
    queries: &GraphQueries,
    cache: Option<&Arc<ParseCache>>,
    resolver: &ScopeResolver,
    targets: Vec<(PathBuf, PathBuf, Language)>,
) -> Vec<ParsedFile> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .min(targets.len())
        .max(1);
    let next = AtomicUsize::new(0);
    let collected = std::sync::Mutex::new(Vec::with_capacity(targets.len()));

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((path, relative, language)) = targets.get(index) else {
                        break;
                    };
                    let parsed = parse_file(
                        queries,
                        cache,
                        resolver,
                        path.clone(),
                        relative.clone(),
                        *language,
                    );
                    if let Some(parsed) = parsed
                        && let Ok(mut collected) = collected.lock()
                    {
                        collected.push((index, parsed));
                    }
                }
            });
        }
    });

    let mut collected = collected
        .into_inner()
        .unwrap_or_else(|error| error.into_inner());
    collected.sort_by_key(|(index, _)| *index);
    collected.into_iter().map(|(_, file)| file).collect()
}

/// Ligações locais por variável, reaproveitadas entre arquivos no build paralelo.
type Bindings = HashMap<String, Vec<(usize, String)>>;
