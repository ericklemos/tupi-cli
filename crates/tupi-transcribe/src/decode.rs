//! Decodificação de áudio para PCM 16 kHz mono usando o ffmpeg.

use std::path::Path;

/// Falhas na decodificação.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("ffmpeg não encontrado no PATH: {0}")]
    MissingTool(String),
    #[error("ffmpeg falhou ({status}): {stderr}")]
    Ffmpeg { status: String, stderr: String },
    #[error("falha de E/S: {0}")]
    Io(#[from] std::io::Error),
}

/// Decodifica qualquer áudio que o ffmpeg entenda para 16 kHz mono.
pub fn decode_to_pcm(ffmpeg: &Path, audio: &Path) -> Result<Vec<f32>, DecodeError> {
    let output = std::process::Command::new(ffmpeg)
        .arg("-v")
        .arg("error")
        .arg("-nostdin")
        .arg("-i")
        .arg(audio)
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg("-f")
        .arg("f32le")
        .arg("-")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => DecodeError::MissingTool(ffmpeg.display().to_string()),
            _ => DecodeError::Io(error),
        })?;

    if !output.status.success() {
        let stderr: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(400)
            .collect();
        return Err(DecodeError::Ffmpeg {
            status: output.status.to_string(),
            stderr,
        });
    }

    let mut samples = Vec::with_capacity(output.stdout.len() / 4);
    for chunk in output.stdout.chunks_exact(4) {
        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silent_wav(path: &Path) {
        let sample_rate = 8_000u32;
        let channels = 2u16;
        let seconds = 1u32;
        let data_bytes = sample_rate * u32::from(channels) * 2 * seconds;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_bytes.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, data_bytes as usize));
        std::fs::write(path, bytes).expect("escrever wav");
    }

    fn ffmpeg_available() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[test]
    fn decodes_to_sixteen_kilohertz_mono() {
        if !ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let wav = dir.path().join("silencio.wav");
        silent_wav(&wav);

        let samples = decode_to_pcm(Path::new("ffmpeg"), &wav).expect("decodificar");

        assert!(
            (15_500..=16_500).contains(&samples.len()),
            "amostras: {}",
            samples.len()
        );
        assert!(samples.iter().all(|sample| sample.abs() < 0.01));
    }
}
