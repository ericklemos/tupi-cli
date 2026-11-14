use std::path::{Path, PathBuf};

pub fn models_dir(base: &Path, repo: &str) -> PathBuf {
    base.join("models").join(repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_cache_path_is_stable() {
        let base = Path::new("/tmp/x");
        assert_eq!(
            models_dir(base, "LiquidAI/LFM2.5-Embedding-350M-GGUF"),
            base.join("models").join("LiquidAI/LFM2.5-Embedding-350M-GGUF")
        );
    }
}
