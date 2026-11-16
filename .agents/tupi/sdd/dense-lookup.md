<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Saída densa canônica e tool `lookup(ops)` — fundir read/read_many/search (`dense-lookup`)

**Status:** closed

## Objetivo

Criar um contrato de saída denso único (seções com prioridade, um orçamento, envelope com handle para `peek`, dedupe por `DeliveryLedger`) e fundir `read`, `read_many` e `search` numa tool `lookup` com `ops: [...]` heterogêneo (`symbol`, `skeleton`, `lines`, `text`, `semantic`, `graph`), absorvendo o plano draft `native-search`. A economia vem de cap por item + honestidade do corte + ledger, não do número de chamadas. Escopo: (a) helper denso + migração das tools de lista e (b) fusão. Fora de escopo: digest via modelo local (`GeneratorSlot`) e qualquer corte de `expose_all`. Fontes: crates/tupi-tools/src/support/budget.rs:16-27, :64-119, :198-223; crates/tupi-tools/src/support/ledger.rs:110-267; crates/tupi-tools/src/tools/read/render.rs:268-289, :390-403; crates/tupi-tools/src/tools/read/mod.rs:35-43, :177-206; crates/tupi-tools/src/tools/read_many/mod.rs:42-181, :185-229; crates/tupi-tools/src/tools/search/mod.rs:28-31, :121-167; crates/tupi-tools/src/tools/search/query.rs:7-321, :323-381; crates/tupi-tools/src/tools/search/render.rs:3-35; crates/tupi-tools/src/tools/search/params.rs:5-30; crates/tupi-tools/src/tools/peek/mod.rs:34-69; .tupi/sdd/native-search/tasks.json; crates/tupi-tools/tests/tool_schema_tokens.rs:30-45.

## Restrições

- `expose_all: true` não muda: ADR 0005 rejeita cortar exposição (prefixo cacheado, não round trip). Prioridade só por ordem de registro, descrição e recusa instrutiva. Fonte: docs/decisions/0005-expose-all.md:1-74; configs/tools.ron:1-9.
- Toda op nova recusa, ensina a alternativa e cabe em `budget_tokens` (ADR 0004, regras 1-3). Fonte: docs/decisions/0004-controle-por-tool.md:1-142.
- Um orçamento por chamada: `budget::select` + `budget::render` com handle do `ResponseStore`, excedente recuperável por `peek`. Fonte: crates/tupi-tools/src/support/budget.rs:64-119, :198-223; crates/tupi-tools/src/tools/peek/mod.rs:34-69.
- Dedupe obrigatório por item via `DeliveryLedger` (`check`/`remember`), como `read` e `search` fazem hoje. Fonte: crates/tupi-tools/src/support/ledger.rs:110-267; crates/tupi-tools/src/tools/search/query.rs:323-381.
- Cap por item e linha de completude obrigatórios: nenhuma op devolve lista sem declarar quantos omitiu. Fonte: crates/tupi-tools/src/tools/search/mod.rs:28-31; crates/tupi-tools/src/tools/search/render.rs:19-35.
- Erro é por item, não do lote: item com problema vira seção de topo com priority 255 (padrão `with_notice`), e o que deu certo continua na saída. Fonte: crates/tupi-tools/src/tools/read_many/mod.rs:150-180.
- Sem dependência nova: reusar `LexicalScanner`, o walk de `exec/render.rs:56-137` e a fusão densa existente sem alterar `space`/`fusion`. Fonte: .tupi/sdd/native-search/tasks.json; crates/tupi-tools/src/tools/search/dense.rs:20-91.
- Tokens de schema não podem crescer: a fusão tem que reduzir o payload, e `the_agent_schema_costs_tokens_on_every_step` trava a tabela (núcleo 12/4.506, all 30/10.651). Fonte: crates/tupi-tools/tests/tool_schema_tokens.rs:30-45; docs/perf/turn-latency.md:1-63.
- Digest por modelo local (`GeneratorSlot`) fica fora desta spec; se voltar, sai sempre com âncoras `caminho:linha` e handle do corpo, nunca resumo sozinho.
- `native-search` é absorvido: faixa de linhas (`lines`/`tail`/`head`) e busca textual exata viram ops de `lookup`; o nome `grep` não nasce como tool.
- Nenhum texto do repo pode apontar tool inexistente depois da fusão: `TUPI_SYSTEM`, `configs/prompts.ron`, `NATIVE`/`native_hint` do shell, `CORE_TOOLS`, `agent_register` e o bloco gerado por `setup` fecham no mesmo commit. Fonte: crates/tupi-app/src/prompt.rs:1-11; crates/tupi-tools/src/tools/shell/mod.rs:362-395; crates/tupi-tools/src/lib.rs:446-457, :494-507; crates/tupi-tools/tests/agent_register.rs:4-39.
- Conhecimento de projeto vai para `docs/`, nunca para `.agents/`. Fonte: docs/rules/knowledge-placement.md.

