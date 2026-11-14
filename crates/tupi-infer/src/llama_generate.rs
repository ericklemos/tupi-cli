use std::sync::Mutex;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use tupi_kernel::generate::{GenerateError, Generator};

use crate::InferError;

/// Gerador de texto curto servido por llama.cpp sobre o GGUF do LFM3.
pub struct LlamaGenerator {
    backend: LlamaBackend,
    model: LlamaModel,
    lock: Mutex<()>,
}

impl LlamaGenerator {
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

    fn run(&self, prompt: &str, max_tokens: usize) -> Result<String, GenerateError> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        if max_tokens == 0 {
            return Ok(String::new());
        }
        let mut context = self
            .model
            .new_context(
                &self.backend,
                LlamaContextParams::default()
                    .with_n_ctx(std::num::NonZeroU32::new(2048)),
            )
            .map_err(|error| GenerateError::Inference(error.to_string()))?;
        let n_ctx = context.n_ctx() as usize;
        let limit = n_ctx.saturating_sub(max_tokens).saturating_sub(1);
        if limit == 0 {
            return Err(GenerateError::Inference("contexto pequeno".to_string()));
        }
        let mut tokens = self.model.vocab().tokenize(prompt.as_bytes(), true, true);
        tokens.truncate(limit);
        if tokens.is_empty() {
            return Err(GenerateError::Inference("prompt vazio".to_string()));
        }
        let mut batch = LlamaBatch::new(n_ctx, 1);
        let last = tokens.len() - 1;
        for (position, token) in tokens.iter().enumerate() {
            batch
                .add(*token, position as i32, &[0], position == last)
                .map_err(|error| GenerateError::Inference(error.to_string()))?;
        }
        context.clear_kv_cache();
        context
            .decode(&mut batch)
            .map_err(|error| GenerateError::Inference(error.to_string()))?;
        let mut sampler = LlamaSampler::greedy();
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut output = String::new();
        let mut position = batch.n_tokens();
        for _ in 0..max_tokens {
            let token = sampler.sample(&context, batch.n_tokens() - 1);
            sampler.accept(token);
            if self.model.vocab().is_eog(token) {
                break;
            }
            let piece = self.model.vocab().token_to_piece(token, true, None);
            let mut text = String::new();
            let _ = decoder.decode_to_string(&piece, &mut text, false);
            output.push_str(&text);
            batch.clear();
            batch
                .add(token, position, &[0], true)
                .map_err(|error| GenerateError::Inference(error.to_string()))?;
            context
                .decode(&mut batch)
                .map_err(|error| GenerateError::Inference(error.to_string()))?;
            position += 1;
        }
        Ok(output.trim().to_string())
    }
}

impl Generator for LlamaGenerator {
    fn model_id(&self) -> &str {
        crate::LFM3_REPO
    }

    fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, GenerateError> {
        self.run(prompt, max_tokens)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn real_generation_produces_text() {
        let Ok(path) = std::env::var("TUPI_TEST_GENERATION_GGUF") else {
            return;
        };
        let generator = super::LlamaGenerator::load(std::path::Path::new(&path)).unwrap();
        let text = tupi_kernel::generate::Generator::generate(
            &generator,
            "Resuma em uma linha: A cobranca e mensal.",
            16,
        )
        .unwrap();
        assert!(!text.trim().is_empty());
    }
}
