use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tupi_kernel::ToolError;

use crate::graph::GraphCache;

/// Observa o root e manda lotes de arquivos alterados para `GraphCache::repair`.
///
/// O watcher vive dentro de uma thread dedicada; o handle devolvido só carrega
/// o canal de shutdown e o `JoinHandle`.
pub struct GraphWatcher {
    shutdown: mpsc::Sender<()>,
    join: Option<JoinHandle<()>>,
}

impl GraphWatcher {
    pub fn start(root: PathBuf, cache: Arc<GraphCache>) -> Result<Self, ToolError> {
        let (shutdown, shutdown_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let join = std::thread::Builder::new()
            .name("tupi-graph-watch".to_string())
            .spawn(move || {
                let (tx, rx) = mpsc::channel::<PathBuf>();
                let watcher = RecommendedWatcher::new(
                    move |result: notify::Result<Event>| {
                        let Ok(event) = result else {
                            return;
                        };
                        if !matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        ) {
                            return;
                        }
                        for path in event.paths {
                            if is_source(&path) {
                                let _ = tx.send(path);
                            }
                        }
                    },
                    Config::default(),
                );

                let mut watcher = match watcher {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                if let Err(error) = watcher.watch(&root, RecursiveMode::Recursive) {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
                let _ = ready_tx.send(Ok(()));

                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }
                    let first = match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(path) => path,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    let mut batch = BTreeSet::new();
                    batch.insert(first);
                    while let Ok(path) = rx.recv_timeout(Duration::from_millis(50)) {
                        batch.insert(path);
                    }
                    let paths: Vec<PathBuf> = batch.into_iter().collect();
                    cache.repair(&paths);
                }
            })
            .map_err(|error| ToolError::Other(format!("watcher do grafo: {error}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                shutdown,
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(ToolError::Other(format!("watcher do grafo: {error}")))
            }
            Err(_) => {
                let _ = join.join();
                Err(ToolError::Other(
                    "watcher do grafo encerrou antes de iniciar".to_string(),
                ))
            }
        }
    }
}

impl Drop for GraphWatcher {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn is_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("rs" | "kt" | "kts")
    )
}
