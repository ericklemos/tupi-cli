# 🧹 Janitor: remove módulos placeholder do SDD em `tupi-store` e `tupi`

## 💡 O que saiu

| Arquivo | Situação |
|---|---|
| `crates/tupi-store/src/sdd.rs` | Arquivo de 1 linha, só com o comentário "Removido: o SDD agora vive em .tupi/sdd" — nenhum item. |
| `crates/tupi-store/src/lib.rs:336` | Declaração `mod sdd;` do módulo acima. |
| `crates/tupi/src/sdd_store.rs` | Arquivo de 1 linha, idêntico, **sem `mod sdd_store;` em `main.rs`**: nunca foi compilado. |

## 🎯 Prova de que estava morto

- Os dois arquivos têm blake3 idêntico (`6ab0bebae0c3`) e conteúdo de exatamente uma
  linha de comentário: nenhum `pub`, nenhum símbolo, nada a importar.
- `crates/tupi-store/src/sdd.rs`: o módulo só podia ser alcançado por `crate::sdd`, e não
  existia nenhum item nele — nenhuma referência é possível.
- `crates/tupi/src/sdd_store.rs`: os `mod` de `main.rs` são `agents_config`, `config`,
  `docs_scaffold`, `lang`, `project_config`, `prompt_config`, `startup`, `tool_config`,
  `token`. `sdd_store` não está na lista, nem aparece no `map` (arquivos sem símbolos são
  omitidos) — arquivo órfão no disco.

## 📊 Impacto

- 2 arquivos removidos, 1 declaração `mod` removida.
- Sem mudança de comportamento: não havia código executável em nenhum dos dois.

## 🔬 Verificação

- `cargo check --workspace` → ok, 0 erros.
- `cargo clippy --workspace` → 0 erros, 35 avisos (mesma contagem de antes da limpeza).
- `cargo test -p tupi-store --all-targets` → 10 passed, 0 failed.
- `cargo test --workspace` compila tudo e roda a suíte; falhas restantes são pré-existentes
  e alheias a esta limpeza:
  - `tupi::prompt_config::tests::the_checked_in_file_is_valid` — prompt do arquivo divergiu
    do default (falta a linha do `/sdd <slug>`); nenhum dos arquivos envolvidos foi tocado.
  - `tupi-kernel` isolado não compila `#[tokio::test]` (falta a feature `macros` do tokio
    no Cargo.toml do crate; só passa por unificação no workspace).
  - `tupi-tools::tools::websearch::tests::temp_live_network_probe` — depende de rede.
  - `cargo fmt --all --check` já falhava em `crates/tupi/tests/release_probe.rs`.

## Observação para a próxima sessão

O grafo de código tem falso negativo de chamadores (`impact_check` disse 0 para
`MetaFile::from_plan`/`TasksFile::from_plan`, que `FsSddStore::save` usa). Toda decisão de
delete precisa de confirmação textual.
