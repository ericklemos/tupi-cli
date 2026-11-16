# 0007 — Fundir `read_many` e `search` em `lookup(ops)`

- **Status:** aceita
- **Contexto:** spec `dense-lookup`, absorvendo o draft `native-search`
- **Substitui:** `docs/features/lookup-tool.md` é o contrato atual

## Contexto

A economia de um turno não vem de juntar chamadas: vem de **cap por item,
honestidade do corte e ledger**. Medido antes desta decisão: dez `read` custam
até 20k tokens (teto de 2.000 por item, `DEFAULT_BUDGET_TOKENS`), enquanto uma
busca densa cobre os mesmos dez em ~2k. Duas tools concorrentes — `search`
(ranking semântico) e `read_many` (lote de leituras) — pagavam schema em todo
passo e obrigavam o modelo a escolher entre elas; e nenhuma das duas devolvia
busca textual exata, o que empurrava `grep -rn` para o shell (6 usos no run
analisado).

## Decisão

Uma tool, `lookup`, com `ops: [...]` heterogêneo (`symbol`, `skeleton`, `lines`,
`text`, `semantic`, `graph`), um `budget_tokens`, um `DeliveryLedger`, erro por
item e uma linha de fronteira no topo. `read_many` e `search` deixam de ser
expostas; seguem existindo como estrutura interna reusada pelo `lookup`.

O contrato de saída é único e vive em `support/dense.rs`: ordenar por
prioridade, `budget::select`, excedente em `store.insert`, `budget::render` com
handle de `peek`, dedupe por `check`/`remember`.

## Consequências

- Schema por passo: núcleo 12 tools · 4.506 → **11 tools · 4.468**;
  `expose_all` 30 · 10.651 → **29 · 10.613**. Menos tools, menos tokens, mais
  capacidade (busca literal e faixa de linhas entram no núcleo).
- `read_many` recusava o lote inteiro em arquivo ausente; agora o erro é do item.
- `NATIVE` do shell, `TUPI_SYSTEM`, `configs/prompts.ron`, `CORE_TOOLS`,
  `register_agent` e `configs/modes/sdd.ron` nomeiam `lookup`; nenhum texto
  aponta tool inexistente.
- `T4.4` (as 10 consultas do run `1794840347760-112433` reproduzíveis só por
  `lookup`) **não** foi fechado nesta rodada: falta o cenário de aceitação sobre
  o log de shell do run, e por isso o `native-search` não é declarado encerrado.

## O que foi rejeitado

- **Fusão com `peek`**: `peek` continua exposto e é o nome que toda mensagem de
  corte cita; cortar a exposição quebraria o contrato do orçamento.
- **Mais de uma tool por intenção** ("`grep`" como tool separada): o nome não
  nasce; `text` é op.
- **Cortar `expose_all`** para "pagar" a fusão: rejeitado em
  [ADR 0005](0005-expose-all.md) — o prefixo é cacheado, o corte não é round
  trip.
- **Digest por modelo local**: fora de escopo; se voltar, sai com âncoras
  `caminho:linha` e handle do corpo.
- **Regex em `text`**: sem dependência nova. A recusa ensina `alternatives` e
  `semantic`.

## Condições de revisão

- Se o histograma por tool (`tupi telemetry`) mostrar `lookup` com muitas ops por
  chamada e `budget_tokens` estourando, o cap por item vira cap por op.
- Se o schema voltar a crescer, o corte é de campo (o `lookup` declara hoje todos
  os campos de todas as ops), não de tool.
- Se `text` continuar perdendo para `grep` no shell, o próximo passo é `regex`
  com dependência explícita, não outra tool.
