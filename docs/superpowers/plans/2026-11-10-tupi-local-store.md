# Tupi Local Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** dar ao Tupi uma base local redb em `{home}/.tupi/tupi.redb`, começando por cachear o catálogo de modelos do `/models`.

**Architecture:** crate novo `tupi-store` (redb síncrono, schema versionado) ligado no composition root; `tupi-app` consulta o store em `spawn_blocking` e mantém a TUI reagindo só a eventos. Disco indisponível degrada para "sem store", nunca derruba o chat.

**Tech Stack:** Rust 2021, redb 2, serde/serde_json, thiserror, tokio.

**Spec:** `docs/superpowers/specs/2026-11-10-tupi-local-store-design.md`

## Global Constraints

- Rust 1.99, edition 2021, resolver 2.
- redb puro Rust; nenhum build C em nenhuma plataforma.
- redb é síncrono: chamadas de banco só em `tokio::task::spawn_blocking`, nunca no loop da TUI.
- Nenhuma dependência nova para resolver o home: função com getter de ambiente injetado.
- Caminho: `$HOME/.tupi/tupi.redb` em Linux/macOS, `%USERPROFILE%\.tupi\tupi.redb` no Windows, `./.tupi/tupi.redb` se o home não existir.
- TTL do catálogo: `24 * 60 * 60` segundos, constante.
- Este diretório **não é um repositório git**: os passos de commit do template são omitidos.

## Review Focus

- Home ausente (`HOME`/`USERPROFILE` vazios): o app abre `.tupi` no diretório atual e continua, sem panic.
- Diretório sem permissão, arquivo travado por outro processo ou arquivo corrompido: o chat abre normalmente, só sem cache.
- Valor de catálogo com JSON inválido: tratado como ausente e sobrescrito na próxima gravação.
- `schema_version` maior que o binário conhece: nada é escrito e o store fica indisponível.
- Relógio do sistema para trás: cache conta como fresco, sem refetch em loop.

---

### Task 1: crate `tupi-store` e frescor do cache

**Files:**
- Create: `crates/tupi-store/Cargo.toml`
- Create: `crates/tupi-store/src/lib.rs`
- Modify: `Cargo.toml` (members e workspace.dependencies)

**Interfaces:**
- Produces: `pub struct CachedModels { pub models: Vec<String>, pub fetched_at_unix: u64 }`
- Produces: `pub fn is_fresh(&self, now_unix: u64, ttl_secs: u64) -> bool`
- Produces: `pub const MODELS_TTL_SECS: u64 = 24 * 60 * 60`

- [ ] **Step 1: escrever o teste que falha**

```rust
#[test]
fn is_fresh_bounds() {
    let entry = CachedModels { models: vec!["m".into()], fetched_at_unix: 1_000 };
    assert!(entry.is_fresh(1_000, 10));
    assert!(entry.is_fresh(1_009, 10));
    assert!(!entry.is_fresh(1_010, 10));
    assert!(entry.is_fresh(999, 10), "relógio para trás conta como fresco");
}
```

- [ ] **Step 2: rodar e ver falhar**

`cargo test -p tupi-store` → erro de compilação (`CachedModels` não existe).

- [ ] **Step 3: criar o crate e implementar**

`redb = "2"`, `serde = { workspace = true, features = ["derive"] }`, `serde_json`, `thiserror`. O valor é serializado com `serde_json` (array JSON de `CachedModels`).

- [ ] **Step 4: rodar e ver passar**

`cargo test -p tupi-store` → 1 teste passa.

---

### Task 2: abrir o banco, schema e migração

**Files:**
- Modify: `crates/tupi-store/src/lib.rs`

**Interfaces:**
- Produces: `pub const SCHEMA_VERSION: u64 = 1`
- Produces: `pub struct Store`
- Produces: `pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError>`
- Produces: `pub enum StoreError` (`Io`, `Database`, `Transaction`, `FutureSchema { found, supported }`, `Corrupt`)

