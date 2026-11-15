//! Spike descartável do "espaço vetorial do repo" (sessão 5).
//!
//! Compara quatro políticas sobre o MESMO corpus de símbolos:
//! lexical (aproximação do LexicalRanker), denso (LFM2.5-350M),
//! RRF ponderado (denso 3x) e "gate" (denso em linguagem natural,
//! RRF só quando a consulta traz um identificador).
//!
//!   cargo test -p tupi --test space_spike -- --nocapture
//!
//! SHARD/SHARDS dividem o corpus por arquivo para caber no teto de tempo.
//! Ajustes por ambiente: TUPI_SPIKE_MAX, TUPI_SPIKE_BATCH, TUPI_SPIKE_TWINS.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use tupi_infer::LlamaEmbedder;
use tupi_kernel::embed::Embedder;

struct Item {
    relative: String,
    symbol: String,
    qualified: String,
    text: String,
    folded: String,
}

struct Query {
    text: &'static str,
    expected_file: &'static str,
    identifier: bool,
}

const NL_QUERIES: &[Query] = &[
    Query {
        text: "onde a sessão lembra o que já leu",
        expected_file: "crates/tupi-tools/src/support/backtrack.rs",
        identifier: false,
    },
    Query {
        text: "o que decide se um arquivo precisa ser relido",
        expected_file: "crates/tupi-tools/src/support/graph/parse_cache.rs",
        identifier: false,
    },
    Query {
        text: "quem detecta que um trecho já foi entregue",
        expected_file: "crates/tupi-tools/src/support/ledger.rs",
        identifier: false,
    },
    Query {
        text: "onde o modelo local de embeddings é carregado",
        expected_file: "crates/tupi-infer/src/llama.rs",
        identifier: false,
    },
    Query {
        text: "onde o resumo de uma linha por documento é produzido",
        expected_file: "crates/tupi-tools/src/tools/docs/index.rs",
        identifier: false,
    },
    Query {
        text: "quem grava o índice de documentação em disco",
        expected_file: "crates/tupi-tools/src/tools/docs/store.rs",
        identifier: false,
    },
    Query {
        text: "onde resultados lexicais e vetoriais são fundidos",
        expected_file: "crates/tupi-tools/src/tools/docs/search.rs",
        identifier: false,
    },
    Query {
        text: "quem ordena símbolos por centralidade",
        expected_file: "crates/tupi-tools/src/tools/map/mod.rs",
        identifier: false,
    },
    Query {
        text: "como nomes parecidos são resolvidos entre arquivos",
        expected_file: "crates/tupi-tools/src/support/graph/scope.rs",
        identifier: false,
    },
    Query {
        text: "quem monta o grafo de chamadas do projeto",
        expected_file: "crates/tupi-tools/src/support/graph/builder.rs",
        identifier: false,
    },
    Query {
        text: "quem baixa e confere o hash do modelo",
        expected_file: "crates/tupi-infer/src/download.rs",
        identifier: false,
    },
    Query {
        text: "onde o orçamento de contexto é decidido",
        expected_file: "crates/tupi-app/src/app.rs",
        identifier: false,
    },
    Query {
        text: "onde um plano de implementação é dividido em subtarefas",
        expected_file: "crates/tupi-tools/src/tools/sdd/mod.rs",
        identifier: false,
    },
    Query {
        text: "quem transforma um diff em símbolos afetados",
        expected_file: "crates/tupi-tools/src/tools/diff/mod.rs",
        identifier: false,
    },
    Query {
        text: "como um agente retoma o trabalho da sessão anterior",
        expected_file: "crates/tupi-tools/src/tools/state/mod.rs",
        identifier: false,
    },
    Query {
        text: "onde a memória do repositório é aplicada ao prompt",
        expected_file: "crates/tupi-app/src/memory.rs",
        identifier: false,
    },
    Query {
        text: "quem mantém o índice de trechos de documentação",
        expected_file: "crates/tupi-tools/src/tools/docs/index.rs",
        identifier: false,
    },
    Query {
        text: "quem decide quais ferramentas o agente enxerga",
        expected_file: "crates/tupi-kernel/src/tool.rs",
        identifier: false,
    },
    Query {
        text: "como o idioma de um arquivo é reconhecido",
        expected_file: "crates/tupi/src/lang.rs",
        identifier: false,
    },
    Query {
        text: "onde o texto de um arquivo é dividido em trechos de código",
        expected_file: "crates/tupi-tools/src/support/ast.rs",
        identifier: false,
    },
];

