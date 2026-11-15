# Relatório de limpeza — auditoria de dependências

Data: 2026-11-15 · Janitor

## Resultado da sessão

Nenhuma deleção. A varredura fechou todas as suspeitas abertas em `memory.md` e o único
candidato encontrado não pôde ser apagado porque a tool `patch` recusa hunks maiores que
~5 linhas (ver journal de 2026-11-15).

## Como foi provado que nada é peso morto

Método: remover a linha de `Cargo.toml`, rodar `cargo check --all-targets` e ler os E0433.

| Crate | Dependência | Chamador real |
|---|---|---|
| tupi-tools | `ron` | `tools/workflow/mod.rs:131` (`ron::from_str::<WorkflowRon>`) |
| tupi-tools | `streaming-iterator` | `support/graph/builder.rs:7` (`StreamingIterator::next` em `QueryMatches`) |
| tupi-tools | `regex` | `support/scanner.rs` (`regex::escape`) |
| tupi-tools | `libc` | `support/proc.rs` `kill_tree` (unix) |
| tupi-tools | `tempfile` | `support/paths.rs:146` (atomic_write), `tools/patch/mod.rs:234` |
| tupi-app | `blake3` | `telemetry.rs:953`, `app.rs:3317` |
| tupi-web | `petgraph` | `graph_api.rs:7-8` |
| tupi-web | `futures-util` | `events.rs:10` |

Nenhuma linha de `Cargo.toml` ficou alterada: árvore de trabalho limpa ao fim da sessão.

## Candidato para a próxima sessão

`AntigravityProvider::with_save_path` (crates/tupi-providers/src/antigravity.rs:590-593),
campo `save_path` (:564), inicialização (:579) e o ramo de `persist_credentials` (:604-606).

- Por que é morto: `assert used_outside` = NÃO; buscas por `with_save_path` só devolvem a
  própria definição; a persistência usada em produção é `with_on_refresh`, chamada por
  `App::register_antigravity_credentials`.
- Cascata: sem `save_path`, `AntigravityCredentials::save` fica só com o teste
  `credentials_file_save_and_load` (:1639) — decidir junto se o método sai e se o teste é
  removido ou reescrito sem ele.
- Deleção: fatiar em patches de ≤5 linhas (campo, init, método, ramo) e validar com
  `cargo check -p tupi-providers` a cada passo.