- [ ] **Step 1: escrever os testes que falham**

```rust
#[test]
fn open_creates_directory_and_file() {
    let path = temp_path("open");
    let store = Store::open(&path).expect("abre");
    drop(store);
    assert!(path.exists());
}

#[test]
fn open_on_garbage_file_is_an_error() {
    let path = temp_path("garbage");
    std::fs::write(&path, b"nao e um redb").unwrap();
    assert!(Store::open(&path).is_err());
}

#[test]
fn future_schema_is_refused_without_writing() {
    let path = temp_path("future");
    Store::open(&path).unwrap();
    set_schema_version_with_redb(&path, 99); // helper do teste, usa redb direto
    let error = Store::open(&path).expect_err("recusa");
    assert!(matches!(error, StoreError::FutureSchema { found: 99, .. }));
    assert_eq!(read_schema_version_with_redb(&path), 99);
}
```

`temp_path(name)` = `std::env::temp_dir().join(format!("tupi-store-{name}-{}", std::process::id()))`, removendo o arquivo antes e depois.

- [ ] **Step 2: rodar e ver falhar**

`cargo test -p tupi-store` → falta `Store`, `StoreError`, `SCHEMA_VERSION`.

- [ ] **Step 3: implementar**

Tabela `meta` (`&str` → JSON) e `models` (`&str` → JSON). Na abertura: criar diretório pai, abrir/criar banco, ler `meta.schema_version`; ausente → gravar `SCHEMA_VERSION`; menor → migrar em ordem (v1: nada); maior → `FutureSchema` sem escrita.

- [ ] **Step 4: rodar e ver passar**

`cargo test -p tupi-store` → 4 testes passam.

---

### Task 3: catálogo por provedor

**Files:**
- Modify: `crates/tupi-store/src/lib.rs`

**Interfaces:**
- Produces: `pub fn models(&self, provider_id: &str) -> Result<Option<CachedModels>, StoreError>`
- Produces: `pub fn save_models(&self, provider_id: &str, models: &[String], fetched_at_unix: u64) -> Result<(), StoreError>`

- [ ] **Step 1: escrever os testes que falham**

```rust
#[test]
fn models_round_trip_per_provider() {
    let store = Store::open(temp_path("round")).unwrap();
    assert!(store.models("deepseek").unwrap().is_none());
    store.save_models("deepseek", &["a".into(), "b".into()], 42).unwrap();
    let entry = store.models("deepseek").unwrap().unwrap();
    assert_eq!(entry.models, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(entry.fetched_at_unix, 42);
    assert!(store.models("outro").unwrap().is_none());
}

#[test]
fn corrupt_value_reads_as_absent() {
    let path = temp_path("corrupt");
    let store = Store::open(&path).unwrap();
    store.save_models("deepseek", &["a".into()], 1).unwrap();
    drop(store);
    overwrite_models_raw_with_redb(&path, "deepseek", b"{nao e json");
    let store = Store::open(&path).unwrap();
    assert!(store.models("deepseek").unwrap().is_none());
}
```

- [ ] **Step 2: rodar e ver falhar**

`cargo test -p tupi-store` → faltam os métodos.

- [ ] **Step 3: implementar**

Leitura/escrita em transação redb; JSON inválido → `Ok(None)`. `save_models` sobrescreve a chave do provedor.

- [ ] **Step 4: rodar e ver passar**

`cargo test -p tupi-store` → 6 testes passam.

---

### Task 4: `tupi-app` usa o cache no `RefreshModels`

**Files:**
- Modify: `crates/tupi-app/Cargo.toml` (dep `tupi-store`)
- Modify: `crates/tupi-app/src/app.rs` (`App`, `new`, `handle`)
- Modify: `crates/tupi-app/tests/list_models.rs`

