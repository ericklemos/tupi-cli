//! `kind: "graph"` do `lookup`: quem chama e o que o símbolo/arquivo consome.
//!
//! Era a tool `context_graph`; virou `kind` para o lote inteiro pagar um
//! orçamento só. O alvo é um símbolo (`symbol`) ou um arquivo/prefixo
//! (`path`), e `depth` limita os saltos.

use std::collections::HashSet;
use std::path::Path;

use tupi_kernel::ToolError;

use crate::ast::compact_signature;
use crate::graph::{Def, GraphCache};
use crate::support::budget::Section;
use crate::support::dense::Item;

/// Teto de saltos: o mesmo da tool que este `kind` absorveu.
const MAX_DEPTH: usize = 3;
/// Quantos itens por nível cabem na seção antes do resumo.
const MAX_LISTED: usize = 12;

/// Monta as seções do `kind: "graph"` num único item por nível.
pub(crate) fn sections(
    root: &Path,
    symbol: Option<&str>,
    path: Option<&str>,
    depth: Option<usize>,
    priority: u8,
) -> Result<Vec<Item>, ToolError> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let graph = GraphCache::shared(&root).get();
    let depth = depth.unwrap_or(1).clamp(1, MAX_DEPTH);
    let prefix = path
        .map(|path| path.trim().trim_end_matches('/'))
        .filter(|path| !path.is_empty());
    let symbol = symbol.map(str::trim).filter(|symbol| !symbol.is_empty());

    let roots: Vec<&Def> = match symbol {
        Some(name) => {
            let mut matched = graph.matching(name);
            if let Some(prefix) = prefix {
                matched.retain(|def| def.relative.starts_with(prefix));
            }
            matched
        }
        None => match prefix {
            Some(prefix) => graph
                .defs
                .iter()
                .filter(|def| {
                    def.relative == prefix || def.relative.starts_with(&format!("{prefix}/"))
                })
                .collect(),
            None => Vec::new(),
        },
    };
    let target = symbol.or(prefix).unwrap_or("?");
    if roots.is_empty() {
        return Err(ToolError::SymbolNotFound(format!(
            "{target} não encontrado no grafo"
        )));
    }

    let mut items = vec![plain(
        format!(
            "### contexto — {} ({} raiz(es), profundidade {depth})",
            target,
            roots.len()
        ),
        String::new(),
        priority,
    )];

    let mut seen: HashSet<(&str, &str)> = roots
        .iter()
        .map(|def| (def.qualified.as_str(), def.relative.as_str()))
        .collect();
    let mut current: Vec<&Def> = roots.clone();

    for level in 1..=depth {
        let mut callers: Vec<&Def> = Vec::new();
        let mut callees: Vec<&Def> = Vec::new();
        for def in &current {
            for caller in graph.callers(&def.name) {
                if seen.insert((caller.qualified.as_str(), caller.relative.as_str())) {
                    callers.push(caller);
                }
            }
            for callee in graph.callees_for(def) {
                if seen.insert((callee.qualified.as_str(), callee.relative.as_str())) {
                    callees.push(callee);
                }
            }
        }

        let step = (level as u8).saturating_mul(10);
        if !callers.is_empty() {
            items.push(list(
                format!("#### nível {level} — chamadores"),
                &callers,
                priority.saturating_sub(step),
            ));
        }
        if !callees.is_empty() {
            items.push(list(
                format!("#### nível {level} — consumidores"),
                &callees,
                priority.saturating_sub(step).saturating_sub(5),
            ));
        }

        current = callers.into_iter().chain(callees).collect();
        if current.is_empty() {
            break;
        }
    }

    Ok(items)
}

fn list(title: String, defs: &[&Def], priority: u8) -> Item {
    let mut body = String::new();
    for def in defs.iter().take(MAX_LISTED) {
        body.push_str(&format!(
            "- {} ({}:{}) · {}\n",
            def.qualified,
            def.relative,
            def.start_line,
            compact_signature(&def.signature)
        ));
    }
    if defs.len() > MAX_LISTED {
        body.push_str(&format!("(+{} itens)\n", defs.len() - MAX_LISTED));
    }
    Item {
        section: Section {
            heading: title,
            body: format!("\n{body}"),
            priority,
        },
        track: None,
    }
}

fn plain(heading: String, body: String, priority: u8) -> Item {
    Item {
        section: Section {
            heading,
            body,
            priority,
        },
        track: None,
    }
}
