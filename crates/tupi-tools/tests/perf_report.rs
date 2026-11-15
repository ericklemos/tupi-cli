//! Harness minimo de medicao dos caminhos quentes: `map` (constroi o grafo de
//! codigo) e `search` (busca lexical). Sem rede, sem dependencia nova.
//!
//! ```text
//! cargo test -p tupi-tools --test perf_report -- --nocapture
//! ```
//!
//! O stdout de um teste aprovado fica capturado pelo harness do cargo, por isso
//! o relatorio tambem e gravado em `target/perf-report.txt`.
//!
//! A suite roda em perfil dev, onde os crates do workspace ficam em
//! `opt-level = 0`: os numeros absolutos nao representam o release. Servem para
//! comparar antes/depois de uma mudanca na mesma maquina e no mesmo perfil.

use std::time::{Duration, Instant};

use serde_json::json;
use tupi_kernel::ToolRegistry;
use tupi_tools::{ToolsOptions, register_with};

const ROUNDS: usize = 3;
const SEARCH_QUERIES: [&str; 3] = ["GraphBuilder", "tree-sitter parse", "perfil de build lto"];

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn report(values: &[f64]) -> String {
    let best = values.iter().cloned().fold(f64::MAX, f64::min);
    format!(
        "{:?} (melhor {best:.1})",
        values
            .iter()
            .map(|value| format!("{value:.1}"))
            .collect::<Vec<_>>()
    )
}

fn registry(root: &std::path::Path) -> (ToolRegistry, tempfile::TempDir) {
    let state = tempfile::tempdir().expect("estado temporario");
    let mut registry = ToolRegistry::new();
    register_with(
        root.to_path_buf(),
        &mut registry,
        ToolsOptions::hermetic(state.path().to_path_buf()),
    );
    (registry, state)
}

#[test]
fn perf_report() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do workspace");
    let (registry, _state) = registry(&root);
    let map = registry.get("map").expect("tool map");
    let search = registry.get("search").expect("tool search");

    let mut lines =
        String::from("# relatorio de performance (perfil dev, workspace em opt-level 0)\n");
    lines.push_str(&format!("raiz: {}\n", root.display()));

    // A primeira chamada de `map(depth="dirs")` constroi o grafo do repositorio
    // inteiro; as seguintes batem no GraphCache. As duas latencias importam.
    let mut map_ms = Vec::with_capacity(ROUNDS);
    let mut map_bytes = 0;
    for _ in 0..ROUNDS {
        let start = Instant::now();
        let output = map.call(json!({"depth": "dirs"})).expect("map(depth=dirs)");
        map_ms.push(ms(start.elapsed()));
        map_bytes = output.len();
    }
    lines.push_str(&format!(
        "map(dirs) ms: {} — saida {map_bytes} bytes (1a = build do grafo, demais = cache)\n",
        report(&map_ms)
    ));

    for query in SEARCH_QUERIES {
        let mut search_ms = Vec::with_capacity(ROUNDS);
        let mut search_bytes = 0;
        for _ in 0..ROUNDS {
            let start = Instant::now();
            let output = search
                .call(json!({"query": query, "repeat": "allow"}))
                .expect("search");
            search_ms.push(ms(start.elapsed()));
            search_bytes = output.len();
        }
        lines.push_str(&format!(
            "busca {query:?} ms: {} — saida {search_bytes} bytes\n",
            report(&search_ms)
        ));
    }

    if let Err(error) = std::fs::write(root.join("target/perf-report.txt"), &lines) {
        eprintln!("nao foi possivel gravar target/perf-report.txt: {error}");
    }
    println!("{lines}");
}