## Nuances

- A economia não vem de juntar chamadas: vem de cap por item, assinatura na cauda e ledger. 10 `read` custam até 20k tokens (teto de 2.000 cada, DEFAULT_BUDGET_TOKENS em crates/tupi-tools/src/support/budget.rs:16); um `search` denso cobre os mesmos 10 em ~2k.
- `read_many` hoje recusa o lote inteiro em arquivo ausente ou leitura repetida (crates/tupi-tools/src/tools/read_many/mod.rs:42-181); em `lookup` isso vira erro do item, senão o modelo perde o que deu certo.
- `search` já tem `detail` Auto/Full/Signatures e corpo capado em 80 linhas: `kind: "semantic"` preserva os dois, e a fusão não pode rebaixar isso. Fonte: crates/tupi-tools/src/tools/search/params.rs:5-13; crates/tupi-tools/src/tools/search/mod.rs:28-31; crates/tupi-tools/src/tools/search/render.rs:3-17.
- `peek` precisa continuar existindo e exposto: toda mensagem de truncamento nomeia `peek`, e o teste do núcleo exige a presença dele. Fonte: crates/tupi/src/tool_config.rs:149-177.
- `BacktrackGraph::visit_level` já separa Body/Signature/Frontier e é usado só para reordenar; expor a fronteira como uma linha no topo do output (~20 tokens) é o ganho mais barato da spec. Fonte: crates/tupi-tools/src/tools/search/query.rs:120-190.
- `pipeline` existe no kernel mas não está em `CORE_TOOLS` nem em `register_with`: hoje não é caminho ensinado ao modelo. Fonte: crates/tupi-kernel/src/pipeline.rs:33-67; crates/tupi-tools/src/lib.rs:167-417, :446-457.
- Sem número, não fica (P5): o histograma por tool é pré-requisito, e `aggregate` hoje ignora o campo `name` já gravado em cada `tool_finished`. Fonte: crates/tupi/src/telemetry_report.rs:46-114; crates/tupi-app/src/telemetry.rs:537-549; docs/philosophy.md.
- A ordem de registro das tools é preservada pelo registry e é sinal grátis de prioridade. Fonte: crates/tupi-kernel/src/tool.rs:164-166.

## Tasks

- [x] **T1** Medição: histograma por tool no `tupi telemetry` (pré-requisito de P5)
  - `aggregate` soma só `tool_calls`/`tool_us` e ignora o `name` que já viaja em cada `tool_finished`. Acrescentar mapa por nome (chamadas, ok, result_bytes, truncated) e top-N no `render`, mais a contagem de bypass (`shell` cujo `summary`/`result` traz rg|grep|cat|sed|tail). Fonte: crates/tupi/src/telemetry_report.rs:46-114, :130-200; crates/tupi-app/src/telemetry.rs:537-549.
  - [x] **T1.1** `aggregate` acumula por `name` — Teste com três tools fixa chamadas, falhas, bytes e truncados por nome; linha malformada continua contada em `malformed`.
  - [x] **T1.2** `render` imprime top-N e a coluna de truncados — Cabeçalho e ordem determinísticos travados por teste, no estilo de `the_report_names_the_columns_of_the_split`.
  - [x] **T1.3** Baseline registrada em docs/ — `docs/perf/turn-latency.md` ganha a tabela por tool do run atual e o comando reproduzível.
- [ ] **T2** Contrato denso canônico: um helper para todas as tools
  - Extrair de `ReadTool::finish`/`ReadTool::settle` e do fim de `SearchTool::search` o caminho único: ordenar por priority, `budget::select`, `store.insert(omitted)`, `budget::render` com handle, ledger por seção. Assinatura alvo: `dense(sections, budget, ledger, store) -> String`. Fonte: crates/tupi-tools/src/tools/read/render.rs:268-289; crates/tupi-tools/src/tools/search/query.rs:323-486; crates/tupi-tools/src/support/budget.rs:64-119, :198-223.
  - [x] **T2.1** Helper + testes de prioridade, envelope e ledger — Teste fixa que seção de maior prioridade entra primeiro, que o excedente vira handle e que seção já entregue vira ponteiro.
  - [x] **T2.2** Migrar `read`/`search` para o helper sem mudar a saída — `a_small_budget_truncates_and_peek_recovers`, `the_batch_shares_one_budget_and_the_tail_is_recoverable` e `a_read_delivery_makes_the_search_a_pointer` seguem verdes sem alteração de expectativa.
  - [ ] **T2.3** Migrar as tools de lista: `impact_check`, `diff`, `trace`, `why` — Cada uma declara `N de M` quando corta e o excedente volta por `peek`; teste por tool.
