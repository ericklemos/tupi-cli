//! Sonda de backend CUDA do `tupi transcribe`.
//!
//! Só age quando `target/cuda-probe-request` existe (mesmo padrão de
//! `release_probe`): grava o ambiente (driver, toolkit, símbolos do binário),
//! o log do whisper.cpp e o tempo real de uma transcrição em
//! `target/cuda-probe.txt`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command) -> String {
    match command.output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            text
        }
        Err(error) => format!("<falhou: {error}>"),
    }
}

fn first_lines(text: &str, count: usize) -> String {
    text.lines().take(count).collect::<Vec<_>>().join("\n")
}

fn interesting(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("cuda")
                || lower.contains("gpu")
                || lower.contains("ggml")
                || lower.contains("backend")
                || lower.contains("cublas")
                || lower.contains("decode")
                || lower.contains("diariza")
                || lower.contains("whisper")
                || lower.contains("modelo")
        })
        .filter(|line| !line.trim().is_empty())
        .collect()
}

fn candidates() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let home = std::env::var("HOME").unwrap_or_default();
    let mut paths = vec![
        root.join("target/release/tupi"),
        root.join("target/debug/tupi"),
    ];
    if !home.is_empty() {
        let home = PathBuf::from(home);
        paths.push(home.join(".cargo/bin/tupi"));
        paths.push(home.join(".local/bin/tupi"));
        paths.push(home.join(".tupi/bin/tupi"));
    }
    paths
}

fn count_hits(bytes: &[u8], needle: &str) -> usize {
    let needle = needle.as_bytes();
    if needle.is_empty() || bytes.len() < needle.len() {
        return 0;
    }
    bytes
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn transcribe(root: &Path, binary: &Path, audio: &Path, report: &mut String) {
    let started = std::time::Instant::now();
    let output = run(Command::new(binary)
        .arg("transcribe")
        .arg(audio)
        .arg("--out")
        .arg(root.join("target/cuda-probe-out.txt")));
    let seconds = started.elapsed().as_secs_f64();
    let _ = writeln!(
        report,
        "== execução de {} ({seconds:.2}s) ==\n{}",
        audio.display(),
        interesting(&output).join("\n")
    );
}

#[test]
fn cuda_probe() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !root.join("target/cuda-probe-request").exists() {
        return;
    }
    let mut report = String::new();
    let smi = run(&mut Command::new("nvidia-smi"));
    let _ = writeln!(report, "== nvidia-smi ==\n{}", first_lines(&smi, 12));
    let nvcc = run(Command::new("/usr/local/cuda/bin/nvcc").arg("--version"));
    let _ = writeln!(report, "== nvcc ==\n{}", first_lines(&nvcc, 4));

    let Some(binary) = candidates().into_iter().find(|path| path.is_file()) else {
        let _ = writeln!(report, "nenhum binário tupi encontrado");
        let _ = std::fs::write(root.join("target/cuda-probe.txt"), report);
        return;
    };
    let _ = writeln!(report, "== binário {} ==", binary.display());
    if let Ok(bytes) = std::fs::read(&binary) {
        for needle in ["ggml_cuda_init", "ggml_backend_cuda", "no GPU found", "libcudart"] {
            let _ = writeln!(report, "  {needle}: {}", count_hits(&bytes, needle));
        }
    }

    let tone = root.join("target/cuda-probe-tone.wav");
    let _ = run(Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=440:duration=2")
        .args(["-ar", "16000", "-ac", "1", "-y"])
        .arg(&tone));
    if tone.is_file() {
        transcribe(&root, &binary, &tone, &mut report);
    }

    let speech = root.join("target/cuda-probe-jfk.wav");
    let _ = run(Command::new("curl")
        .args(["-sL", "--max-time", "60", "-o"])
        .arg(&speech)
        .arg("https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/samples/jfk.wav"));
    if speech.is_file() && std::fs::metadata(&speech).is_ok_and(|m| m.len() > 10_000) {
        transcribe(&root, &binary, &speech, &mut report);
    } else {
        let _ = writeln!(report, "jfk.wav indisponível (sem rede?)");
    }

    let _ = std::fs::write(root.join("target/cuda-probe.txt"), report);
}
