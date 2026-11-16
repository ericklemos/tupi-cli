pub mod archive;
pub mod space;
mod support;
mod tools;

pub use support::{ast, graph, scanner};
pub(crate) use support::{
    backtrack, budget, ledger, paths, proc, ranking, span, store, terms, web,
};

pub use tools::sdd;
pub(crate) use tools::{
    assert, clock, context_graph, diff, edit, env, exec, facts, feedback, fetch, impact, lookup, map, patch,
    peek, read, read_many, relocate, research, search, setup, shell, state, test_many, trace,
    websearch, why, workflow, write, write_many,
};

pub use ast::{AstChunker, AstSnippet};
pub use budget::{
    CHARS_PER_TOKEN, DEFAULT_BUDGET_TOKENS, Section, Selection, estimate_tokens, render, select,
    split_section,
};
pub use store::ResponseStore;

pub use edit::{EditParams, EditTool};
pub use peek::PeekTool;
pub use read::{ReadParams, ReadTool};
pub use read_many::{ReadItem, ReadManyParams, ReadManyTool};
pub use search::{SearchDetail, SearchParams, SearchTool};
pub use workflow::{
    Catalog, WorkflowTool, catalog, global_agents_dir, local_agents_dir, prompt_section,
};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tupi_kernel::ToolRegistry;

use crate::ledger::DeliveryLedger;
pub use agent_tools::{DocsAgentOptions, register_docs_agent};
pub use paths::repo_key;
pub use sdd::{
    InMemorySddStore, SddPlan, SddPlanStatus, SddStore, SddSubtask, SddSubtaskStatus, SddTask,
    SddTaskStatus, SddTool,
};
pub use tools::docs::{DocsRefresh, DocsTool, DocsWrite};

pub use agent_tools::{AgentToolsOptions, register_agent};
#[derive(Clone)]
pub struct ToolsOptions {
    /// Diretório de estado durável. `None` usa o padrão XDG do Tupi.
    pub state_dir: Option<std::path::PathBuf>,
    /// Liga o watcher do grafo.
    pub watcher: bool,
    /// Liga o cache em RAM de `exec` (`false` desliga o fingerprint inteiro).
    pub exec_cache: bool,
    /// Persistência de planos SDD. `None` usa `InMemorySddStore`.
    pub sdd_store: Option<Arc<dyn crate::sdd::SddStore>>,
    /// Célula compartilhada com o `AgentMode` ativo (`0=Execution`, `1=Sdd`).
    pub mode_cell: Option<Arc<std::sync::atomic::AtomicU8>>,
    /// Executor disparado ao fechar um plano SDD.
    pub sdd_executor:
        Option<Arc<dyn Fn(&crate::sdd::SddPlan) -> Result<String, String> + Send + Sync>>,
    /// Pastas com documentação declarada no projeto. Vazio desliga o RAG.
    pub docs_folders: Vec<std::path::PathBuf>,
    /// Slot do embedder local; None mantém a busca lexical.
    pub embedder: Option<Arc<tupi_kernel::embed::EmbedderSlot>>,
    /// Dispara o worker de background do índice; testes desligam.
    pub space_worker: bool,
    /// Índice vetorial local; None mantém a busca lexical.
    pub space: Option<Arc<crate::space::SpaceIndex>>,

    /// Slot do gerador local de resumos; None desliga os resumos.
    pub generator: Option<Arc<tupi_kernel::generate::GeneratorSlot>>,
    /// Acervo do modo docs; None desliga o modo.
    pub archive: Option<crate::tools::archive::ArchiveOptions>,
}

impl Default for ToolsOptions {
    fn default() -> Self {
        Self {
            space_worker: true,
            state_dir: None,
            watcher: true,
            exec_cache: true,
            sdd_store: None,
            mode_cell: None,
            sdd_executor: None,
            docs_folders: Vec::new(),
            embedder: None,
            space: None,
            generator: None,
            archive: None,
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
            sdd_store: None,
            mode_cell: None,
            sdd_executor: None,
            docs_folders: Vec::new(),
            embedder: None,
            space_worker: false,
            space: None,
            generator: None,
            archive: None,
        }
    }
}

