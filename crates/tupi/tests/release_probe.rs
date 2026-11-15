//! Sonda manual dos perfis de build (release / profiling).
//!
//! Nao roda na suite normal: so age quando `target/perf-probe-request` existe, e
//! o conteudo desse arquivo escolhe o perfil (`release` ou `profiling`). E o
//! unico jeito de medir build release dentro do harness do agente, que executa
//! apenas `cargo build/check/test/clippy` no perfil dev.
//!
//! ```text
//! printf release > target/perf-probe-request
//! cargo test -p tupi --test release_probe -- --nocapture
//! ```
//!
//! O relatorio sai em `target/build-probe.txt`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do workspace")
}

fn size(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => format!("{} bytes", metadata.len()),
        Err(error) => format!("indisponivel: {error}"),
    }
}

fn build(root: &Path, args: &[&str]) -> (bool, f64) {
    let start = Instant::now();
    let output = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .output()
        .expect("cargo");
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    (output.status.success(), start.elapsed().as_secs_f64())
}

#[test]
fn build_profile_probe() {
    let root = repo_root();
    let request = root.join("target/perf-probe-request");
    let Ok(profile) = std::fs::read_to_string(&request) else {
        eprintln!("sem target/perf-probe-request: sonda ignorada");
        return;
    };
    let profile = profile.trim();

    let mut report = String::from("# sonda de perfis de build (tupi sem features de GPU)\n");
    let (ok, seconds, dir, extra) = if profile == "profiling" {
        let (ok, seconds) = build(
            &root,
            &["build", "--profile", "profiling", "-p", "tupi", "--no-default-features"],
        );
        (ok, seconds, "profiling", vec!["--profile".to_string(), "profiling".to_string()])
    } else {
        let (ok, seconds) = build(
            &root,
            &["build", "--release", "-p", "tupi", "--no-default-features"],
        );
        (ok, seconds, "release", vec!["--release".to_string()])
    };
    let _ = extra;

    report.push_str(&format!("perfil: {profile} · ok={ok} · {seconds:.1}s\n"));
    let binary = root.join(format!("target/{dir}/tupi"));
    report.push_str(&format!("tamanho {binary:?}: {}\n", size(&binary)));
    if ok {
        match Command::new(&binary).arg("--help").output() {
            Ok(output) => report.push_str(&format!(
                "--help: status={} · {} bytes de stdout\n",
                output.status,
                output.stdout.len()
            )),
            Err(error) => report.push_str(&format!("--help falhou: {error}\n")),
        }
    }

    if let Err(error) = std::fs::write(root.join("target/build-probe.txt"), &report) {
        eprintln!("nao foi possivel gravar target/build-probe.txt: {error}");
    }
    eprintln!("{report}");
    assert!(ok, "sonda de perfis falhou; ver target/build-probe.txt");
}
