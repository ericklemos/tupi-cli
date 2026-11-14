pub(crate) use super::diagnostics::sections_for;
use super::*;

pub(crate) fn clip(text: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= head + tail {
        return text.to_string();
    }
    let mut out: String = chars[..head].iter().collect();
    out.push_str(&format!(
        "\n... ({} caracteres omitidos)\n",
        chars.len() - head - tail
    ));
    out.extend(&chars[chars.len() - tail..]);
    out
}

pub(crate) fn finish(
    store: &ResponseStore,
    sections: Vec<Section>,
    budget_tokens: usize,
) -> String {
    let selection = budget::select(&sections, budget_tokens);
    let handle = if selection.omitted.is_empty() {
        None
    } else {
        Some(store.insert(selection.omitted_body()))
    };
    budget::render(&selection, handle.as_deref())
}

pub(crate) fn fingerprint(root: &Path, limit: usize) -> Option<String> {
    use ignore::WalkBuilder;

    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(true)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != "target");

    let mut entries: Vec<(PathBuf, u64, u64)> = Vec::new();
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        if !matches!(extension, "rs" | "toml" | "lock") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        if entries.len() >= limit {
            return None;
        }
        entries.push((path.to_path_buf(), metadata.len(), modified));
    }
    entries.sort();

    let mut hasher = blake3::Hasher::new();
    for (path, length, modified) in entries {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&length.to_le_bytes());
        hasher.update(&modified.to_le_bytes());
    }
    Some(hasher.finalize().to_hex().to_string())
}