const ID_QUERIES: &[Query] = &[
    Query {
        text: "BacktrackGraph::visit_level",
        expected_file: "crates/tupi-tools/src/support/backtrack.rs",
        identifier: true,
    },
    Query {
        text: "GraphCache::shared",
        expected_file: "crates/tupi-tools/src/support/graph/cache.rs",
        identifier: true,
    },
    Query {
        text: "DocsIndex::refresh",
        expected_file: "crates/tupi-tools/src/tools/docs/index.rs",
        identifier: true,
    },
    Query {
        text: "DeliveryLedger::check",
        expected_file: "crates/tupi-tools/src/support/ledger.rs",
        identifier: true,
    },
    Query {
        text: "slice_lines",
        expected_file: "crates/tupi-tools/src/tools/docs/store.rs",
        identifier: true,
    },
    Query {
        text: "ToolRegistry::get",
        expected_file: "crates/tupi-kernel/src/tool.rs",
        identifier: true,
    },
    Query {
        text: "LlamaEmbedder::load",
        expected_file: "crates/tupi-infer/src/llama.rs",
        identifier: true,
    },
    Query {
        text: "parse_context",
        expected_file: "crates/tupi/src/config.rs",
        identifier: true,
    },
];

/// Sharding por arquivo para caber no teto de tempo do runner.
const SHARD: usize = 1;
const SHARDS: usize = 2;

/// Peso do denso no RRF ponderado (o lexical entra com peso 1).
const DENSE_WEIGHT: f32 = 3.0;

/// Stopwords mínimas de PT/EN; o extrator real do Tupi tem uma lista maior.
const STOPWORDS: &[&str] = &[
    "a", "o", "as", "os", "um", "uma", "de", "do", "da", "dos", "das", "em", "no", "na", "nos",
    "nas", "por", "para", "com", "que", "quem", "onde", "como", "se", "e", "é", "ja", "já", "foi",
    "ser", "tem", "the", "of", "to", "in", "is", "and", "or", "for", "on", "at", "by", "with",
    "what", "who", "where", "how", "it", "its",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("raiz do repositório")
}

fn model_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".tupi/models/LiquidAI/LFM2.5-Embedding-350M-GGUF/LFM2.5-Embedding-350M-Q8_0.gguf")
}

fn norm(vector: &[f32]) -> f32 {
    vector.iter().map(|value| value * value).sum::<f32>().sqrt()
}

fn cosine(a: &[f32], b: &[f32], norm_a: f32, norm_b: f32) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    dot / (norm_a * norm_b).max(f32::EPSILON)
}

