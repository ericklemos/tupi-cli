use std::path::PathBuf;

use anyhow::Context;
use tupi_app::App;
use tupi_kernel::{CommandBus, EventBus, ProviderRegistry, QueryBus, ToolRegistry};
use tupi_providers::deepseek;
use tupi_tui::run::BusSet;

mod agents_config;
mod config;
mod project_config;
mod prompt_config;
mod sdd_store;
mod tool_config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    struct AgentExecutor {
        store: std::sync::Arc<dyn tupi_tools::sdd::SddStore>,
        provider: std::sync::Arc<dyn tupi_kernel::Provider>,
        model: String,
        root: std::path::PathBuf,
        events: tupi_kernel::EventBus,
        agent_tools: std::sync::Arc<dyn Fn() -> tupi_kernel::ToolRegistry + Send + Sync>,
        max_parallel: usize,
        agent_max_steps: usize,
        agent_timeout: std::time::Duration,
        repo_key: String,
    }

    impl AgentExecutor {
        fn run(&self, plan: &tupi_tools::sdd::SddPlan) -> Result<String, String> {
            let request = tupi_app::sdd_api::SddExecRequest {
                root: self.root.clone(),
                store: self.store.clone(),
                repo_key: self.repo_key.clone(),
                tools: (self.agent_tools)(),
                agent_tools: self.agent_tools.clone(),
                provider: self.provider.clone(),
                model: self.model.clone(),
                agent_prefix: tupi_app::sdd_api::AGENT_PREFIX.to_string(),
                max_parallel: self.max_parallel,
                agent_max_steps: self.agent_max_steps,
                agent_timeout: self.agent_timeout,
                events: self.events.clone(),
                request_id: 0,
                cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                in_flight: std::sync::Arc::new(tokio::sync::Notify::new()),
                in_flight_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            };
            let outcome = tokio::runtime::Handle::current()
                .block_on(tupi_app::sdd_api::execute(request, &plan.slug));
            write_report(&self.root, &plan.slug, &outcome.report);
            if outcome.ok {
                Ok(outcome.report)
            } else {
                Err(outcome.report)
            }
        }
    }

    fn write_report(root: &std::path::Path, slug: &str, report: &str) {
        let dir = root.join(".agents/tupi/sdd");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let _ = std::fs::write(dir.join(format!("{slug}.execution.md")), report);
    }

    fn setting_usize(store: &tupi_store::Store, key: &str, default: usize) -> usize {
        store
            .setting(key)
            .ok()
            .flatten()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(default)
    }

    let cli = parse_cli_options(&std::env::args().collect::<Vec<_>>())?;
    let dotenv_path = dotenvy::dotenv().ok();

    let store_path = data_dir(|key| std::env::var(key).ok()).join("tupi.redb");
    let store = std::sync::Arc::new(tupi_store::Store::open(&store_path).map_err(|error| {
        anyhow::anyhow!(
            "falha ao abrir banco local redb em {}: {error}",
            store_path.display()
        )
    })?);

    let tupi_config_dir = state_dir();
    let opencode_config_dir = data_dir(|key| std::env::var(key).ok())
        .parent()
        .map(|p| p.join("opencode"));

    let mut loaded_from_store = false;
    let antigravity_creds = tupi_app::app::App::load_antigravity_credentials(&store)
        .map(|creds| {
            loaded_from_store = true;
            creds
        })
        .or_else(|| {
            tupi_providers::antigravity::AntigravityCredentials::load_from_dirs(
                &tupi_config_dir,
                opencode_config_dir.as_deref(),
            )
        });

    if !loaded_from_store
        && let Some(creds) = antigravity_creds.as_ref()
        && let Err(error) = tupi_app::app::App::save_antigravity_credentials(&store, creds)
    {
        tracing::warn!(%error, "falha ao migrar credenciais Antigravity para o redb");
    }

    let config = match config::from_env(|key| std::env::var(key).ok()) {
        Ok(cfg) => cfg,
        Err(err) if antigravity_creds.is_some() => {
            tracing::debug!(%err, "iniciando via Antigravity salvo sem chave DeepSeek");
            config::Config {
                api_key: String::new(),
                model: std::env::var("TUPI_MODEL")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or_else(|| tupi_providers::antigravity::DEFAULT_MODEL.to_string()),
                log_filter: std::env::var("TUPI_LOG")
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .unwrap_or_else(|| "warn".to_string()),
                provider: "antigravity".to_string(),
            }
        }
        Err(err) => return Err(err.into()),
    };

    let _log_guard = init_tracing(&config.log_filter);

    let run_started_at = std::time::Instant::now();

    if let Some(path) = dotenv_path {
        tracing::debug!(path = %path.display(), "arquivo .env carregado");
    }

    let events = EventBus::new(256);
    let mut registry = ProviderRegistry::new();
    if !config.api_key.is_empty() {
        deepseek::register_all(&mut registry, &config.api_key);
    }

    if let Some(creds) = antigravity_creds {
        tracing::debug!(project = %creds.project_id, "credenciais Antigravity carregadas");
        tupi_app::app::App::register_antigravity_credentials(
            &registry,
            creds,
            Some(std::sync::Arc::clone(&store)),
        );
    }

    let (provider, model) = if let Some((saved_provider, saved_model)) =
        tupi_app::app::App::load_selected_model(&store)
        && registry.get(&saved_provider).is_some()
    {
        (saved_provider, saved_model)
    } else {
        (config.provider.clone(), config.model.clone())
    };

    let root = tool_root()?;
    let root_label = root.display().to_string();
    let project = project_config::load_or_create(&root);
    tracing::debug!(
        name = %project.name,
        models = project.models.len(),
        path = %project_config::project_path(&root).display(),
        "configuração do projeto"
    );
    let docs_folders = project.docs_folders(&root);
    tracing::debug!(
        count = docs_folders.len(),
        folders = %docs_folders
            .iter()
            .map(|folder| folder.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        "pastas de documentação do projeto"
    );
    let tokens = token::load_token(&root);
    tracing::debug!(
        max_tokens = tokens.max_tokens,
        path = %token::token_path(&root).display(),
        "teto de tokens por resposta do modelo"
    );
    let tool_limits = tool_config::load_tools(&root);
    tracing::debug!(
        max_tool_steps = tool_limits.max_tool_steps,
        path = %tool_config::tools_path(&root).display(),
        "limite de passos de tool call por turno"
    );
    let context_limits = config::load_context(&root);
    tracing::debug!(
        budget = context_limits.budget,
        target = context_limits.target,
        recent_results = context_limits.recent_results,
        path = %config::context_path(&root).display(),
        "configuração de compactação de contexto"
    );
    let context_profile = {
        let base = tupi_app::app::context::ContextLimits {
            budget: context_limits.budget,
            target: context_limits.target,
            recent_results: context_limits.recent_results,
        };
        let mut profile = tupi_app::app::context::ContextProfile::new(base);
        for (key, entry) in &project.models {
            profile = profile.with_model(key.clone(), entry.context_window);
        }
        profile
    };
    let resolved_context = context_profile.resolve(&provider, &model);
    tracing::debug!(
        provider = %provider,
        model = %model,
        budget = resolved_context.budget,
        target = resolved_context.target,
        "janela de contexto do modelo"
    );
    let prompt = prompt_config::load_prompt(&root);
    tracing::debug!(
        path = %prompt_config::prompt_path(&root).display(),
        "prompt do sistema"
    );
    let prefix = tupi_app::prompt::compose(&prompt.system);
    let available = tupi_tools::catalog(&root);
    tracing::debug!(count = available.len(), "workflows disponíveis");
    let workflows_section = tupi_tools::prompt_section(&available);
    let agents_section = agents_config::load_agents_section(&root);
    tracing::debug!(
        path = %agents_config::agents_path(&root).display(),
        loaded = agents_section.is_some(),
        "instruções do projeto (AGENTS.md)"
    );
    let prefix = compose_startup_prefix(&prefix, &workflows_section, agents_section.as_deref());
    let workflows = std::sync::Arc::new(tupi_tools::Catalog::new(root.clone()));
    let mut tools = ToolRegistry::new();
    let debug_dir = debug_dir(&root);
    let telemetry = tupi_app::telemetry::Telemetry::from_env(telemetry_default_path(&root));
    let mode_cell = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
        tupi_kernel::AgentMode::Execution.as_u8(),
    ));
    let repo_key = tupi_tools::repo_key(&root);
    let sdd_store_for_tools: std::sync::Arc<dyn tupi_tools::sdd::SddStore> =
        std::sync::Arc::new(sdd_store::RedbSddStore::new(std::sync::Arc::clone(&store)));
    let agent_root = root.clone();
    let agent_tools = std::sync::Arc::new(move || {
        let mut registry = tupi_kernel::ToolRegistry::new();
        tupi_tools::register_agent(
            agent_root.clone(),
            &mut registry,
            tupi_tools::AgentToolsOptions::default(),
        );
        registry
    });
    let auto_execute = store
        .setting("sdd.auto_execute")
        .ok()
        .flatten()
        .map(|value| value.trim() != "false")
        .unwrap_or(true);
    let mut tool_options = tupi_tools::ToolsOptions::default();
    tool_options.sdd_store = Some(sdd_store_for_tools.clone());
    tool_options.mode_cell = Some(std::sync::Arc::clone(&mode_cell));
    if auto_execute {
        if let Some(provider_impl) = registry.get(&provider) {
            let executor = std::sync::Arc::new(AgentExecutor {
                store: sdd_store_for_tools,
                provider: provider_impl,
                model: model.clone(),
                root: root.clone(),
                events: events.clone(),
                agent_tools,
                max_parallel: setting_usize(&store, "sdd.max_parallel", 4),
                agent_max_steps: setting_usize(&store, "sdd.agent_max_steps", 24),
                agent_timeout: std::time::Duration::from_secs(setting_usize(
                    &store,
                    "sdd.agent_timeout_seconds",
                    600,
                ) as u64),
                repo_key: repo_key.clone(),
            });
            tool_options.sdd_executor = Some(std::sync::Arc::new(
                move |plan: &tupi_tools::sdd::SddPlan| executor.run(plan),
            ));
        }
    }
    tool_options.docs_folders = docs_folders.clone();
    let embedder_slot = std::sync::Arc::new(tupi_kernel::embed::EmbedderSlot::empty());
    if config::docs_embedding_enabled(
        &docs_folders,
        std::env::var("TUPI_RAG_EMBEDDING").ok().as_deref(),
    ) {
        let slot = std::sync::Arc::clone(&embedder_slot);
        let base = data_dir(|key| std::env::var(key).ok());
        tokio::spawn(async move {
            match tupi_infer::ensure_embedding_model(&base, &tupi_infer::HttpFetcher).await {
                Ok(path) => match tupi_infer::LlamaEmbedder::load(&path) {
                    Ok(embedder) => slot.set(std::sync::Arc::new(embedder)),
                    Err(error) => tracing::warn!(%error, "embedder nao carregou"),
                },
                Err(error) => tracing::warn!(%error, "download do embedder falhou"),
            }
        });
    }
    tool_options.embedder = Some(embedder_slot);
    if config::summaries_enabled(
        &docs_folders,
        std::env::var("TUPI_RAG_SUMMARIES").ok().as_deref(),
    ) {
        let generator_slot =
            std::sync::Arc::new(tupi_kernel::generate::GeneratorSlot::empty());
        let slot = std::sync::Arc::clone(&generator_slot);
        let base = data_dir(|key| std::env::var(key).ok());
        tokio::spawn(async move {
            match tupi_infer::ensure_generation_model(&base, &tupi_infer::HttpFetcher).await {
                Ok(path) => match tupi_infer::LlamaGenerator::load(&path) {
                    Ok(generator) => slot.set(std::sync::Arc::new(generator)),
                    Err(error) => tracing::warn!(%error, "gerador de resumos nao carregou"),
                },
                Err(error) => tracing::warn!(%error, "download do gerador falhou"),
            }
        });
        tool_options.generator = Some(generator_slot);
    } else {
        tool_options.generator = None;
    }
    tupi_tools::register_exposed_with(
        root.clone(),
        &mut tools,
        tool_limits.exposure(),
        tool_options,
    );

    let tool_names: Vec<String> = tools.specs().iter().map(|spec| spec.name.clone()).collect();
    let workflow_names: Vec<String> = available
        .iter()
        .map(|workflow| workflow.name.clone())
        .collect();
    let prompt_chars = prefix.chars().count();
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.display().to_string());
    let state_label = state_dir().display().to_string();
    let telemetry_path = telemetry.path().map(|path| path.display().to_string());

    telemetry.record(tupi_app::telemetry::TelemetryEvent::RunStarted(
        tupi_app::telemetry::RunStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
            model: model.clone(),
            provider: provider.clone(),
            cwd,
            root: root_label,
            state_dir: state_label,
            telemetry_path,
            log_filter: config.log_filter.clone(),
            max_tokens: tokens.max_tokens as usize,
            max_tool_steps: tool_limits.max_tool_steps,
            context_budget: resolved_context.budget,
            prompt_chars,
            tool_count: tool_names.len(),
            tools: tool_names,
            workflow_count: workflow_names.len(),
            workflows: workflow_names,
            store_opened: true,
            store_path: store_path.display().to_string(),
            store_error: None,
            store_error_kind: None,
        },
    ));

    let app = App::with_tools(
        registry,
        provider.clone(),
        model.clone(),
        events.clone(),
        Some(store),
        tools,
        prefix,
    )
    .with_telemetry(telemetry.clone())
    .with_max_tool_steps(tool_limits.max_tool_steps)
    .with_context_budget(context_limits.budget)
    .with_context_target(context_limits.target)
    .with_context_recent_results(context_limits.recent_results)
    .with_context_profile(context_profile)
    .with_debug_dir(debug_dir);
    let app = app.with_repo_key(repo_key).with_mode_cell(mode_cell);
    let app = app.with_workflows(workflows);
    let app = app.with_memory(root.clone());

    let web_port = resolve_web_port(cli.web_port, project.dashboard_web_port);
    let dashboard = std::sync::Arc::new(DashboardController {
        ctx: std::sync::Arc::new(tupi_web::DashboardContext {
            root: root.clone(),
            state_dir: state_dir(),
            graph: tupi_tools::graph::GraphCache::shared(&root),
            events: events.clone(),
            queries: QueryBus::new(app.clone()),
            telemetry: telemetry.clone(),
            model: model.clone(),
            provider: provider.clone(),
        }),
        port: web_port,
        handle: tokio::sync::Mutex::new(None),
    });

    let buses = BusSet {
        commands: CommandBus::new(app.clone()),
        queries: QueryBus::new(app.clone()),
        events,
    };

    if cli.web
        && let Err(error) = tupi_tui::run::DashboardLauncher::open(dashboard.as_ref()).await
    {
        tracing::warn!(%error, "falha ao subir o dashboard");
    }

    let launcher: std::sync::Arc<dyn tupi_tui::run::DashboardLauncher> = dashboard.clone();
    let outcome = tupi_tui::run::run_with_dashboard(buses, model, provider, Some(launcher)).await;
    dashboard.shutdown().await;
    let reason = if outcome.is_ok() { "quit" } else { "error" };
    let error = outcome.as_ref().err().map(|error| error.to_string());
    telemetry.record(telemetry.run_finished(run_started_at, reason, error));
    outcome?;
    Ok(())
}

