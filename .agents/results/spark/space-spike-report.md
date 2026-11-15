# Spike space — recall@5 lexical vs. denso vs. híbrido

- corpus: **60 símbolos** (max=60)
- modelo: /home/desktop/.tupi/models/LiquidAI/LFM2.5-Embedding-350M-GGUF/LFM2.5-Embedding-350M-Q8_0.gguf
- embed: 13.1s

| # | pergunta | gabarito | lexical | denso | híbrido |
|---|---|---|---|---|---|
| 1 | onde a sessão lembra o que já leu | BacktrackGraph | - | - | - |
| 2 | o que decide se um arquivo precisa ser relido | ParseCache | - | - | - |
| 3 | quem detecta que um trecho já foi entregue | DeliveryLedger | - | - | - |
| 4 | onde o modelo local de embeddings é carregado | LlamaEmbedder | - | 2 | 3 |
| 5 | onde o resumo de uma linha por documento é produzido | process_one_summary | - | 1 | 1 |
| 6 | quem grava o índice de documentação em disco | save_index | - | 4 | 8 |
| 7 | onde resultados lexicais e vetoriais são fundidos | fuse | - | 1 | 1 |
| 8 | quem ordena símbolos por centralidade | MapTool | - | - | - |
| 9 | como nomes parecidos são resolvidos entre arquivos | ScopeResolver | - | 1 | 1 |
| 10 | quem monta o grafo de chamadas do projeto | GraphBuilder | - | 3 | 5 |
| 11 | quem baixa e confere o hash do modelo | ensure_model | - | 1 | 2 |
| 12 | onde o orçamento de contexto é decidido | ContextProfile | - | 4 | 8 |
| 13 | onde um plano de implementação é dividido em subtarefas | SddTool | - | - | - |
| 14 | quem transforma um diff em símbolos afetados | DiffTool | 10 | 3 | 2 |
| 15 | como um agente retoma o trabalho da sessão anterior | StateTool | - | - | - |
| 16 | onde a memória do repositório é aplicada ao prompt | Memory | - | 1 | 1 |
| 17 | quem mantém o índice de trechos de documentação | DocsIndex | - | 1 | 2 |
| 18 | quem decide quais ferramentas o agente enxerga | ToolRegistry | - | 4 | 7 |
| 19 | como o idioma de um arquivo é reconhecido | Lang | - | 1 | 1 |
| 20 | onde o texto de um arquivo é dividido em trechos de código | snippets_at_lines | - | 2 | 4 |

## Recall@5

- lexical: **0/20**
- denso: **14/20**
- híbrido: **11/20**

## MRR@10

- lexical: **0.005**
- denso: **0.471**
- híbrido: **0.384**

## Twins semânticos (top 20, 0.0s)

- **0.910** StateTool (crates/tupi-tools/src/tools/state/mod.rs) ↔ MapTool (crates/tupi-tools/src/tools/map/mod.rs)
- **0.882** DiffTool (crates/tupi-tools/src/tools/diff/mod.rs) ↔ MapTool (crates/tupi-tools/src/tools/map/mod.rs)
- **0.859** StateTool (crates/tupi-tools/src/tools/state/mod.rs) ↔ SddTool (crates/tupi-tools/src/tools/sdd/mod.rs)
- **0.858** StateTool (crates/tupi-tools/src/tools/state/mod.rs) ↔ DiffTool (crates/tupi-tools/src/tools/diff/mod.rs)
- **0.857** SddTool (crates/tupi-tools/src/tools/sdd/mod.rs) ↔ MapTool (crates/tupi-tools/src/tools/map/mod.rs)
- **0.849** SddTool (crates/tupi-tools/src/tools/sdd/mod.rs) ↔ DiffTool (crates/tupi-tools/src/tools/diff/mod.rs)
- **0.848** ToolRegistry (crates/tupi-kernel/src/tool.rs) ↔ MapTool (crates/tupi-tools/src/tools/map/mod.rs)
- **0.827** ToolRegistry (crates/tupi-kernel/src/tool.rs) ↔ StateTool (crates/tupi-tools/src/tools/state/mod.rs)
- **0.824** ParseCache (crates/tupi-tools/src/support/graph/parse_cache.rs) ↔ GraphBuilder (crates/tupi-tools/src/support/graph/builder.rs)
- **0.820** ToolRegistry (crates/tupi-kernel/src/tool.rs) ↔ SddTool (crates/tupi-tools/src/tools/sdd/mod.rs)
- **0.812** DocsIndex (crates/tupi-tools/src/tools/docs/index.rs) ↔ save_index (crates/tupi-tools/src/tools/docs/store.rs)
- **0.810** ToolRegistry (crates/tupi-kernel/src/tool.rs) ↔ DiffTool (crates/tupi-tools/src/tools/diff/mod.rs)
- **0.787** BacktrackGraph (crates/tupi-tools/src/support/backtrack.rs) ↔ GraphBuilder (crates/tupi-tools/src/support/graph/builder.rs)
- **0.785** MapTool (crates/tupi-tools/src/tools/map/mod.rs) ↔ GraphBuilder (crates/tupi-tools/src/support/graph/builder.rs)
- **0.782** BacktrackGraph (crates/tupi-tools/src/support/backtrack.rs) ↔ ParseCache (crates/tupi-tools/src/support/graph/parse_cache.rs)
- **0.782** ToolRegistry (crates/tupi-kernel/src/tool.rs) ↔ GraphBuilder (crates/tupi-tools/src/support/graph/builder.rs)
- **0.780** ToolRegistry (crates/tupi-kernel/src/tool.rs) ↔ ParseCache (crates/tupi-tools/src/support/graph/parse_cache.rs)
- **0.769** ToolRegistry (crates/tupi-kernel/src/tool.rs) ↔ DocsIndex (crates/tupi-tools/src/tools/docs/index.rs)
- **0.751** MapTool (crates/tupi-tools/src/tools/map/mod.rs) ↔ ParseCache (crates/tupi-tools/src/support/graph/parse_cache.rs)
- **0.751** DocsIndex (crates/tupi-tools/src/tools/docs/index.rs) ↔ MapTool (crates/tupi-tools/src/tools/map/mod.rs)

_tempo total: 20.1s_
