//! Candidatos densos do índice do espaço.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tupi_kernel::embed::Embedder;

use super::SearchTool;
use crate::space::SpaceSnapshot;
use crate::support::ast::AstSnippet;
use crate::support::ledger::content_hash;

impl SearchTool {
    pub(super) fn current_embedder(&self) -> Option<Arc<dyn Embedder>> {
        self.embedder.as_ref().and_then(|slot| slot.get())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn dense_snippets(
        &self,
        snapshot: &SpaceSnapshot,
        query: &str,
        path: Option<&str>,
        snippets: &mut Vec<AstSnippet>,
        files: &mut Vec<(PathBuf, String, String, blake3::Hash)>,
        file_indices: &mut Vec<usize>,
        matched_hashes: &mut HashMap<String, blake3::Hash>,
    ) -> Vec<usize> {
        let Some(embedder) = self.current_embedder() else {
            return Vec::new();
        };
        let Ok(query_vector) = embedder.embed_query(query) else {
            return Vec::new();
        };
        if query_vector.len() != snapshot.dim {
            return Vec::new();
        }
        let mut scored: Vec<(usize, f32)> = snapshot
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| path.is_none_or(|prefix| entry.relative.starts_with(prefix)))
            .filter_map(|(index, _)| {
                snapshot
                    .vectors
                    .get(index)
                    .map(|vector| (index, cosine(vector, &query_vector)))
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));

        let mut seen: HashMap<(String, usize), usize> = HashMap::new();
        for (index, &file_index) in file_indices.iter().enumerate() {
            let relative = files[file_index].2.clone();
            seen.insert((relative, snippets[index].start_line), index);
        }

        let mut contents: HashMap<String, String> = HashMap::new();
        let mut out = Vec::new();
        for (entry_index, _) in scored.into_iter().take(10) {
            let entry = &snapshot.entries[entry_index];
            let key = (entry.relative.clone(), entry.start_line);
            if seen.contains_key(&key) {
                continue;
            }
            let file_path = self.root_dir.join(&entry.relative);
            let content = contents
                .entry(entry.relative.clone())
                .or_insert_with(|| std::fs::read_to_string(&file_path).unwrap_or_default())
                .clone();
            if content.is_empty() {
                continue;
            }
            let mut found =
                self.chunker
                    .snippets_at_lines(&file_path, &content, &[entry.start_line]);
            let Some(snippet) = found.drain(..).next() else {
                continue;
            };
            let hash = content_hash(&content);
            let file_index = files.len();
            files.push((file_path, content, entry.relative.clone(), hash));
            matched_hashes.insert(entry.relative.clone(), hash);
            snippets.push(snippet);
            file_indices.push(file_index);
            seen.insert(key, snippets.len() - 1);
            out.push(snippets.len() - 1);
        }
        out
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|value| value * value).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|value| value * value).sum::<f32>().sqrt();
    dot / (na * nb).max(f32::EPSILON)
}