fn tool_root() -> anyhow::Result<PathBuf> {
    let root = std::env::current_dir().context("não foi possível obter o diretório atual")?;
    root.canonicalize()
        .context("não foi possível resolver o diretório atual")
}

fn init_tracing(filter: &str) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let env_filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    let directory = state_dir();
    let _ = std::fs::create_dir_all(&directory);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("tupi.log"));

    match file {
        Ok(file) => {
            let (writer, guard) = tracing_appender::non_blocking(file);
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(writer)
                .with_ansi(false)
                .try_init();
            Some(guard)
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_writer(std::io::sink)
                .with_ansi(false)
                .try_init();
            None
        }
    }
}

fn data_dir<F>(get_env: F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    let home = if cfg!(windows) {
        get_env("USERPROFILE").or_else(|| get_env("HOME"))
    } else {
        get_env("HOME")
    };

    PathBuf::from(home.unwrap_or_else(|| ".".to_string())).join(".tupi")
}

fn state_dir() -> PathBuf {
    if let Ok(directory) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(directory).join("tupi");
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("tupi")
}

fn debug_dir(root: &std::path::Path) -> PathBuf {
    tupi_domain::memory::dir(root).join("debug")
}

fn telemetry_default_path(root: &std::path::Path) -> PathBuf {
    debug_dir(root).join("telemetry.jsonl")
}

