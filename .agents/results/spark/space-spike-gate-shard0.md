# Spike space — gate denso↔lexical (shard 0/2)

- corpus: **1314 símbolos**, 466 chars em média
- modelo: /home/desktop/.tupi/models/LiquidAI/LFM2.5-Embedding-350M-GGUF/LFM2.5-Embedding-350M-Q8_0.gguf
- embed: 252.8s
- RRF: denso peso 3

## Linguagem natural

| # | consulta | arquivo esperado | lexical | denso | RRF 3x | gate |
|---|---|---|---|---|---|---|
| 1 | onde a sessão lembra o que já leu | crates/tupi-tools/src/support/backtrack.rs | 9 | 7 | 3 | 7 |
| 2 | o que decide se um arquivo precisa ser relido | crates/tupi-tools/src/support/graph/parse_cache.rs | - | - | - | - |
| 3 | quem detecta que um trecho já foi entregue | crates/tupi-tools/src/support/ledger.rs | 1 | 1 | 1 | 1 |
| 4 | onde o modelo local de embeddings é carregado | crates/tupi-infer/src/llama.rs | - | 4 | 4 | 4 |
| 5 | onde o resumo de uma linha por documento é produzido | crates/tupi-tools/src/tools/docs/index.rs | - | 4 | 4 | 4 |
| 6 | quem grava o índice de documentação em disco | crates/tupi-tools/src/tools/docs/store.rs | - | - | - | - |
| 7 | onde resultados lexicais e vetoriais são fundidos | crates/tupi-tools/src/tools/docs/search.rs | - | 5 | 5 | 5 |
| 8 | quem ordena símbolos por centralidade | crates/tupi-tools/src/tools/map/mod.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 9 | como nomes parecidos são resolvidos entre arquivos | crates/tupi-tools/src/support/graph/scope.rs | - | 4 | 4 | 4 |
| 10 | quem monta o grafo de chamadas do projeto | crates/tupi-tools/src/support/graph/builder.rs | - | 5 | 5 | 5 |
| 11 | quem baixa e confere o hash do modelo | crates/tupi-infer/src/download.rs | - | 1 | 1 | 1 |
| 12 | onde o orçamento de contexto é decidido | crates/tupi-app/src/app.rs | 5 | 3 | 3 | 3 |
| 13 | onde um plano de implementação é dividido em subtarefas | crates/tupi-tools/src/tools/sdd/mod.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 14 | quem transforma um diff em símbolos afetados | crates/tupi-tools/src/tools/diff/mod.rs | 1 | 2 | 1 | 2 |
| 15 | como um agente retoma o trabalho da sessão anterior | crates/tupi-tools/src/tools/state/mod.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 16 | onde a memória do repositório é aplicada ao prompt | crates/tupi-app/src/memory.rs | - | 2 | 4 | 2 |
| 17 | quem mantém o índice de trechos de documentação | crates/tupi-tools/src/tools/docs/index.rs | - | 1 | 2 | 1 |
| 18 | quem decide quais ferramentas o agente enxerga | crates/tupi-kernel/src/tool.rs | - | 1 | 2 | 1 |
| 19 | como o idioma de um arquivo é reconhecido | crates/tupi/src/lang.rs | fora do shard | fora do shard | fora do shard | fora do shard |
| 20 | onde o texto de um arquivo é dividido em trechos de código | crates/tupi-tools/src/support/ast.rs | - | - | - | - |

## Com identificador

| # | consulta | arquivo esperado | lexical | denso | RRF 3x | gate |
|---|---|---|---|---|---|---|
| 1 | BacktrackGraph::visit_level | crates/tupi-tools/src/support/backtrack.rs | 1 | 1 | 1 | 1 |
| 2 | GraphCache::shared | crates/tupi-tools/src/support/graph/cache.rs | 1 | 1 | 1 | 1 |
| 3 | DocsIndex::refresh | crates/tupi-tools/src/tools/docs/index.rs | 3 | 1 | 1 | 1 |
| 4 | DeliveryLedger::check | crates/tupi-tools/src/support/ledger.rs | 1 | 1 | 1 | 1 |
| 5 | slice_lines | crates/tupi-tools/src/tools/docs/store.rs | 1 | 1 | 1 | 1 |
| 6 | ToolRegistry::get | crates/tupi-kernel/src/tool.rs | 10 | 1 | 1 | 1 |
| 7 | LlamaEmbedder::load | crates/tupi-infer/src/llama.rs | 1 | 1 | 1 | 1 |
| 8 | parse_context | crates/tupi/src/config.rs | fora do shard | fora do shard | fora do shard | fora do shard |

