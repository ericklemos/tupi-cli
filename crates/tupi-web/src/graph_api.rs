use std::collections::{HashSet, VecDeque};

use axum::extract::{Query as QueryParams, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use petgraph::visit::EdgeRef;
use petgraph::graph::NodeIndex;
use serde::{Deserialize, Serialize};
use tupi_tools::graph::{Def, ProjectCodeGraph, RelationKind, SymbolNode};

use crate::routes::AppState;

const DEFAULT_LIMIT: usize = 120;
const MAX_LIMIT: usize = 200;
const DEFAULT_DEPTH: usize = 1;
const MAX_DEPTH: usize = 3;

#[derive(Serialize)]
pub(crate) struct GraphSymbolDto {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) qualified: String,
    pub(crate) file: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) signature: String,
    pub(crate) is_public: bool,
    pub(crate) is_callable: bool,
    pub(crate) is_test: bool,
    pub(crate) kind: &'static str,
    pub(crate) callers: usize,
    pub(crate) callees: usize,
}

#[derive(Serialize)]
pub(crate) struct GraphEdgeDto {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) kind: &'static str,
    pub(crate) line: usize,
}

fn kind_name(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::BelongsTo => "belongs_to",
        RelationKind::Calls => "calls",
        RelationKind::TypeUse => "type_use",
        RelationKind::Implements => "implements",
        RelationKind::References => "references",
        RelationKind::Call => "call",
        RelationKind::Import => "import",
    }
}

fn symbol_dto(graph: &ProjectCodeGraph, node: &SymbolNode) -> GraphSymbolDto {
    GraphSymbolDto {
        id: node.id.as_str().to_string(),
        name: node.name.clone(),
        qualified: node.qualified.clone(),
        file: node.relative.clone(),
        start_line: node.start_line,
        end_line: node.end_line,
        signature: node.signature.clone(),
        is_public: node.is_public,
        is_callable: node.is_callable,
        is_test: node.is_test,
        kind: match node.kind {
            tupi_tools::graph::SectionKind::Container => "container",
            tupi_tools::graph::SectionKind::Member => "member",
            tupi_tools::graph::SectionKind::DocumentChunk => "document_chunk",
        },
        callers: graph.incoming_count(&node.name),
        callees: graph.callees(&node.name).len(),
    }
}

fn def_dto(graph: &ProjectCodeGraph, def: &Def) -> GraphSymbolDto {
    GraphSymbolDto {
        id: format!("{}::{}", def.relative, def.qualified),
        name: def.name.clone(),
        qualified: def.qualified.clone(),
        file: def.relative.clone(),
        start_line: def.start_line,
        end_line: def.end_line,
        signature: def.signature.clone(),
        is_public: def.is_public,
        is_callable: def.is_callable,
        is_test: def.is_test,
        kind: if def.is_callable { "member" } else { "container" },
        callers: graph.callers_for(def).len(),
        callees: def.calls.len(),
    }
}

fn index_for(graph: &ProjectCodeGraph, symbol: &str) -> Option<NodeIndex> {
    let symbol = symbol.trim();
    let symbol = symbol.strip_suffix("()").unwrap_or(symbol).trim();
    graph.graph.node_indices().find(|&index| {
        let Some(node) = graph.graph.node_weight(index) else {
            return false;
        };
        node.id.as_str() == symbol
            || node.qualified == symbol
            || node.name == symbol
            || node.qualified.ends_with(&format!("::{symbol}"))
    })
}

#[derive(Serialize)]
pub(crate) struct SummaryDto {
    pub(crate) node_count: usize,
    pub(crate) edge_count: usize,
    pub(crate) file_count: usize,
    pub(crate) files: Vec<String>,
    pub(crate) top_symbols: Vec<GraphSymbolDto>,
}

pub(crate) async fn summary(State(state): State<AppState>) -> Json<SummaryDto> {
    let graph = state.ctx.graph.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let graph = graph.get();
        let mut files: Vec<String> = graph
            .file_hashes
            .keys()
            .map(|path| path.display().to_string())
            .collect();
        files.sort();
        files.truncate(500);

        let mut ranked: Vec<(usize, NodeIndex)> = graph
            .graph
            .node_indices()
            .map(|index| {
                (
                    graph
                        .graph
                        .edges_directed(index, petgraph::Direction::Incoming)
                        .count(),
                    index,
                )
            })
            .collect();
        ranked.sort_by(|left, right| right.0.cmp(&left.0));
        ranked.truncate(10);
        let top_symbols = ranked
            .into_iter()
            .filter_map(|(_, index)| graph.graph.node_weight(index))
            .map(|node| symbol_dto(&graph, node))
            .collect();

        SummaryDto {
            node_count: graph.graph.node_count(),
            edge_count: graph.graph.edge_count(),
            file_count: graph.file_hashes.len(),
            files,
            top_symbols,
        }
    })
    .await
    .unwrap_or_else(|_| SummaryDto {
        node_count: 0,
        edge_count: 0,
        file_count: 0,
        files: Vec::new(),
        top_symbols: Vec::new(),
    });

    Json(summary)
}

