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
    // A chave do cache de `exec` precisa cobrir o conteúdo, não os metadados:
    // `(len, mtime)` mente (mesmo tamanho + mtime preservado por `cp -p`,
    // `rsync -t`, `tar x`) e uma chave que mente devolve verde velho para o
    // único falsificador do projeto. Além dos bytes, entram na chave as
    // variáveis de ambiente que mudam o build sem tocar em arquivo nenhum.
    let mut environment = String::new();
    for name in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTUP_TOOLCHAIN",
    ] {
        environment.push_str(name);
        environment.push('=');
        environment.push_str(&std::env::var(name).unwrap_or_default());
        environment.push('\n');
    }

    fingerprint_with(root, limit, &environment)
}

/// Hash do conteúdo de todo arquivo que o cargo pode ler, mais `environment`.
pub(crate) fn fingerprint_with(root: &Path, limit: usize, environment: &str) -> Option<String> {
    const MAX_BYTES: u64 = 64 * 1024 * 1024;

    use ignore::WalkBuilder;
    use std::io::Read;

    // Nenhum filtro de gitignore/`hidden`: arquivo ignorado pelo git continua
    // sendo entrada do build (fixture gerada, `.env` de teste). Ficam de fora
    // só caminhos que o cargo comprovadamente não lê: artefatos (`target/`), o
    // banco do git (`.git/`) e o estado que este agente grava (`.agents/tupi/`,
    // `.brain/`). Uma exclusão sem justificativa aqui não existe.
    let base = root.to_path_buf();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false)
        // Link simbólico de diretório é seguido: fonte vendada por link é
        // entrada como qualquer outra. Um ciclo de links não trava o walk: o
        // teto de arquivos transforma o laço em `None`, e sem chave não há cache.
        .follow_links(true)
        .filter_entry(move |entry| {
            let Ok(relative) = entry.path().strip_prefix(&base) else {
                return true;
            };
            if relative.as_os_str().is_empty() {
                return true;
            }
            if relative.starts_with(Path::new(".agents").join("tupi")) {
                return false;
            }
            !relative.components().any(|component| {
                matches!(
                    component.as_os_str().to_str(),
                    Some("target" | ".git" | ".brain")
                )
            })
        });

    let mut entries: Vec<(PathBuf, u64)> = Vec::new();
    let mut remaining = MAX_BYTES;
    for entry in builder.build().flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let length = metadata.len();
        if entries.len() >= limit || length > remaining {
            return None;
        }
        remaining -= length;
        entries.push((path.to_path_buf(), length));
    }
    entries.sort();

    let mut hasher = blake3::Hasher::new();
    hasher.update(environment.as_bytes());
    let mut buffer = vec![0u8; 64 * 1024];
    for (path, length) in entries {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&length.to_le_bytes());
        let mut file = std::io::BufReader::new(std::fs::File::open(&path).ok()?);
        let mut read_total = 0u64;
        loop {
            let read = file.read(&mut buffer).ok()?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            read_total += read as u64;
        }
        hasher.update(&read_total.to_le_bytes());
    }
    Some(hasher.finalize().to_hex().to_string())
}
