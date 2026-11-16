//! Catálogo dos modelos locais usados pela transcrição.

/// Um arquivo de modelo publicado no Hugging Face, com tamanho e checksum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelSpec {
    pub repo: &'static str,
    pub file: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

/// Modelo de transcrição usado em CPU.
pub const DEFAULT_WHISPER: &str = "small";

/// Modelo de transcrição usado quando há GPU/CUDA: large-v3-turbo quantizado.
pub const GPU_WHISPER: &str = "turbo";

const WHISPER_REPO: &str = "ggerganov/whisper.cpp";

/// Modelo padrão conforme a build e o hardware.
///
/// Com CUDA compilado e GPU presente, o turbo é o melhor custo/benefício;
/// sem GPU (ou numa build sem CUDA) ele seria lento, então fica o small.
pub fn default_model(cuda_built: bool, gpu_available: bool) -> &'static str {
    if cuda_built && gpu_available {
        GPU_WHISPER
    } else {
        DEFAULT_WHISPER
    }
}

/// Resolve um apelido de modelo de transcrição para o arquivo ggml do Whisper.
pub fn whisper_spec(name: &str) -> Option<ModelSpec> {
    let (file, sha256, bytes) = match name {
        "base" => (
            "ggml-base-q5_1.bin",
            "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
            59_707_625,
        ),
        "small" => (
            "ggml-small-q5_1.bin",
            "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
            190_085_487,
        ),
        "medium" => (
            "ggml-medium-q5_0.bin",
            "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
            539_212_467,
        ),
        "turbo" => (
            "ggml-large-v3-turbo-q5_0.bin",
            "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
            574_041_195,
        ),
        _ => return None,
    };

    Some(ModelSpec {
        repo: WHISPER_REPO,
        file,
        sha256,
        bytes,
    })
}

/// Segmentação de fala da diarização (pyannote, quantizada).
pub fn segmentation_spec() -> ModelSpec {
    ModelSpec {
        repo: "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0",
        file: "model.int8.onnx",
        sha256: "d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d",
        bytes: 1_540_506,
    }
}

/// Embedding de locutor da diarização (3D-Speaker ERes2Net).
pub fn embedding_spec() -> ModelSpec {
    ModelSpec {
        repo: "onemira/eres2net-base-3dspeaker-onnx",
        file: "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx",
        sha256: "1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b",
        bytes: 39_593_761,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_whisper_names_map_to_the_hf_files() {
        let small = whisper_spec("small").expect("small");

        assert_eq!(small.repo, "ggerganov/whisper.cpp");
        assert_eq!(small.file, "ggml-small-q5_1.bin");
        assert_eq!(small.bytes, 190_085_487);
    }

    #[test]
    fn every_spec_carries_a_full_sha256() {
        let specs = [
            whisper_spec("base").unwrap(),
            whisper_spec("small").unwrap(),
            whisper_spec("medium").unwrap(),
            whisper_spec("turbo").unwrap(),
            segmentation_spec(),
            embedding_spec(),
        ];

        for spec in specs {
            assert_eq!(spec.sha256.len(), 64, "{}", spec.file);
            assert!(
                spec.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}",
                spec.file
            );
        }
    }

    #[test]
    fn an_unknown_name_is_rejected() {
        assert!(whisper_spec("huge").is_none());
    }

    #[test]
    fn the_cpu_default_is_small() {
        assert_eq!(DEFAULT_WHISPER, "small");
    }

    #[test]
    fn with_cuda_and_a_gpu_the_default_is_turbo() {
        assert_eq!(default_model(true, true), "turbo");
    }

    #[test]
    fn without_a_gpu_the_default_is_small() {
        assert_eq!(default_model(true, false), "small");
    }

    #[test]
    fn without_cuda_the_default_is_small_even_with_a_gpu() {
        assert_eq!(default_model(false, true), "small");
    }
}
