//! Adaptador do Whisper: tokens com timestamp viram palavras.

use crate::merge::Word;
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Um token do Whisper, com tempo em centésimos de segundo.
#[derive(Debug, Clone, PartialEq)]
pub struct RawToken {
    pub text: String,
    pub t0_cs: i64,
    pub t1_cs: i64,
}

/// Falhas do reconhecimento de fala.
#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("não foi possível carregar o modelo do Whisper: {0}")]
    Load(String),
    #[error("falha ao transcrever: {0}")]
    Run(String),
}

/// Contexto do Whisper carregado uma vez e reaproveitado entre arquivos.
pub struct Asr {
    state: std::sync::Mutex<whisper_rs::WhisperState>,
}

impl Asr {
    /// Carrega o modelo ggml informado.
    pub fn load(model: &Path) -> Result<Self, AsrError> {
        Ok(Self::load_detailed(model)?.0)
    }

    /// Carrega o modelo e informa se o backend GPU do whisper.cpp entrou em uso.
    ///
    /// `Some(motivo)` significa que o whisper.cpp caiu para CPU, junto com as
    /// linhas que ele mesmo registrou sobre GPU/CUDA.
    pub fn load_detailed(model: &Path) -> Result<(Self, Option<String>), AsrError> {
        unsafe extern "C" fn capture(
            _level: u32,
            text: *const std::ffi::c_char,
            user_data: *mut std::ffi::c_void,
        ) {
            if text.is_null() {
                return;
            }
            // SAFETY: `text` vem do whisper.cpp como C string terminada em NUL.
            let line = unsafe { std::ffi::CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned();
            {
                use std::io::Write as _;
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
            if user_data.is_null() {
                return;
            }
            // SAFETY: `user_data` aponta para o `Mutex` que vive em `load_detailed`.
            let buffer = unsafe { &*(user_data as *const std::sync::Mutex<Vec<String>>) };
            if let Ok(mut lines) = buffer.try_lock() {
                lines.push(line);
            }
        }

        let logs: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        // SAFETY: o callback só toca o buffer local, que sobrevive ao carregamento.
        unsafe {
            whisper_rs::set_log_callback(
                Some(capture),
                &logs as *const std::sync::Mutex<Vec<String>> as *mut std::ffi::c_void,
            );
        }
        let loaded = WhisperContext::new_with_params(model, WhisperContextParameters::default());
        // SAFETY: volta ao callback padrão antes de o buffer sair de escopo.
        unsafe {
            whisper_rs::set_log_callback(None, std::ptr::null_mut());
        }
        let context = loaded.map_err(|error| AsrError::Load(error.to_string()))?;

        let lines = logs.into_inner().unwrap_or_default();
        // Sem a feature `cuda` o whisper.cpp nem tenta GPU, então não há log de
        // falha; a ausência de backend só pode ser inferida pelo build.
        let gpu_built = cfg!(feature = "cuda");
        let refused = lines.iter().any(|line| line.contains("no GPU found"));
        let gpu = gpu_built && !refused;
        let note = if gpu {
            None
        } else if !gpu_built {
            Some("build sem a feature cuda".to_string())
        } else {
            let evidence: Vec<String> = lines
                .iter()
                .filter(|line| {
                    line.contains("GPU")
                        || line.contains("CUDA")
                        || line.contains("cuda")
                        || line.contains("ggml")
                })
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();
            Some(if evidence.is_empty() {
                "backend GPU não inicializou".to_string()
            } else {
                evidence.join("; ")
            })
        };

        // O estado guarda o contexto (Arc) e é reaproveitado entre arquivos: o
        // whisper.cpp realoca ~370 MB de buffers a cada `create_state`.
        let state = context
            .create_state()
            .map_err(|error| AsrError::Load(error.to_string()))?;

        Ok((
            Self {
                state: std::sync::Mutex::new(state),
            },
            note,
        ))
    }

    /// Transcreve amostras de 16 kHz mono, devolvendo palavras com tempo.
    pub fn transcribe(
        &self,
        samples: &[f32],
        language: Option<&str>,
        threads: usize,
    ) -> Result<Vec<Word>, AsrError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_translate(false);
        params.set_language(language);
        params.set_token_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_n_threads(threads.max(1) as i32);

        state
            .full(params, samples)
            .map_err(|error| AsrError::Run(error.to_string()))?;

        let segments = state.full_n_segments();
        let mut tokens = Vec::new();
        for segment_index in 0..segments {
            let segment = state.get_segment(segment_index).ok_or_else(|| {
                AsrError::Run(format!("segmento {segment_index} fora do intervalo"))
            })?;
            let count = segment.n_tokens();
            for token_index in 0..count {
                let token = segment.get_token(token_index).ok_or_else(|| {
                    AsrError::Run(format!("token {token_index} fora do intervalo"))
                })?;
                let data = token.token_data();
                tokens.push(RawToken {
                    text: token
                        .to_str_lossy()
                        .map_err(|error| AsrError::Run(error.to_string()))?
                        .to_string(),
                    t0_cs: data.t0,
                    t1_cs: data.t1,
                });
            }
        }

        Ok(words_from_tokens(&tokens))
    }
}

/// Agrupa tokens em palavras: token com espaço à frente começa palavra nova.
///
/// Tokens especiais (`[_...]`) e vazios são descartados.
pub fn words_from_tokens(tokens: &[RawToken]) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut current: Option<Word> = None;

    for token in tokens {
        let text = token.text.as_str();
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.starts_with("[_") {
            continue;
        }

        let starts_word = text.chars().next().is_some_and(char::is_whitespace);
        match current.as_mut() {
            Some(word) if !starts_word => {
                word.text.push_str(trimmed);
                word.end_cs = token.t1_cs;
            }
            _ => {
                if let Some(word) = current.take() {
                    words.push(word);
                }
                current = Some(Word {
                    start_cs: token.t0_cs,
                    end_cs: token.t1_cs,
                    text: trimmed.to_string(),
                });
            }
        }
    }

    if let Some(word) = current {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str, t0_cs: i64, t1_cs: i64) -> RawToken {
        RawToken {
            text: text.to_string(),
            t0_cs,
            t1_cs,
        }
    }

    #[test]
    fn a_leading_space_starts_a_new_word() {
        let tokens = [token(" ola", 0, 10), token(" mundo", 10, 20)];

        let words = words_from_tokens(&tokens);

        let texts: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(texts, vec!["ola", "mundo"]);
    }

    #[test]
    fn punctuation_stays_in_the_same_word() {
        let tokens = [token(" ola", 0, 10), token(",", 10, 10)];

        let words = words_from_tokens(&tokens);

        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "ola,");
    }

    #[test]
    fn special_tokens_are_dropped() {
        let tokens = [
            token("[_BEG_]", 0, 0),
            token(" ola", 0, 10),
            token("[_TT_]", 10, 10),
        ];

        let words = words_from_tokens(&tokens);

        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "ola");
    }

    #[test]
    fn the_word_span_covers_all_its_tokens() {
        let tokens = [token(" ol", 5, 10), token("a", 10, 12)];

        let words = words_from_tokens(&tokens);

        assert_eq!(words.len(), 1);
        assert_eq!(words[0].start_cs, 5);
        assert_eq!(words[0].end_cs, 12);
    }
}
