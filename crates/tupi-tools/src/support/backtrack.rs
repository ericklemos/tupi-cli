use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use blake3::Hash;

pub(crate) const OUTLINE_SYMBOL: &str = "__outline__";
pub(crate) const DEFAULT_MAX_NODES: usize = 1024;
const DEFAULT_MAX_QUERIES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    pub relative: String,
    pub symbol: String,
}

impl NodeId {
    pub fn new(relative: impl Into<String>, symbol: impl Into<String>) -> Self {
        Self {
            relative: relative.into(),
            symbol: symbol.into(),
        }
    }

    pub fn display_label(&self) -> String {
        if self.symbol == OUTLINE_SYMBOL {
            self.relative.clone()
        } else {
            format!("`{}` ({})", self.symbol, self.relative)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VisitLevel {
    Unseen,
    Frontier,
    SignatureSeen,
    BodySeen,
}

#[derive(Debug, Clone)]
pub struct NeighborRef {
    pub relative: String,
    pub symbol: String,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct BacktrackHint {
    pub visited: Vec<String>,
    pub frontier: Vec<String>,
    pub via: Option<String>,
}

impl BacktrackHint {
    pub fn format_line(&self) -> Option<String> {
        if self.visited.is_empty() && self.frontier.is_empty() {
            return None;
        }
        if self.frontier.is_empty() {
            return Some(
                "> **Backtrack**: ramo já fechado nesta sessão (vizinhos conhecidos já abertos).\n"
                    .to_string(),
            );
        }
        let mut parts = Vec::new();
        if let Some(via) = &self.via {
            parts.push(format!("origem: {via}"));
        }
        if !self.visited.is_empty() {
            parts.push(format!("já abertos: {}", format_list(&self.visited, 4)));
        }
        parts.push(format!(
            "fronteira ainda não aberta: {}",
            format_list(&self.frontier, 4)
        ));
        Some(format!("> **Backtrack**: {}\n", parts.join(" | ")))
    }
}

fn format_list(items: &[String], max: usize) -> String {
    if items.len() <= max {
        items.join(", ")
    } else {
        let shown = items[..max].join(", ");
        format!("{shown}, +{}", items.len() - max)
    }
}

#[derive(Debug, Clone)]
struct NodeRecord {
    level: VisitLevel,
    file_hash: Option<Hash>,
    via: Option<String>,
    neighbors: BTreeSet<NodeId>,
    neighbor_lines: HashMap<NodeId, usize>,
    last_touched: u64,
}

#[derive(Debug, Clone)]
struct QueryRecord {
    terms: BTreeSet<String>,
    path_prefix: Option<String>,
    file_hashes: HashMap<String, Hash>,
}

#[derive(Default)]
struct State {
    clock: u64,
    nodes: HashMap<NodeId, NodeRecord>,
    queries: Vec<QueryRecord>,
}

impl State {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn evict_if_needed(&mut self, max_nodes: usize) {
        while self.nodes.len() > max_nodes {
            if self.nodes.len() <= 1 {
                break;
            }
            let Some(oldest) = self
                .nodes
                .iter()
                .min_by_key(|(_, record)| record.last_touched)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            self.nodes.remove(&oldest);
        }
    }
}

pub struct BacktrackGraph {
    max_nodes: usize,
    state: Mutex<State>,
}

impl Default for BacktrackGraph {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_NODES)
    }
}

impl BacktrackGraph {
    pub fn new(max_nodes: usize) -> Self {
        Self {
            max_nodes: max_nodes.max(1),
            state: Mutex::new(State::default()),
        }
    }

    pub fn visit_level(&self, relative: &str, symbol: &str, file_hash: &Hash) -> VisitLevel {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let id = NodeId::new(relative, symbol);
        let Some(record) = state.nodes.get(&id) else {
            return VisitLevel::Unseen;
        };
        match record.file_hash {
            Some(recorded_hash) if recorded_hash == *file_hash => record.level,
            None => record.level,
            Some(_) => VisitLevel::Unseen,
        }
    }

    pub fn record_visit(
        &self,
        relative: &str,
        symbol: &str,
        file_hash: Hash,
        level: VisitLevel,
        via: Option<&str>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let clock = state.tick();
        let id = NodeId::new(relative, symbol);
        let via_owned = via
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        match state.nodes.get_mut(&id) {
            Some(record) => {
                record.last_touched = clock;
                match record.file_hash {
                    Some(existing) if existing == file_hash => {
                        record.level = record.level.max(level);
                        if record.via.is_none() {
                            record.via = via_owned;
                        }
                    }
                    Some(_) => {
                        record.level = level;
                        record.file_hash = Some(file_hash);
                        record.via = via_owned;
                        record.neighbors.clear();
                        record.neighbor_lines.clear();
                    }
                    None => {
                        record.level = record.level.max(level);
                        record.file_hash = Some(file_hash);
                        if via_owned.is_some() {
                            record.via = via_owned;
                        }
                    }
                }
            }
            None => {
                state.nodes.insert(
                    id,
                    NodeRecord {
                        level,
                        file_hash: Some(file_hash),
                        via: via_owned,
                        neighbors: BTreeSet::new(),
                        neighbor_lines: HashMap::new(),
                        last_touched: clock,
                    },
                );
            }
        }
        state.evict_if_needed(self.max_nodes);
    }

    pub fn record_neighbors(
        &self,
        relative: &str,
        symbol: &str,
        file_hash: Hash,
        neighbors: &[NeighborRef],
    ) {
        if neighbors.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let clock = state.tick();
        let source_id = NodeId::new(relative, symbol);

        {
            let record = state.nodes.entry(source_id).or_insert_with(|| NodeRecord {
                level: VisitLevel::BodySeen,
                file_hash: Some(file_hash),
                via: None,
                neighbors: BTreeSet::new(),
                neighbor_lines: HashMap::new(),
                last_touched: clock,
            });
            record.last_touched = clock;
            if record.file_hash.is_none() {
                record.file_hash = Some(file_hash);
            }
            for neighbor in neighbors {
                let target = NodeId::new(&neighbor.relative, &neighbor.symbol);
                record.neighbors.insert(target.clone());
                record.neighbor_lines.insert(target, neighbor.line);
            }
        }

        let origin_note = format!("vizinho de `{symbol}`");
        for neighbor in neighbors {
            let target = NodeId::new(&neighbor.relative, &neighbor.symbol);
            state.nodes.entry(target).or_insert_with(|| NodeRecord {
                level: VisitLevel::Frontier,
                file_hash: None,
                via: Some(origin_note.clone()),
                neighbors: BTreeSet::new(),
                neighbor_lines: HashMap::new(),
                last_touched: clock,
            });
        }

        state.evict_if_needed(self.max_nodes);
    }

    pub fn backtrack_hint(&self, relative: &str, symbol: &str) -> Option<BacktrackHint> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let id = NodeId::new(relative, symbol);
        let record = state.nodes.get(&id)?;
        if record.neighbors.is_empty() {
            return None;
        }

        let mut visited = Vec::new();
        let mut frontier = Vec::new();
        for neighbor_id in &record.neighbors {
            let is_body_seen = state
                .nodes
                .get(neighbor_id)
                .is_some_and(|n| n.level == VisitLevel::BodySeen);
            if is_body_seen {
                visited.push(neighbor_id.display_label());
            } else {
                let label = match record.neighbor_lines.get(neighbor_id) {
                    Some(line) if *line > 0 => {
                        format!("`{}` ({}:{line})", neighbor_id.symbol, neighbor_id.relative)
                    }
                    _ => neighbor_id.display_label(),
                };
                frontier.push(label);
            }
        }

        if visited.is_empty() && frontier.is_empty() {
            return None;
        }

        Some(BacktrackHint {
            visited,
            frontier,
            via: record.via.clone(),
        })
    }

    pub fn outline_summary(
        &self,
        relative: &str,
        file_hash: &Hash,
    ) -> Option<(Vec<String>, Vec<String>)> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let outline_id = NodeId::new(relative, OUTLINE_SYMBOL);
        let outline = state.nodes.get(&outline_id)?;
        if outline.file_hash != Some(*file_hash) || outline.level != VisitLevel::BodySeen {
            return None;
        }

        let mut opened = Vec::new();
        let mut unopened = Vec::new();
        for (id, rec) in &state.nodes {
            if id.relative != relative || id.symbol == OUTLINE_SYMBOL {
                continue;
            }
            if rec.file_hash != Some(*file_hash) {
                continue;
            }
            match rec.level {
                VisitLevel::BodySeen => opened.push(id.symbol.clone()),
                VisitLevel::SignatureSeen | VisitLevel::Frontier => {
                    unopened.push(id.symbol.clone());
                }
                VisitLevel::Unseen => {}
            }
        }
        opened.sort();
        unopened.sort();
        Some((opened, unopened))
    }

