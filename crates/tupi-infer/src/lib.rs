//! Runtime local de modelos para o RAG de documentação.

use thiserror::Error;

pub mod cache;
pub mod download;
#[cfg(feature = "local-models")]
pub mod llama;
#[cfg(feature = "local-models")]
pub mod llama_colbert;
#[cfg(feature = "local-models")]
pub mod llama_generate;

pub use cache::models_dir;
pub use download::{
    Fetcher, HttpFetcher, ensure_embedding_model, ensure_generation_model, ensure_model,
};
#[cfg(feature = "local-models")]
pub use llama::LlamaEmbedder;
#[cfg(feature = "local-models")]
pub use llama_colbert::{LlamaColBert, maxsim};
#[cfg(feature = "local-models")]
pub use llama_generate::LlamaGenerator;

pub const EMBEDDING_REPO: &str = "LiquidAI/LFM2.5-Embedding-350M-GGUF";
pub const EMBEDDING_FILE: &str = "LFM2.5-Embedding-350M-Q8_0.gguf";
pub const EMBEDDING_SHA256: &str =
    "6ec5f8e8750dbc8a0e40c431fd1b7b07a13688136b2244c5a1364b54d9032599";
pub const EMBEDDING_BYTES: u64 = 379_216_640;

pub const LFM3_REPO: &str = "LiquidAI/LFM3-1.7B-GGUF";
pub const LFM3_FILE: &str = "LFM3-1.7B-Q4_K_M.gguf";
pub const LFM3_SHA256: &str = "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4";
pub const LFM3_BYTES: u64 = 1_099_627_808;

#[derive(Debug, Error)]
pub enum InferError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("checksum inválido")]
    Checksum,
    #[error("tamanho inesperado: esperado {expected}, veio {actual}")]
    Size { expected: u64, actual: u64 },
    #[error("modelo indisponível: {0}")]
    Unavailable(String),
    #[error("inferência: {0}")]
    Inference(String),
}
mod backend;

