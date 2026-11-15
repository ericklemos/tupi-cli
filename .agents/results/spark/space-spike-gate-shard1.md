# Spike space — gate denso↔lexical (shard 1/2)

- corpus: **976 símbolos**, 487 chars em média
- modelo: /home/desktop/.tupi/models/LiquidAI/LFM2.5-Embedding-350M-GGUF/LFM2.5-Embedding-350M-Q8_0.gguf
- embed: 198.8s
- RRF: denso peso 3

## Linguagem natural

| # | consulta | arquivo esperado | lexical | denso | RRF 3x | gate |
|---|---|---|---|---|---|---|
| 1 | onde a sessão lembra o que já leu | crates/tupi-tools/src/support/backtrack.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 2 | o que decide se um arquivo precisa ser relido | crates/tupi-tools/src/support/graph/parse_cache.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 3 | quem detecta que um trecho já foi entregue | crates/tupi-tools/src/support/ledger.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 4 | onde o modelo local de embeddings é carregado | crates/tupi-infer/src/llama.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 5 | onde o resumo de uma linha por documento é produzido | crates/tupi-tools/src/tools/docs/index.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 6 | quem grava o índice de documentação em disco | crates/tupi-tools/src/tools/docs/store.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 7 | onde resultados lexicais e vetoriais são fundidos | crates/tupi-tools/src/tools/docs/search.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 8 | quem ordena símbolos por centralidade | crates/tupi-tools/src/tools/map/mod.rs | 1 | 1 | 1 | 1 |
| 9 | como nomes parecidos são resolvidos entre arquivos | crates/tupi-tools/src/support/graph/scope.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 10 | quem monta o grafo de chamadas do projeto | crates/tupi-tools/src/support/graph/builder.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 11 | quem baixa e confere o hash do modelo | crates/tupi-infer/src/download.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 12 | onde o orçamento de contexto é decidido | crates/tupi-app/src/app.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 13 | onde um plano de implementação é dividido em subtarefas | crates/tupi-tools/src/tools/sdd/mod.rs | 1 | 1 | 1 | 1 |
| 14 | quem transforma um diff em símbolos afetados | crates/tupi-tools/src/tools/diff/mod.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 15 | como um agente retoma o trabalho da sessão anterior | crates/tupi-tools/src/tools/state/mod.rs | 6 | - | - | - |
| 16 | onde a memória do repositório é aplicada ao prompt | crates/tupi-app/src/memory.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 17 | quem mantém o índice de trechos de documentação | crates/tupi-tools/src/tools/docs/index.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 18 | quem decide quais ferramentas o agente enxerga | crates/tupi-kernel/src/tool.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 19 | como o idioma de um arquivo é reconhecido | crates/tupi/src/lang.rs | - | 1 | 1 | 1 |
| 20 | onde o texto de um arquivo é dividido em trechos de código | crates/tupi-tools/src/support/ast.rs | fora do shard | fora do shard | fora do shard | fora do shard |

## Com identificador

| # | consulta | arquivo esperado | lexical | denso | RRF 3x | gate |
|---|---|---|---|---|---|---|
| 1 | BacktrackGraph::visit_level | crates/tupi-tools/src/support/backtrack.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 2 | GraphCache::shared | crates/tupi-tools/src/support/graph/cache.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 3 | DocsIndex::refresh | crates/tupi-tools/src/tools/docs/index.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 4 | DeliveryLedger::check | crates/tupi-tools/src/support/ledger.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 5 | slice_lines | crates/tupi-tools/src/tools/docs/store.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 6 | ToolRegistry::get | crates/tupi-kernel/src/tool.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 7 | LlamaEmbedder::load | crates/tupi-infer/src/llama.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 8 | parse_context | crates/tupi/src/config.rs | 1 | 1 | 1 | 1 |

## Recall@5 por grupo

### Linguagem natural

