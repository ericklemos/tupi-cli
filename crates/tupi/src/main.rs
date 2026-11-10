use std::path::PathBuf;

use tupi_app::App;
use tupi_kernel::{CommandBus, EventBus, ProviderRegistry, QueryBus};
use tupi_providers::deepseek;
use tupi_tui::{BusSet, run};

mod config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dotenv_path = dotenvy::dotenv().ok();
    let config = config::from_env(|key| std::env::var(key).ok())?;
    let _log_guard = init_tracing(&config.log_filter);

    if let Some(path) = dotenv_path {
        tracing::debug!(path = %path.display(), "arquivo .env carregado");
    }

    let events = EventBus::new(256);
    let mut registry = ProviderRegistry::new();
    deepseek::register_all(&mut registry, &config.api_key);

    let provider = "deepseek".to_string();
    let store =
        match tupi_store::Store::open(data_dir(|key| std::env::var(key).ok()).join("tupi.redb")) {
            Ok(store) => Some(std::sync::Arc::new(store)),
            Err(error) => {
                tracing::warn!(%error, "base local indisponível; seguindo sem cache");
                None
            }
        };
    let app = App::with_store(
        registry,
        provider.clone(),
        config.model.clone(),
        events.clone(),
        store,
    );

    let buses = BusSet {
        commands: CommandBus::new(app.clone()),
        queries: QueryBus::new(app.clone()),
        events,
    };

    run(buses, config.model, provider).await?;
    Ok(())
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
