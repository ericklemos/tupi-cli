pub(crate) use super::diagnostics::sections_for;
use super::*;

use std::io::Read;
use std::sync::OnceLock;

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

/// Teto de bytes lidos por chave. Estourado, o cache desliga (fail-closed).
pub(crate) const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Caminhos que o `cargo check/build/test/clippy` não lê como entrada.
///
/// Cada entrada precisa de justificativa escrita aqui: excluir da chave um
/// arquivo que o build lê devolve verde velho.
pub(crate) const CACHE_EXCLUSIONS: &[&str] = &[
    // Artefatos de compilação: reescritos a cada rodada, nunca entrada de
    // build (`CARGO_TARGET_DIR` fora do root é lacuna de `fingerprint`).
    "target",
    // Banco do git: o cargo lê a árvore de trabalho, nunca `.git/`.
    ".git",
    // Estado do agente (grafo, `.brain`): escrito por esta sessão.
    ".brain",
    // Memória durável e journals (`.agents/tupi/`): mudam a cada tool call e
    // não entram em compilação nenhuma.
    ".agents/tupi",
    // Log da sessão corrente (`.tupi/sessions/<id>/log.jsonl`): cresce a cada
    // interação (medido 139.534 -> 141.168 bytes em 4 s). Sem esta exclusão a
    // chave muda entre duas chamadas idênticas e o cache nunca acerta.
    ".tupi",
];

/// `true` se `relative` (relativo ao root) está fora da chave.
///
/// Entrada com `/` casa como prefixo de caminho; entrada de um componente casa
/// em qualquer profundidade (`vendor/target` também é artefato).
pub(crate) fn is_excluded(relative: &Path) -> bool {
    CACHE_EXCLUSIONS.iter().any(|exclusion| {
        let exclusion = Path::new(exclusion);
        if exclusion.components().count() > 1 {
            relative.starts_with(exclusion)
        } else {
            relative
                .components()
                .any(|component| component.as_os_str() == exclusion.as_os_str())
        }
    })
}

/// Por que a chave do cache não pode ser construída. Nunca é *hit*.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CacheOff {
    Files(usize),
    Bytes(u64),
    Unreadable,
    Toolchain,
}

impl CacheOff {
    /// Frase curta para o resumo (uma linha, sem custo de tokens relevante).
    pub(crate) fn reason(&self) -> String {
        match self {
            CacheOff::Files(limit) => format!("mais de {limit} arquivos sob o root"),
            CacheOff::Bytes(limit) => format!("mais de {} MB sob o root", limit / (1024 * 1024)),
            CacheOff::Unreadable => "arquivo ilegivel sob o root".to_string(),
            CacheOff::Toolchain => "versao do toolchain ilegivel".to_string(),
        }
    }
}

/// Walk determinístico dos arquivos sob `root`, com as exclusões nomeadas.
///
/// Só metadados: separar o walk do hash deixa medir as duas fases (T1.1).
pub(crate) fn fingerprint_entries(
    root: &Path,
    limit: usize,
) -> Result<Vec<(PathBuf, u64)>, CacheOff> {
    use ignore::WalkBuilder;

    // Nenhum filtro de gitignore/`hidden`: arquivo ignorado pelo git continua
    // sendo entrada do build (fixture gerada, `.env` de teste). Ficam de fora
    // só os caminhos de `CACHE_EXCLUSIONS`.
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
        // teto de arquivos devolve `CacheOff::Files`, e sem chave não há cache.
        .follow_links(true)
        .filter_entry(move |entry| {
            let Ok(relative) = entry.path().strip_prefix(&base) else {
                return true;
            };
            relative.as_os_str().is_empty() || !is_excluded(relative)
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
        if entries.len() >= limit {
            return Err(CacheOff::Files(limit));
        }
        if length > remaining {
            return Err(CacheOff::Bytes(MAX_BYTES));
        }
        remaining -= length;
        entries.push((path.to_path_buf(), length));
    }
    entries.sort();
    Ok(entries)
}

/// Hash de `entries` (caminho + tamanho + bytes) mais o bloco de ambiente.
pub(crate) fn fingerprint_digest(
    entries: &[(PathBuf, u64)],
    environment: &str,
) -> Result<String, CacheOff> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(environment.as_bytes());
    let mut buffer = vec![0u8; 64 * 1024];
    for (path, length) in entries {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&length.to_le_bytes());
        let mut file =
            std::io::BufReader::new(std::fs::File::open(path).map_err(|_| CacheOff::Unreadable)?);
        let mut read_total = 0u64;
        loop {
            let read = file.read(&mut buffer).map_err(|_| CacheOff::Unreadable)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            read_total += read as u64;
        }
        hasher.update(&read_total.to_le_bytes());
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Variáveis de ambiente que mudam o resultado do cargo.
const ENV_PREFIXES: &[&str] = &["CARGO_", "RUSTUP_", "RUSTFLAGS", "RUSTC_", "SQLX_"];
const ENV_NAMES: &[&str] = &["DATABASE_URL", "CC", "CFLAGS", "PATH"];

/// Bloco de ambiente da chave: variáveis selecionadas, versão resolvida do
/// toolchain e conteúdo dos `config.toml` do cargo. Ordenado e puro, para o
/// teste trocar cada peça sem tocar no processo.
///
/// Fora de `CARGO_`/`RUSTUP_`/`SQLX_` ficam `RUST_LOG` e `RUST_BACKTRACE`:
/// mudam só o ruído do log, nunca o binário — incluí-los faria a chave mudar
/// sem necessidade.
pub(crate) fn environment_block_from(
    vars: impl IntoIterator<Item = (String, String)>,
    toolchain: &str,
    configs: &[(String, Option<String>)],
) -> String {
    let mut selected: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(name, _)| {
            ENV_NAMES.contains(&name.as_str())
                || ENV_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        })
        .collect();
    selected.sort();

    let mut block = String::new();
    for (name, value) in selected {
        block.push_str(&format!("{name}={value}\n"));
    }
    block.push_str("toolchain=");
    block.push_str(toolchain);
    block.push('\n');
    for (path, content) in configs {
        block.push_str("config:");
        block.push_str(path);
        block.push('=');
        block.push_str(content.as_deref().unwrap_or("absent"));
        block.push('\n');
    }
    block
}

