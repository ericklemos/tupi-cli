pub mod ast;
mod budget;
mod edit;
mod ledger;
mod peek;
mod ranking;
mod read;
mod read_many;
pub mod scanner;
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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
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
pub mod graph;
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
/// Opções de composição das tools.
#[derive(Debug, Clone)]
pub struct ToolsOptions {
    /// Diretório de estado durável. `None` usa o padrão XDG do Tupi.
    pub state_dir: Option<std::path::PathBuf>,
    /// Liga o watcher do grafo.
    pub watcher: bool,
    /// Liga o cache persistente de `exec`.
    pub exec_cache: bool,
    /// Liga o cache persistente de web.
    pub web_cache: bool,
}

impl Default for ToolsOptions {
    fn default() -> Self {
        Self {
            state_dir: None,
            watcher: true,
            exec_cache: true,
            web_cache: true,
        }
    }
}

impl ToolsOptions {
    /// Opções para testes e bench: estado no diretório dado, sem watcher nem caches.
    pub fn hermetic(state_dir: std::path::PathBuf) -> Self {
        Self {
            state_dir: Some(state_dir),
            watcher: false,
            exec_cache: false,
            web_cache: false,
        }
    }
}

/// Registra o catálogo de produção.
pub fn register(root_dir: std::path::PathBuf, registry: &mut ToolRegistry) {
    register_with(root_dir, registry, ToolsOptions::default());
}

/// Registra o catálogo com opções explícitas de composição.
pub fn register_with(
    root_dir: std::path::PathBuf,
    registry: &mut ToolRegistry,
    options: ToolsOptions,
) {
    let state_dir = options.state_dir.clone();
    let watcher = options.watcher;
    let store = Arc::new(ResponseStore::new(
        Duration::from_secs(600),
        8 * 1024 * 1024,
    ));
    let ledger = Arc::new(DeliveryLedger::new(
        crate::ledger::DEFAULT_ENTRIES,
        crate::ledger::DEFAULT_MAX_BYTES,
    ));
    let parse_cache = Arc::new(crate::graph::ParseCache::new());
    let graph = crate::graph::GraphCache::shared_with_parse_cache(&root_dir, parse_cache.clone());
    if watcher {
        start_watcher(&root_dir, graph.clone());
    }
    let read = Arc::new(
        ReadTool::new(
            root_dir.clone(),
            store.clone(),
            ledger.clone(),
            DEFAULT_BUDGET_TOKENS,
        )
        .with_parse_cache(parse_cache.clone()),
    );

    registry.register(read.clone());
    registry.register(Arc::new(ReadManyTool::new(read)));
    registry.register(Arc::new(
        SearchTool::new(
            root_dir.clone(),
            store.clone(),
            ledger,
            DEFAULT_BUDGET_TOKENS,
        )
        .with_parse_cache(parse_cache),
    ));
    registry.register(Arc::new(WorkflowTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    registry.register(Arc::new(crate::pipeline::PipelineTool));
    registry.register(Arc::new(crate::graph::RepairingTool::new(
        Arc::new(EditTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::RepairingTool::new(
        Arc::new(crate::write::WriteTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::RepairingTool::new(
        Arc::new(crate::write_many::WriteManyTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::RepairingTool::new(
        Arc::new(crate::relocate::MoveTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::RepairingTool::new(
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
    registry.register(Arc::new(crate::graph::RepairingTool::new(
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
    let facts = match state_dir.clone() {
        Some(dir) => crate::facts::FactsTool::with_state_dir_and_store(
            root_dir.clone(),
            store.clone(),
            dir,
            DEFAULT_BUDGET_TOKENS,
        ),
        None => crate::facts::FactsTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        ),
    };
    registry.register(Arc::new(facts));
    let state = match state_dir {
        Some(dir) => crate::state::StateTool::with_state_dir_and_store(
            root_dir.clone(),
            store.clone(),
            dir,
            DEFAULT_BUDGET_TOKENS,
        ),
        None => crate::state::StateTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        ),
    };
    registry.register(Arc::new(state));
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
fn start_watcher(root: &std::path::Path, cache: Arc<crate::graph::GraphCache>) {
    static WATCHERS: OnceLock<Mutex<HashMap<PathBuf, crate::graph::GraphWatcher>>> = OnceLock::new();
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut watchers = WATCHERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if watchers.contains_key(&key) {
        return;
    }
    if let Ok(watcher) = crate::graph::GraphWatcher::start(key.clone(), cache) {
        watchers.insert(key, watcher);
    }
}

mod pipeline;

/// Tools expostas ao modelo quando a configuração não pede um perfil próprio.
pub const CORE_TOOLS: &[&str] = &["edit", "exec", "map", "read", "read_many", "search"];

/// Quais tools do catálogo ficam visíveis ao modelo.
#[derive(Debug, Clone, Copy)]
pub enum Exposure<'a> {
    /// Só o conjunto núcleo (`CORE_TOOLS`).
    Core,
    /// Todas as tools registradas.
    All,
    /// Somente as tools cujo nome está na lista.
    Only(&'a [String]),
}

impl Exposure<'_> {
    /// `true` se `name` fica exposta ao modelo neste perfil.
    fn admits(&self, name: &str) -> bool {
        match self {
            Self::Core => CORE_TOOLS.contains(&name),
            Self::All => true,
            Self::Only(names) => names.iter().any(|wanted| wanted == name),
        }
    }
}

/// Registra no `registry` só as tools que `exposure` admite.
pub fn register_exposed(
    root_dir: std::path::PathBuf,
    registry: &mut ToolRegistry,
    exposure: Exposure<'_>,
) {
    let mut staged = ToolRegistry::new();
    register(root_dir, &mut staged);
    for tool in staged.into_tools() {
        if exposure.admits(&tool.spec().name) {
            registry.register(tool);
        }
    }
}