## Recall@5 por grupo

### Linguagem natural

- lexical: **3/16** (MRR 0.144)
- denso: **12/16** (MRR 0.414)
- RRF 3x: **13/16** (MRR 0.379)
- gate: **12/16** (MRR 0.414)

### Com identificador

- lexical: **6/7** (MRR 0.776)
- denso: **7/7** (MRR 1.000)
- RRF 3x: **7/7** (MRR 1.000)
- gate: **7/7** (MRR 1.000)

## Twins semânticos (top 20, 5.4s)

- **0.981** terms (crates/tupi-tools/src/support/mod.rs) ↔ terms (crates/tupi-tools/src/tools/docs/mod.rs)
- **0.980** hash_prefix (crates/tupi-tools/src/support/paths.rs) ↔ hash_prefix (crates/tupi-tools/src/support/span.rs)
- **0.979** store (crates/tupi-tools/src/support/mod.rs) ↔ store (crates/tupi-tools/src/tools/docs/mod.rs)
- **0.969** memory (crates/tupi-app/src/lib.rs) ↔ memory (crates/tupi-domain/src/lib.rs)
- **0.966** hash_matches (crates/tupi-tools/src/support/paths.rs) ↔ hash_matches (crates/tupi-tools/src/support/span.rs)
- **0.966** workflow (crates/tupi-domain/src/lib.rs) ↔ workflow (crates/tupi-kernel/src/lib.rs)
- **0.963** normalize (crates/tupi-tools/src/support/paths.rs) ↔ normalize (crates/tupi-tools/src/support/scanner.rs)
- **0.962** tool (crates/tupi-kernel/src/lib.rs) ↔ tools (crates/tupi-tools/src/lib.rs)
- **0.962** sdd (crates/tupi-app/src/lib.rs) ↔ sdd (crates/tupi-store/src/lib.rs)
- **0.958** pipeline (crates/tupi-app/src/lib.rs) ↔ pipeline (crates/tupi-kernel/src/lib.rs)
- **0.952** tests (crates/tupi-tools/src/tools/docs/index.rs) ↔ tests (crates/tupi-tools/src/tools/docs/mod.rs)
- **0.949** SddExecOutcome (crates/tupi-app/src/sdd_api.rs) ↔ SddRunOutcome (crates/tupi-app/src/sdd_run.rs)
- **0.945** tests (crates/tupi-tools/src/tools/assert/mod.rs) ↔ tests (crates/tupi-tools/src/tools/context_graph/mod.rs)
- **0.942** ContextTool::sdd_allowed (crates/tupi-tools/src/tools/context_graph/mod.rs) ↔ DocsTool::sdd_allowed (crates/tupi-tools/src/tools/docs/mod.rs)
- **0.941** tests (crates/tupi-domain/src/context.rs) ↔ tests (crates/tupi-domain/src/lib.rs)
- **0.940** workflow (crates/tupi-app/src/lib.rs) ↔ workflow (crates/tupi-kernel/src/lib.rs)
- **0.939** AssertTool::sdd_allowed (crates/tupi-tools/src/tools/assert/mod.rs) ↔ DocsTool::sdd_allowed (crates/tupi-tools/src/tools/docs/mod.rs)
- **0.938** AssertTool::sdd_allowed (crates/tupi-tools/src/tools/assert/mod.rs) ↔ ContextTool::sdd_allowed (crates/tupi-tools/src/tools/context_graph/mod.rs)
- **0.938** url_encode (crates/tupi-providers/src/antigravity.rs) ↔ url_encode (crates/tupi-tools/src/support/web.rs)
- **0.937** workflow (crates/tupi-app/src/lib.rs) ↔ workflow (crates/tupi-domain/src/lib.rs)

_tempo total: 259.7s_
