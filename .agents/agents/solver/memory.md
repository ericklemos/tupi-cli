# Solver — memória

## Problemas resolvidos

- **ParseCache: índice de despejo sem limite** (`crates/tupi-tools/src/support/graph/parse_cache.rs`) — `VecDeque<(PathBuf, u64)>` com marcadores mortos só varridos sob pressão de bytes crescia um `PathBuf` por parse. Trocado por `BTreeMap<u64, PathBuf>` chaveado pela `generation`; invariante `order.len() == entries.len()`. Testes: `reparsing_the_same_path_does_not_grow_the_eviction_index`, `the_oldest_entry_leaves_first_when_the_budget_is_exceeded`, `invalidating_a_path_releases_its_index_slot`.

## Riscos e armadilhas conhecidas

- `App::cancel` espera `spawn_blocking` esvaziar via `Notify` + `AtomicUsize` (`run_tool` em `crates/tupi-app/src/app.rs`). **Não** é lost wakeup: `tokio::sync::Notify::notified()` fotografa o contador de `notify_waiters` na criação. A ordem "criar futura → checar contador → await" é obrigatória; invertê-la cria o bug de verdade.
- `ParseCache::insert` conta bytes como `tree.root_node().end_byte()` (tamanho do fonte), não memória real da árvore: o teto `DEFAULT_MAX_BYTES` subestima o consumo de ASTs grandes.
- `GraphCache::repair` invalida e reconstrói o grafo inteiro quando qualquer hash muda — cada rebuild reparseia todos os arquivos (é o multiplicador que expôs o leak do índice).

## Débito algorítmico identificado (não atacado)

- `GraphCache::repair` é tudo-ou-nada por rebuild; reparo incremental por arquivo evitaria o reparse completo.
