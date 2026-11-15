# Janitor — registro de peso morto

## Já limpo

- 2026-02-14 — `crates/tupi-store/src/sdd.rs` (placeholder vazio) + `mod sdd;` em
  `crates/tupi-store/src/lib.rs`; `crates/tupi/src/sdd_store.rs` (órfão, nunca compilado).
- 2026-02-14 — `crates/tupi-store/src/lib.rs`: auditado inteiro (Store, CachedModels,
  `is_fresh`, `MODELS_TTL_SECS`, `connected_providers`) — tudo em uso.
- 2026-02-14 — `crates/tupi-tools/src/tools/sdd/meta.rs`: auditado — `MetaFile`/`TasksFile`
  são usados por `FsSddStore::save`/`load`. Não mexer.

## Dependências: auditadas por compilação (2026-11-15) — não reauditar

Método: removidas do `Cargo.toml` e provadas com `cargo check --all-targets`.

- `tupi-tools`: `ron` (workflow/mod.rs:131), `streaming-iterator`
  (support/graph/builder.rs:7), `regex` (support/scanner.rs `regex::escape`), `libc`
  (support/proc.rs `kill_tree` unix), `tempfile` (support/paths.rs:146 e
  tools/patch/mod.rs:234 — é dependência de LIB, não só de teste). Todas em uso.
- `tupi-app`: `blake3` em uso (telemetry.rs:953, app.rs:3317).
- `tupi-web`: `petgraph` (graph_api.rs:7-8) e `futures-util` (events.rs:10) em uso.
- Conclusão: nenhuma dependência do workspace é removível hoje.

## Candidato identificado (não confirmado, não apagado)

- `AntigravityProvider::with_save_path` (crates/tupi-providers/src/antigravity.rs:590) +
  campo `save_path` + ramo de `persist_credentials`: nenhum chamador no repo. A persistência
  viva é `with_on_refresh` (usada por `App::register_antigravity_credentials`). O 
  `credentials_file_save_and_load` (:1639) ainda exercita `AntigravityCredentials::save`.
  Ficou pendente porque a tool `patch` não aplicou hunks desse tamanho.

## Áreas com suspeita de acúmulo (não confirmadas)

- `crates/tupi-tools/src/space/*` — `SpaceIndex` não é usado fora de tupi-tools/src, mas a
  feature é configurável (`space_enabled`) e há spike em `crates/tupi/tests/space_spike.rs`.
  Não é peso morto provado. (Componente grande: perguntar antes.)

## Falhas de teste pré-existentes (não são regressão)

- `crates/tupi/src/prompt_config.rs::the_checked_in_file_is_valid` — prompt do arquivo
  divergiu do default (falta a linha do `/sdd <slug>`).
- `cargo test -p tupi-kernel` não compila: `#[tokio::test]` sem a feature `macros` no
  tokio do crate (só funciona por unificação dentro do workspace).
- `tupi-tools` `tools::websearch::tests::temp_live_network_probe` — depende de rede.
- `cargo fmt --all --check` já falhava em `crates/tupi/tests/release_probe.rs`.