- lexical: **2/4** (MRR 0.542)
- denso: **3/4** (MRR 0.750)
- RRF 3x: **3/4** (MRR 0.750)
- gate: **3/4** (MRR 0.750)

### Com identificador

- lexical: **1/1** (MRR 1.000)
- denso: **1/1** (MRR 1.000)
- RRF 3x: **1/1** (MRR 1.000)
- gate: **1/1** (MRR 1.000)

## Twins semânticos (top 20, 3.5s)

- **0.991** read (crates/tupi-tools/src/tools/read/ref_tests.rs) ↔ read (crates/tupi-tools/src/tools/read/tests.rs)
- **0.987** now (crates/tupi-tools/src/tools/sdd/mod.rs) ↔ now (crates/tupi-tools/src/tools/sdd/store.rs)
- **0.987** params (crates/tupi-tools/src/tools/edit/mod.rs) ↔ params (crates/tupi-tools/src/tools/search/mod.rs)
- **0.983** tests (crates/tupi-tools/src/tools/edit/mod.rs) ↔ tests (crates/tupi-tools/src/tools/exec/mod.rs)
- **0.981** tool_for (crates/tupi-tools/src/tools/read/ref_tests.rs) ↔ tool_for (crates/tupi-tools/src/tools/read/tests.rs)
- **0.980** tests (crates/tupi-tools/src/tools/edit/mod.rs) ↔ tests (crates/tupi-tools/src/tools/search/mod.rs)
- **0.979** render (crates/tupi-tools/src/tools/exec/mod.rs) ↔ render (crates/tupi-tools/src/tools/read/mod.rs)
- **0.978** tests (crates/tupi-tools/src/tools/exec/mod.rs) ↔ tests (crates/tupi-tools/src/tools/search/mod.rs)
- **0.977** render (crates/tupi-tools/src/tools/exec/mod.rs) ↔ render (crates/tupi-tools/src/tools/search/mod.rs)
- **0.976** tests (crates/tupi-tools/src/tools/sdd/render.rs) ↔ tests (crates/tupi-tools/src/tools/sdd/store.rs)
- **0.973** tests (crates/tupi-tools/src/tools/edit/mod.rs) ↔ tests (crates/tupi-tools/src/tools/read/mod.rs)
- **0.971** tests (crates/tupi-tools/src/tools/exec/mod.rs) ↔ tests (crates/tupi-tools/src/tools/read/mod.rs)
- **0.968** render (crates/tupi-tools/src/tools/read/mod.rs) ↔ render (crates/tupi-tools/src/tools/search/mod.rs)
- **0.967** EnvTool::sdd_allowed (crates/tupi-tools/src/tools/env/mod.rs) ↔ SddTool::sdd_allowed (crates/tupi-tools/src/tools/sdd/mod.rs)
- **0.965** tests (crates/tupi-tools/src/tools/read/mod.rs) ↔ tests (crates/tupi-tools/src/tools/search/mod.rs)
- **0.965** MapTool::sdd_allowed (crates/tupi-tools/src/tools/map/mod.rs) ↔ SddTool::sdd_allowed (crates/tupi-tools/src/tools/sdd/mod.rs)
- **0.961** SearchTool::sdd_allowed (crates/tupi-tools/src/tools/search/mod.rs) ↔ WebSearchTool::sdd_allowed (crates/tupi-tools/src/tools/websearch/mod.rs)
- **0.960** fixture (crates/tupi-tools/src/tools/edit/tests.rs) ↔ fixture (crates/tupi-tools/src/tools/read/tests.rs)
- **0.959** ReadTool::sdd_allowed (crates/tupi-tools/src/tools/read/mod.rs) ↔ SddTool::sdd_allowed (crates/tupi-tools/src/tools/sdd/mod.rs)
- **0.959** SddTool::sdd_allowed (crates/tupi-tools/src/tools/sdd/mod.rs) ↔ TraceTool::sdd_allowed (crates/tupi-tools/src/tools/trace/mod.rs)

_tempo total: 203.7s_
