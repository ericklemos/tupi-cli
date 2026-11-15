mod dense;
mod fusion;
mod params;

mod query;

mod render;

#[cfg(test)]
mod tests;

pub use params::{SearchDetail, SearchParams};

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::ast::{AstChunker, AstSnippet, compact_signature};
use crate::budget::Section;
use crate::ledger::DeliveryLedger;
use crate::ranking::LexicalRanker;
use crate::scanner::LexicalScanner;
use crate::store::ResponseStore;
use crate::terms::extract_terms;

const MAX_SEARCH_BODY_LINES: usize = 80;
const MAX_FULL_BODIES_IN_AUTO: usize = 2;
const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_MAX_RESULTS: usize = 20;

struct Candidate {
    path: PathBuf,
    content: String,
    snippet: AstSnippet,
}

struct Pending {
    path: PathBuf,
    relative: String,
    id: String,
    file_hash: blake3::Hash,
    text: String,
    complete: bool,
    omitted: bool,
    record_ledger: bool,
    visit_level: crate::backtrack::VisitLevel,
    via: String,
}

struct Rendered {
    section: Section,
    pending: Option<Pending>,
}

pub struct SearchTool {
    root_dir: PathBuf,
    scanner: LexicalScanner,
    pub(crate) store: Arc<ResponseStore>,
    ledger: Arc<DeliveryLedger>,
    default_budget_tokens: usize,
    chunker: AstChunker,
    graph: Option<Arc<crate::graph::GraphCache>>,
    space: Option<Arc<crate::space::SpaceIndex>>,
    embedder: Option<Arc<tupi_kernel::embed::EmbedderSlot>>,
    backtrack: Arc<crate::backtrack::BacktrackGraph>,
}

impl SearchTool {
    pub fn new(
        root_dir: PathBuf,
        store: Arc<ResponseStore>,
        ledger: Arc<DeliveryLedger>,
        default_budget_tokens: usize,
    ) -> Self {
        Self {
            scanner: LexicalScanner::new(root_dir.clone()),
            root_dir,
            store,
            ledger,
            default_budget_tokens,
            chunker: AstChunker::new(),
            graph: None,
            space: None,
            embedder: None,
            backtrack: Arc::new(crate::backtrack::BacktrackGraph::default()),
        }
    }

    pub fn with_optional_space(mut self, space: Option<Arc<crate::space::SpaceIndex>>) -> Self {
        self.space = space;
        self
    }

    pub fn with_optional_embedder(
        mut self,
        embedder: Option<Arc<tupi_kernel::embed::EmbedderSlot>>,
    ) -> Self {
        self.embedder = embedder;
        self
    }

    pub fn with_parse_cache(mut self, cache: Arc<crate::graph::ParseCache>) -> Self {
        self.chunker = AstChunker::with_cache(cache);
        self
    }

    pub fn with_backtrack(mut self, backtrack: Arc<crate::backtrack::BacktrackGraph>) -> Self {
        self.backtrack = backtrack;
        self
    }

    pub fn with_graph_cache(mut self, graph: Arc<crate::graph::GraphCache>) -> Self {
        self.graph = Some(graph);
        self
    }
}

impl Tool for SearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search".to_string(),
            description: "Busca lexical no projeto por descrição natural ou nome: devolve corpo dos primeiros resultados e assinatura do resto, dentro do orçamento.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Descrição em linguagem natural do que se procura."
                    },
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Opcional: nomes/termos exatos que devem pesar mais."
                    },
                    "path": {
                        "type": "string",
                        "description": "Opcional: prefixo relativo ao root para restringir a varredura."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 20,
                        "default": 5,
                        "description": "Quantos resultados devolver."
                    },
                    "detail": {
                        "type": "string",
                        "enum": ["auto", "full", "signatures"],
                        "description": "auto: corpo dos 2 primeiros e assinatura do resto; full: corpo de todos; signatures: só assinaturas."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    },
                    "repeat": {
                        "type": "string",
                        "enum": ["allow"],
                        "description": "`allow` devolve corpos mesmo se esta sessão já os recebeu."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: SearchParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.search(params)
    }

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        if let Some(query) = arguments.get("query").and_then(|value| value.as_str()) {
            parts.push(format!("query={query}"));
        }
        if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
            parts.push(format!("path={path}"));
        }
        if let Some(detail) = arguments.get("detail").and_then(|value| value.as_str())
            && detail != "auto"
        {
            parts.push(format!("detail={detail}"));
        }
        if parts.is_empty() {
            "search".to_string()
        } else {
            parts.join(" · ")
        }
    }

    fn sdd_allowed(&self) -> bool {
        true
    }
}
