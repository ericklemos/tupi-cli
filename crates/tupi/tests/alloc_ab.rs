//! A/B do alocador global: alocador do sistema × `--features mimalloc`.
//!
//! Complementa `alloc_bench.rs`, que mede **só o perfil dev** e **nunca linka o
//! alocador**: `crates/tupi` não tem alvo de biblioteca, então a `static` de
//! `src/main.rs` não entra num teste de integração. Aqui a `#[global_allocator]`
//! vive no próprio binário de teste, e `--features mimalloc` troca mesmo o
//! alocador de quem faz a medição.
//!
//! Duas rodadas, sempre `--release` e `--no-default-features` (sem `local-models`,
//! para não compilar o llama.cpp):
//!
//! ```text
//! printf ab > target/alloc-ab-request
//! cargo test -p tupi --release --no-default-features --test alloc_ab -- --nocapture
//! printf ab > target/alloc-ab-request
//! cargo test -p tupi --release --no-default-features --features mimalloc \
//!     --test alloc_ab -- --nocapture
//! ```
//!
//! Cada rodada grava `target/alloc-ab-{sistema,mimalloc}.txt`. O arquivo
//! `target/alloc-ab-request` é o portão (mesma ideia de `link_probe.rs`): sem
//! ele o teste sai na hora e não pesa na suíte normal.

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tupi_kernel::{Tool, ToolRegistry};

const ALLOCATOR: &str = if cfg!(feature = "mimalloc") {
    "mimalloc"
} else {
    "sistema"
};
const PROFILE: &str = if cfg!(debug_assertions) {
    "dev"
} else {
    "release"
};

const WARM_ROUNDS: usize = 5;
const THREADS: usize = 8;
const CALLS_PER_THREAD: usize = 12;
const LONG_ROUNDS: usize = 10;
const SEARCH_QUERIES: [&str; 3] = ["GraphBuilder", "tree-sitter parse", "perfil de build lto"];

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn status_kb(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find(|line| line.starts_with(field))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
}

fn rss_kb() -> Option<u64> {
    status_kb("VmRSS:")
}

fn peak_rss_kb() -> Option<u64> {
    status_kb("VmHWM:")
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn registry(root: &std::path::Path, state: &std::path::Path) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    tupi_tools::register_with(
        root.to_path_buf(),
        &mut registry,
        tupi_tools::ToolsOptions::hermetic(state.to_path_buf()),
    );
    registry
}

#[test]
fn alloc_ab() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do workspace");
    if !root.join("target/alloc-ab-request").exists() {
        println!(
            "ALLOCAB ignorado: crie target/alloc-ab-request (ver docs/perf/allocator-and-linker.md)"
        );
        return;
    }

    let state = tempfile::tempdir().expect("estado temporario");
    let registry = registry(&root, state.path());
    let map: Arc<dyn Tool> = registry.get("map").expect("tool map");
    let search: Arc<dyn Tool> = registry.get("search").expect("tool search");

    let mut lines = vec![format!("ALLOCAB alocador={ALLOCATOR} perfil={PROFILE}")];

    // 1. Grafo frio: o custo que a primeira tool call paga.
    let start = Instant::now();
    let output = map.call(json!({"depth": "dirs"})).expect("map(depth=dirs)");
    lines.push(format!(
        "ALLOCAB grafo_ms={:.1} bytes_saida={}",
        ms(start),
        output.len()
    ));
    lines.push(format!(
        "ALLOCAB pico_rss_kb_apos_grafo={}",
        peak_rss_kb().unwrap_or(0)
    ));

    // 2. Caminho quente, single-thread.
    let mut warm = f64::MAX;
    for _ in 0..WARM_ROUNDS {
        let start = Instant::now();
        let _ = map.call(json!({"depth": "dirs"})).expect("map quente");
        warm = warm.min(ms(start));
    }
    lines.push(format!("ALLOCAB map_quente_ms={warm:.2}"));

    for query in SEARCH_QUERIES {
        let mut best = f64::MAX;
        for _ in 0..WARM_ROUNDS {
            let start = Instant::now();
            let _ = search
                .call(json!({"query": query, "repeat": "allow"}))
                .expect("search quente");
            best = best.min(ms(start));
        }
        lines.push(format!("ALLOCAB busca_ms={best:.2} query={query:?}"));
    }

    // 3. Concorrência: várias threads batendo na mesma tool, que é o regime real
    //    de TUI + servidor web. Latência por chamada, não só o total.
    let start = Instant::now();
    let mut latencies: Vec<f64> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..THREADS)
            .map(|thread| {
                let search = Arc::clone(&search);
                scope.spawn(move || {
                    let mut latencies = Vec::with_capacity(CALLS_PER_THREAD);
                    for call in 0..CALLS_PER_THREAD {
                        let query = SEARCH_QUERIES[(thread + call) % SEARCH_QUERIES.len()];
                        let start = Instant::now();
                        let _ = search
                            .call(json!({"query": query, "repeat": "allow"}))
                            .expect("search concorrente");
                        latencies.push(ms(start));
                    }
                    latencies
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("thread"))
            .collect()
    });
    let concurrent_ms = ms(start);
    latencies.sort_by(|a, b| a.partial_cmp(b).expect("latencia comparavel"));
    lines.push(format!(
        "ALLOCAB concorrente_ms={concurrent_ms:.1} threads={THREADS} chamadas={} p50_ms={:.2} p95_ms={:.2} max_ms={:.2}",
        latencies.len(),
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.95),
        latencies.last().copied().unwrap_or(0.0),
    ));
    lines.push(format!(
        "ALLOCAB pico_rss_kb_apos_concorrencia={}",
        peak_rss_kb().unwrap_or(0)
    ));

    // 4. Regime longo: RSS depois de cada rodada. É o proxy de fragmentação que
    //    uma rodada única não enxerga.
    let mut rss = Vec::with_capacity(LONG_ROUNDS);
    let start = Instant::now();
    for _ in 0..LONG_ROUNDS {
        let _ = map.call(json!({"depth": "dirs"})).expect("map longo");
        for query in SEARCH_QUERIES {
            let _ = search
                .call(json!({"query": query, "repeat": "allow"}))
                .expect("search longo");
        }
        rss.push(rss_kb().unwrap_or(0));
    }
    lines.push(format!(
        "ALLOCAB longo_ms={:.1} rodadas={LONG_ROUNDS} rss_primeira_kb={} rss_ultima_kb={} rss_max_kb={}",
        ms(start),
        rss.first().copied().unwrap_or(0),
        rss.last().copied().unwrap_or(0),
        rss.iter().copied().max().unwrap_or(0),
    ));
    lines.push(format!(
        "ALLOCAB pico_rss_kb_final={}",
        peak_rss_kb().unwrap_or(0)
    ));

    let report = format!(
        "# A/B do alocador global ({ALLOCATOR})\nalocador: {ALLOCATOR}\n{}\n",
        lines.join("\n")
    );
    println!("{report}");
    let path = root.join(format!("target/alloc-ab-{ALLOCATOR}.txt"));
    if let Err(error) = std::fs::write(&path, &report) {
        eprintln!("nao foi possivel gravar {}: {error}", path.display());
    }
}
