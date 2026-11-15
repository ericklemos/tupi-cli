//! Índice incremental da documentação do projeto.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::WalkBuilder;
use tupi_kernel::ToolError;
use tupi_kernel::embed::Embedder;
use tupi_kernel::generate::Generator;

use crate::support::budget::{Section, estimate_tokens};

use super::chunk::{ChunkSpec, chunk_text};
use super::store::{self, ChunkMeta, FileEntry, Index, Skipped};
use super::terms::terms;

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const HARD_MAX_TOKENS: usize = 512;
const FALLBACK_MAX_TOKENS: usize = 480;
const MIN_TOKENS: usize = 80;

/// Resultado de uma verificação do índice.
#[allow(dead_code)]
pub struct RefreshReport {
    pub files: usize,
    pub chunks: usize,
    pub embedded: usize,
    pub pending_vectors: usize,
    pub skipped: usize,
    pub touched: Vec<String>,
    pub error: Option<String>,
}

#[allow(dead_code)]
pub struct SummaryOutcome {
    pub path: String,
    pub summary: Option<String>,
    pub error: Option<String>,
}

/// Índice em memória, recarregado do disco na primeira chamada.
pub struct DocsIndex {
    root_dir: PathBuf,
    folders: Vec<PathBuf>,
    state: Mutex<State>,
}

struct State {
    index: Index,
    vectors: HashMap<String, Vec<Vec<f32>>>,
    loaded: bool,
    pending: VecDeque<String>,
}

impl DocsIndex {
    pub fn new(root_dir: PathBuf, folders: Vec<PathBuf>) -> Self {
        Self {
            root_dir,
            folders,
            state: Mutex::new(State {
                index: Index::empty(Vec::new()),
                vectors: HashMap::new(),
                loaded: false,
                pending: VecDeque::new(),
            }),
        }
    }

    fn relative_folders(&self) -> Vec<String> {
        self.folders
            .iter()
            .map(|folder| relative_path(folder, &self.root_dir))
            .collect()
    }

