//! Contrato de geração de texto usado pelos resumos de documentação.

use std::sync::{Arc, RwLock};

/// Modelo que gera texto curto a partir de um prompt.
pub trait Generator: Send + Sync {
    /// Identificador estável do modelo.
    fn model_id(&self) -> &str;
    /// Gera até `max_tokens` tokens e devolve o texto.
    fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, GenerateError>;
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("modelo indisponível: {0}")]
    Unavailable(String),
    #[error("falha na geração: {0}")]
    Inference(String),
}

/// Slot compartilhado: o load em background preenche, a tool lê.
#[derive(Default)]
pub struct GeneratorSlot(RwLock<Option<Arc<dyn Generator>>>);

impl GeneratorSlot {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(generator: Arc<dyn Generator>) -> Self {
        Self(RwLock::new(Some(generator)))
    }

    pub fn get(&self) -> Option<Arc<dyn Generator>> {
        self.0
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn set(&self, generator: Arc<dyn Generator>) {
        *self
            .0
            .write()
            .unwrap_or_else(|error| error.into_inner()) = Some(generator);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_starts_empty_and_accepts_one_generator() {
        struct Fake;
        impl Generator for Fake {
            fn model_id(&self) -> &str { "fake-gen" }
            fn generate(&self, _prompt: &str, _max_tokens: usize) -> Result<String, GenerateError> {
                Ok("uma linha".to_string())
            }
        }

        let slot = GeneratorSlot::empty();
        assert!(slot.get().is_none());
        slot.set(std::sync::Arc::new(Fake));
        let generator = slot.get().expect("generator presente");
        assert_eq!(generator.model_id(), "fake-gen");
        assert_eq!(generator.generate("x", 8).unwrap(), "uma linha");
    }
}