    pub fn was_query_seen(
        &self,
        terms: &[String],
        path_prefix: Option<&str>,
        matched_hashes: &HashMap<String, Hash>,
    ) -> bool {
        let norm_terms = normalize_terms(terms);
        if norm_terms.is_empty() {
            return false;
        }
        let norm_prefix = normalize_prefix(path_prefix);
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.queries.iter().any(|query| {
            query.terms == norm_terms
                && query.path_prefix == norm_prefix
                && query.file_hashes.len() == matched_hashes.len()
                && matched_hashes
                    .iter()
                    .all(|(path, hash)| query.file_hashes.get(path) == Some(hash))
        })
    }

    pub fn record_query(
        &self,
        terms: &[String],
        path_prefix: Option<&str>,
        matched_hashes: HashMap<String, Hash>,
    ) {
        let norm_terms = normalize_terms(terms);
        if norm_terms.is_empty() {
            return;
        }
        let norm_prefix = normalize_prefix(path_prefix);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .queries
            .retain(|query| !(query.terms == norm_terms && query.path_prefix == norm_prefix));
        state.queries.push(QueryRecord {
            terms: norm_terms,
            path_prefix: norm_prefix,
            file_hashes: matched_hashes,
        });
        while state.queries.len() > DEFAULT_MAX_QUERIES {
            state.queries.remove(0);
        }
    }

