# ADR 0005 — `expose_all` fica: schema é prefixo cacheado, não passo extra

## Contexto

`configs/tools.ron:8` traz `expose_all: true`: o schema de **todas** as 30
tools registradas viaja em toda requisição, e esses tokens entram como
`tool_tokens`, subtraídos do orçamento de mensagens em `plan_context`
(`crates/tupi-app/src/app.rs:142-168`). O teto de exposição alternativa já
existe no código — `expose: Vec<String>` em `ToolConfig`
(`crates/tupi/src/tool_config.rs:16-25`) virando `Exposure::Only`
(`crates/tupi/src/tool_config.rs:29-37`), aplicado por
`register_exposed_with`/`Exposure::admits`
(`crates/tupi-tools/src/lib.rs:465-507`).

Números medidos, sem API, em `crates/tupi-tools/tests/tool_schema_tokens.rs`
(tabela em [`../perf/turn-latency.md`](../perf/turn-latency.md#tokens-de-schema-por-passo-t31)):

| exposição | tools | tokens de schema por passo |
| --- | --- | --- |
| `Exposure::Core` | 12 | 4.506 |
| `Exposure::All` (`expose_all`) | 30 | 10.651 |
| diferença | +18 | **+6.145** |

E o split por turno da telemetria real (`tupi telemetry`,
`crates/tupi/tests/turn_baseline.rs`): turno interativo de 12 passos a ~5,4 s
por passo, 48% no provedor, 46% nas tools. Um passo a mais custa ~5,4 s; um
schema fixo no cabeçalho da requisição custa tokens de **prefixo cacheado**
(`cached_tokens` em `ProviderResponse`,
`crates/tupi-app/src/telemetry.rs:483-513`), que é justamente o prefixo que a
poda amortizada se recusa a invalidar (`configs/context.ron:1-4`).

## Decisão

**Não cortar a exposição.** `configs/tools.ron` continua com
`expose_all: true` e nenhuma tool sai do conjunto enviado.

A conta que decide: o corte troca 6.145 tokens por passo de prefixo cacheado
por uma chance de passo extra — e não existe passo extra para economizar, porque
schema não é round trip. Se o modelo precisar de uma tool que saiu da lista
(`write_many`, `test_many`, `trace`, `diff`, `why`, `move`, `assert`, …), o
turno gasta um passo para descobrir a recusa e outro para fazer o trabalho por
outro caminho: ≥2 × 5,4 s ≈ 10,8 s contra um ganho que, no melhor caso, é
processamento de 73.740 tokens cacheados por turno (6.145 × 12) — grandeza que
não aparece no `provider_us` de um passo que já custa 2,6 s de ida e volta.

O argumento de orçamento também não fecha: com `expose_all` sobram 53.349
tokens de mensagem (64.000 − 10.651), acima de qualquer turno medido; o próprio
escopo do épico declara o orçamento (64k/32k) folgado e fora de alcance. Cortar
exposição para "sobrar histórico" resolve um truncamento que a telemetria não
mostra.

## Alternativas rejeitadas

- **`expose: [...]` explícito no `configs/tools.ron`** (núcleo de 12 tools,
  −6.145 tokens por passo): exigiria uma recusa acionável para tool não exposta,
  hoje inexistente, e pagaria o risco de passo extra descrito acima. Fica como
  knob do usuário, não como default do repositório.
- **Encurtar descrições de schema para caber no núcleo**: muda o contrato de
  cada tool e o `refusal` que ensina a alternativa — custo de manutenção sem
  número que o sustente (o número aqui é do tamanho, não da descrição).
- **`Exposure::Core` fixo em código**: tira do usuário a decisão e não muda a
  conta; mesmo custo de passo extra.

## Consequências

- `expose_all: true` permanece; `expose` continua disponível para quem quiser
  cortar por conta própria, com o número desta ADR em mãos.
- O teste `the_agent_schema_costs_tokens_on_every_step`
  (`crates/tupi-tools/tests/tool_schema_tokens.rs:30-45`) trava a tabela: tool
  nova ou descrição maior falha e obriga a revisar esta decisão no mesmo commit.
- **Revisar quando** a telemetria mostrar (a) `cached_tokens` ≈ 0 nos passos
  intermediários de um turno — o schema deixou de ser prefixo amortizado —, ou
  (b) `prompt_tokens` encostando no `budget` de 64.000 com re-leitura de
  resultado podado. Aí o número muda de lado e o corte volta à mesa.