**Interfaces:**
- Consumes: `Store::models`, `Store::save_models`, `CachedModels::is_fresh`, `MODELS_TTL_SECS`.
- Produces: `pub fn with_store(registry: ProviderRegistry, provider_id: impl Into<String>, model: String, events: EventBus, store: Option<Arc<Store>>) -> Arc<Self>`; `App::new` delega com `None`.

- [ ] **Step 1: escrever os testes que falham**

No teste de integração, com `Store::open(temp_path())`:

```rust
#[tokio::test]
async fn fresh_cache_skips_the_provider() {
    // store com save_models(provider, &["cache"], now) e provider que entra em panic!
    // handle(RefreshModels) → rx.recv() == ModelsLoaded { models: vec!["cache"] }
}

#[tokio::test]
async fn stale_cache_refreshes_and_saves() {
    // cache salvo com fetched_at 0 + provider estático ["novo"]
    // evento ModelsLoaded ["novo"] e store.models(id) == ["novo"] com fetched_at > 0
}

#[tokio::test]
async fn provider_failure_falls_back_to_stale_cache() {
    // cache fetched_at 0 + provider que devolve Err(Network)
    // evento ModelsLoaded com o cache; sem ModelsFailed
}

#[tokio::test]
async fn provider_failure_without_cache_reports_models_failed() {
    // store vazio + provider Err → ModelsFailed { message }
}
```

- [ ] **Step 2: rodar e ver falhar**

`cargo test -p tupi-app --test list_models` → não compila (`with_store` não existe).

- [ ] **Step 3: implementar**

No braço `Command::RefreshModels`: ler o cache em `spawn_blocking`; se `is_fresh(now, MODELS_TTL_SECS)` publicar `ModelsLoaded` e sair; senão buscar o provedor, gravar best-effort em `spawn_blocking`, publicar; em erro, publicar o cache vencido se houver, senão `ModelsFailed`. `now` = `SystemTime::now().duration_since(UNIX_EPOCH)`.

- [ ] **Step 4: rodar e ver passar**

`cargo test -p tupi-app` → suíte atual + 4 novos testes passam, incluindo `refresh_models_does_not_wait_for_a_slow_provider`.

---

### Task 5: composição, caminho e degradação

**Files:**
- Modify: `crates/tupi/Cargo.toml` (dep `tupi-store`)
- Modify: `crates/tupi/src/main.rs`

**Interfaces:**
- Produces: `fn data_dir<F>(get_env: F) -> PathBuf where F: Fn(&str) -> Option<String>`

- [ ] **Step 1: escrever os testes que falham**

```rust
#[test]
fn data_dir_uses_home() {
    let dir = super::data_dir(|key| (key == "HOME").then(|| "/home/ana".to_string()));
    assert_eq!(dir, std::path::PathBuf::from("/home/ana/.tupi"));
}

#[test]
fn data_dir_falls_back_to_current_dir() {
    assert_eq!(super::data_dir(|_| None), std::path::PathBuf::from("./.tupi"));
}

#[cfg(windows)]
#[test]
fn data_dir_prefers_userprofile_on_windows() {
    let dir = super::data_dir(|key| match key { "USERPROFILE" => Some("C:\\Users\\ana".into()), _ => Some("/home/ana".into()) });
    assert_eq!(dir, std::path::PathBuf::from("C:\\Users\\ana\\.tupi"));
}
```

- [ ] **Step 2: rodar e ver falhar**

`cargo test -p tupi` → falta `data_dir`.

- [ ] **Step 3: implementar e ligar**

`data_dir` usa `USERPROFILE` no Windows e `HOME` fora dele, com `get_env` injetado. Em `main`, abrir `Store::open(data_dir(|k| std::env::var(k).ok()).join("tupi.redb"))`; em erro `tracing::warn!` e `None`; passar para `App::with_store`.

- [ ] **Step 4: rodar e ver passar**

`cargo test -p tupi` → 2 testes (3 no Windows) passam; `cargo test` no workspace fica verde; `cargo clippy` e `cargo fmt --check` limpos.

