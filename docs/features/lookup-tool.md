# `lookup(ops)` — um lote, um orçamento, um contrato

`lookup` substitui `search` e `read_many` como tool exposta: em vez de uma
chamada por intenção, o modelo manda um **lote heterogêneo** de operações e
recebe uma saída densa única, com cap por item, corte anunciado e dedupe por
`DeliveryLedger`.

Fonte: `crates/tupi-tools/src/tools/lookup/{params,exec,render}.rs`,
`crates/tupi-tools/src/support/dense.rs`.

## Ops

| `kind` | campos obrigatórios | devolve |
| --- | --- | --- |
| `symbol` | `path`, `symbol` | corpo do símbolo (esqueleto quando `skeleton`) |
| `skeleton` | `path` | esqueleto do arquivo com linhas |
| `lines` | `path` (+`start`/`end`, `tail`, `head`) | faixa numerada, `N de M linhas` no topo |
| `text` | `text` | `caminho:linha:texto` agrupado por arquivo |
| `semantic` | `query` | busca densa de símbolos (a antiga `search`) |
| `graph` | `symbol` | quem chama (delega ao `context_graph`) |

Campos de refinamento: `rev` (lê `rev:path`), `alternatives` (outros literais na
mesma varredura de `text`), `glob`/`exclude` (filtros de caminho em `text`),
`keywords`, `detail` (`auto`/`full`/`signatures`), `max_results`, `depth`,
`budget_tokens` e `repeat: allow`.

`mode` é atalho de op única: o `kind` vai em `mode` e os campos da op na raiz.
Não combine com `ops`.

## Contrato de saída

- **Um orçamento por chamada**: `budget_tokens` do lote inteiro passa por
  `budget::select`/`budget::render`; o excedente sai em `handle` e volta por
  `peek`. Toda mensagem de corte nomeia `peek`.
- **Cap por item e honestidade**: nenhuma op devolve lista sem declarar quantos
  omitiu (`N de M`, `(de M linhas)`). `text` corta em `max_results` (padrão 80)
  e diz `N de M casamentos`.
- **Erro é por item**: op com problema vira seção de topo (priority 255) e o
  resto do lote continua. Arquivo ausente deixa de derrubar as outras leituras —
  a diferença central em relação ao `read_many`.
- **Dedupe**: seção já entregue nesta sessão vira ponteiro para o corpo anterior;
  `repeat: "allow"` força o corpo de novo.
- **Fronteira no topo**: o cabeçalho mostra quantos símbolos da cadeia ainda
  estão `Frontier` (`BacktrackGraph::frontier_count`), sinal que o `search` já
  calculava e não expunha.

## Recusas instrutivas

- `regex: true` em `text` — não existe casamento por expressão regular. A recusa
  nomeia as alternativas: `alternatives` (lista de literais) ou `kind: semantic`.
- Op sem o campo do seu `kind` — a recusa nomeia o campo que faltou.
- Alvo repetido sem `repeat` — a recusa aponta o corpo anterior.

## Exposição e schema

`lookup` entra em `CORE_TOOLS`, em `register_with`, no subagente
(`register_agent`) e no agente de docs; `ModeMask` cobre `EXECUTION ∪ SDD`,
com `lookup` na lista de tools do planejamento (`configs/modes/sdd.ron`).
`read_many` e `search` deixaram de ser expostas — continuam existindo como
estrutura interna (`ReadTool`, `SearchTool`) que o `lookup` reusa.

Números de schema por passo (`tool_schema_tokens`):

| exposição | antes | depois |
| --- | --- | --- |
| núcleo | 12 tools · 4.506 | **11 tools · 4.468** |
| `expose_all` | 30 tools · 10.651 | **29 tools · 10.613** |

A fusão devolve 38 tokens em cada exposição e elimina duas tools do payload: o
`lookup` custa menos que a soma de `search` + `read_many` + os campos que eles
declaravam. O próximo passo de schema é cortar campo, não juntar tool.

## Provas

- `crates/tupi-tools/tests/lookup.rs` — `text` agrupado, cap declarado,
  `glob`/`exclude`/`alternatives`, `lines` com `tail` e faixa, item recusado sem
  derrubar o lote, `regex` recusado com a alternativa.
- `crates/tupi-tools/src/tools/lookup/render.rs` — ordem por prioridade,
  envelope com handle e dedupe.
- `crates/tupi-tools/tests/tool_schema_tokens.rs` — a tabela acima.

## O que ficou fora

- Digest por modelo local: se voltar, sai com âncoras `caminho:linha` e handle do
  corpo, nunca resumo sozinho.
- `kind: graph` hoje delega ao `context_graph`; a fusão de grafos em ops próprias
  não foi feita.
- Corte de `expose_all` continua rejeitado ([ADR 0005](../decisions/0005-expose-all.md)).
