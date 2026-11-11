use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::Direction;

use crate::ast::Language;
use crate::graph::scope::{FileScope, ScopeResolver};

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

/// View read-only sobre `SymbolNode`, mantida para as tools que já usavam
/// `Def` no grafo antigo.
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
    pub(crate) node: NodeIndex,
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

#[derive(Debug, Default, Clone)]
pub struct ProjectCodeGraph {
    pub graph: StableDiGraph<SymbolNode, RelationEdge>,
    pub file_hashes: BTreeMap<PathBuf, blake3::Hash>,
    pub dropped: BTreeMap<String, usize>,
    pub resolver: ScopeResolver,
    pub scopes: HashMap<PathBuf, FileScope>,
    pub defs: Vec<Def>,
    def_index: HashMap<NodeIndex, usize>,
}

pub type CallGraph = ProjectCodeGraph;

impl ProjectCodeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn nodes(&self) -> impl Iterator<Item = &SymbolNode> {
        self.graph.node_weights()
    }

    /// Reconstroi a view `Def` a partir dos nós e das arestas resolvidas.
    pub fn rebuild_defs(&mut self) {
        self.defs.clear();
        self.def_index.clear();
        for index in self.graph.node_indices() {
            let Some(node) = self.graph.node_weight(index) else {
                continue;
            };
            let calls = self
                .graph
                .neighbors_directed(index, Direction::Outgoing)
                .filter_map(|target| {
                    self.graph
                        .node_weight(target)
                        .map(|target| target.name.clone())
                })
                .collect();
            let def = Def {
                name: node.name.clone(),
                qualified: node.qualified.clone(),
                relative: node.relative.clone(),
                start_line: node.start_line,
                end_line: node.end_line,
                signature: node.signature.clone(),
                is_public: node.is_public,
                is_callable: node.is_callable,
                is_test: node.is_test,
                calls,
                node: index,
            };
            self.def_index.insert(index, self.defs.len());
            self.defs.push(def);
        }
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

    fn matching_indices(&self, query: &str) -> Vec<NodeIndex> {
        self.matching(query).into_iter().map(|def| def.node).collect()
    }

    pub fn callers(&self, name: &str) -> Vec<&Def> {
        let targets = self.matching_indices(name);
        if targets.is_empty() {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for target in targets {
            for caller in self.graph.neighbors_directed(target, Direction::Incoming) {
                if seen.insert(caller)
                    && let Some(def) = self.def_for(caller)
                {
                    out.push(def);
                }
            }
        }
        out
    }

    pub fn callers_in_file(&self, name: &str, relative: &str) -> Vec<&Def> {
        self.callers(name)
            .into_iter()
            .filter(|def| def.relative == relative)
            .collect()
    }

    pub fn callees(&self, name: &str) -> Vec<&Def> {
        let starts = self.matching_indices(name);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for start in starts {
            for callee in self.graph.neighbors_directed(start, Direction::Outgoing) {
                if seen.insert(callee)
                    && let Some(def) = self.def_for(callee)
                {
                    out.push(def);
                }
            }
        }
        out
    }

    pub fn trace(&self, from: &str, to: &str, max_depth: usize) -> Option<Vec<String>> {
        let starts = self.matching_indices(from);
        if starts.is_empty() {
            return None;
        }
        let targets: HashSet<NodeIndex> = self.matching_indices(to).into_iter().collect();
        let target_matches = |index: NodeIndex| {
            targets.contains(&index)
                || self
                    .graph
                    .node_weight(index)
                    .is_some_and(|node| node.name == to || node.qualified == to)
        };

        if let Some(&start) = starts.iter().find(|&&index| target_matches(index)) {
            return Some(vec![self.node_name(start)]);
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
                        return Some(path.into_iter().map(|index| self.node_name(index)).collect());
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

    pub fn reverse_deps(&self, name: &str, depth: usize) -> Vec<&Def> {
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
                    && let Some(def) = self.def_for(caller)
                {
                    out.push(def);
                    frontier.push_back((caller, level + 1));
                }
            }
        }
        out
    }

    fn def_for(&self, index: NodeIndex) -> Option<&Def> {
        self.def_index.get(&index).and_then(|&at| self.defs.get(at))
    }

    fn node_name(&self, index: NodeIndex) -> String {
        self.graph
            .node_weight(index)
            .map(|node| node.name.clone())
            .unwrap_or_default()
    }
}
