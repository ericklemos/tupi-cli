use std::sync::Mutex;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use tupi_kernel::embed::{EmbedError, Embedder};

use crate::InferError;

/// Embedder denso servido por llama.cpp sobre o GGUF do LFM2.5.
pub struct LlamaEmbedder {
    backend: LlamaBackend,
    model: LlamaModel,
    lock: Mutex<()>,
}

impl LlamaEmbedder {
    pub fn load(path: &std::path::Path) -> Result<Self, InferError> {
        let backend =
            LlamaBackend::init().map_err(|error| InferError::Unavailable(error.to_string()))?;
        let model = LlamaModel::load_from_file(&backend, path, &LlamaModelParams::default())
            .map_err(|error| InferError::Unavailable(error.to_string()))?;
        Ok(Self {
            backend,
            model,
            lock: Mutex::new(()),
        })
    }

    fn embed_one(&self, text: &str, max_tokens: usize) -> Result<Vec<f32>, EmbedError> {
        let mut tokens = self.model.vocab().tokenize(text.as_bytes(), true, true);
        tokens.truncate(max_tokens.max(1));
        if tokens.is_empty() {
            return Err(EmbedError::Inference("texto sem tokens".to_string()));
        }
        let mut context = self
            .model
            .new_context(
                &self.backend,
                LlamaContextParams::default()
                    .with_embeddings(true)
                    .with_n_ctx(std::num::NonZeroU32::new(2048)),
            )
            .map_err(|error| EmbedError::Inference(error.to_string()))?;
        let mut batch = LlamaBatch::new(context.n_ctx() as usize, 1);
        batch
            .add_sequence(&tokens, 0, false)
            .map_err(|error| EmbedError::Inference(error.to_string()))?;
        context.clear_kv_cache();
        context
            .decode(&mut batch)
            .map_err(|error| EmbedError::Inference(error.to_string()))?;
        let vector = context
            .embeddings_seq_ith(0)
            .map_err(|error| EmbedError::Inference(error.to_string()))?
            .to_vec();
        if vector.len() != self.dim() {
            return Err(EmbedError::Dimension {
                expected: self.dim(),
                actual: vector.len(),
            });
        }
        Ok(vector)
    }

    fn embed(&self, texts: &[&str], max_tokens: usize) -> Result<Vec<Vec<f32>>, EmbedError> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        texts
            .iter()
            .map(|text| self.embed_one(text, max_tokens))
            .collect()
    }
}

impl Embedder for LlamaEmbedder {
    fn model_id(&self) -> &str {
        crate::EMBEDDING_REPO
    }

    fn dim(&self) -> usize {
        1024
    }

    fn max_doc_tokens(&self) -> usize {
        512
    }

    fn max_query_tokens(&self) -> usize {
        32
    }

    fn token_len(&self, text: &str) -> usize {
        self.model.vocab().tokenize(text.as_bytes(), false, true).len()
    }

    fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|text| format!("document: {text}"))
            .collect();
        let refs: Vec<&str> = prefixed.iter().map(String::as_str).collect();
        self.embed(&refs, self.max_doc_tokens())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        self.embed_one(&format!("query: {text}"), self.max_query_tokens())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn real_model_produces_1024_dims() {
        let Ok(path) = std::env::var("TUPI_TEST_EMBEDDING_GGUF") else {
            return;
        };
        let embedder = super::LlamaEmbedder::load(std::path::Path::new(&path)).unwrap();
        let vectors =
            tupi_kernel::embed::Embedder::embed_documents(&embedder, &["um texto"]).unwrap();
        assert_eq!(vectors[0].len(), 1024);
    }
}
