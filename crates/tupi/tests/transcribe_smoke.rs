//! Smoke de ponta a ponta do `tupi transcribe`: baixa modelos, decodifica,
//! transcreve e grava o `.txt`. Fica `#[ignore]` porque depende de rede e de
//! ~100 MB de download; rode com `cargo test -p tupi --test transcribe_smoke -- --ignored`.

use std::path::Path;
use std::process::Command;

fn tone_wav(path: &Path) {
    let sample_rate = 16_000u32;
    let seconds = 4u32;
    let samples = sample_rate * seconds;
    let data_bytes = samples * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for index in 0..samples {
        let value = ((index as f32 * 440.0 * std::f32::consts::TAU / sample_rate as f32).sin()
            * 8_000.0) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(path, bytes).expect("escrever wav");
}

#[test]
#[ignore = "baixa ~100 MB de modelo e precisa de rede"]
fn transcribes_a_file_end_to_end() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("home");
    let audio = dir.path().join("tom.wav");
    tone_wav(&audio);

    let output = Command::new(env!("CARGO_BIN_EXE_tupi"))
        .arg("transcribe")
        .arg(&audio)
        .arg("--model")
        .arg("base")
        .env("HOME", &home)
        .env("TUPI_API_KEY", "")
        .output()
        .expect("executar tupi");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "codigo {:?}\nstderr: {stderr}",
        output.status.code()
    );
    let transcript = std::fs::read_to_string(dir.path().join("tom.txt")).expect("ler txt");
    eprintln!("stderr: {stderr}\ntamanho do txt: {}", transcript.len());
}
