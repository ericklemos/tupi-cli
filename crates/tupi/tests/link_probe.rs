//! Sonda de tempo de link (T4.1): linker padrão vs. mold opt-in.
//!
//! Só roda quando `target/link-probe-request` existe, para não pesar na suíte
//! normal. Uso:
//!
//! ```text
//! printf plain > target/link-probe-request   # linker padrão
//! printf mold  > target/link-probe-request   # --config scripts/perf/mold.toml
//! cargo test -p tupi --test link_probe -- --nocapture
//! ```
//!
//! O resultado é acumulado em `target/link-probe.txt`, uma linha por execução:
//! tempo do build completo e tempo do relink isolado (binário apagado, deps em
//! cache), que é onde a escolha do linker aparece.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do workspace")
}

/// Um `cargo build --release -p tupi` headless (sem llama.cpp) com ou sem mold.
/// Devolve (sucesso, segundos).
fn build(root: &Path, mold: bool) -> (bool, f64) {
    let mut command = Command::new("cargo");
    command
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg("tupi")
        .arg("--no-default-features");
    if mold {
        command.arg("--config").arg("scripts/perf/mold.toml");
    }
    let start = Instant::now();
    let output = command.current_dir(root).output().expect("cargo");
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    (output.status.success(), start.elapsed().as_secs_f64())
}

#[test]
fn link_probe() {
    let root = repo_root();
    let Ok(request) = std::fs::read_to_string(root.join("target/link-probe-request")) else {
        eprintln!("sem target/link-probe-request: sonda ignorada");
        return;
    };
    let mode = request.trim().to_string();
    let mold = match mode.as_str() {
        "mold" => true,
        "plain" => false,
        other => {
            eprintln!("modo desconhecido {other:?}: use `plain` ou `mold`");
            return;
        }
    };

    let binary = root.join("target/release/tupi");

    // 1) build completo. Com mold, o `rustflags` muda e tudo é recompilado;
    //    sem mold, num target/ quente o número é ~0 s.
    let (full_ok, full) = build(&root, mold);

    // 2) rebuild isolado: apaga o binário da crate `tupi` em `target/release/deps`
    //    (o `target/release/tupi` da raiz é só um hardlink, apagá-lo custa 0,1 s).
    //    Sobra a compilação da crate `tupi` + LTO + link, onde a escolha do linker
    //    aparece.
    let removed = {
        let mut removed = false;
        if let Ok(entries) = std::fs::read_dir(root.join("target/release/deps")) {
            for entry in entries.flatten() {
                let path = entry.path();
                // `deps/tupi-<hash>` é o binário; `.d`, `.rlib` e afins têm ponto.
                let is_bin = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("tupi-") && !name.contains('.'));
                if is_bin && std::fs::remove_file(&path).is_ok() {
                    removed = true;
                }
            }
        }
        removed
    };
    let (relink_ok, relink) = build(&root, mold);

    let size = std::fs::metadata(&binary).map(|m| m.len()).unwrap_or(0);
    let line = format!(
        "modo={mode} (mold={mold}) · build completo {full:.2}s (ok={full_ok}) · rebuild do binario {relink:.2}s (ok={relink_ok}, artefato apagado={removed}) · target/release/tupi {size} bytes\n"
    );
    let mut report = std::fs::read_to_string(root.join("target/link-probe.txt")).unwrap_or_default();
    report.push_str(&line);
    let _ = std::fs::write(root.join("target/link-probe.txt"), &report);
    eprintln!("{line}");

    assert!(
        full_ok && relink_ok,
        "sonda de link falhou; ver target/link-probe.txt"
    );
}


