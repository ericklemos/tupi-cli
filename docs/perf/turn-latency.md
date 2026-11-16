# Latência do turno

Tempo de parede de um turno interativo é dominado por round trips ao provedor:
cada passo é uma requisição com histórico + schema de tool. Este arquivo guarda
os números medidos que decidem o que cortar — sem número, nada muda
([P5](../philosophy.md)).

## Split provider × tool por turno

Fonte: `tupi telemetry` (`crates/tupi/src/telemetry_report.rs`), que soma por
turno `ProviderResponse.duration_us` e `ToolFinished.duration_us` e compara com
`TurnFinished.duration_us` (`crates/tupi-app/src/telemetry.rs:442-513`).
O turno de referência (12 passos, `configs/tools.ron`) custa ~5,4 s por passo,
48% no provedor e 46% nas tools; o split somado é impresso em
`render` (`crates/tupi/src/telemetry_report.rs:193-199`). `StatsSnapshot` só
conta eventos, não tempo — o split é por turno, não agregado.
Contrato do subcomando em [`tupi telemetry`](../features/tupi-telemetry.md).

## Teto de passos (T2)

O teto configurado era 200 passos — 6,7× o default de código
(`DEFAULT_MAX_TOOL_STEPS = 30`, `crates/tupi/src/tool_config.rs`). Com 12 passos
medidos no turno de referência a ~5,4 s por passo, o teto era multiplicador puro
do pior caso: 200 passos ≈ 18 min no pior cenário, e o aviso do freio (60%) só
aparecia no passo ~120.

| teto | folga sobre 12 medidos | aviso (60%) | síntese sem tools |
| --- | --- | --- | --- |
| 200 (antes) | 16,7× | passo ~120 | passo 200 |
| **30 (atual)** | **2,5×** | passo 18 | passo 30 |

Ficou 30: o default de código, com 2,5× de folga sobre o uso observado. O
racional está no comentário de `configs/tools.ron` e a decisão — incluindo os dois
tetos separados, turno e subagente do SDD — em
[ADR 0006](../decisions/0006-teto-de-passos.md).
`the_agent_ceiling_is_per_request_not_the_turn_constant`
(`crates/tupi-app/src/sdd_run.rs`) prova que o subagente obedece
`agent_max_steps` do request, não `MAX_TOOL_STEPS`.

## Tokens de schema por passo (T3.1)

O schema de tools é custo **fixo por passo**: ele viaja em toda requisição, e
com `expose_all: true` (`configs/tools.ron:8`) vai o schema de todas as tools
registradas, não só o núcleo. Número medido localmente (nenhuma API),
`crates/tupi-tools/tests/tool_schema_tokens.rs`:

| exposição | tools | tokens de schema por passo |
| --- | --- | --- |
| `Exposure::Core` (12 tools) | 12 | 4.506 |
| `Exposure::All` (30 tools, `expose_all`) | 30 | 10.651 |
| diferença | +18 | **+6.145** |

Leitura: `expose_all` custa 2,4× o núcleo em tokens de schema e soma 6.145
tokens a **cada** passo — 9,6% do `budget` de 64.000 e 19,2% do `target` de
32.000 (`configs/context.ron`). Como `plan_context` subtrai esses tokens do
orçamento de mensagens (`crates/tupi-app/src/app.rs:142-168`), o schema encurta
o histórico útil antes do previsto: com `expose_all` sobram 53.349 tokens de
mensagem em vez de 59.494.

O número é medido com o mesmo estimador do planejador
(`tool_spec_tokens`, `crates/tupi-app/src/app.rs:170-174`, sobre
`tools_payload` de `tupi_kernel`), e o teste falha se o schema derivar — tool
nova ou descrição maior obriga a atualizar esta tabela no mesmo commit.
Decisão de manter `expose_all` em [ADR 0005](../decisions/0005-expose-all.md).

## `estimate_tokens` × `prompt_tokens`

Os dois são contados no evento `ProviderResponse`
(`crates/tupi-app/src/telemetry.rs:483-513`) e **não** são a mesma grandeza:

- `estimate_tokens` é a estimativa local, feita **antes** do envio, com o
  word-piece de `tupi_domain::tokens` aplicado às mensagens montadas
  (`PromptSent.estimate_tokens`, `crates/tupi-app/src/telemetry.rs:460-480`).
  Ela não conhece o template do provedor nem conta o payload de tools — que é
  estimado à parte, por `tool_spec_tokens`, e subtraído do orçamento.
- `prompt_tokens` é o que o provedor **cobra e reporta**: template da conversa
  + payload de tools serializado no formato dele + mensagens. Por isso é sempre
  maior que `estimate_tokens` sozinho.

A diferença entre os dois é o que a correção de escala mede: `estimate_scaled`
(`estimate_tokens` × `scale_ppm`) é o valor que se compara com `prompt_tokens`,
e `scale_ppm` guarda a correção em vigor no passo. Para atribuir tokens de
schema no relatório, a conta é `prompt_tokens − estimate_scaled` no passo, e o
piso dessa diferença é o schema: 4.506 tokens no núcleo, 10.651 com
`expose_all`. Não somar `tool_tokens` com `estimate_tokens` para "prever"
`prompt_tokens` — há também o template, que não é medido localmente.

## Re-leituras por turno (T6.2)

