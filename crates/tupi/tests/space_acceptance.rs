//! Aceite do índice denso com a tool real.
//!
//! Roda só manualmente (o build do índice leva minutos):
//!   cargo test -p tupi --test space_acceptance -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tupi_infer::LlamaEmbedder;
use tupi_kernel::ToolRegistry;
use tupi_kernel::embed::EmbedderSlot;

struct Case {
    query: &'static str,
    file: &'static str,
    identifier: bool,
}

const CASES: &[Case] = &[
    Case {
        query: "onde a sessão lembra o que já leu",
        file: "crates/tupi-tools/src/support/backtrack.rs",
        identifier: false,
    },
    Case {
        query: "o que decide se um arquivo precisa ser relido",
        file: "crates/tupi-tools/src/support/graph/parse_cache.rs",
        identifier: false,
    },
    Case {
        query: "quem detecta que um trecho já foi entregue",
        file: "crates/tupi-tools/src/support/ledger.rs",
        identifier: false,
    },
    Case {
        query: "onde o modelo local de embeddings é carregado",
        file: "crates/tupi-infer/src/llama.rs",
        identifier: false,
    },
    Case {
        query: "onde o resumo de uma linha por documento é produzido",
        file: "crates/tupi-tools/src/tools/docs/index.rs",
        identifier: false,
    },
    Case {
        query: "quem grava o índice de documentação em disco",
        file: "crates/tupi-tools/src/tools/docs/store.rs",
        identifier: false,
    },
    Case {
        query: "onde resultados lexicais e vetoriais são fundidos",
        file: "crates/tupi-tools/src/tools/docs/search.rs",
        identifier: false,
    },
    Case {
        query: "quem ordena símbolos por centralidade",
        file: "crates/tupi-tools/src/tools/map/mod.rs",
        identifier: false,
    },
    Case {
        query: "como nomes parecidos são resolvidos entre arquivos",
        file: "crates/tupi-tools/src/support/graph/scope.rs",
        identifier: false,
    },
    Case {
        query: "quem monta o grafo de chamadas do projeto",
        file: "crates/tupi-tools/src/support/graph/builder.rs",
        identifier: false,
    },
    Case {
        query: "quem baixa e confere o hash do modelo",
        file: "crates/tupi-infer/src/download.rs",
        identifier: false,
    },
    Case {
        query: "onde o orçamento de contexto é decidido",
        file: "crates/tupi-app/src/app.rs",
        identifier: false,
    },
    Case {
        query: "onde um plano de implementação é dividido em subtarefas",
        file: "crates/tupi-tools/src/tools/sdd/mod.rs",
        identifier: false,
    },
    Case {
        query: "quem transforma um diff em símbolos afetados",
        file: "crates/tupi-tools/src/tools/diff/mod.rs",
        identifier: false,
    },
    Case {
        query: "como um agente retoma o trabalho da sessão anterior",
        file: "crates/tupi-tools/src/tools/state/mod.rs",
        identifier: false,
    },
    Case {
        query: "onde a memória do repositório é aplicada ao prompt",
        file: "crates/tupi-app/src/memory.rs",
        identifier: false,
    },
    Case {
        query: "quem mantém o índice de trechos de documentação",
        file: "crates/tupi-tools/src/tools/docs/index.rs",
        identifier: false,
    },
    Case {
        query: "quem decide quais ferramentas o agente enxerga",
        file: "crates/tupi-kernel/src/tool.rs",
        identifier: false,
    },
    Case {
        query: "como o idioma de um arquivo é reconhecido",
        file: "crates/tupi/src/lang.rs",
        identifier: false,
    },
    Case {
        query: "onde o texto de um arquivo é dividido em trechos de código",
        file: "crates/tupi-tools/src/support/ast.rs",
        identifier: false,
    },
    Case {
        query: "BacktrackGraph::visit_level",
        file: "crates/tupi-tools/src/support/backtrack.rs",
        identifier: true,
    },
    Case {
        query: "GraphCache::shared",
        file: "crates/tupi-tools/src/support/graph/cache.rs",
        identifier: true,
    },
    Case {
        query: "DocsIndex::refresh",
        file: "crates/tupi-tools/src/tools/docs/index.rs",
        identifier: true,
    },
    Case {
        query: "DeliveryLedger::check",
        file: "crates/tupi-tools/src/support/ledger.rs",
        identifier: true,
    },
    Case {
        query: "slice_lines",
        file: "crates/tupi-tools/src/tools/docs/store.rs",
        identifier: true,
    },
    Case {
        query: "ToolRegistry::get",
        file: "crates/tupi-kernel/src/tool.rs",
        identifier: true,
    },
    Case {
        query: "LlamaEmbedder::load",
        file: "crates/tupi-infer/src/llama.rs",
        identifier: true,
    },
    Case {
        query: "parse_context",
        file: "crates/tupi/src/config.rs",
        identifier: true,
    },
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn model_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".tupi/models/LiquidAI/LFM2.5-Embedding-350M-GGUF/LFM2.5-Embedding-350M-Q8_0.gguf")
}

fn hits_file(output: &str, file: &str) -> bool {
    let needle = format!("\u{60}{file}:");
    output
        .lines()
        .filter(|line| line.starts_with("#### ") && line.contains("\u{60} ("))
        .take(5)
        .any(|line| line.contains(&needle))
}

#[test]
#[ignore = "aceite real: cargo test -p tupi --test space_acceptance -- --ignored --nocapture"]
fn space_acceptance_recall() {
    let root = repo_root();
    let path = model_path();
    if !path.exists() {
        eprintln!("modelo ausente em {}", path.display());
        return;
    }

    let embedder = LlamaEmbedder::load(&path).expect("carrega o 350M");
    let index = Arc::new(tupi_tools::space::SpaceIndex::new());
    let report = tupi_tools::space::rebuild(&root, &index, &embedder);
    eprintln!(
        "índice: {} símbolos, {} arquivos embedados, {} erros",
        report.symbols, report.embedded_files, report.errors
    );

    let mut registry = ToolRegistry::new();
    let options = tupi_tools::ToolsOptions {
        space: Some(Arc::clone(&index)),
        embedder: Some(Arc::new(EmbedderSlot::new(Arc::new(embedder)))),
        space_worker: false,
        ..tupi_tools::ToolsOptions::default()
    };
    tupi_tools::register_with(root.clone(), &mut registry, options);
    let search = registry.get("search").expect("search");

    let mut nl_hits = 0;
    let mut nl_total = 0;
    let mut id_hits = 0;
    let mut id_total = 0;
    for case in CASES {
        let output = search
            .call(json!({
                "query": case.query,
                "max_results": 5,
                "detail": "Signatures",
            }))
            .expect("search");
        let hit = hits_file(&output, case.file);
        if case.identifier {
            id_total += 1;
            id_hits += usize::from(hit);
        } else {
            nl_total += 1;
            nl_hits += usize::from(hit);
        }
        eprintln!(
            "{} {:<60} {}",
            if hit { "ok  " } else { "miss" },
            case.query,
            case.file
        );
    }

    eprintln!("NL recall@5: {nl_hits}/{nl_total}");
    eprintln!("ID recall@5: {id_hits}/{id_total}");
    assert!(nl_hits >= 15, "NL recall@5 {nl_hits}/{nl_total}");
    assert!(id_hits >= 7, "ID recall@5 {id_hits}/{id_total}");
}
