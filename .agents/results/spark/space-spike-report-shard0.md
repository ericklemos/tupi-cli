# Spike space — recall@5 por arquivo (shard 0/2)

- corpus: **1310 símbolos**, 466 chars em média
- modelo: /home/desktop/.tupi/models/LiquidAI/LFM2.5-Embedding-350M-GGUF/LFM2.5-Embedding-350M-Q8_0.gguf
- embed: 251.7s

| # | pergunta | arquivo esperado | lexical | denso | híbrido |
|---|---|---|---|---|---|
| 1 | onde a sessão lembra o que já leu | crates/tupi-tools/src/support/backtrack.rs | 9 | 7 | 3 |
| 2 | o que decide se um arquivo precisa ser relido | crates/tupi-tools/src/support/graph/parse_cache.rs | - | - | - |
| 3 | quem detecta que um trecho já foi entregue | crates/tupi-tools/src/support/ledger.rs | 1 | 1 | 1 |
| 4 | onde o modelo local de embeddings é carregado | crates/tupi-infer/src/llama.rs | - | 4 | 7 |
| 5 | onde o resumo de uma linha por documento é produzido | crates/tupi-tools/src/tools/docs/index.rs | - | 4 | 7 |
| 6 | quem grava o índice de documentação em disco | crates/tupi-tools/src/tools/docs/store.rs | - | - | - |
| 7 | onde resultados lexicais e vetoriais são fundidos | crates/tupi-tools/src/tools/docs/search.rs | - | 5 | 10 |
| 8 | quem ordena símbolos por centralidade | crates/tupi-tools/src/tools/map/mod.rs | fora do shard | fora do shard | fora do shard |
| 9 | como nomes parecidos são resolvidos entre arquivos | crates/tupi-tools/src/support/graph/scope.rs | - | 4 | 7 |
| 10 | quem monta o grafo de chamadas do projeto | crates/tupi-tools/src/support/graph/builder.rs | - | 5 | 9 |
| 11 | quem baixa e confere o hash do modelo | crates/tupi-infer/src/download.rs | - | 1 | 1 |
| 12 | onde o orçamento de contexto é decidido | crates/tupi-app/src/app.rs | 5 | 3 | 4 |
| 13 | onde um plano de implementação é dividido em subtarefas | crates/tupi-tools/src/tools/sdd/mod.rs | fora do shard | fora do shard | fora do shard |
| 14 | quem transforma um diff em símbolos afetados | crates/tupi-tools/src/tools/diff/mod.rs | 1 | 2 | 1 |
| 15 | como um agente retoma o trabalho da sessão anterior | crates/tupi-tools/src/tools/state/mod.rs | fora do shard | fora do shard | fora do shard |
| 16 | onde a memória do repositório é aplicada ao prompt | crates/tupi-app/src/memory.rs | - | 2 | 5 |
| 17 | quem mantém o índice de trechos de documentação | crates/tupi-tools/src/tools/docs/index.rs | - | 1 | 2 |
| 18 | quem decide quais ferramentas o agente enxerga | crates/tupi-kernel/src/tool.rs | - | 1 | 3 |
| 19 | como o idioma de um arquivo é reconhecido | crates/tupi/src/lang.rs | fora do shard | fora do shard | fora do shard |
| 20 | onde o texto de um arquivo é dividido em trechos de código | crates/tupi-tools/src/support/ast.rs | - | - | - |

## Recall@5

- lexical: **3/16**
- denso: **12/16**
- híbrido: **8/16**

## MRR@10

- lexical: **0.144**
- denso: **0.414**
- híbrido: **0.329**

## Twins semânticos (top 20, 7.0s)

- **0.981** terms (crates/tupi-tools/src/support/mod.rs) ↔ terms (crates/tupi-tools/src/tools/docs/mod.rs)
- **0.980** hash_prefix (crates/tupi-tools/src/support/paths.rs) ↔ hash_prefix (crates/tupi-tools/src/support/span.rs)
- **0.979** store (crates/tupi-tools/src/support/mod.rs) ↔ store (crates/tupi-tools/src/tools/docs/mod.rs)
- **0.975** tests::text_event (crates/tupi-app/src/app.rs) ↔ tests::text_event (crates/tupi-app/src/sdd_run.rs)
- **0.969** tests::ScriptedProvider::list_models (crates/tupi-app/src/app.rs) ↔ tests::Scripted::list_models (crates/tupi-app/src/sdd_run.rs)
- **0.969** memory (crates/tupi-app/src/lib.rs) ↔ memory (crates/tupi-domain/src/lib.rs)
- **0.967** tests::write (crates/tupi-tools/src/tools/assert/mod.rs) ↔ tests::write (crates/tupi-tools/src/tools/docs/index.rs)
- **0.966** hash_matches (crates/tupi-tools/src/support/paths.rs) ↔ hash_matches (crates/tupi-tools/src/support/span.rs)
- **0.966** workflow (crates/tupi-domain/src/lib.rs) ↔ workflow (crates/tupi-kernel/src/lib.rs)
- **0.963** tests::impl EchoTool (crates/tupi-app/src/sdd_run.rs) ↔ tests::impl EchoTool (crates/tupi-kernel/src/tool.rs)
- **0.963** tests::impl EchoTool@2682 (crates/tupi-app/src/app.rs) ↔ tests::impl EchoTool (crates/tupi-app/src/sdd_run.rs)
- **0.963** normalize (crates/tupi-tools/src/support/paths.rs) ↔ normalize (crates/tupi-tools/src/support/scanner.rs)
- **0.962** tool (crates/tupi-kernel/src/lib.rs) ↔ tools (crates/tupi-tools/src/lib.rs)
- **0.962** tests::write (crates/tupi-tools/src/tools/context_graph/mod.rs) ↔ tests::write (crates/tupi-tools/src/tools/docs/index.rs)
- **0.962** sdd (crates/tupi-app/src/lib.rs) ↔ sdd (crates/tupi-store/src/lib.rs)
- **0.961** tests::write (crates/tupi-tools/src/tools/assert/mod.rs) ↔ tests::write (crates/tupi-tools/src/tools/context_graph/mod.rs)
- **0.958** pipeline (crates/tupi-app/src/lib.rs) ↔ pipeline (crates/tupi-kernel/src/lib.rs)
- **0.958** tests::EchoTool::spec (crates/tupi-app/src/app.rs) ↔ tests::EchoTool::spec (crates/tupi-app/src/sdd_run.rs)
- **0.958** tests::EchoTool::spec (crates/tupi-app/src/sdd_run.rs) ↔ tests::EchoTool::spec (crates/tupi-kernel/src/tool.rs)
- **0.956** tests::ScriptedProvider::stream_chat (crates/tupi-app/src/app.rs) ↔ tests::Scripted::stream_chat (crates/tupi-app/src/sdd_run.rs)

_tempo total: 260.8s_
