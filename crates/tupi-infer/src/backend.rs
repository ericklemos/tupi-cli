//! Backend do llama.cpp compartilhado por todo o processo.
//!
//! `LlamaBackend::init` só pode rodar uma vez por processo; a segunda chamada
//! devolve `BackendAlreadyInitialized`. Como o app pode carregar mais de um
//! modelo (embeddings dos docs e geração de resumos, por exemplo), o backend
//! mora num `OnceLock` e todos os loaders recebem a mesma referência estática.

use std::sync::{Mutex, OnceLock};

use llama_cpp_2::llama_backend::LlamaBackend;

use crate::InferError;

static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
static INIT: Mutex<()> = Mutex::new(());

/// Devolve o backend único do processo, inicializando-o na primeira chamada.
pub(crate) fn shared() -> Result<&'static LlamaBackend, InferError> {
    if let Some(backend) = BACKEND.get() {
        return Ok(backend);
    }
    // Sem o mutex, duas threads poderiam chamar `init` ao mesmo tempo e a
    // segunda receberia `BackendAlreadyInitialized`.
    let _guard = INIT.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(backend) = BACKEND.get() {
        return Ok(backend);
    }
    let backend =
        LlamaBackend::init().map_err(|error| InferError::Unavailable(error.to_string()))?;
    Ok(BACKEND.get_or_init(|| backend))
}

#[cfg(test)]
mod tests {
    use super::shared;

    #[test]
    fn the_backend_is_initialized_once_and_reused() {
        let first = shared().expect("primeiro init");
        let second = shared().expect("segundo init reaproveita o backend");

        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn concurrent_callers_share_the_same_backend() {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| shared().map(|backend| std::ptr::from_ref(backend) as usize))
            })
            .collect();
        let addresses: Vec<usize> = threads
            .into_iter()
            .map(|thread| thread.join().expect("thread").expect("backend"))
            .collect();

        assert!(
            addresses.windows(2).all(|pair| pair[0] == pair[1]),
            "endereços divergentes: {addresses:?}"
        );
    }
}