fn compose_startup_prefix(
    system_prompt: &str,
    workflows_section: &str,
    agents_section: Option<&str>,
) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !system_prompt.is_empty() {
        parts.push(system_prompt);
    }
    if !workflows_section.is_empty() {
        parts.push(workflows_section);
    }
    if let Some(section) = agents_section.filter(|section| !section.is_empty()) {
        parts.push(section);
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_root_is_absolute_and_exists() {
        let root = tool_root().expect("root");
        assert!(root.is_absolute());
        assert!(root.exists());
    }

    #[test]
    fn data_dir_uses_home() {
        let dir = data_dir(|key| (key == "HOME").then(|| "/home/ana".to_string()));

        assert_eq!(dir, PathBuf::from("/home/ana/.tupi"));
    }

    #[test]
    fn data_dir_falls_back_to_current_dir() {
        assert_eq!(data_dir(|_| None), PathBuf::from("./.tupi"));
    }

    #[cfg(windows)]
    #[test]
    fn data_dir_prefers_userprofile_on_windows() {
        let dir = data_dir(|key| match key {
            "USERPROFILE" => Some("C:\\Users\\ana".to_string()),
            _ => Some("/home/ana".to_string()),
        });

        assert_eq!(dir, PathBuf::from("C:\\Users\\ana\\.tupi"));
    }

    #[test]
    fn telemetry_defaults_to_repo_debug_dir() {
        let path = telemetry_default_path(std::path::Path::new("/repo"));
        assert_eq!(
            path,
            PathBuf::from("/repo/.agents/tupi/debug/telemetry.jsonl")
        );
    }

    #[test]
    fn compose_startup_prefix_includes_agents_after_workflows() {
        let out = compose_startup_prefix(
            "BASE",
            "## Workflows\n\nDisponíveis: feature.",
            Some("## Instruções do projeto (AGENTS.md)\n\nRegras."),
        );
        assert_eq!(
            out,
            "BASE\n\n## Workflows\n\nDisponíveis: feature.\n\n## Instruções do projeto (AGENTS.md)\n\nRegras."
        );
    }

    #[test]
    fn compose_startup_prefix_skips_empty_sections() {
        assert_eq!(compose_startup_prefix("BASE", "", None), "BASE");
        assert_eq!(
            compose_startup_prefix(
                "BASE",
                "",
                Some("## Instruções do projeto (AGENTS.md)\n\nRegras.")
            ),
            "BASE\n\n## Instruções do projeto (AGENTS.md)\n\nRegras."
        );
    }
}
mod token;

