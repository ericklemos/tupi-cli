//! Dashboard web local, somente leitura, para a sessão ao vivo do tupi.
//!
//! O servidor escuta apenas em `127.0.0.1` e serve a API JSON consumida pelo
//! frontend embutido em `assets/`.

mod routes;
mod graph_api;
mod inspect_api;
mod events;
mod assets;
#[cfg(test)]
mod graph_api_tests;
#[cfg(test)]
mod inspect_api_tests;
#[cfg(test)]
mod events_tests;
#[cfg(test)]
mod assets_tests;

use std::sync::Arc;

use axum::Router;

pub use routes::DashboardContext;

/// Servidor em execução: guarda a URL escolhida pelo sistema e as alças de
/// encerramento gracioso.
pub struct DashboardHandle {
    url: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl DashboardHandle {
    /// URL base do dashboard, por exemplo `http://127.0.0.1:39281`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Encerra o servidor e espera a tarefa terminar.
    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Monta o roteador HTTP completo da sessão.
pub fn build_router(ctx: Arc<DashboardContext>) -> Router {
    routes::router(ctx)
}

/// Sobe o servidor em `127.0.0.1:port` (`0` escolhe uma porta livre).
pub async fn start_server(ctx: Arc<DashboardContext>, port: u16) -> anyhow::Result<DashboardHandle> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    let url = format!("http://127.0.0.1:{}", addr.port());

    let router = build_router(ctx);
    let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let served = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
        if let Err(error) = served {
            tracing::warn!(%error, "servidor do dashboard encerrou com erro");
        }
    });

    Ok(DashboardHandle {
        url,
        shutdown: Some(shutdown),
        task: Some(task),
    })
}
