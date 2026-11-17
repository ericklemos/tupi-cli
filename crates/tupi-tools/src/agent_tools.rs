//! Registro de tools dos agentes de implementação do SDD.
//!
//! O agente implementa, não orquestra: fica sem `sdd`, `pipeline`,
//! `workflow`, `shell`, web e setup. Cada chamada cria `ResponseStore` e
//! `DeliveryLedger` próprios; o grafo é o cache compartilhado da raiz.

use std::sync::Arc;
use std::time::Duration;

use tupi_kernel::ToolRegistry;

use crate::backtrack::BacktrackGraph;
use crate::graph::{GraphCache, ParseCache, RepairingTool};
use crate::ledger::DeliveryLedger;
use crate::{DEFAULT_BUDGET_TOKENS, ResponseStore};

/// Opções do registro enxuto usado pelos agentes de implementação do SDD.
#[derive(Debug, Clone, Default)]
pub struct AgentToolsOptions {
    /// Diretório de estado durável. Reservado para tools futuras.
    pub state_dir: Option<std::path::PathBuf>,
}

/// Registra o subconjunto de tools de um agente de implementação: leitura,
/// edição, execução e tools de grafo. Sem `sdd`, `pipeline`, `workflow`,
/// `shell`, web ou setup.
pub fn register_agent(
    root_dir: std::path::PathBuf,
    registry: &mut ToolRegistry,
    _options: AgentToolsOptions,
) {
    let store = Arc::new(ResponseStore::new(
        Duration::from_secs(600),
        8 * 1024 * 1024,
    ));
    let ledger = Arc::new(DeliveryLedger::new(
        crate::ledger::DEFAULT_ENTRIES,
        crate::ledger::DEFAULT_MAX_BYTES,
    ));
    let parse_cache = Arc::new(ParseCache::new());
    let graph = GraphCache::shared_with_parse_cache(&root_dir, parse_cache.clone());
    let backtrack = Arc::new(BacktrackGraph::default());
    let read = Arc::new(
        crate::read::ReadTool::new(
            root_dir.clone(),
            store.clone(),
            ledger.clone(),
            DEFAULT_BUDGET_TOKENS,
        )
        .with_parse_cache(parse_cache.clone())
        .with_graph_cache(graph.clone())
        .with_backtrack(backtrack.clone()),
    );

    registry.register(read.clone());
    let search = Arc::new(
        crate::search::SearchTool::new(
            root_dir.clone(),
            store.clone(),
            ledger.clone(),
            DEFAULT_BUDGET_TOKENS,
        )
        .with_parse_cache(parse_cache)
        .with_graph_cache(graph.clone())
        .with_backtrack(backtrack),
    );
    registry.register(Arc::new(crate::lookup::LookupTool::new(
        read,
        search,
        root_dir.clone(),
        store.clone(),
        ledger.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(RepairingTool::new(
        Arc::new(crate::edit::EditTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(RepairingTool::new(
        Arc::new(crate::write::WriteTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(RepairingTool::new(
        Arc::new(crate::write_many::WriteManyTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(RepairingTool::new(
        Arc::new(crate::relocate::MoveTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(RepairingTool::new(
        Arc::new(crate::patch::PatchTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::exec::ExecTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::test_many::TestManyTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::map::MapTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::graph_ops::GraphTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::peek::PeekTool::new(
        store,
        DEFAULT_BUDGET_TOKENS,
    )));
}
pub struct DocsAgentOptions {
    pub docs_folders: Vec<std::path::PathBuf>,
    pub embedder: Option<Arc<tupi_kernel::embed::EmbedderSlot>>,
    pub generator: Option<Arc<tupi_kernel::generate::GeneratorSlot>>,
}

pub fn register_docs_agent(
    root_dir: std::path::PathBuf,
    registry: &mut ToolRegistry,
    options: DocsAgentOptions,
) -> Arc<crate::tools::docs::DocsTool> {
    let store = Arc::new(ResponseStore::new(
        Duration::from_secs(600),
        8 * 1024 * 1024,
    ));
    let ledger = Arc::new(DeliveryLedger::new(
        crate::ledger::DEFAULT_ENTRIES,
        crate::ledger::DEFAULT_MAX_BYTES,
    ));
    let parse_cache = Arc::new(ParseCache::new());
    let graph = GraphCache::shared_with_parse_cache(&root_dir, parse_cache.clone());
    let backtrack = Arc::new(BacktrackGraph::default());
    let read = Arc::new(
        crate::read::ReadTool::new(
            root_dir.clone(),
            store.clone(),
            ledger.clone(),
            DEFAULT_BUDGET_TOKENS,
        )
        .with_parse_cache(parse_cache.clone())
        .with_graph_cache(graph.clone())
        .with_backtrack(backtrack.clone()),
    );

    registry.register(read.clone());
    let search = Arc::new(
        crate::search::SearchTool::new(
            root_dir.clone(),
            store.clone(),
            ledger.clone(),
            DEFAULT_BUDGET_TOKENS,
        )
        .with_parse_cache(parse_cache)
        .with_graph_cache(graph.clone())
        .with_backtrack(backtrack),
    );
    registry.register(Arc::new(crate::lookup::LookupTool::new(
        read,
        search,
        root_dir.clone(),
        store.clone(),
        ledger.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::map::MapTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::peek::PeekTool::new(
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    let docs = Arc::new(
        crate::tools::docs::DocsTool::new(
            root_dir,
            options.docs_folders,
            store,
            ledger,
            options.embedder,
            options.generator,
            DEFAULT_BUDGET_TOKENS,
        )
        .with_write(true),
    );
    registry.register(docs.clone());
    docs
}