fn dense_top(vectors: &[Vec<f32>], norms: &[f32], query: &[f32], limit: usize) -> Vec<usize> {
    let query_norm = norm(query);
    let mut scored: Vec<(usize, f32)> = vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| (index, cosine(vector, query, norms[index], query_norm)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
        .into_iter()
        .take(limit)
        .map(|(index, _)| index)
        .collect()
}

fn terms_of(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for word in query.split(|character: char| !character.is_alphanumeric() && character != '_') {
        let term = word.trim().to_lowercase();
        if term.chars().count() < 2 || STOPWORDS.contains(&term.as_str()) {
            continue;
        }
        if seen.insert(term.clone()) {
            terms.push(term);
        }
    }
    terms
}

/// Aproximação do LexicalRanker, restrita ao mesmo corpus do braço denso.
fn lexical_top(items: &[Item], query: &str, limit: usize) -> Vec<usize> {
    let terms = terms_of(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, f32)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut score = 0.0f32;
            for term in &terms {
                let occurrences = item.folded.matches(term.as_str()).count() as f32;
                if occurrences == 0.0 {
                    continue;
                }
                let in_name = item.symbol.to_lowercase().contains(term.as_str())
                    || item.qualified.to_lowercase().contains(term.as_str());
                score += occurrences * if in_name { 3.0 } else { 1.0 };
            }
            (score > 0.0).then_some((index, score))
        })
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
        .into_iter()
        .take(limit)
        .map(|(index, _)| index)
        .collect()
}

fn weighted_rrf(lexical: &[usize], dense: &[usize], dense_weight: f32, limit: usize) -> Vec<usize> {
    let mut scores: HashMap<usize, f32> = HashMap::new();
    for (rank, &index) in lexical.iter().enumerate() {
        *scores.entry(index).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
    }
    for (rank, &index) in dense.iter().enumerate() {
        *scores.entry(index).or_default() += dense_weight / (60.0 + rank as f32 + 1.0);
    }
    let mut ranked: Vec<(usize, f32)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked
        .into_iter()
        .take(limit)
        .map(|(index, _)| index)
        .collect()
}

fn rank_of_file(items: &[Item], indices: &[usize], expected_file: &str) -> Option<usize> {
    indices
        .iter()
        .position(|&index| items[index].relative == expected_file)
        .map(|position| position + 1)
}

