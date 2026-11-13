use std::path::PathBuf;

use anyhow::Context;
use tupi_app::App;
use tupi_kernel::{CommandBus, EventBus, ProviderRegistry, QueryBus, ToolRegistry};
use tupi_providers::deepseek;
use tupi_tui::run::{BusSet, run};

mod config;
mod project_config;
mod prompt_config;
mod tool_config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let telemetry = tupi_app::telemetry::Telemetry::from_env(state_dir().join("telemetry.jsonl"));
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
        path = %project_config::project_path(&root).display(),
        "configuração do projeto"
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
    let prompt = prompt_config::load_prompt(&root);
    tracing::debug!(
        path = %prompt_config::prompt_path(&root).display(),
        "prompt do sistema"
    );
    let prefix = tupi_app::prompt::compose(&prompt.system);
    let available = tupi_tools::catalog(&root);
    tracing::debug!(count = available.len(), "workflows disponíveis");
    let prefix = format!("{prefix}\n\n{}", tupi_tools::prompt_section(&available));
    let workflows = std::sync::Arc::new(tupi_tools::Catalog::new(root.clone()));
    let mut tools = ToolRegistry::new();
    let debug_dir = tupi_domain::memory::dir(&root).join("debug");
    tupi_tools::register_exposed(root.clone(), &mut tools, tool_limits.exposure());

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
            context_budget: context_limits.budget,
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
    .with_debug_dir(debug_dir);
    let app = app.with_workflows(workflows);
    let app = app.with_memory(root.clone());

    let buses = BusSet {
        commands: CommandBus::new(app.clone()),
        queries: QueryBus::new(app.clone()),
        events,
    };

    let outcome = run(buses, model, provider).await;
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
}
mod token;