/// Opções de linha de comando do binário.
#[derive(Debug, Default, PartialEq, Eq)]
struct CliOptions {
    web: bool,
    web_port: u16,
}

fn parse_cli_options(args: &[String]) -> anyhow::Result<CliOptions> {
    let mut options = CliOptions::default();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--web" => options.web = true,
            "--web-port" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("--web-port exige um número de porta"))?;
                options.web_port = value.parse::<u16>().map_err(|error| {
                    anyhow::anyhow!("porta inválida para --web-port: {value} ({error})")
                })?;
            }
            _ => {}
        }
        index += 1;
    }
    Ok(options)
}

/// Mantém o servidor do dashboard vivo durante a sessão e o reabre sob demanda.
/// Porta efetiva do dashboard: a flag explícita vence; senão, o projeto define.
fn resolve_web_port(cli_port: u16, project_port: Option<u16>) -> u16 {
    if cli_port > 0 {
        cli_port
    } else {
        project_port.unwrap_or(0)
    }
}

struct DashboardController {
    ctx: std::sync::Arc<tupi_web::DashboardContext>,
    port: u16,
    handle: tokio::sync::Mutex<Option<tupi_web::DashboardHandle>>,
}

impl DashboardController {
    async fn open(&self) -> Result<String, String> {
        let mut handle = self.handle.lock().await;
        if handle.is_none() {
            let started = tupi_web::start_server(std::sync::Arc::clone(&self.ctx), self.port)
                .await
                .map_err(|error| error.to_string())?;
            tupi_web::browser::open_browser(started.url());
            *handle = Some(started);
        }
        Ok(handle
            .as_ref()
            .map(|handle| handle.url().to_string())
            .unwrap_or_default())
    }

