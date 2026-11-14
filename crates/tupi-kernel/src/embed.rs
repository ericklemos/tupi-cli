//! Contrato de embedding usado pelo RAG de documentação.

use std::sync::{Arc, RwLock};

/// Modelo que transforma texto em vetores.
pub trait Embedder: Send + Sync {
    /// Identificador estável do modelo (ex.: repositório do Hugging Face).
    fn model_id(&self) -> &str;
    /// Dimensão do vetor devolvido.
    fn dim(&self) -> usize;
    /// Teto de tokens por documento aceito pelo modelo.
    fn max_doc_tokens(&self) -> usize;
    /// Teto de tokens por consulta aceito pelo modelo.
    fn max_query_tokens(&self) -> usize;
    /// Quantos tokens o modelo vê no texto.
    fn token_len(&self, text: &str) -> usize;
    /// Embeddings de documentos. Cada texto respeita `max_doc_tokens`.
    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
    /// Embedding de consulta, já truncada em `max_query_tokens`.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError>;
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("modelo indisponível: {0}")]
    Unavailable(String),
    #[error("falha na inferência: {0}")]
    Inference(String),
    #[error("dimensão inesperada: esperado {expected}, veio {actual}")]
    Dimension { expected: usize, actual: usize },
}

/// Slot compartilhado: o download em background preenche, a tool lê.
#[derive(Default)]
pub struct EmbedderSlot(RwLock<Option<Arc<dyn Embedder>>>);

impl EmbedderSlot {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self(RwLock::new(Some(embedder)))
    }

    pub fn get(&self) -> Option<Arc<dyn Embedder>> {
        self.0
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn set(&self, embedder: Arc<dyn Embedder>) {
        *self
            .0
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(embedder);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_starts_empty_and_accepts_one_embedder() {
        struct Fake;
        impl Embedder for Fake {
            fn model_id(&self) -> &str { "fake" }
            fn dim(&self) -> usize { 2 }
            fn max_doc_tokens(&self) -> usize { 8 }
            fn max_query_tokens(&self) -> usize { 4 }
            fn token_len(&self, text: &str) -> usize { text.split_whitespace().count() }
            fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
                Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
            }
            fn embed_query(&self, _text: &str) -> Result<Vec<f32>, EmbedError> {
                Ok(vec![1.0, 0.0])
            }
        }

        let slot = EmbedderSlot::empty();
        assert!(slot.get().is_none());
        slot.set(std::sync::Arc::new(Fake));
        let embedder = slot.get().expect("embedder presente");
        assert_eq!(embedder.dim(), 2);
        assert_eq!(embedder.max_doc_tokens(), 8);
    }
}
