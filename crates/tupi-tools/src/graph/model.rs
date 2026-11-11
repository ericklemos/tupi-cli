use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::Direction;

use crate::ast::Language;

#[derive(Debug, Clone)]
pub struct SymbolNode {
    pub name: String,
    pub qualified: String,
    pub relative: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub is_public: bool,
    pub is_callable: bool,
    pub is_test: bool,
    pub language: Language,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    Call,
    Import,
}

#[derive(Debug, Clone)]
pub struct RelationEdge {
    pub line: usize,
    pub kind: RelationKind,
}

#[derive(Debug, Default)]
pub struct ProjectCodeGraph {
    pub graph: StableDiGraph<SymbolNode, RelationEdge>,
    pub file_hashes: BTreeMap<PathBuf, blake3::Hash>,
    pub dropped: BTreeMap<String, usize>,
}

impl ProjectCodeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn nodes(&self) -> impl Iterator<Item = &SymbolNode> {
        self.graph.node_weights()
    }

    pub fn matching(&self, query: &str) -> Vec<&SymbolNode> {
        let query = query.trim();
        let query = query.strip_suffix("()").unwrap_or(query).trim();
        self.graph
            .node_indices()
            .filter_map(|index| {
                let node = self.graph.node_weight(index)?;
                (node.name == query
                    || node.qualified == query
                    || node.qualified.ends_with(&format!("::{query}")))
                .then_some(node)
            })
            .collect()
    }

    fn matching_indices(&self, query: &str) -> Vec<NodeIndex> {
        let query = query.trim();
        let query = query.strip_suffix("()").unwrap_or(query).trim();
        self.graph
            .node_indices()
            .filter(|&index| {
                self.graph.node_weight(index).is_some_and(|node| {
                    node.name == query
                        || node.qualified == query
                        || node.qualified.ends_with(&format!("::{query}"))
                })
            })
            .collect()
    }

    pub fn callers(&self, name: &str) -> Vec<&SymbolNode> {
        let targets = self.matching_indices(name);
        if targets.is_empty() {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for target in targets {
            for caller in self.graph.neighbors_directed(target, Direction::Incoming) {
                if seen.insert(caller)
                    && let Some(node) = self.graph.node_weight(caller)
                {
                    out.push(node);
                }
            }
        }
        out
    }

    pub fn callers_in_file(&self, name: &str, relative: &str) -> Vec<&SymbolNode> {
        self.callers(name)
            .into_iter()
            .filter(|node| node.relative == relative)
            .collect()
    }

    pub fn callees(&self, name: &str) -> Vec<&SymbolNode> {
        let starts = self.matching_indices(name);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for start in starts {
            for callee in self.graph.neighbors_directed(start, Direction::Outgoing) {
                if seen.insert(callee)
                    && let Some(node) = self.graph.node_weight(callee)
                {
                    out.push(node);
                }
            }
        }
        out
    }

    pub fn trace(&self, from: &str, to: &str, max_depth: usize) -> Option<Vec<&SymbolNode>> {
        let starts = self.matching_indices(from);
        if starts.is_empty() {
            return None;
        }
        let targets: HashSet<NodeIndex> = self.matching_indices(to).into_iter().collect();
        let target_matches = |index: NodeIndex| {
            targets.contains(&index)
                || self.graph.node_weight(index).is_some_and(|node| {
                    node.name == to || node.qualified == to
                })
        };

        if let Some(&start) = starts.iter().find(|&&index| target_matches(index)) {
            return Some(vec![self.graph.node_weight(start)?]);
        }

        let mut parents: HashMap<NodeIndex, NodeIndex> = HashMap::new();
        let mut visited: HashSet<NodeIndex> = starts.iter().copied().collect();
        let mut frontier = starts;

        for _ in 0..max_depth {
            let mut next = Vec::new();
            for current in frontier {
                for callee in self.graph.neighbors_directed(current, Direction::Outgoing) {
                    if !visited.insert(callee) {
                        continue;
                    }
                    parents.insert(callee, current);
                    if target_matches(callee) {
                        let mut path = vec![callee];
                        let mut cursor = callee;
                        while let Some(parent) = parents.get(&cursor).copied() {
                            path.push(parent);
                            cursor = parent;
                        }
                        path.reverse();
                        return path
                            .into_iter()
                            .map(|index| self.graph.node_weight(index))
                            .collect();
                    }
                    next.push(callee);
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        None
    }

    pub fn dropped_for(&self, name: &str) -> usize {
        self.dropped.get(name).copied().unwrap_or(0)
    }

    pub fn incoming_count(&self, name: &str) -> usize {
        self.matching_indices(name)
            .into_iter()
            .map(|index| self.graph.edges_directed(index, Direction::Incoming).count())
            .sum()
    }

    pub fn reverse_deps(&self, name: &str, depth: usize) -> Vec<&SymbolNode> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut frontier: VecDeque<(NodeIndex, usize)> = self
            .matching_indices(name)
            .into_iter()
            .map(|index| (index, 0))
            .collect();

        while let Some((current, level)) = frontier.pop_front() {
            if level >= depth {
                continue;
            }
            for caller in self.graph.neighbors_directed(current, Direction::Incoming) {
                if seen.insert(caller)
                    && let Some(node) = self.graph.node_weight(caller)
                {
                    out.push(node);
                    frontier.push_back((caller, level + 1));
                }
            }
        }
        out
    }
}
