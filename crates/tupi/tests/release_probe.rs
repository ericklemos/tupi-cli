//! Sonda manual dos perfis de build (release / profiling).
//!
//! Nao roda na suite normal: so age quando `target/perf-probe-request` existe.
//! O conteudo do arquivo escolhe o perfil e o pacote no formato `<perfil>[:<pacote>]`,
//! por exemplo `release` (binario `tupi` sem features de GPU) ou
//! `release:tupi-tools` (crate pure-Rust, bem mais rapido de compilar).
//!
//! E o unico jeito de medir build release dentro do harness do agente, que
//! executa apenas `cargo build/check/test/clippy` no perfil dev.
//!
//! ```text
//! printf release:tupi-tools > target/perf-probe-request
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
        Err(_) => "ausente".to_string(),
    }
}

#[test]
fn build_profile_probe() {
    let root = repo_root();
    let Ok(request) = std::fs::read_to_string(root.join("target/perf-probe-request")) else {
        eprintln!("sem target/perf-probe-request: sonda ignorada");
        return;
    };
    let request = request.trim().to_string();
    let (profile, package) = match request.split_once(':') {
        Some((profile, package)) => (profile.to_string(), package.to_string()),
        None => (request.clone(), "tupi".to_string()),
    };

    let mut args: Vec<String> = vec!["build".to_string()];
    let out_dir = if profile == "profiling" {
        args.push("--profile".to_string());
        args.push("profiling".to_string());
        "profiling"
    } else {
        args.push("--release".to_string());
        "release"
    };
    args.push("-p".to_string());
    args.push(package.clone());
    if package == "tupi" {
        args.push("--no-default-features".to_string());
    }

    let start = Instant::now();
    let output = Command::new("cargo")
        .args(&args)
        .current_dir(&root)
        .output()
        .expect("cargo");
    let seconds = start.elapsed().as_secs_f64();
    let ok = output.status.success();
    if !ok {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }

    let mut report = format!("# sonda de perfis de build\ncargo {}\n", args.join(" "));
    report.push_str(&format!(
        "perfil: {profile} · pacote: {package} · ok={ok} · {seconds:.1}s\n"
    ));

    let binary = root.join(format!("target/{out_dir}/{package}"));
    let rlib = root.join(format!(
        "target/{out_dir}/lib{}.rlib",
        package.replace('-', "_")
    ));
    report.push_str(&format!("binario {binary:?}: {}\n", size(&binary)));
    report.push_str(&format!("rlib {rlib:?}: {}\n", size(&rlib)));
    if ok && binary.exists() {
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
