use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use crate::{
    EMBEDDING_BYTES, EMBEDDING_FILE, EMBEDDING_REPO, EMBEDDING_SHA256, InferError, LFM3_BYTES,
    LFM3_FILE, LFM3_REPO, LFM3_SHA256, models_dir,
};

#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn download(&self, url: &str, dest: &Path) -> Result<u64, InferError>;
}

pub struct HttpFetcher;

#[async_trait]
impl Fetcher for HttpFetcher {
    async fn download(&self, url: &str, dest: &Path) -> Result<u64, InferError> {
        let mut response = reqwest::get(url).await?.error_for_status()?;
        let mut file = tokio::fs::File::create(dest).await?;
        let mut size = 0u64;
        while let Some(chunk) = response.chunk().await? {
            file.write_all(&chunk).await?;
            size += chunk.len() as u64;
        }
        file.flush().await?;
        Ok(size)
    }
}

pub async fn ensure_embedding_model(
    base: &Path,
    fetcher: &dyn Fetcher,
) -> Result<PathBuf, InferError> {
    ensure_model(
        base,
        EMBEDDING_REPO,
        EMBEDDING_FILE,
        EMBEDDING_SHA256,
        EMBEDDING_BYTES,
        fetcher,
    )
    .await
}

pub async fn ensure_generation_model(
    base: &Path,
    fetcher: &dyn Fetcher,
) -> Result<PathBuf, InferError> {
    ensure_model(base, LFM3_REPO, LFM3_FILE, LFM3_SHA256, LFM3_BYTES, fetcher).await
}

pub async fn ensure_model(
    base: &Path,
    repo: &str,
    file: &str,
    sha256: &str,
    bytes: u64,
    fetcher: &dyn Fetcher,
) -> Result<PathBuf, InferError> {
    let dir = models_dir(base, repo);
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(file);
    if target.exists() && verify(&target, sha256)? {
        return Ok(target);
    }
    let _ = std::fs::remove_file(&target);
    let tmp = dir.join(format!("{file}.part"));
    let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
    let size = fetcher.download(&url, &tmp).await?;
    if size != bytes {
        let _ = std::fs::remove_file(&tmp);
        return Err(InferError::Size {
            expected: bytes,
            actual: size,
        });
    }
    if !verify(&tmp, sha256)? {
        let _ = std::fs::remove_file(&tmp);
        return Err(InferError::Checksum);
    }
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

fn verify(path: &Path, expected: &str) -> Result<bool, InferError> {
    use sha2::{Digest, Sha256};

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct StubFetcher {
        bytes: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl Fetcher for StubFetcher {
        async fn download(&self, _url: &str, dest: &Path) -> Result<u64, InferError> {
            std::fs::write(dest, &self.bytes)?;
            Ok(self.bytes.len() as u64)
        }
    }

    #[tokio::test]
    async fn a_bad_download_is_refused_and_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let fetcher = StubFetcher {
            bytes: b"errado".to_vec(),
        };
        let result = ensure_embedding_model(dir.path(), &fetcher).await;
        assert!(result.is_err());
        assert!(
            !models_dir(dir.path(), "LiquidAI/LFM2.5-Embedding-350M-GGUF")
                .join("LFM2.5-Embedding-350M-Q8_0.gguf")
                .exists()
        );
    }

    #[tokio::test]
    async fn a_bad_generation_download_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let fetcher = StubFetcher {
            bytes: b"errado".to_vec(),
        };
        let result = ensure_generation_model(dir.path(), &fetcher).await;
        assert!(result.is_err());
        assert!(
            !models_dir(dir.path(), "LiquidAI/LFM3-1.7B-GGUF")
                .join("LFM3-1.7B-Q4_K_M.gguf")
                .exists()
        );
    }
}
