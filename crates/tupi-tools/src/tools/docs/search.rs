//! Ranking lexical dos chunks de documentação.

use std::collections::HashMap;

use super::store::Index;
use super::terms::{normalize, query_terms};

/// Trecho candidato devolvido pela busca.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub chunk: usize,
    pub score: f32,
}

/// BM25 sobre os termos normalizados dos chunks.
pub fn lexical(index: &Index, query: &str, limit: usize) -> Vec<Hit> {
    let terms = query_terms(query);
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let chunks: Vec<(&str, usize, &super::store::ChunkMeta)> = index
        .files
        .iter()
        .flat_map(|file| {
            file.chunks
                .iter()
                .enumerate()
                .map(move |(chunk, meta)| (file.path.as_str(), chunk, meta))
        })
        .collect();
    let count = chunks.len() as f32;
    if count == 0.0 {
        return Vec::new();
    }
    let average = chunks
        .iter()
        .map(|(_, _, chunk)| chunk.tokens as f32)
        .sum::<f32>()
        / count;
    let mut frequency: HashMap<&str, usize> = HashMap::new();
    for term in &terms {
        let matches = chunks
            .iter()
            .filter(|(_, _, chunk)| chunk.terms.contains_key(term.as_str()))
            .count();
        frequency.insert(term.as_str(), matches);
    }
    let k1 = 1.2f32;
    let b = 0.75f32;
    let mut hits = Vec::new();
    for (path, chunk_index, chunk) in chunks {
        let mut score = 0.0;
        for term in &terms {
            let Some(&tf) = chunk.terms.get(term.as_str()) else {
                continue;
            };
            let df = *frequency.get(term.as_str()).unwrap_or(&0) as f32;
            if df == 0.0 {
                continue;
            }
            let idf = ((count - df + 0.5) / (df + 0.5) + 1.0).ln();
            let tf = tf as f32;
            let length = chunk.tokens as f32;
            let mut part = idf
                * (tf * (k1 + 1.0))
                / (tf + k1 * (1.0 - b + b * length / average.max(1.0)));
            if normalize(&chunk.heading).contains(term) {
                part *= 2.0;
            }
            score += part;
        }
        if score > 0.0 {
            hits.push(Hit {
                path: path.to_string(),
                chunk: chunk_index,
                score,
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.chunk.cmp(&right.chunk))
    });
    hits.truncate(limit);
    hits
}
pub fn dense(
    index: &Index,
    vectors: &HashMap<String, Vec<Vec<f32>>>,
    query: &[f32],
    limit: usize,
) -> Vec<Hit> {
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for file in &index.files {
        let Some(file_vectors) = vectors.get(&file.path) else {
            continue;
        };
        for (chunk_index, vector) in file_vectors.iter().enumerate() {
            if chunk_index >= file.chunks.len() {
                break;
            }
            let score: f32 = vector
                .iter()
                .zip(query)
                .map(|(left, right)| left * right)
                .sum();
            if score > 0.0 {
                hits.push(Hit {
                    path: file.path.clone(),
                    chunk: chunk_index,
                    score,
                });
            }
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.chunk.cmp(&right.chunk))
    });
    hits.truncate(limit);
    hits
}

pub fn fuse(lexical: Vec<Hit>, dense: Vec<Hit>, k: f32, limit: usize) -> Vec<Hit> {
    let mut scores: HashMap<(String, usize), f32> = HashMap::new();
    for (rank, hit) in lexical.into_iter().enumerate() {
        *scores
            .entry((hit.path, hit.chunk))
            .or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }
    for (rank, hit) in dense.into_iter().enumerate() {
        *scores
            .entry((hit.path, hit.chunk))
            .or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
    }
    let mut hits: Vec<Hit> = scores
        .into_iter()
        .map(|((path, chunk), score)| Hit { path, chunk, score })
        .collect();
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.chunk.cmp(&right.chunk))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::tools::docs::test_support::FakeEmbedder;
    use tupi_kernel::embed::Embedder;
    use crate::tools::docs::store::{ChunkMeta, FileEntry, Index, hash_hex};
    use crate::tools::docs::terms::terms;

    fn file_with_chunk(path: &str, text: &str, heading: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            hash: hash_hex(text.as_bytes()),
            bytes: text.len() as u64,
            mtime: 0,
            summary: None,
            chunks: vec![ChunkMeta {
                heading: heading.to_string(),
                start: 1,
                end: 1,
                hash: hash_hex(text.as_bytes()),
                tokens: text.split_whitespace().count(),
                terms: terms(text),
            }],
            vector_count: 0,
        }
    }

    #[test]
    fn lexical_prefers_the_chunk_with_the_query_term() {
        let mut index = Index::empty(vec!["docs".to_string()]);
        index.files.push(file_with_chunk(
            "docs/a.md",
            "A cobranca recorrente e mensal.",
            "Guia > Cobranca",
        ));
        index.files.push(file_with_chunk(
            "docs/b.md",
            "O contrato vence em maio.",
            "Guia > Juridico",
        ));

        let hits = lexical(&index, "cobranca recorrente", 5);

        assert_eq!(hits[0].path, "docs/a.md");
        assert!(hits[0].score > 0.0);
    }
    #[test]
    fn dense_finds_a_chunk_without_the_query_word() {
        let mut index = Index::empty(vec!["docs".to_string()]);
        index.files.push(file_with_chunk("docs/a.md", "O automovel e novo.", "Texto > Frota"));
        index.files.push(file_with_chunk("docs/b.md", "O contrato vence.", "Texto > Juridico"));
        let query = FakeEmbedder::default().embed_query("carro").unwrap();
        let vectors = HashMap::from([("docs/a.md".to_string(), vec![query.clone()])]);
        let hits = dense(&index, &vectors, &query, 5);
        assert_eq!(hits[0].path, "docs/a.md");
    }

    #[test]
    fn rrf_keeps_both_lists_without_score_calibration() {
        let a = Hit { path: "docs/a.md".into(), chunk: 0, score: 100.0 };
        let b = Hit { path: "docs/b.md".into(), chunk: 0, score: 1.0 };
        let fused = fuse(vec![a], vec![b], 60.0, 2);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].path, "docs/a.md");
    }

    #[test]
    fn query_longer_than_the_model_limit_is_truncated_for_embedding() {
        let embedder = FakeEmbedder::default();
        let text = "palavra ".repeat(40);
        assert!(embedder.token_len(&text) > embedder.max_query_tokens());
        assert_eq!(embedder.truncate_query(&text).split_whitespace().count(), 32);
    }

    #[test]
    fn hybrid_keeps_lexical_results_when_dense_is_empty() {
        let hit = Hit { path: "docs/a.md".into(), chunk: 0, score: 2.0 };
        let fused = fuse(vec![hit], Vec::new(), 60.0, 5);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].path, "docs/a.md");
    }
}