/// Versão resolvida do toolchain (`rustc -Vv`), uma vez por processo.
///
/// Falha ao ler é fail-closed: sem versão, sem chave.
fn toolchain_version() -> Option<&'static str> {
    static VERSION: OnceLock<Option<String>> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let output = std::process::Command::new("rustc")
                .arg("-Vv")
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        })
        .as_deref()
}

/// `config.toml` do cargo que ficam fora do root e mudam o build.
///
/// Ausência vira `absent`: é um estado, não um erro.
fn cargo_configs(
    cargo_home: Option<&Path>,
    home: Option<&Path>,
) -> Vec<(String, Option<String>)> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(cargo_home) = cargo_home {
        paths.push(cargo_home.join("config.toml"));
    }
    if let Some(home) = home {
        paths.push(home.join(".cargo").join("config.toml"));
    }
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let content = std::fs::read_to_string(&path).ok();
            (path.to_string_lossy().into_owned(), content)
        })
        .collect()
}

/// Chave do cache de `exec`: conteúdo dos arquivos que o cargo lê + ambiente.
///
/// Cobre: bytes de todo arquivo sob o root (exceto `CACHE_EXCLUSIONS`), as
/// variáveis `CARGO_`/`RUSTUP_`/`RUSTFLAGS`/`RUSTC_`/`SQLX_`, `DATABASE_URL`,
/// `CC`, `CFLAGS`, `PATH`, a versão resolvida de `rustc -Vv` e o conteúdo de
/// `$CARGO_HOME/config.toml` e `~/.cargo/config.toml`.
///
/// Lacunas conhecidas (só viram *miss* quando a leitura falha):
/// - `CARGO_TARGET_DIR` fora do root: artefato de fora não entra na chave, e
///   o `target/` sob o root é excluído por definição.
/// - build script que lê rede, relógio ou `git describe`: a saída dele não
///   está em arquivo nenhum sob o root.
/// - variável de ambiente fora da lista acima: fica fora por custo — a lista
///   cobre o que o cargo documenta como entrada de build.
pub(crate) fn fingerprint(root: &Path, limit: usize) -> Result<String, CacheOff> {
    let entries = fingerprint_entries(root, limit)?;
    let toolchain = toolchain_version().ok_or(CacheOff::Toolchain)?;
    let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let configs = cargo_configs(cargo_home.as_deref(), home.as_deref());
    let environment = environment_block_from(std::env::vars(), toolchain, &configs);
    fingerprint_digest(&entries, &environment)
}

/// O que o resumo precisa saber sobre o cache nesta chamada.
#[derive(Debug, Clone, Default)]
pub(crate) struct CacheNote {
    /// Idade do hit; `None` é *miss*.
    pub age: Option<std::time::Duration>,
    /// Custo do walk + blake3 desta chamada (`None` com o cache desligado).
    pub key_cost: Option<std::time::Duration>,
    /// Motivo de o cache estar desligado.
    pub off: Option<CacheOff>,
}
