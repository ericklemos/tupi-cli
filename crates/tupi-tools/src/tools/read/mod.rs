mod render;

mod source;

#[cfg(test)]
mod ref_tests;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blake3::Hash;
use serde::Deserialize;
use tupi_kernel::{Tool, ToolError, ToolSpec};

use crate::ast::AstChunker;
use crate::budget::{self, Section};
use crate::ledger::{self, Delivery, DeliveryLedger, Seen, Verdict};
use crate::store::ResponseStore;

pub struct ReadTool {
    root_dir: PathBuf,
    pub(crate) store: Arc<ResponseStore>,
    ledger: Arc<DeliveryLedger>,
    default_budget_tokens: usize,
    chunker: AstChunker,
    graph: Option<Arc<crate::graph::GraphCache>>,
    backtrack: Arc<crate::backtrack::BacktrackGraph>,
}

#[derive(Debug, Deserialize)]
pub struct ReadParams {
    pub path: String,
    pub symbol: Option<String>,
    pub budget_tokens: Option<usize>,
    /// `"allow"` pede o corpo mesmo se esta sessão já o recebeu.
    pub repeat: Option<String>,
    /// Opcional: lê `rev:path` (commit, tag ou ref) em vez da árvore de trabalho.
    pub rev: Option<String>,
}

/// Uma seção e o que dizer ao ledger quando se decidir se ela entrou na resposta.
pub(crate) struct Rendered {
    pub(crate) section: Section,
    pub(crate) pending: Option<Pending>,
}

#[derive(Clone)]
pub(crate) enum Pending {
    Symbol {
        path: PathBuf,
        id: String,
        file_hash: Hash,
        text: String,
        complete: bool,
        omitted: bool,
        relative: String,
        neighbors: Vec<crate::backtrack::NeighborRef>,
    },
    Outline {
        relative: String,
        file_hash: Hash,
        symbols: Vec<String>,
    },
}

const MAX_NEIGHBORS: usize = 6;

fn format_neighbors(label: &str, defs: &[&crate::graph::Def]) -> String {
    let items: Vec<String> = defs
        .iter()
        .take(MAX_NEIGHBORS)
        .map(|def| format!("`{}` ({}:{})", def.qualified, def.relative, def.start_line))
        .collect();
    if defs.len() > MAX_NEIGHBORS {
        format!(
            "> {label}: {}, +{} mais",
            items.join(", "),
            defs.len() - MAX_NEIGHBORS
        )
    } else {
        format!("> {label}: {}", items.join(", "))
    }
}

fn format_symbol_list(symbols: &[String], max: usize) -> String {
    let items: Vec<String> = symbols.iter().take(max).map(|s| format!("`{s}`")).collect();
    if symbols.len() > max {
        format!("{}, +{} mais", items.join(", "), symbols.len() - max)
    } else {
        items.join(", ")
    }
}

impl ReadTool {
    pub fn new(
        root_dir: PathBuf,
        store: Arc<ResponseStore>,
        ledger: Arc<DeliveryLedger>,
        default_budget_tokens: usize,
    ) -> Self {
        Self {
            root_dir,
            store,
            ledger,
            default_budget_tokens,
            chunker: AstChunker::new(),
            graph: None,
            backtrack: Arc::new(crate::backtrack::BacktrackGraph::default()),
        }
    }

    pub fn with_parse_cache(mut self, cache: Arc<crate::graph::ParseCache>) -> Self {
        self.chunker = AstChunker::with_cache(cache);
        self
    }

    pub fn with_graph_cache(mut self, graph: Arc<crate::graph::GraphCache>) -> Self {
        self.graph = Some(graph);
        self
    }

    pub fn with_backtrack(mut self, backtrack: Arc<crate::backtrack::BacktrackGraph>) -> Self {
        self.backtrack = backtrack;
        self
    }

    pub fn read(&self, params: ReadParams) -> Result<String, ToolError> {
        if let Some(rev) = params
            .rev
            .as_deref()
            .map(str::trim)
            .filter(|rev| !rev.is_empty())
        {
            let (reference, sections) = self.rev_sections(&params, rev)?;
            let rendered = sections
                .into_iter()
                .map(|section| Rendered {
                    section,
                    pending: None,
                })
                .collect();
            return Ok(format!(
                "{reference}\n\n{}",
                self.finish(rendered, params.budget_tokens)
            ));
        }

        let (path, content, relative) = self.prepare(&params)?;
        let rendered = self.render(&params, &path, &content, &relative)?;

        if rendered.is_empty() {
            return Err(ToolError::SymbolNotFound(format!(
                "símbolo `{}` não encontrado em `{relative}`",
                params.symbol.as_deref().unwrap_or_default()
            )));
        }

        let hash = crate::ledger::short_hash(&crate::ledger::content_hash(&content));
        let mut reference = format!("[ref] {relative} @{hash}");
        if let Some(symbol) = &params.symbol {
            reference.push_str(" :: ");
            reference.push_str(symbol);
        }

        Ok(format!(
            "{reference}\n\n{}",
            self.finish(rendered, params.budget_tokens)
        ))
    }
}

impl Tool for ReadTool {
    fn effect(&self, _arguments: &serde_json::Value) -> tupi_kernel::Effect {
        tupi_kernel::Effect::ReadOnly
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".to_string(),
            description: "Esqueleto do arquivo ou corpo de um símbolo, com blake3.".to_string(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Caminho relativo ao diretório raiz do projeto."
                    },
                    "symbol": {
                        "type": "string",
                        "description": "Opcional: nome da função/struct, simples (`new`) ou qualificado (`Foo::new`), cujo corpo será devolvido."
                    },
                    "budget_tokens": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Teto de tokens (excedente em peek)."
                    },
                    "repeat": {
                        "type": "string",
                        "enum": ["allow"],
                        "description": "`allow` devolve o corpo mesmo se esta sessão já o recebeu; o padrão aponta para o corpo anterior."
                    }
                },
                "required": ["path"]
            }),
        }
    }

    fn call(&self, arguments: serde_json::Value) -> Result<String, ToolError> {
        let params: ReadParams = serde_json::from_value(arguments)
            .map_err(|error| ToolError::InvalidArguments(error.to_string()))?;
        self.read(params)
    }

    fn describe_call(&self, arguments: &serde_json::Value) -> String {
        let mut parts = Vec::new();
        if let Some(path) = arguments.get("path").and_then(|value| value.as_str()) {
            parts.push(format!("path={path}"));
        }
        if let Some(symbol) = arguments.get("symbol").and_then(|value| value.as_str()) {
            parts.push(format!("symbol={symbol}"));
        }
        if let Some(budget) = arguments
            .get("budget_tokens")
            .and_then(|value| value.as_u64())
        {
            parts.push(format!("budget={budget}"));
        }
        if let Some(repeat) = arguments.get("repeat").and_then(|value| value.as_str()) {
            parts.push(format!("repeat={repeat}"));
        }

        if parts.is_empty() {
            "read".to_string()
        } else {
            parts.join(" · ")
        }
    }

    fn mode_mask(&self) -> tupi_kernel::ModeMask {
        tupi_kernel::ModeMask::EXECUTION.union(tupi_kernel::ModeMask::SDD)
    }
}
