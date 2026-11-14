use std::sync::Mutex;

use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::token::LlamaToken;

use crate::InferError;

const QUERY_PREFIX: &str = "[Q] ";
const DOC_PREFIX: &str = "[D] ";
const QUERY_LEN: usize = 32;
const DOC_LEN: usize = 512;
const SKIPLIST: &[&str] = &[
    "!", "\"", "#", "$", "%", "&", "'", "(", ")", "*", "+", ",", "-", ".", "/", ":", ";", "<", "=",
    ">", "?", "@", "[", "\\", "]", "^", "_", "`", "{", "|", "}", "~",
];

/// Spike: extrai vetores por token do ColBERT via llama.cpp (pooling none).
pub struct LlamaColBert {
    backend: LlamaBackend,
    model: LlamaModel,
    lock: Mutex<()>,
}

impl LlamaColBert {
    pub fn load(path: &std::path::Path) -> Result<Self, InferError> {
        llama_cpp_2::send_logs_to_tracing(
            llama_cpp_2::LogOptions::default().with_logs_enabled(false),
        );
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

    fn skiplist(&self) -> Vec<LlamaToken> {
        SKIPLIST
            .iter()
            .flat_map(|word| self.model.vocab().tokenize(word.as_bytes(), false, true))
            .collect()
    }

    pub fn embed_tokens(&self, text: &str, is_query: bool) -> Result<Vec<Vec<f32>>, InferError> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        let prefix = if is_query { QUERY_PREFIX } else { DOC_PREFIX };
        let mut tokens = self
            .model
            .vocab()
            .tokenize(format!("{prefix}{text}").as_bytes(), true, true);
        let skip = if is_query { Vec::new() } else { self.skiplist() };
        if is_query {
            let pad = self.model.vocab().pad();
            while tokens.len() < QUERY_LEN {
                tokens.push(pad);
            }
            tokens.truncate(QUERY_LEN);
        } else {
            tokens.truncate(DOC_LEN);
        }
        let mut context = self
            .model
            .new_context(
                &self.backend,
                LlamaContextParams::default()
                    .with_embeddings(true)
                    .with_pooling_type(LlamaPoolingType::None)
                    .with_n_ctx(std::num::NonZeroU32::new(2048)),
            )
            .map_err(|error| InferError::Inference(error.to_string()))?;
        let mut batch = LlamaBatch::new(context.n_ctx() as usize, 1);
        batch
            .add_sequence(&tokens, 0, false)
            .map_err(|error| InferError::Inference(error.to_string()))?;
        context.clear_kv_cache();
        context
            .decode(&mut batch)
            .map_err(|error| InferError::Inference(error.to_string()))?;
        let mut out = Vec::new();
        for (position, token) in tokens.iter().enumerate() {
            if skip.contains(token) {
                continue;
            }
            let vector = context
                .embeddings_ith(position as i32)
                .map_err(|error| InferError::Inference(error.to_string()))?
                .to_vec();
            let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            if norm > 0.0 {
                out.push(vector.into_iter().map(|value| value / norm).collect());
            }
        }
        Ok(out)
    }
}

/// MaxSim oficial: para cada vetor de query, o maior produto escalar no documento.
pub fn maxsim(query: &[Vec<f32>], document: &[Vec<f32>]) -> f32 {
    query
        .iter()
        .map(|q| {
            document
                .iter()
                .map(|d| q.iter().zip(d).map(|(left, right)| left * right).sum::<f32>())
                .fold(f32::MIN, f32::max)
        })
        .sum()
}
