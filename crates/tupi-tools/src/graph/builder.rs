use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use petgraph::stable_graph::NodeIndex;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, QueryCursor};
use tupi_kernel::ToolError;

use crate::ast::{AstChunker, Language};
use crate::graph::model::{ProjectCodeGraph, RelationEdge, RelationKind, SymbolNode};
use crate::graph::queries::GraphQueries;

pub struct GraphBuilder {
    queries: GraphQueries,
}

#[allow(dead_code)]
struct CallSite {
    name: String,
    line: usize,
    receiver: Option<String>,
    path: Option<String>,
}

impl GraphBuilder {
    pub fn new() -> Result<Self, ToolError> {
        Ok(Self {
            queries: GraphQueries::new()?,
        })
    }

    pub fn build(&self, root: &Path) -> Result<ProjectCodeGraph, ToolError> {
        let files = collect_files(root)?;
        let mut graph = ProjectCodeGraph::new();

        for path in &files {
            let Ok(content) = std::fs::read_to_string(path) else {
                continue;
            };
            let relative = crate::paths::relative(root, path);
            let hash = blake3::hash(content.as_bytes());
            graph.file_hashes.insert(path.clone(), hash);

            let Some(language) = Language::from_path(path) else {
                continue;
            };
            let snippets = AstChunker::new().snippets(path, &content);
            for snippet in snippets {
                let is_callable = snippet.signature.contains("fn ");
                let is_public = snippet.signature.trim_start().starts_with("pub");
                let is_test = relative.contains("/tests/")
                    || relative.starts_with("tests/")
                    || has_test_attribute(&content, snippet.start_line)
                    || snippet.code.contains("#[cfg(test)]");
                graph.graph.add_node(SymbolNode {
                    name: snippet.symbol_name,
                    qualified: snippet.qualified_name,
                    relative: relative.clone(),
                    start_line: snippet.start_line,
                    end_line: snippet.end_line,
                    signature: snippet.signature,
                    is_public,
                    is_callable,
                    is_test,
                    language,
                });
            }

            let Some(tree) = parse(path, &content, language) else {
                continue;
            };
            let calls = collect_calls(&self.queries, language, &tree.root_node(), &content);
            let callers = graph
                .graph
                .node_indices()
                .filter(|&index| {
                    graph
                        .graph
                        .node_weight(index)
                        .is_some_and(|node| node.relative == relative && node.is_callable)
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
                let targets = targets_by_name(&graph, &call.name);
                match targets.len() {
                    0 => {}
                    1 => {
                        graph.graph.add_edge(
                            caller,
                            targets[0],
                            RelationEdge {
                                line: call.line,
                                kind: RelationKind::Call,
                            },
                        );
                    }
                    _ => {
                        *graph.dropped.entry(call.name).or_default() += 1;
                    }
                }
            }
        }

        Ok(graph)
    }
}

fn targets_by_name(graph: &ProjectCodeGraph, name: &str) -> Vec<NodeIndex> {
    graph
        .graph
        .node_indices()
        .filter(|&index| {
            graph
                .graph
                .node_weight(index)
                .is_some_and(|node| node.name == name && node.is_callable)
        })
        .collect()
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

fn parse(_path: &Path, content: &str, language: Language) -> Option<tree_sitter::Tree> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language.tree_sitter_language()).ok()?;
    parser.parse(content, None)
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

fn has_test_attribute(content: &str, start_line: usize) -> bool {
    let from = start_line.saturating_sub(4);
    content
        .lines()
        .skip(from)
        .take(start_line.saturating_sub(from))
        .any(|line| line.contains("#[test]") || line.contains("::test]"))
}