    pub fn refresh(&self, embedder: Option<&dyn Embedder>, max_embeds: usize) -> RefreshReport {
        let folders = self.relative_folders();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.loaded {
            state.index = store::load_index(&self.root_dir)
                .filter(|index| index.schema == 1 && index.folders == folders)
                .unwrap_or_else(|| Index::empty(folders.clone()));
            let files = state.index.files.clone();
            for file in files {
                if file.vector_count == file.chunks.len()
                    && let Ok(Some((_dim, vectors))) =
                        store::load_vectors(&self.root_dir, &file.path)
                {
                    state.vectors.insert(file.path, vectors);
                }
            }
            state.loaded = true;
        }

        let _ = store::ensure_gitignore(&self.root_dir);

        let token_limit = embedder
            .map_or(FALLBACK_MAX_TOKENS, |embedder| {
                embedder.max_doc_tokens().min(HARD_MAX_TOKENS)
            })
            .max(1);
        let spec = ChunkSpec {
            max_tokens: token_limit,
            target_tokens: (token_limit * 4 / 5).max(1),
            min_tokens: MIN_TOKENS.min(token_limit / 4).max(1),
        };
        let token_len = |text: &str| {
            embedder.map_or_else(
                || estimate_tokens(text),
                |embedder| embedder.token_len(text),
            )
        };

        let old: HashMap<String, FileEntry> = state
            .index
            .files
            .drain(..)
            .map(|file| (file.path.clone(), file))
            .collect();
        let mut files = Vec::new();
        let mut skipped = Vec::new();
        let mut touched = Vec::new();
        let mut total_bytes = 0u64;

        for folder in &self.folders {
            if !folder.exists() {
                skipped.push(Skipped {
                    path: relative_path(folder, &self.root_dir),
                    reason: "missing_folder".to_string(),
                });
                continue;
            }
            for entry in WalkBuilder::new(folder).build() {
                let Ok(entry) = entry else {
                    continue;
                };
                let Some(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_file() {
                    continue;
                }
                let path = entry.path();
                let relative = relative_path(path, &self.root_dir);
                if relative.is_empty() || has_ignored_component(&relative) {
                    continue;
                }
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let bytes_len = metadata.len();
                let mtime = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as u64)
                    .unwrap_or(0);

                if let Some(previous) = old.get(&relative)
                    && previous.bytes == bytes_len
                    && previous.mtime == mtime
                {
                    files.push(previous.clone());
                    total_bytes += previous.bytes;
                    continue;
                }

                if bytes_len > MAX_FILE_BYTES {
                    skipped.push(Skipped {
                        path: relative,
                        reason: "too_large".to_string(),
                    });
                    continue;
                }
                if total_bytes + bytes_len > MAX_TOTAL_BYTES {
                    skipped.push(Skipped {
                        path: relative,
                        reason: "too_large".to_string(),
                    });
                    continue;
                }
                let Ok(bytes) = std::fs::read(path) else {
                    skipped.push(Skipped {
                        path: relative,
                        reason: "unreadable".to_string(),
                    });
                    continue;
                };
                if bytes.contains(&0) {
                    skipped.push(Skipped {
                        path: relative,
                        reason: "binary".to_string(),
                    });
                    continue;
                }
                let Ok(text) = std::str::from_utf8(&bytes) else {
                    skipped.push(Skipped {
                        path: relative,
                        reason: "not_utf8".to_string(),
                    });
                    continue;
                };
                let chunks = chunk_text(text, spec, &token_len)
                    .into_iter()
                    .map(|chunk| ChunkMeta {
                        heading: chunk.heading,
                        start: chunk.start,
                        end: chunk.end,
                        hash: store::hash_hex(chunk.text.as_bytes()),
                        tokens: chunk.tokens,
                        terms: terms(&chunk.text),
                    })
                    .collect();
                files.push(FileEntry {
                    path: relative.clone(),
                    hash: store::hash_hex(&bytes),
                    bytes: bytes_len,
                    mtime,
                    summary: None,
                    chunks,
                    vector_count: 0,
                });
                touched.push(relative);
                total_bytes += bytes_len;
            }
        }

        let kept: HashSet<&str> = files.iter().map(|file| file.path.as_str()).collect();
        for path in old.keys() {
            if !kept.contains(path.as_str()) {
                let _ = store::remove_vectors(&self.root_dir, path);
                state.vectors.remove(path);
                touched.push(path.clone());
            }
        }
        for file in &files {
            if let Some(previous) = old.get(&file.path)
                && previous.hash != file.hash
            {
                let _ = store::remove_vectors(&self.root_dir, &file.path);
                state.vectors.remove(&file.path);
            }
        }

        files.sort_by(|left, right| left.path.cmp(&right.path));
        touched.sort();
        touched.dedup();
        state.index.files = files;
        state.index.folders = folders;
        state.index.skipped = skipped;
        let missing: Vec<String> = state
            .index
            .files
            .iter()
            .filter(|file| file.summary.is_none())
            .map(|file| file.path.clone())
            .collect();
        for path in missing {
            if !state.pending.contains(&path) {
                state.pending.push_back(path);
            }
        }

        let mut embedded = 0usize;
        let mut pending_vectors = 0usize;
        let mut error = None;
        if let Some(embedder) = embedder {
            let model = embedder.model_id().to_string();
            let dim = embedder.dim();
            let reset = state
                .index
                .embedding
                .as_ref()
                .map_or(true, |meta| meta.model != model || meta.dim != dim);
            if reset {
                for file in &mut state.index.files {
                    file.vector_count = 0;
                }
                for file in &state.index.files {
                    let _ = store::remove_vectors(&self.root_dir, &file.path);
                }
                state.vectors.clear();
                state.index.embedding = Some(super::store::EmbeddingMeta {
                    model,
                    dim,
                    revision: "local".to_string(),
                });
            }
            let mut budget = max_embeds;
            let mut files = std::mem::take(&mut state.index.files);
            for file in &mut files {
                if budget == 0 {
                    break;
                }
                if file.vector_count >= file.chunks.len() {
                    continue;
                }
                let Ok(full) = std::fs::read_to_string(self.root_dir.join(&file.path)) else {
                    continue;
                };
                let mut vectors = state.vectors.remove(&file.path).unwrap_or_default();
                vectors.truncate(file.vector_count);
                let first = file.vector_count;
                let mut texts = Vec::new();
                for chunk in &file.chunks[first..] {
                    if budget == 0 {
                        break;
                    }
                    let Some(text) = store::slice_lines(&full, chunk.start, chunk.end) else {
                        continue;
                    };
                    texts.push(text);
                    budget -= 1;
                }
                if texts.is_empty() {
                    state.vectors.insert(file.path.clone(), vectors);
                    continue;
                }
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                match embedder.embed_documents(&refs) {
                    Ok(new_vectors) => {
                        for vector in new_vectors {
                            vectors.push(normalize_vector(vector));
                        }
                        file.vector_count = vectors.len().min(file.chunks.len());
                        embedded += file.vector_count - first;
                        if let Err(save_error) =
                            store::save_vectors(&self.root_dir, &file.path, dim, &vectors)
                        {
                            error = Some(save_error.to_string());
                        }
                        state.vectors.insert(file.path.clone(), vectors);
                    }
                    Err(embed_error) => {
                        error = Some(embed_error.to_string());
                        state.vectors.insert(file.path.clone(), vectors);
                    }
                }
            }
            state.index.files = files;
            pending_vectors = state
                .index
                .files
                .iter()
                .map(|file| file.chunks.len().saturating_sub(file.vector_count))
                .sum();
        }

        let mut report = RefreshReport {
            files: state.index.files.len(),
            chunks: state.index.files.iter().map(|file| file.chunks.len()).sum(),
            embedded,
            pending_vectors,
            skipped: state.index.skipped.len(),
            touched,
            error,
        };
        if (!report.touched.is_empty() || report.embedded > 0)
            && let Err(save_error) = store::save_index(&self.root_dir, &state.index)
        {
            report.error = Some(save_error.to_string());
        }
        report
    }

