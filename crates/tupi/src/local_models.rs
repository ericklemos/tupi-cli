//! Carregamento dos modelos locais (llama.cpp), atrás da feature `local-models`.
//!
//! O binário declara `tupi-infer` sem `local-models` fixo: quem liga o
//! `llama-cpp-2` são as features `local-models`/`cuda`/`metal`/`vulkan`. Sem elas
//! o build fica puramente Rust (sem a compilação C++ do `llama.cpp`), o que
//! permite `cargo build -p tupi --no-default-features` e reduz o custo de build
//! nativo. O default continua ligando `local-models`, então nada muda para quem
//! builda normalmente.

use std::sync::Arc;

/// Carrega o embedder local. Sem a feature, apenas avisa e devolve `None`.
#[cfg(feature = "local-models")]
pub fn load_embedder(path: &std::path::Path) -> Option<Arc<dyn tupi_kernel::embed::Embedder>> {
    match tupi_infer::LlamaEmbedder::load(path) {
        Ok(embedder) => Some(Arc::new(embedder)),
        Err(error) => {
            tracing::warn!(%error, "embedder nao carregou");
            None
        }
    }
}

/// Variante headless: o binário não trouxe `llama.cpp`.
#[cfg(not(feature = "local-models"))]
pub fn load_embedder(path: &std::path::Path) -> Option<Arc<dyn tupi_kernel::embed::Embedder>> {
    tracing::warn!(
        path = %path.display(),
        "binário compilado sem `local-models`: embeddings locais desligados"
    );
    None
}

/// Carrega o gerador local de resumos. Sem a feature, apenas avisa e devolve `None`.
#[cfg(feature = "local-models")]
pub fn load_generator(path: &std::path::Path) -> Option<Arc<dyn tupi_kernel::generate::Generator>> {
    match tupi_infer::LlamaGenerator::load(path) {
        Ok(generator) => Some(Arc::new(generator)),
        Err(error) => {
            tracing::warn!(%error, "gerador de resumos nao carregou");
            None
        }
    }
}

/// Variante headless: o binário não trouxe `llama.cpp`.
#[cfg(not(feature = "local-models"))]
pub fn load_generator(path: &std::path::Path) -> Option<Arc<dyn tupi_kernel::generate::Generator>> {
    tracing::warn!(
        path = %path.display(),
        "binário compilado sem `local-models`: resumos locais desligados"
    );
    None
}