/// Registra o catálogo de produção.
/// Um símbolo extraído do repositório para o índice vetorial do spike `space`.
pub struct CorpusItem {
    pub relative: String,
    pub symbol: String,
    pub qualified: String,
    pub signature: String,
    pub code: String,
}

/// Extrai os símbolos de código do repositório inteiro.
///
/// Spike `space`: é a semente do indexador denso; não é API estável.
pub fn symbol_corpus(root: &std::path::Path) -> std::io::Result<Vec<CorpusItem>> {
    use std::collections::HashMap;

    let graph = crate::graph::GraphCache::shared(root).get();
    let mut contents: HashMap<String, String> = HashMap::new();
    let mut items = Vec::new();
    for def in graph.defs.iter() {
        if def.is_test {
            continue;
        }
        let content = contents.entry(def.relative.clone()).or_insert_with(|| {
            std::fs::read_to_string(root.join(&def.relative)).unwrap_or_default()
        });
        let Some(code) =
            crate::tools::docs::store::slice_lines(content, def.start_line, def.end_line)
        else {
            continue;
        };
        items.push(CorpusItem {
            relative: def.relative.clone(),
            symbol: def.name.clone(),
            qualified: def.qualified.clone(),
            signature: def.signature.clone(),
            code,
        });
    }
    Ok(items)
}

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
    if let Some(archive) = options.archive.clone() {
        let session = crate::tools::archive::ArchiveSession::new(archive);
        registry.register(Arc::new(crate::tools::archive::ArchiveTool::new(Arc::clone(
            &session,
        ))));
        let reports_dir = state_dir
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from(".tupi"))
            .join("archive")
            .join("reports");
        registry.register(Arc::new(crate::tools::investigate::InvestigateTool::new(
            session,
            options.generator.clone(),
            crate::tools::investigate::InvestigateOptions {
                reports_dir,
                model: "local".to_string(),
                batch: 32,
            },
        )));
    }
    let watcher = options.watcher;
    let exec_cache = options.exec_cache;
    let sdd_store = options
        .sdd_store
        .unwrap_or_else(|| Arc::new(crate::sdd::InMemorySddStore::default()));
    let mode_cell = options.mode_cell;
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
    if options.space_worker
        && let (Some(space), Some(embedder)) = (options.space.clone(), options.embedder.clone())
    {
        crate::space::build::spawn(root_dir.clone(), space, embedder);
    }

    if watcher {
        start_watcher(&root_dir, graph.clone());
    }
    let backtrack = Arc::new(crate::backtrack::BacktrackGraph::default());
    let read = Arc::new(
        ReadTool::new(
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
        SearchTool::new(
            root_dir.clone(),
            store.clone(),
            ledger.clone(),
            DEFAULT_BUDGET_TOKENS,
        )
        .with_parse_cache(parse_cache)
        .with_graph_cache(graph.clone())
        .with_optional_space(options.space.clone())
        .with_optional_embedder(options.embedder.clone())
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
    registry.register(Arc::new(
        WorkflowTool::new(root_dir.clone(), store.clone(), DEFAULT_BUDGET_TOKENS)
            .with_docs_folders(options.docs_folders.clone()),
    ));
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
    registry.register(Arc::new(crate::graph::RepairingTool::new(
        Arc::new(
            crate::exec::ExecTool::new(
                root_dir.clone(),
                store.clone(),
                DEFAULT_BUDGET_TOKENS,
            )
            .with_cache(exec_cache),
        ),
        graph.clone(),
    )));
    registry.register(Arc::new(crate::graph::RepairingTool::new(
        Arc::new(crate::test_many::TestManyTool::new(
            root_dir.clone(),
            store.clone(),
            DEFAULT_BUDGET_TOKENS,
        )),
        graph.clone(),
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
    // A memória é por repositório (.agents/tupi), não pelo diretório de estado.
    registry.register(Arc::new(crate::facts::FactsTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    // O feedback é um sink local de atrito (.agents/tupi/debug), separado do ledger.
    registry.register(Arc::new(feedback::FeedbackTool::new(
        root_dir.clone(),
        store.clone(),
        DEFAULT_BUDGET_TOKENS,
    )));
    let mut sdd_tool = crate::sdd::SddTool::new(
        root_dir.clone(),
        sdd_store,
        mode_cell,
        tupi_kernel::sdd_config().planning.tool_budget_tokens.unwrap_or(DEFAULT_BUDGET_TOKENS),
    )
    .with_response_store(store.clone())
    .with_docs(!options.docs_folders.is_empty());
    sdd_tool.executor = options.sdd_executor.clone();
    registry.register(Arc::new(sdd_tool));
    if !options.docs_folders.is_empty() {
        registry.register(Arc::new(crate::tools::docs::DocsTool::new(
            root_dir.clone(),
            options.docs_folders.clone(),
            store.clone(),
            ledger.clone(),
            options.embedder.clone(),
            options.generator.clone(),
            DEFAULT_BUDGET_TOKENS,
        )));
    }
    let state = match state_dir {
        Some(dir) => crate::state::StateTool::with_state_dir_and_store(
            root_dir.clone(),
            store.clone(),
            dir,
            DEFAULT_BUDGET_TOKENS,
        ),
        None => {
            crate::state::StateTool::new(root_dir.clone(), store.clone(), DEFAULT_BUDGET_TOKENS)
        }
    };
    registry.register(Arc::new(state));
    registry.register(Arc::new(crate::clock::ClockTool::new()));
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
    static WATCHERS: OnceLock<Mutex<HashMap<PathBuf, crate::graph::GraphWatcher>>> =
        OnceLock::new();
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut watchers = WATCHERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if watchers.contains_key(&key) {
        return;
    }
    // Aquece o grafo fora do caminho crítico: a primeira leitura de símbolo
    // não paga a construção completa do projeto.
    let warm = cache.clone();
    std::thread::spawn(move || {
        let _ = warm.get();
    });
    if let Ok(watcher) = crate::graph::GraphWatcher::start(key.clone(), cache) {
        watchers.insert(key, watcher);
    }
}

mod agent_tools;

/// Tools expostas ao modelo quando a configuração não pede um perfil próprio.
///
/// `write`/`patch` ficam aqui porque `edit` só cobre símbolo Rust: sem eles
/// `.md`, `.toml` e `.json` seriam somente leitura no modo exec.
pub const CORE_TOOLS: &[&str] = &[
    "clock",
    "edit",
    "exec",
    "feedback",
    "lookup",
    "map",
    "patch",
    "read",
    "write",
];

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
            // A tool de recuperação é a saída de emergência de todo
            // truncamento: sem ela no núcleo, o corpo omitido fica inalcançável.
            Self::Core => CORE_TOOLS.contains(&name) || matches!(name, "sdd" | "peek" | "docs"),
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
    register_exposed_with(root_dir, registry, exposure, ToolsOptions::default());
}

pub use state::{StateEntry, default_state_dir, read_entries_for_root as read_state_entries};

pub fn register_exposed_with(
    root_dir: std::path::PathBuf,
    registry: &mut ToolRegistry,
    exposure: Exposure<'_>,
    options: ToolsOptions,
) {
    let mut staged = ToolRegistry::new();
    register_with(root_dir, &mut staged, options);
    for tool in staged.into_tools() {
        if exposure.admits(&tool.spec().name) {
            registry.register(tool);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn register_with_shares_backtrack_between_read_and_search() {
        let dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.rs"),
            "pub fn alpha_one() -> u8 {\n    1\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.rs"),
            "pub fn alpha_two() -> u8 {\n    2\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("token.rs"),
            "pub fn decode_jwt(raw: &str) -> bool {\n    !raw.is_empty()\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("auth.rs"),
            "use crate::token::decode_jwt;\n\npub fn verify_token(raw: &str) -> bool {\n    decode_jwt(raw)\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("api.rs"),
            "use crate::auth::verify_token;\n\npub fn login(raw: &str) -> bool {\n    verify_token(raw)\n}\n",
        )
        .unwrap();

        let mut registry = ToolRegistry::new();
        register_with(
            dir.path().to_path_buf(),
            &mut registry,
            ToolsOptions::hermetic(state_dir.path().to_path_buf()),
        );

        let read = registry.get("read").unwrap();
        let lookup = registry.get("lookup").unwrap();

        read.call(serde_json::json!({ "path": "a.rs" })).unwrap();
        let search_alpha = lookup
            .call(serde_json::json!({ "ops": [{ "kind": "semantic", "query": "alpha" }] }))
            .unwrap();
        assert!(search_alpha.contains("1. `alpha_two`"), "{search_alpha}");
        assert!(search_alpha.contains("2. `alpha_one`"), "{search_alpha}");

        lookup
            .call(serde_json::json!({ "ops": [{ "kind": "semantic", "query": "verify_token" }] }))
            .unwrap();
        read.call(serde_json::json!({ "path": "token.rs", "symbol": "decode_jwt" }))
            .unwrap();
        let revisit = read
            .call(serde_json::json!({ "path": "auth.rs", "symbol": "verify_token" }))
            .unwrap();
        assert!(revisit.contains("Já entregue nesta sessão"), "{revisit}");
        assert!(revisit.contains("> **Backtrack**:"), "{revisit}");
        assert!(revisit.contains("decode_jwt"), "{revisit}");
        assert!(revisit.contains("login"), "{revisit}");
    }

    #[test]
    fn sdd_mode_tool_list_excludes_all_workspace_mutators() {
        let dir = tempdir().unwrap();
        let state_dir = tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        register_with(
            dir.path().to_path_buf(),
            &mut registry,
            ToolsOptions::hermetic(state_dir.path().to_path_buf()),
        );

        let sdd_names: Vec<String> = registry
            .specs_for_mode(tupi_kernel::AgentMode::Sdd)
            .into_iter()
            .map(|s| s.name)
            .collect();

        for required in ["sdd", "read", "lookup", "map", "peek"] {
            assert!(
                sdd_names.iter().any(|n| n == required),
                "esperava {required} em SDD: {sdd_names:?}"
            );
        }
        for forbidden in [
            "edit",
            "write",
            "write_many",
            "move",
            "patch",
            "exec",
            "test_many",
            "shell",
            "setup",
            "state",
        ] {
            assert!(
                !sdd_names.iter().any(|n| n == forbidden),
                "não deveria permitir {forbidden} em SDD: {sdd_names:?}"
            );
        }
    }
    #[test]
    fn docs_is_registered_only_when_folders_are_declared() {
        let dir = tempdir().unwrap();
        let mut with = tupi_kernel::ToolRegistry::new();
        let mut options = ToolsOptions::default();
        options.docs_folders = vec![dir.path().join("docs")];
        register_with(dir.path().to_path_buf(), &mut with, options);
        assert!(with.has("docs"));

        let mut without = tupi_kernel::ToolRegistry::new();
        register_with(
            dir.path().to_path_buf(),
            &mut without,
            ToolsOptions::default(),
        );
        assert!(!without.has("docs"));
    }

    #[test]
    fn docs_is_core_and_only_in_sdd_mode() {
        use tupi_kernel::AgentMode;
        let dir = tempdir().unwrap();
        let mut registry = tupi_kernel::ToolRegistry::new();
        let mut options = ToolsOptions::default();
        options.docs_folders = vec![dir.path().join("docs")];
        register_with(dir.path().to_path_buf(), &mut registry, options);

        let sdd: Vec<_> = registry
            .specs_for_mode(AgentMode::Sdd)
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        let exec: Vec<_> = registry
            .specs_for_mode(AgentMode::Execution)
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        assert!(sdd.contains(&"docs".to_string()), "{sdd:?}");
        assert!(!exec.contains(&"docs".to_string()), "{exec:?}");
        assert!(Exposure::Core.admits("docs"));
    }
}