    pub fn map_sections(&self, path: Option<&str>, max_files: usize) -> Vec<Section> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut body = format!(
            "- arquivos: {}\n- chunks: {}\n- pulados: {}",
            state.index.files.len(),
            state
                .index
                .files
                .iter()
                .map(|file| file.chunks.len())
                .sum::<usize>(),
            state.index.skipped.len()
        );
        for skipped in &state.index.skipped {
            body.push_str(&format!(
                "\n- pulado: {} ({})",
                skipped.path, skipped.reason
            ));
        }
        let mut sections = vec![Section {
            heading: "### docs — mapa".to_string(),
            body,
            priority: 255,
        }];

        let mut listed = 0;
        for file in &state.index.files {
            if let Some(prefix) = path
                && !file.path.starts_with(prefix)
            {
                continue;
            }
            if listed >= max_files {
                break;
            }
            let mut seen = HashSet::new();
            let headings: Vec<&str> = file
                .chunks
                .iter()
                .map(|chunk| chunk.heading.as_str())
                .filter(|heading| !heading.is_empty() && seen.insert(*heading))
                .collect();
            let mut file_body = String::new();
            if let Some(summary) = &file.summary {
                file_body.push_str(summary);
                file_body.push('\n');
            }
            for heading in headings {
                file_body.push_str(&format!("- {heading}\n"));
            }
            file_body.push_str(&format!(
                "- linhas: {}",
                file.chunks.iter().map(|chunk| chunk.end).max().unwrap_or(0)
            ));
            sections.push(Section {
                heading: format!("### {}", file.path),
                body: file_body,
                priority: 200,
            });
            listed += 1;
        }
        sections
    }

    pub fn status_text(&self, report: &RefreshReport, embedder: Option<&dyn Embedder>) -> String {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let model = state
            .index
            .embedding
            .as_ref()
            .map(|meta| format!("{} ({} dims)", meta.model, meta.dim))
            .or_else(|| {
                embedder
                    .map(|embedder| format!("{} ({} dims)", embedder.model_id(), embedder.dim()))
            })
            .unwrap_or_else(|| "nenhum (busca lexical)".to_string());
        let mut text = format!(
            "- arquivos: {}\n- chunks: {}\n- pulados: {}\n- modelo: {}\n- vetores pendentes: {}\n- tocados nesta chamada: {}",
            state.index.files.len(),
            state
                .index
                .files
                .iter()
                .map(|file| file.chunks.len())
                .sum::<usize>(),
            state.index.skipped.len(),
            model,
            report.pending_vectors,
            report.touched.len()
        );
        text.push_str(&format!(
            "\n- resumos: {} prontos, {} pendentes",
            state
                .index
                .files
                .iter()
                .filter(|file| file.summary.is_some())
                .count(),
            state.pending.len()
        ));
        if let Some(error) = &report.error {
            text.push_str(&format!("\n- erro: {error}"));
        }
        text
    }

    pub fn read_lines(
        &self,
        path: &str,
        start: Option<usize>,
        end: Option<usize>,
    ) -> Result<Option<String>, ToolError> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.index.files.iter().any(|file| file.path == path) {
            return Ok(None);
        }
        let full = std::fs::read_to_string(self.root_dir.join(path))
            .map_err(|error| ToolError::Other(error.to_string()))?;
        Ok(match (start, end) {
            (None, None) => Some(full),
            (Some(start), end) => store::slice_lines(&full, start, end.unwrap_or(usize::MAX)),
            (None, Some(end)) => store::slice_lines(&full, 1, end),
        })
    }

    #[allow(dead_code)]
    pub fn files_len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .index
            .files
            .len()
    }
    #[allow(dead_code)]
    pub fn pending_summaries(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .len()
    }

    #[allow(dead_code)]
    pub fn summaries_ready(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .index
            .files
            .iter()
            .filter(|file| file.summary.is_some())
            .count()
    }

    pub fn process_one_summary(
        &self,
        generator: &dyn Generator,
        max_chars: usize,
        max_tokens: usize,
    ) -> Option<SummaryOutcome> {
        let path = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.pending.pop_front()?
        };
        let (hash, headings, excerpt) = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let file = state.index.files.iter().find(|file| file.path == path)?;
            if let Some(summary) = &file.summary {
                return Some(SummaryOutcome {
                    path,
                    summary: Some(summary.clone()),
                    error: None,
                });
            }
            let mut seen = HashSet::new();
            let headings: Vec<String> = file
                .chunks
                .iter()
                .map(|chunk| chunk.heading.clone())
                .filter(|heading| !heading.is_empty() && seen.insert(heading.clone()))
                .take(12)
                .collect();
            let full = std::fs::read_to_string(self.root_dir.join(&path)).ok()?;
            (
                file.hash.clone(),
                headings,
                super::summary::truncate_chars(&full, 4000),
            )
        };
        let prompt = super::summary::build_prompt(&path, &headings, &excerpt);
        let (summary, error) = match generator.generate(&prompt, max_tokens) {
            Ok(raw) => (super::summary::sanitize(&raw, max_chars), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let current = std::fs::read(self.root_dir.join(&path))
            .map(|bytes| store::hash_hex(&bytes))
            .unwrap_or_default();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if current != hash {
            state.pending.push_back(path.clone());
            return Some(SummaryOutcome {
                path,
                summary: None,
                error: Some("arquivo mudou".to_string()),
            });
        }
        if let Some(file) = state.index.files.iter_mut().find(|file| file.path == path) {
            file.summary = summary.clone();
            let _ = store::save_index(&self.root_dir, &state.index);
        }
        Some(SummaryOutcome {
            path,
            summary,
            error,
        })
    }

    #[allow(dead_code)]
    pub fn chunks_len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .index
            .files
            .iter()
            .map(|file| file.chunks.len())
            .sum()
    }
    pub fn lexical_hits(&self, query: &str, limit: usize) -> Vec<super::search::Hit> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        super::search::lexical(&state.index, query, limit)
    }
    pub fn query(
        &self,
        text: &str,
        limit: usize,
        embedder: Option<&dyn Embedder>,
    ) -> Vec<super::search::Hit> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let depth = 50.min(limit.max(1).saturating_mul(10));
        let lexical_hits = super::search::lexical(&state.index, text, depth);
        let Some(embedder) = embedder else {
            return lexical_hits.into_iter().take(limit).collect();
        };
        let Ok(query) = embedder.embed_query(text) else {
            return lexical_hits.into_iter().take(limit).collect();
        };
        let query = normalize_vector(query);
        let dense_hits = super::search::dense(&state.index, &state.vectors, &query, depth);
        super::search::fuse(lexical_hits, dense_hits, 60.0, limit)
    }

    pub fn chunk_body(
        &self,
        hit: &super::search::Hit,
    ) -> Option<(String, usize, usize, String, blake3::Hash)> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let file = state
            .index
            .files
            .iter()
            .find(|file| file.path == hit.path)?;
        let chunk = file.chunks.get(hit.chunk)?;
        let full = std::fs::read_to_string(self.root_dir.join(&file.path)).ok()?;
        let text = store::slice_lines(&full, chunk.start, chunk.end)?;
        Some((
            file.path.clone(),
            chunk.start,
            chunk.end,
            text,
            blake3::Hash::from_hex(&file.hash).ok()?,
        ))
    }
}
fn normalize_vector(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn has_ignored_component(path: &str) -> bool {
    path.split('/')
        .any(|part| part == "target" || part == "node_modules")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::docs::test_support::{FakeEmbedder, FakeGenerator};
    use tupi_kernel::generate::{GenerateError, Generator};

    fn write(dir: &std::path::Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn first_refresh_indexes_text_and_skips_binary_and_large() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "docs/guia.md",
            "# Guia\n\nA cobranca e mensal.\n",
        );
        write(dir.path(), "docs/bin.dat", "abc\0def");
        write(dir.path(), "docs/grande.md", &"x".repeat(2 * 1024 * 1024));
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);

        let report = index.refresh(None, 0);

        assert_eq!(report.files, 1);
        assert!(report.skipped >= 2);
        assert!(report.touched.contains(&"docs/guia.md".to_string()));
        assert_eq!(index.files_len(), 1);
        assert!(index.chunks_len() >= 1);
    }

    #[test]
    fn second_refresh_without_changes_touches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/guia.md", "# Guia\n\ntexto\n");
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        index.refresh(None, 0);
        let report = index.refresh(None, 0);
        assert!(report.touched.is_empty(), "{:?}", report.touched);
    }

    #[test]
    fn editing_one_file_touches_only_that_file() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/a.md", "# A\n\nantes\n");
        write(dir.path(), "docs/b.md", "# B\n\ndepois\n");
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        index.refresh(None, 0);
        std::thread::sleep(std::time::Duration::from_millis(10));
        write(dir.path(), "docs/a.md", "# A\n\nmudou\n");

        let report = index.refresh(None, 0);

        assert_eq!(report.touched, vec!["docs/a.md".to_string()]);
    }

    #[test]
    fn missing_folder_is_reported_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let index = DocsIndex::new(
            dir.path().to_path_buf(),
            vec![dir.path().join("nao-existe")],
        );
        let report = index.refresh(None, 0);
        assert_eq!(report.files, 0);
        assert!(report.error.is_none());
    }

    #[test]
    fn removing_a_file_drops_it_from_the_index() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/a.md", "# A\n\ntexto\n");
        write(dir.path(), "docs/b.md", "# B\n\ntexto\n");
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        index.refresh(None, 0);
        std::fs::remove_file(dir.path().join("docs/a.md")).unwrap();
        let report = index.refresh(None, 0);
        assert_eq!(report.files, 1);
        assert_eq!(index.files_len(), 1);
        assert!(!report.touched.contains(&"docs/b.md".to_string()));
    }
    #[test]
    fn a_fake_embedder_fills_vectors_in_batches() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/a.md", "# A\n\num texto\n");
        write(dir.path(), "docs/b.md", "# B\n\noutro texto\n");
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        let embedder = FakeEmbedder::default();

        let first = index.refresh(Some(&embedder), 1);
        assert_eq!(first.embedded, 1);
        assert!(first.pending_vectors >= 1);

        let second = index.refresh(Some(&embedder), 10);
        assert_eq!(second.pending_vectors, 0);
        let (dim, vectors) = crate::tools::docs::store::load_vectors(dir.path(), "docs/a.md")
            .unwrap()
            .expect("blob");
        assert_eq!(dim, 4);
        assert_eq!(vectors.len(), 1);
    }

    #[test]
    fn changing_the_model_discards_old_vectors() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/a.md", "# A\n\ntexto\n");
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        index.refresh(Some(&FakeEmbedder::named("um")), 10);
        let report = index.refresh(Some(&FakeEmbedder::named("dois")), 10);
        assert_eq!(report.embedded, 1);
    }
    #[test]
    fn process_one_summary_stores_a_sanitized_line() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/a.md", "# A\n\ntexto unico\n");
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        index.refresh(None, 0);
        let generator = FakeGenerator::default();

        let outcome = index
            .process_one_summary(&generator, 160, 64)
            .expect("outcome");

        assert_eq!(outcome.path, "docs/a.md");
        assert_eq!(outcome.summary.as_deref(), Some("Resumo de teste."));
        assert_eq!(index.summaries_ready(), 1);
    }

    #[test]
    fn a_changed_file_discards_the_stale_summary() {
        struct MutatingGenerator {
            path: std::path::PathBuf,
        }
        impl Generator for MutatingGenerator {
            fn model_id(&self) -> &str {
                "mutating"
            }
            fn generate(&self, _prompt: &str, _max_tokens: usize) -> Result<String, GenerateError> {
                std::fs::write(&self.path, "# A\n\nmudou\n").unwrap();
                Ok("Resumo velho.".to_string())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/a.md", "# A\n\nantes\n");
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        index.refresh(None, 0);
        let generator = MutatingGenerator {
            path: dir.path().join("docs/a.md"),
        };

        let outcome = index
            .process_one_summary(&generator, 160, 64)
            .expect("outcome");

        assert!(outcome.summary.is_none());
        assert_eq!(index.summaries_ready(), 0);
        assert!(index.pending_summaries() >= 1);
    }

    #[test]
    fn a_reload_enqueues_files_without_summary() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "docs/a.md", "# A\n\ntexto\n");
        write(dir.path(), "docs/b.md", "# B\n\ntexto\n");
        {
            let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
            index.refresh(None, 0);
            index
                .process_one_summary(&FakeGenerator::default(), 160, 64)
                .unwrap();
        }
        let index = DocsIndex::new(dir.path().to_path_buf(), vec![dir.path().join("docs")]);
        index.refresh(None, 0);
        assert_eq!(index.summaries_ready(), 1);
        assert!(index.pending_summaries() >= 1);
    }
}
