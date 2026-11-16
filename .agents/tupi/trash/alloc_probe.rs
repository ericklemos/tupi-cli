//! Sonda A/B do alocador global (T3.2): roda `alloc_bench` duas vezes, com e
//! sem a feature `mimalloc`, e acumula o resultado em `target/alloc-probe.txt`.
//!
//! Só roda quando `target/alloc-probe-request` existe, para não disparar builds
//! aninhados na suíte normal:
//!
//! ```text
//! printf bench > target/alloc-probe-request
//! cargo test -p tupi --test alloc_probe -- --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do workspace")
}

/// Roda `alloc_bench` com ou sem a feature e devolve (sucesso, linhas ALLOC).
fn run_bench(root: &Path, with_mimalloc: bool) -> (bool, Vec<String>) {
    let mut command = Command::new("cargo");
    command
        .arg("test")
        .arg("-p")
        .arg("tupi")
        .arg("--test")
        .arg("alloc_bench");
    if with_mimalloc {
        command.arg("--features").arg("mimalloc");
    }
    command.arg("--").arg("--nocapture");

    let output = command.current_dir(root).output().expect("cargo");
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter(|line| line.starts_with("ALLOC "))
        .map(|line| line.to_string())
        .collect();
    (output.status.success(), lines)
}

#[test]
fn alloc_probe() {
    let root = repo_root();
    if std::fs::read_to_string(root.join("target/alloc-probe-request")).is_err() {
        eprintln!("sem target/alloc-probe-request: sonda ignorada");
        return;
    }

    let (sistema_ok, sistema) = run_bench(&root, false);
    let (mimalloc_ok, mimalloc) = run_bench(&root, true);

    let mut report = String::from("# sonda do alocador global (T3.2)\n");
    for line in sistema.iter().chain(mimalloc.iter()) {
        report.push_str(line);
        report.push('\n');
    }
    let _ = std::fs::write(root.join("target/alloc-probe.txt"), &report);
    eprintln!("{report}");

    assert!(
        sistema_ok && mimalloc_ok,
        "sonda do alocador falhou; ver target/alloc-probe.txt"
    );
    assert!(
        !sistema.is_empty() && !mimalloc.is_empty(),
        "nenhuma linha ALLOC capturada; ver target/alloc-probe.txt"
    );
}