- [ ] **T3** Tool `lookup`: fusão de `read`, `read_many` e `search`
  - Schema `ops: [{kind, ...}]` com `kind` em symbol|skeleton|lines|text|semantic|graph, `mode` como atalho de op única, um `budget_tokens`, um ledger. Erro por item em seção priority 255; `describe_call` resume o lote. Fonte: crates/tupi-tools/src/tools/read/mod.rs:35-43, :177-206; crates/tupi-tools/src/tools/read_many/mod.rs:42-181, :185-229; crates/tupi-tools/src/tools/search/mod.rs:121-167.
  - [ ] **T3.1** Params, schema e parse por `kind` — Op sem o campo obrigatório do seu `kind` é recusada nomeando o campo; `mode` gera uma op única equivalente.
  - [ ] **T3.2** Execução por `kind` reusando `ReadTool::prepare`/`render` e `SearchTool::search` — Lote misto (symbol + skeleton + semantic) devolve as três seções na ordem pedida, com um único orçamento.
  - [ ] **T3.3** Saída densa única, erro por item e linha de fronteira — Item inexistente vira seção de topo e os demais continuam; o topo mostra quantos símbolos da cadeia ainda estão `Frontier`.
  - [ ] **T3.4** Registro, exposição e prompts — `lookup` entra em `register_with`/`CORE_TOOLS` e no `agent_register`; `TUPI_SYSTEM`, `configs/prompts.ron` e `NATIVE`/`native_hint` nomeiam `lookup`; `setup` regenera AGENTS.md e skill sem menção a tool removida.
  - [ ] **T3.5** Aposentar `read_many` e `search` como tools — Nenhum teste de exposição espera as duas; `tool_schema_tokens` mostra queda no total de tokens de schema (ou o topo justifica por escrito se não houver queda).
- [ ] **T4** Ops `text` e `lines`: absorver o draft `native-search`
  - `kind: text` devolve `caminho:linha:texto` agrupado por arquivo, com `alternatives`, `regex` opt-in, `glob`/`exclude`, `max_matches`; `kind: lines` cobre faixa, `tail` e `head` numerados. Reusar `LexicalScanner` e o walk existente; excluir `.agents/tupi`, `target` e `.git`. Fonte: .tupi/sdd/native-search/tasks.json; crates/tupi-tools/src/support/scanner.rs:24-200; crates/tupi-tools/src/tools/exec/render.rs:56-137.
  - [ ] **T4.1** Estender o scanner com glob, exclude e alternatives — Teste por filtro; `an_empty_term_list_finds_nothing` e o caminho semântico continuam verdes.
  - [ ] **T4.2** Render `caminho:linha:texto` com contagem e cap — Saída agrupada por arquivo com total de casamentos e corte anunciado; excedente em `peek`.
  - [ ] **T4.3** Faixa de linhas, `tail` e `head` numerados — `lines: "66-73"` de um `.ron` sem símbolos devolve 8 linhas numeradas; `tail: 6` de um `.md` devolve as 6 últimas sem descartar nenhuma.
  - [ ] **T4.4** Cenário de aceitação derivado do log de shell — As 10 consultas do run 1794840347760-112433 são reproduzíveis por `lookup` sozinho, com saída equivalente ou melhor.
- [ ] **T5** Fechamento: números, doc e aposentadoria do plano `native-search`
  - Medir antes/depois no cenário fixo e fechar contrato em `docs/`. Fonte: docs/philosophy.md (P5, P6); docs/perf/turn-latency.md:1-63.
  - [ ] **T5.1** Comparativo do cenário fixo — 0 chamadas de shell para busca/leitura por faixa; `tool_calls` por turno ≤ 60% do baseline; tokens de schema não crescem.
  - [ ] **T5.2** Doc de contrato e ADR da fusão — `docs/features/lookup-tool.md` descreve ops, orçamento, erro por item e dedupe; ADR registra a fusão, o que foi rejeitado e as condições de revisão; `docs/index.md` e `docs/perf/turn-latency.md` atualizados.
  - [ ] **T5.3** Encerrar `native-search` como substituído — Plano `native-search` fechado apontando para `dense-lookup`, sem task órfã.
