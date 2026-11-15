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

/// Uma GPU (dedicada ou integrada) está disponível neste build e máquina.
pub(crate) fn gpu_offload_available() -> bool {
    llama_cpp_2::list_llama_ggml_backend_devices()
        .iter()
        .any(|device| {
            matches!(
                device.device_type,
                llama_cpp_2::LlamaBackendDeviceType::Gpu
                    | llama_cpp_2::LlamaBackendDeviceType::IntegratedGpu
            )
        })
}

/// Camadas a mandar para a GPU. 999 cobre qualquer modelo atual; o llama.cpp
/// põe na VRAM o que couber e mantém o resto mapeado na RAM.
pub(crate) const ALL_GPU_LAYERS: u32 = 999;

pub(crate) fn gpu_layers(has_gpu: bool) -> u32 {
    if has_gpu { ALL_GPU_LAYERS } else { 0 }
}

/// Parâmetros de carga: GPU com offload quando existe; CPU caso contrário.
/// use_mmap mantém os pesos na RAM e só as camadas vão para a VRAM.
pub(crate) fn model_params() -> llama_cpp_2::model::params::LlamaModelParams {
    let params = llama_cpp_2::model::params::LlamaModelParams::default().with_use_mmap(true);
    if gpu_offload_available() {
        params.with_n_gpu_layers(gpu_layers(true))
    } else {
        params.with_n_gpu_layers(gpu_layers(false))
    }
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
    fn gpu_layers_offloads_everything_only_with_a_gpu() {
        assert_eq!(super::gpu_layers(false), 0);
        assert_eq!(super::gpu_layers(true), 999);
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