    /// Quantos símbolos da cadeia estão conhecidos mas ainda sem corpo
    /// entregue — a linha de fronteira do `lookup` sai daqui.
    pub fn frontier_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .nodes
            .values()
            .filter(|record| matches!(record.level, VisitLevel::Frontier))
            .count()
    }

    pub fn node_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .nodes
            .len()
    }
}

fn normalize_terms(terms: &[String]) -> BTreeSet<String> {
    terms
        .iter()
        .map(|term| term.trim().to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

fn normalize_prefix(prefix: Option<&str>) -> Option<String> {
    prefix
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
        .map(|prefix| prefix.trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(text: &str) -> Hash {
        blake3::hash(text.as_bytes())
    }

    #[test]
    fn body_visit_is_tracked_and_invalidated_when_file_hash_changes() {
        let graph = BacktrackGraph::new(64);
        let h1 = hash("v1");
        let h2 = hash("v2");

        assert_eq!(
            graph.visit_level("src/auth.rs", "verify_token", &h1),
            VisitLevel::Unseen
        );

        graph.record_visit(
            "src/auth.rs",
            "verify_token",
            h1,
            VisitLevel::BodySeen,
            Some("search: auth"),
        );

        assert_eq!(
            graph.visit_level("src/auth.rs", "verify_token", &h1),
            VisitLevel::BodySeen
        );
        assert_eq!(
            graph.visit_level("src/auth.rs", "verify_token", &h2),
            VisitLevel::Unseen
        );
    }

    #[test]
    fn signature_visit_does_not_downgrade_body_seen() {
        let graph = BacktrackGraph::new(64);
        let h = hash("same");

        graph.record_visit("src/auth.rs", "verify_token", h, VisitLevel::BodySeen, None);
        graph.record_visit(
            "src/auth.rs",
            "verify_token",
            h,
            VisitLevel::SignatureSeen,
            None,
        );

        assert_eq!(
            graph.visit_level("src/auth.rs", "verify_token", &h),
            VisitLevel::BodySeen
        );
    }

    #[test]
    fn backtrack_hint_reports_visited_and_unexplored_frontier_and_closed_branch() {
        let graph = BacktrackGraph::new(64);
        let h_auth = hash("auth");
        let h_tok = hash("tok");
        let h_api = hash("api");

        graph.record_visit(
            "src/auth.rs",
            "verify_token",
            h_auth,
            VisitLevel::BodySeen,
            Some("search: auth"),
        );
        graph.record_neighbors(
            "src/auth.rs",
            "verify_token",
            h_auth,
            &[
                NeighborRef {
                    relative: "src/token.rs".to_string(),
                    symbol: "decode_jwt".to_string(),
                    line: 10,
                },
                NeighborRef {
                    relative: "src/api.rs".to_string(),
                    symbol: "login".to_string(),
                    line: 25,
                },
            ],
        );

        graph.record_visit(
            "src/token.rs",
            "decode_jwt",
            h_tok,
            VisitLevel::BodySeen,
            Some("read: verify_token"),
        );

        let hint = graph
            .backtrack_hint("src/auth.rs", "verify_token")
            .expect("hint esperado");
        let line = hint.format_line().expect("linha esperada");
        assert!(line.contains("já abertos"), "{line}");
        assert!(line.contains("decode_jwt"), "{line}");
        assert!(line.contains("fronteira ainda não aberta"), "{line}");
        assert!(line.contains("login"), "{line}");

        graph.record_visit(
            "src/api.rs",
            "login",
            h_api,
            VisitLevel::BodySeen,
            Some("read: verify_token"),
        );

        let closed = graph
            .backtrack_hint("src/auth.rs", "verify_token")
            .expect("hint esperado");
        let closed_line = closed.format_line().expect("linha esperada");
        assert!(closed_line.contains("ramo já fechado"), "{closed_line}");
    }

    #[test]
    fn query_seen_detects_repeated_equivalent_queries_while_hashes_match() {
        let graph = BacktrackGraph::new(64);
        let mut hashes = HashMap::new();
        hashes.insert("src/auth.rs".to_string(), hash("v1"));

        let terms = vec!["verify".to_string(), "token".to_string()];
        assert!(!graph.was_query_seen(&terms, None, &hashes));

        graph.record_query(&terms, None, hashes.clone());
        assert!(graph.was_query_seen(&["token".to_string(), "verify".to_string()], None, &hashes));
        assert!(!graph.was_query_seen(&terms, Some("src/api"), &hashes));

        let mut changed = HashMap::new();
        changed.insert("src/auth.rs".to_string(), hash("v2"));
        assert!(!graph.was_query_seen(&terms, None, &changed));
    }

    #[test]
    fn outline_summary_lists_opened_and_unopened_symbols_in_file() {
        let graph = BacktrackGraph::new(64);
        let h = hash("auth");
        graph.record_visit("src/auth.rs", OUTLINE_SYMBOL, h, VisitLevel::BodySeen, None);
        graph.record_visit("src/auth.rs", "verify_token", h, VisitLevel::BodySeen, None);
        graph.record_visit("src/auth.rs", "logout", h, VisitLevel::SignatureSeen, None);

        let (opened, unopened) = graph.outline_summary("src/auth.rs", &h).unwrap();
        assert_eq!(opened, vec!["verify_token".to_string()]);
        assert_eq!(unopened, vec!["logout".to_string()]);
    }

    #[test]
    fn evicts_oldest_nodes_at_ceiling() {
        let graph = BacktrackGraph::new(2);
        let h = hash("x");
        graph.record_visit("a.rs", "f1", h, VisitLevel::BodySeen, None);
        graph.record_visit("a.rs", "f2", h, VisitLevel::BodySeen, None);
        graph.record_visit("a.rs", "f3", h, VisitLevel::BodySeen, None);

        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.visit_level("a.rs", "f1", &h), VisitLevel::Unseen);
    }
}
