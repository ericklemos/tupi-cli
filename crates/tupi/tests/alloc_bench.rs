//! Harness de latência e pico de RSS dos caminhos quentes (T2/T3.2/T6.4).
//!
//! Mede o build do grafo, o `map` com cache quente, três buscas lexicais e o
//! `VmHWM` (pico de RSS) do processo. É a base numérica para decidir qualquer
//! mudança de alocador ou de estrutura do grafo:
//!
//! ```text
//! cargo test -p tupi --test alloc_bench -- --nocapture
//! ```
//!
//! Não usa rede. Grava o resultado em `target/alloc-bench.txt` e imprime as
//! linhas `ALLOC` no stdout. Para comparar duas configurações (ex.: um
//! `#[global_allocator]` alternativo declarado neste arquivo atrás de uma
//! feature), rode uma variante por vez e compare as linhas.

use std::time::Instant;

use serde_json::json;

const WARM_ROUNDS: usize = 3;
const SEARCH_QUERIES: [&str; 3] = ["GraphBuilder", "tree-sitter parse", "perfil de build lto"];

/// Pico de RSS do processo em kB (`VmHWM` do `/proc/self/status`).
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmHWM:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
fn alloc_bench() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do workspace");
    let state = tempfile::tempdir().expect("estado temporario");
    let mut registry = tupi_kernel::ToolRegistry::new();
    tupi_tools::register_with(
        root.to_path_buf(),
        &mut registry,
        tupi_tools::ToolsOptions::hermetic(state.path().to_path_buf()),
    );

    let map = registry.get("map").expect("tool map");
    let search = registry.get("search").expect("tool search");
    let mut lines: Vec<String> = Vec::new();

    // Primeira chamada de `map(depth="dirs")`: constrói o grafo do repositório.
    let start = Instant::now();
    let output = map.call(json!({"depth": "dirs"})).expect("map(depth=dirs)");
    let graph_ms = ms(start);
    lines.push(format!(
        "ALLOC grafo_ms={graph_ms:.1} saida_bytes={}",
        output.len()
    ));
    lines.push(format!(
        "ALLOC pico_rss_kb_apos_grafo={}",
        peak_rss_kb().unwrap_or(0)
    ));

    // Cache quente: é o caminho que a TUI bate a cada chamada de tool.
    let mut warm_best = f64::MAX;
    for _ in 0..WARM_ROUNDS {
        let start = Instant::now();
        let _ = map.call(json!({"depth": "dirs"})).expect("map quente");
        warm_best = warm_best.min(ms(start));
    }
    lines.push(format!("ALLOC map_quente_ms={warm_best:.2}"));

    for query in SEARCH_QUERIES {
        let mut best = f64::MAX;
        for _ in 0..WARM_ROUNDS {
            let start = Instant::now();
            let _ = search
                .call(json!({"query": query, "repeat": "allow"}))
                .expect("search");
            best = best.min(ms(start));
        }
        lines.push(format!("ALLOC busca_ms={best:.2} query={query:?}"));
    }

    lines.push(format!(
        "ALLOC pico_rss_kb_final={}",
        peak_rss_kb().unwrap_or(0)
    ));

    let report = format!(
        "# latencia e pico de RSS dos caminhos quentes (perfil dev)\nraiz: {}\n{}\n",
        root.display(),
        lines.join("\n")
    );
    if let Err(error) = std::fs::write(root.join("target/alloc-bench.txt"), &report) {
        eprintln!("nao foi possivel gravar target/alloc-bench.txt: {error}");
    }
    println!("{report}");
}
