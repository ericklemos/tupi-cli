mod ast;
mod budget;
mod edit;
mod ledger;
mod peek;
mod ranking;
mod read;
mod read_many;
mod scanner;
mod search;
mod span;
mod store;
mod terms;

pub use ast::{AstChunker, AstSnippet};
pub use budget::{
    CHARS_PER_TOKEN, DEFAULT_BUDGET_TOKENS, Section, Selection, estimate_tokens, render, select,
    split_section,
};
pub use edit::{EditParams, EditTool};
pub use peek::PeekTool;
pub use read::{ReadParams, ReadTool};
pub use read_many::{ReadItem, ReadManyParams, ReadManyTool};
pub use search::{SearchDetail, SearchParams, SearchTool};
pub use store::ResponseStore;
pub use workflow::{
    Catalog, WorkflowTool, catalog, global_agents_dir, local_agents_dir, prompt_section,
};

use std::sync::Arc;
use std::time::Duration;

use tupi_kernel::ToolRegistry;

use crate::ledger::DeliveryLedger;

mod assert;
mod context_graph;
mod diff;
mod env;
/// Registra `read`, `read_many`, `search` e `peek`, compartilhando store e ledger.
mod exec;
mod facts;
mod fetch;
mod graph;
mod impact;
mod map;
mod patch;
mod paths;
mod proc;
mod relocate;
mod research;
mod setup;
mod shell;
mod state;
mod test_many;
mod trace;
mod web;
mod websearch;
mod why;
mod workflow;
mod write;
mod write_many;
pub fn register(root_dir: std::path::PathBuf, registry: &mut ToolRegistry) {
    let store = Arc::new(ResponseStore::new(
        Duration::from_secs(600),
        8 * 1024 * 1024,
    ));
    let ledger = Arc::new(DeliveryLedger::new(
        crate::ledger::DEFAULT_ENTRIES,
        crate::ledger::DEFAULT_MAX_BYTES,
    ));
    let graph = crate::graph::GraphCache::shared(&root_dir);
    let read = Arc::new(ReadTool::new(
        root_dir.clone(),
        store.clone(),
        ledger.clone(),
        DEFAULT_BUDGET_TOKENS,
    ));

    registry.register(read.clone());
    registry.register(Arc::new(ReadManyTool::new(read)));
    registry.register(Arc::new(SearchTool::new(
        root_dir.clone(),
        store.clone(),
        ledger,
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(WorkflowTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::graph::InvalidatingTool::new(
        Arc::new(EditTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::InvalidatingTool::new(
        Arc::new(crate::write::WriteTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::InvalidatingTool::new(
        Arc::new(crate::write_many::WriteManyTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::InvalidatingTool::new(
        Arc::new(crate::relocate::MoveTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::InvalidatingTool::new(
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
    registry.register(Arc::new(crate::graph::InvalidatingTool::new(
        Arc::new(crate::shell::ShellTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::map::MapTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::assert::AssertTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::impact::ImpactTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::context_graph::ContextTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::trace::TraceTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::diff::DiffTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::why::WhyTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::fetch::FetchTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::websearch::WebSearchTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::research::ResearchTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::facts::FactsTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::state::StateTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::env::EnvTool::new()));
    registry.register(Arc::new(PeekTool::new(
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    let specs = registry.specs();
    registry.register(Arc::new(crate::setup::SetupTool::new(
        root_dir,
        store,
        specs,
        DEFAULT_BUDGET_TOKENS,
    )));
}