    async fn shutdown(&self) {
        if let Some(handle) = self.handle.lock().await.take() {
            handle.shutdown().await;
        }
    }
}

#[async_trait::async_trait]
impl tupi_tui::run::DashboardLauncher for DashboardController {
    async fn open(&self) -> Result<String, String> {
        DashboardController::open(self).await
    }
}

#[cfg(test)]
mod cli_tests {
    use super::parse_cli_options;

    #[test]
    fn parse_cli_options_handles_web_flags_and_rejects_invalid_port() {
        let default = parse_cli_options(&["tupi".to_string()]).unwrap();
        assert!(!default.web);
        assert_eq!(default.web_port, 0);

        let enabled = parse_cli_options(&[
            "tupi".to_string(),
            "--web".to_string(),
            "--web-port".to_string(),
            "7788".to_string(),
        ])
        .unwrap();
        assert!(enabled.web);
        assert_eq!(enabled.web_port, 7788);

        assert!(
            parse_cli_options(&[
                "tupi".to_string(),
                "--web-port".to_string(),
                "abc".to_string()
            ])
            .is_err()
        );
        assert!(parse_cli_options(&["tupi".to_string(), "--web-port".to_string()]).is_err());
    }
}

#[cfg(test)]
mod web_port_tests {
    use super::resolve_web_port;

    #[test]
    fn cli_port_wins_over_the_project_configuration() {
        assert_eq!(resolve_web_port(7788, Some(9000)), 7788);
    }

    #[test]
    fn the_project_port_applies_when_the_flag_is_absent() {
        assert_eq!(resolve_web_port(0, Some(9000)), 9000);
    }

    #[test]
    fn without_any_port_the_os_picks_one() {
        assert_eq!(resolve_web_port(0, None), 0);
    }
}
