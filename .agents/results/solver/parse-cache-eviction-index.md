# 🧠 Solver: índice de despejo do `ParseCache` deixa de crescer sem limite

## 💡 Problema

`ParseCache` (`crates/tupi-tools/src/support/graph/parse_cache.rs`) limita o total de **bytes** das ASTs, mas o índice que define a ordem de despejo era uma fila de marcadores preguiçosos:

```rust
order: VecDeque<(PathBuf, u64)>,   // (path, generation)
```

Cada `insert` empurrava um item em `order` e a remoção só acontecia dentro de:

```rust
while inner.bytes > self.max_bytes {
    let Some((old_path, old_generation)) = inner.order.pop_front() else { break };
    ...
}
```

Consequências:

1. **Crescimento sem limite.** Reinserir o mesmo `path` (o normal: cada rebuild do grafo reparseia todos os arquivos) deixa um marcador morto no `VecDeque`. Se o orçamento de bytes **não** for estourado — o caso comum — a fila nunca é varrida. O crescimento é `rebuilds × arquivos` de `PathBuf`, sem teto e invisível em `len()`/`is_empty()`.
2. **Pico de latência sob pressão.** Quando o limite finalmente estoura, o laço despeja de uma vez toda a pilha de marcadores mortos acumulada, tudo sob o `write()` do cache.

Ou seja: o limite de memória virava leak exatamente no regime em que o cache *não* estava sob pressão.

## 📐 Abordagem

O índice passa a ser um mapa ordenado chaveado pela `generation` (estritamente crescente), não uma fila de eventos:

```rust
/// Uma chave por entrada **viva**.
order: BTreeMap<u64, PathBuf>,
```

- `insert`: ao substituir um `path`, a chave antiga sai do índice junto com a entrada (`inner.order.remove(&old.generation)`); a nova entra em `order.insert(generation, path)`.
- `invalidate`: idem, libera a chave da entrada removida.
- despejo: `order.pop_first()` — a menor `generation` é a entrada mais antiga ainda viva.

Invariante que substitui a varredura de marcadores mortos:

> `inner.order.len() == inner.entries.len()`, e as chaves de `order` são exatamente as `generation`s vivas.

Com isso a checagem `entries.get(&old_path).is_some_and(|e| e.generation == old_generation)` do laço antigo deixa de existir: a chave *é* a geração viva. O despejo continua FIFO por inserção — a política do código original é preservada.

## 🔄 Concorrência

Nenhuma primitiva nova; o `RwLock<Inner>` continua sendo o único ponto de sincronização. O que muda é o tempo de posse do `write()`: o laço de despejo deixa de poder varrer N marcadores mortos e passa a fazer no máximo um `pop_first()` + `HashMap::remove` por entrada despejada.

## 📊 Complexidade

| Operação | Antes | Depois |
|---|---|---|
| `insert` (fila/índice) | O(1) amortizado, mas deixa lixo permanente | O(log n) |
| `insert` com despejo | O(n) no pior caso (varredura de lixo) | O(k · log n), k = entradas despejadas |
| `invalidate` (índice) | O(1) e deixava lixo | O(log n) |
| Espaço do índice | O(parses) — **sem teto** | O(entradas vivas) |
| Espaço total | O(entradas) + lixo crescente | O(entradas) |

`n` = arquivos no cache. O custo logarítmico é irrelevante: reparsear um arquivo com tree-sitter é ordens de magnitude mais caro.

## ✅ Verificação

- `cargo test -p tupi-tools` → **379 + integrações, 0 falhas**.
- Novos testes (3) em `parse_cache.rs`:
  - `reparsing_the_same_path_does_not_grow_the_eviction_index` — 64 reinserções do mesmo `path` com orçamento folgado; afirma `len() == 1` **e** `order_len() == 1` (antes: 64).
  - `the_oldest_entry_leaves_first_when_the_budget_is_exceeded` — política FIFO preservada; só a entrada mais antiga sai.
  - `invalidating_a_path_releases_its_index_slot` — `invalidate` zera entrada e chave.
- `cargo clippy -p tupi-tools` e `-p tupi-app`: sem avisos novos no arquivo alterado.
- `order_len()` é `#[cfg(test)]`: métrica de invariante sem custo em produção.

## 🧹 Nota de honestidade

A sessão começou por outro alvo: um suposto *lost wakeup* no laço de `App::cancel` que espera as tools de `spawn_blocking`. O teste escrito para provar o bug **falhou**, mostrando que `tokio::sync::Notify::notified()` fotografa o contador de `notify_waiters` na criação e, portanto, o laço original já era correto. A mudança foi revertida por completo (`crates/tupi-app/src/app.rs` está intacto) e a lição foi para o journal; o trabalho entregue é este, no `ParseCache`.
