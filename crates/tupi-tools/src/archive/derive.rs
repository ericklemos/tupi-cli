//! Extração de texto de mídia, com cache endereçado por conteúdo.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use super::error::ArchiveError;

/// Trecho de texto extraído de mídia, com tempo (ou página) de origem.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Converte áudio/vídeo em texto com timestamps.
pub trait Transcriber: Send + Sync {
    fn transcribe(&self, path: &Path) -> Result<Vec<Cue>, ArchiveError>;
}

/// Converte imagem em texto com coordenadas.
pub trait Ocr: Send + Sync {
    fn read(&self, path: &Path) -> Result<Vec<Cue>, ArchiveError>;
}

/// Cache em disco dos derivados, endereçado pelo hash do conteúdo.
pub struct DerivedCache {
    dir: PathBuf,
}

impl DerivedCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn get_or_compute(
        &self,
        hash: &str,
        compute: impl FnOnce() -> Result<Vec<Cue>, ArchiveError>,
    ) -> Result<Vec<Cue>, ArchiveError> {
        let path = self.dir.join(format!("{hash}.jsonl"));
        if let Ok(text) = std::fs::read_to_string(&path) {
            let mut cues = Vec::new();
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                cues.push(
                    serde_json::from_str(line)
                        .map_err(|error| ArchiveError::Db(error.to_string()))?,
                );
            }
            return Ok(cues);
        }
        let cues = compute()?;
        std::fs::create_dir_all(&self.dir)?;
        let mut body = String::new();
        for cue in &cues {
            body.push_str(
                &serde_json::to_string(cue).map_err(|error| ArchiveError::Db(error.to_string()))?,
            );
            body.push('\n');
        }
        std::fs::write(&path, body)?;
        Ok(cues)
    }
}

/// OCR por `tesseract` (opcional, resolvido em `PATH`).
pub struct CommandOcr {
    bin: String,
}

impl CommandOcr {
    pub fn new(bin: &str) -> Self {
        Self { bin: bin.to_string() }
    }
}

impl Ocr for CommandOcr {
    fn read(&self, path: &Path) -> Result<Vec<Cue>, ArchiveError> {
        let output = Command::new(&self.bin)
            .arg(path)
            .arg("stdout")
            .output()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => ArchiveError::MissingTool(self.bin.clone()),
                _ => ArchiveError::Io(error),
            })?;
        if !output.status.success() {
            return Err(ArchiveError::Contract(format!(
                "{} saiu com {}",
                self.bin, output.status
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| Cue {
                start: 0.0,
                end: 0.0,
                text: line.trim().to_string(),
            })
            .collect())
    }
}

/// Transcrição por `whisper-cli` (opcional, resolvido em `PATH`).
///
/// O binário oficial do projeto (`tupi-transcribe`) é injetado pelo crate
/// `tupi` como `Transcriber`; este é o caminho de fallback.
pub struct CommandTranscriber {
    bin: String,
}

impl CommandTranscriber {
    pub fn new(bin: &str) -> Self {
        Self { bin: bin.to_string() }
    }
}

impl Transcriber for CommandTranscriber {
    fn transcribe(&self, path: &Path) -> Result<Vec<Cue>, ArchiveError> {
        let output = Command::new(&self.bin)
            .arg("-f")
            .arg(path)
            .arg("-oj")
            .output()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => ArchiveError::MissingTool(self.bin.clone()),
                _ => ArchiveError::Io(error),
            })?;
        if !output.status.success() {
            return Err(ArchiveError::Contract(format!(
                "{} saiu com {}",
                self.bin, output.status
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| Cue {
                start: 0.0,
                end: 0.0,
                text: line.trim().to_string(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_transcript_is_not_recomputed() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DerivedCache::new(dir.path().to_path_buf());
        let mut calls = 0;
        let compute = |calls: &mut i32| {
            *calls += 1;
            Ok(vec![Cue { start: 0.0, end: 1.0, text: "oi".into() }])
        };
        let first = cache.get_or_compute("abc", || compute(&mut calls)).unwrap();
        let second = cache.get_or_compute("abc", || compute(&mut calls)).unwrap();
        assert_eq!(calls, 1);
        assert_eq!(first, second);
    }

    #[test]
    fn a_missing_binary_is_reported_as_missing_tool() {
        let transcriber = CommandTranscriber::new("binario-que-nao-existe-xyz");
        assert!(matches!(
            transcriber.transcribe(Path::new("x.mp4")),
            Err(ArchiveError::MissingTool(_))
        ));
        let ocr = CommandOcr::new("binario-que-nao-existe-xyz");
        assert!(matches!(
            ocr.read(Path::new("x.png")),
            Err(ArchiveError::MissingTool(_))
        ));
    }
}