fn rank_str(rank: Option<usize>) -> String {
    rank.map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn is_test_item(item: &Item) -> bool {
    item.relative.contains("/tests/")
        || item.relative.ends_with("_test.rs")
        || item.qualified.starts_with("tests::")
        || item.symbol.starts_with("test_")
}

fn truncate(mut text: String, limit: usize) -> String {
    if text.len() <= limit {
        return text;
    }
    let mut cut = limit;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    text
}

fn cap_corpus(mut items: Vec<Item>, max: usize) -> Vec<Item> {
    if max == 0 || items.len() <= max {
        return items;
    }
    let keep: std::collections::HashSet<&str> = NL_QUERIES
        .iter()
        .chain(ID_QUERIES)
        .map(|query| query.expected_file)
        .collect();
    let mut flags = vec![false; items.len()];
    for (index, item) in items.iter().enumerate() {
        if keep.contains(item.relative.as_str()) {
            flags[index] = true;
        }
    }
    let already = flags.iter().filter(|kept| **kept).count();
    let extra = max.saturating_sub(already).max(1);
    let stride = (items.len() / extra).max(1);
    for index in (0..items.len()).step_by(stride) {
        if flags.iter().filter(|kept| **kept).count() >= max {
            break;
        }
        flags[index] = true;
    }
    let mut index = 0;
    items.retain(|_| {
        let kept = flags[index];
        index += 1;
        kept
    });
    items
}

#[derive(Default)]
struct Tally {
    hits: usize,
    evaluated: usize,
    mrr: f32,
}

impl Tally {
    fn record(&mut self, rank: Option<usize>) {
        self.evaluated += 1;
        if let Some(rank) = rank {
            self.mrr += 1.0 / rank as f32;
            if rank <= 5 {
                self.hits += 1;
            }
        }
    }

    fn line(&self, label: &str) -> String {
        let mrr = if self.evaluated == 0 {
            0.0
        } else {
            self.mrr / self.evaluated as f32
        };
        format!(
            "- {label}: **{}/{}** (MRR {mrr:.3})",
            self.hits, self.evaluated
        )
    }
}

#[test]
#[ignore = "spike descartável: cargo test -p tupi --test space_spike -- --ignored --nocapture"]
fn space_spike_recall() {
    let root = repo_root();
    let model = model_path();
    let report_path = root
        .join(".agents/results/spark")
        .join(format!("space-spike-gate-shard{SHARD}.md"));
    if !model.exists() {
        let message = format!("modelo ausente em {}\n", model.display());
        eprintln!("{message}");
        let _ = std::fs::create_dir_all(report_path.parent().unwrap());
        let _ = std::fs::write(&report_path, format!("# Spike space\n\n{message}"));
        return;
    }

    let started = Instant::now();
    let raw = tupi_tools::symbol_corpus(&root).expect("corpus");
    let max = std::env::var("TUPI_SPIKE_MAX")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let raw = cap_corpus(
        raw.into_iter()
            .map(|item| {
                let text = truncate(
                    format!(
                        "{}\n{}\n{}\n{}",
                        item.qualified, item.relative, item.signature, item.code
                    ),
                    1000,
                );
                Item {
                    folded: text.to_lowercase(),
                    text,
                    relative: item.relative,
                    symbol: item.symbol,
                    qualified: item.qualified,
                }
            })
            .collect(),
        max,
    );
    let raw = if SHARDS > 1 {
        let mut files: Vec<String> = raw.iter().map(|item| item.relative.clone()).collect();
        files.sort();
        files.dedup();
        let chunk = files.len().div_ceil(SHARDS).max(1);
        let keep: std::collections::HashSet<String> = files
            .into_iter()
            .enumerate()
            .filter(|(index, _)| index / chunk == SHARD)
            .map(|(_, file)| file)
            .collect();
        raw.into_iter()
            .filter(|item| keep.contains(&item.relative))
            .collect()
    } else {
        raw
    };
    let total_chars: usize = raw.iter().map(|item| item.text.len()).sum();
    let progress_path = root.join(".agents/results/spark/space-spike-progress.txt");
    let _ = std::fs::create_dir_all(report_path.parent().unwrap());
    let _ = std::fs::write(
        &progress_path,
        format!("gate shard={SHARD} corpus={}", raw.len()),
    );

    let embedder = LlamaEmbedder::load(&model).expect("carrega o 350M");
    let batch_size = std::env::var("TUPI_SPIKE_BATCH")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8)
        .max(1);
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(raw.len());
    let embed_started = Instant::now();
    for (batch, chunk) in raw.chunks(batch_size).enumerate() {
        let texts: Vec<&str> = chunk.iter().map(|item| item.text.as_str()).collect();
        match embedder.embed_documents(&texts) {
            Ok(mut batch_vectors) => vectors.append(&mut batch_vectors),
            Err(error) => panic!("embedding falhou no lote {batch}: {error:?}"),
        }
        let _ = std::fs::write(
            &progress_path,
            format!(
                "gate shard={} {}/{} em {:.1}s",
                SHARD,
                vectors.len(),
                raw.len(),
                embed_started.elapsed().as_secs_f32()
            ),
        );
    }
    assert_eq!(vectors.len(), raw.len(), "um vetor por símbolo");
    let norms: Vec<f32> = vectors.iter().map(|vector| norm(vector)).collect();

    let mut nl = [
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
    ];
    let mut id = [
        Tally::default(),
        Tally::default(),
        Tally::default(),
        Tally::default(),
    ];
    let mut tables = String::new();

    for (group, label, queries, tallies) in [
        ("nl", "Linguagem natural", NL_QUERIES, &mut nl),
        ("id", "Com identificador", ID_QUERIES, &mut id),
    ] {
        tables.push_str(&format!("## {label}\n\n"));
        tables.push_str("| # | consulta | arquivo esperado | lexical | denso | RRF 3x | gate |\n");
        tables.push_str("|---|---|---|---|---|---|---|\n");
        for (index, query) in queries.iter().enumerate() {
            if !raw.iter().any(|item| item.relative == query.expected_file) {
                tables.push_str(&format!(
                    "| {} | {} | {} | fora do shard | fora do shard | fora do shard | fora do shard |\n",
                    index + 1,
                    query.text,
                    query.expected_file
                ));
                continue;
            }
            let lexical = lexical_top(&raw, query.text, 10);
            let query_vector = embedder.embed_query(query.text).expect("embed da consulta");
            let dense = dense_top(&vectors, &norms, &query_vector, 10);
            let fused = weighted_rrf(&lexical, &dense, DENSE_WEIGHT, 10);
            let gated = if query.identifier {
                weighted_rrf(&lexical, &dense, DENSE_WEIGHT, 10)
            } else {
                dense.clone()
            };

            let ranks = [
                rank_of_file(&raw, &lexical, query.expected_file),
                rank_of_file(&raw, &dense, query.expected_file),
                rank_of_file(&raw, &fused, query.expected_file),
                rank_of_file(&raw, &gated, query.expected_file),
            ];
            for (tally, rank) in tallies.iter_mut().zip(ranks.iter()) {
                tally.record(*rank);
            }

            tables.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                index + 1,
                query.text,
                query.expected_file,
                rank_str(ranks[0]),
                rank_str(ranks[1]),
                rank_str(ranks[2]),
                rank_str(ranks[3])
            ));
            let _ = group;
        }
        tables.push('\n');
    }

    let mut lines = String::new();
    lines.push_str(&format!(
        "# Spike space — gate denso↔lexical (shard {SHARD}/{SHARDS})\n\n"
    ));
    lines.push_str(&format!(
        "- corpus: **{} símbolos**, {} chars em média\n- modelo: {}\n- embed: {:.1}s\n- RRF: denso peso {DENSE_WEIGHT}\n\n",
        raw.len(),
        total_chars / raw.len().max(1),
        model.display(),
        embed_started.elapsed().as_secs_f32()
    ));
    lines.push_str(&tables);
    lines.push_str("## Recall@5 por grupo\n\n");
    lines.push_str("### Linguagem natural\n\n");
    for (tally, name) in nl.iter().zip(["lexical", "denso", "RRF 3x", "gate"]) {
        lines.push_str(&tally.line(name));
        lines.push('\n');
    }
    lines.push_str("\n### Com identificador\n\n");
    for (tally, name) in id.iter().zip(["lexical", "denso", "RRF 3x", "gate"]) {
        lines.push_str(&tally.line(name));
        lines.push('\n');
    }

    let mut twins = String::new();
    if std::env::var("TUPI_SPIKE_TWINS")
        .map(|value| value != "0")
        .unwrap_or(true)
    {
        let twins_started = Instant::now();
        let mut pairs: Vec<(usize, usize, f32)> = Vec::new();
        for left in 0..raw.len() {
            if is_test_item(&raw[left]) {
                continue;
            }
            for right in (left + 1)..raw.len() {
                if is_test_item(&raw[right]) || raw[left].relative == raw[right].relative {
                    continue;
                }
                let score = cosine(&vectors[left], &vectors[right], norms[left], norms[right]);
                pairs.push((left, right, score));
            }
        }
        pairs.sort_by(|a, b| b.2.total_cmp(&a.2));
        twins.push_str(&format!(
            "\n## Twins semânticos (top 20, {:.1}s)\n\n",
            twins_started.elapsed().as_secs_f32()
        ));
        for (left, right, score) in pairs.into_iter().take(20) {
            twins.push_str(&format!(
                "- **{score:.3}** {} ({}) ↔ {} ({})\n",
                raw[left].qualified, raw[left].relative, raw[right].qualified, raw[right].relative
            ));
        }
    }

    lines.push_str(&twins);
    lines.push_str(&format!(
        "\n_tempo total: {:.1}s_\n",
        started.elapsed().as_secs_f32()
    ));
    std::fs::write(&report_path, &lines).expect("grava o relatório");
    println!("{lines}");
    println!("relatório: {}", report_path.display());
}