#[derive(Deserialize)]
pub(crate) struct SearchParams {
    q: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct SearchDto {
    pub(crate) results: Vec<GraphSymbolDto>,
}

pub(crate) async fn search(
    State(state): State<AppState>,
    QueryParams(params): QueryParams<SearchParams>,
) -> Json<SearchDto> {
    let query = params.q.unwrap_or_default();
    let limit = params.limit.unwrap_or(50).clamp(1, MAX_LIMIT);
    let graph = state.ctx.graph.clone();
    let results = tokio::task::spawn_blocking(move || {
        let graph = graph.get();
        graph
            .matching(&query)
            .into_iter()
            .take(limit)
            .map(|def| def_dto(&graph, def))
            .collect()
    })
    .await
    .unwrap_or_default();

    Json(SearchDto { results })
}

#[derive(Deserialize)]
pub(crate) struct NeighborhoodParams {
    symbol: Option<String>,
    depth: Option<usize>,
    limit: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct NeighborhoodDto {
    pub(crate) center: GraphSymbolDto,
    pub(crate) nodes: Vec<GraphSymbolDto>,
    pub(crate) edges: Vec<GraphEdgeDto>,
    pub(crate) truncated: bool,
    pub(crate) total_candidates: usize,
}

pub(crate) async fn neighborhood(
    State(state): State<AppState>,
    QueryParams(params): QueryParams<NeighborhoodParams>,
) -> Response {
    let Some(symbol) = params.symbol.filter(|symbol| !symbol.trim().is_empty()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "parâmetro symbol é obrigatório" })),
        )
            .into_response();
    };
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let depth = params.depth.unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH);
    let graph = state.ctx.graph.clone();

    let computed = tokio::task::spawn_blocking(move || {
        let graph = graph.get();
        let center = index_for(&graph, &symbol)?;
        let center_node = graph.graph.node_weight(center)?.clone();

        let mut visited: HashSet<NodeIndex> = HashSet::new();
        visited.insert(center);
        let mut collected = vec![center];
        let mut frontier = VecDeque::from([(center, 0usize)]);
        let mut truncated = false;
        let mut candidates = 0usize;

        while let Some((current, distance)) = frontier.pop_front() {
            if distance >= depth {
                continue;
            }
            for neighbor in graph.graph.neighbors_undirected(current) {
                if visited.contains(&neighbor) {
                    continue;
                }
                candidates += 1;
                if collected.len() >= limit {
                    truncated = true;
                    continue;
                }
                visited.insert(neighbor);
                collected.push(neighbor);
                frontier.push_back((neighbor, distance + 1));
            }
        }

        let nodes: Vec<GraphSymbolDto> = collected
            .iter()
            .filter_map(|&index| graph.graph.node_weight(index))
            .map(|node| symbol_dto(&graph, node))
            .collect();

        let present: HashSet<NodeIndex> = collected.iter().copied().collect();
        let mut edges = Vec::new();
        for &index in &collected {
            for edge in graph.graph.edges(index) {
                if !present.contains(&edge.target()) {
                    continue;
                }
                if let (Some(from), Some(to)) = (
                    graph.graph.node_weight(edge.source()),
                    graph.graph.node_weight(edge.target()),
                ) {
                    edges.push(GraphEdgeDto {
                        from: from.id.as_str().to_string(),
                        to: to.id.as_str().to_string(),
                        kind: kind_name(edge.weight().kind),
                        line: edge.weight().line,
                    });
                }
            }
        }

        Some(NeighborhoodDto {
            center: symbol_dto(&graph, &center_node),
            nodes,
            edges,
            truncated,
            total_candidates: candidates,
        })
    })
    .await
    .ok()
    .flatten();

    match computed {
        Some(dto) => Json(dto).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "símbolo não encontrado no grafo" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct TraceParams {
    from: Option<String>,
    to: Option<String>,
    max_depth: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct TraceDto {
    pub(crate) found: bool,
    pub(crate) path: Vec<String>,
    pub(crate) nodes: Vec<GraphSymbolDto>,
    pub(crate) edges: Vec<GraphEdgeDto>,
}

pub(crate) async fn trace(
    State(state): State<AppState>,
    QueryParams(params): QueryParams<TraceParams>,
) -> Response {
    let (Some(from), Some(to)) = (params.from, params.to) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "parâmetros from e to são obrigatórios" })),
        )
            .into_response();
    };
    let max_depth = params.max_depth.unwrap_or(6).clamp(1, 12);
    let graph = state.ctx.graph.clone();

    let dto = tokio::task::spawn_blocking(move || {
        let graph = graph.get();
        let path = graph.trace(&from, &to, max_depth).unwrap_or_default();
        let indices: Vec<NodeIndex> = path
            .iter()
            .filter_map(|name| index_for(&graph, name))
            .collect();
        let nodes: Vec<GraphSymbolDto> = indices
            .iter()
            .filter_map(|&index| graph.graph.node_weight(index))
            .map(|node| symbol_dto(&graph, node))
            .collect();
        let mut edges = Vec::new();
        for pair in indices.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            if let Some(edge) = graph
                .graph
                .edges(left)
                .find(|edge| edge.target() == right)
                && let (Some(from), Some(to)) = (
                    graph.graph.node_weight(edge.source()),
                    graph.graph.node_weight(edge.target()),
                )
            {
                edges.push(GraphEdgeDto {
                    from: from.id.as_str().to_string(),
                    to: to.id.as_str().to_string(),
                    kind: kind_name(edge.weight().kind),
                    line: edge.weight().line,
                });
            }
        }

        TraceDto {
            found: !path.is_empty(),
            path,
            nodes,
            edges,
        }
    })
    .await
    .unwrap_or_else(|_| TraceDto {
        found: false,
        path: Vec::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
    });

    Json(dto).into_response()
}