`tupi telemetry` conta, por turno, as chamadas de leitura — `read`,
`read_many` e `peek` — no campo `re-read` da tabela e no total
(`crates/tupi/src/telemetry_report.rs:142-219`). O nome da tool vem de
`ToolFinished.name` (`crates/tupi-app/src/telemetry.rs:537`); o resto das tools
fica fora da conta, então uma chamada de `exec` não infla o número.

Mecanismo medido, por resultado podado que o modelo precisa reabrir
(`shrink_old_results`, `crates/tupi-domain/src/context.rs:368-422`):

| caminho | chamadas de leitura por resultado | o que o modelo recebe |
| --- | --- | --- |
| sem manifesto (baseline T1.3) | 2 | `read` devolve ponteiro, `peek` devolve o corpo |
| com manifesto (T6.1) | 1 | o prefixo já traz path · símbolo · blake3 · handle de peek |

O baseline paga duas idas ao provedor por reabertura (a segunda só para
descobrir que o corpo estava a um `peek` de distância); com o manifesto a
mesma informação chega antes da primeira chamada, e a linha do turno mostra
quantas sobraram. A queda esperada é **−50% por resultado reaberto**; o
`re-read` do `total` confirma ou refuta isso num run real — comparar o turno
com manifesto contra o baseline de T1.3 no **mesmo** arquivo, não somar turnos
de configurações diferentes.

Regra sem verificação é pedido (`../rules/model-control.md`): o número só
serve porque o handle que o manifesto imprime é o mesmo que `peek` aceita.
Contrato do manifesto em [prefixo do turno](../features/turn-prefix.md).

## Histograma por tool (T1.3)

A sonda `crates/tupi/tests/turn_baseline.rs` agrega, por `ToolFinished.name`, as
chamadas · ok · falhas · `result_bytes` · `truncated` do run **mais recente** do
JSONL — as mesmas colunas de `ToolRow`
(`crates/tupi/src/telemetry_report.rs:147-153`). Reproduzível, sem requisição
nenhuma:

```text
cargo test -p tupi --test turn_baseline -- --nocapture
# outro arquivo:
TUPI_TELEMETRY_PATH=/caminho/telemetry.jsonl cargo test -p tupi --test turn_baseline -- --nocapture
```

Run `1794850302390-350503` (antigravity, `max_tool_steps` 30, 2 turnos, 13
calls), o último do arquivo no momento da medição:

| tool | chamadas | ok | falhas | bytes | truncados |
| --- | --- | --- | --- | --- | --- |
| workflow | 8 | 8 | 0 | 60693 | 0 |
| map | 2 | 2 | 0 | 280 | 0 |
| sdd | 1 | 1 | 0 | 1885 | 0 |
| state | 1 | 1 | 0 | 358 | 0 |
| write | 1 | 1 | 0 | 114 | 0 |

5 tools chamadas · 13 chamadas · 0 truncadas. O histograma **mede**, não julga:
este run é de execução SDD (`workflow`, `sdd`, `write`), não o turno de código de
12 passos, e `read`, `read_many` e `search` — as tools que a fusão de `lookup`
ataca — têm **zero** chamadas aqui. A economia por item da spec (cap + ledger)
ainda não tem número próprio; este run só prova o instrumento e o cap de
`result_bytes` por tool. `tupi telemetry` imprime o top-8 somando o arquivo
inteiro (`TOP_TOOLS`, `crates/tupi/src/telemetry_report.rs:172`); a sonda olha um
run só, porque somar runs de configurações diferentes mede a mudança de
configuração, não a latência.

## Calls read-only do mesmo passo (T5.1)

Duas leituras na mesma resposta do modelo rodam juntas; mutante continua serial
e na ordem declarada. A regra vive em `call_is_parallel`
(`crates/tupi-app/src/tool_batch.rs`): só entra no lote o que é leitura barata —
`read`, `read_many`, `peek`, `search`, `docs`, `assert` — e que não declara
`mutates()`. `exec`, `test_many` e `map` ficam fora: disputam `target/` ou pagam
construção de índice, e escondem trabalho longo da linha viva da TUI.

Prova em `crates/tupi-app/tests/tool_batch_reads.rs`: duas leituras de 120 ms
terminam no tempo de uma (pico de concorrência 2, turno abaixo de 210 ms) e a
história grava `c1` antes de `c2`, na ordem declarada, não na de conclusão. O
lado mutante continua travado por
`mutating_calls_stay_serial_in_declared_order`
(`crates/tupi-app/tests/tool_parallelism.rs`).

Ganho: N leituras do mesmo passo deixam de custar N execuções seriais. Não é
ganho de round trip — o passo continua sendo um passo —, é o tempo de tool que
sai; o teto do ganho é o número de leituras do lote.

## Lacunas

- **Juiz de memória no turno (T4.2):** no máximo uma requisição de julgamento
  por turno. O julgamento de `begin_turn` marca `turn_judged`
  (`crates/tupi-app/src/memory.rs`), e `recall_for_tool` devolve `None` sem ir ao
  provedor quando o turno já julgou — com ou sem bloco. Prova em
  `a_turn_that_already_judged_never_waits_for_the_memory_judge`
  (`crates/tupi-app/tests/shell_memory.rs`), com provider que demora 2 s. O
  julgamento continua no caminho do turno (uma ida), não em modelo local.
- O split de T1.3 vem de um turno de referência; turnos de outra natureza
  (execução SDD, `docs` mode) não foram medidos.
