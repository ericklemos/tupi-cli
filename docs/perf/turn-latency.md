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
| `Exposure::Core` (11 tools) | 11 | 4.468 |
| `Exposure::All` (29 tools, `expose_all`) | 29 | 10.613 |
| diferença | +18 | **+6.145** |

Leitura: `expose_all` custa 2,4× o núcleo em tokens de schema e soma 6.145
tokens a **cada** passo — 9,6% do `budget` de 64.000 e 19,2% do `target` de
32.000 (`configs/context.ron`). Como `plan_context` subtrai esses tokens do
orçamento de mensagens (`crates/tupi-app/src/app.rs:142-168`), o schema encurta
o histórico útil antes do previsto: com `expose_all` sobram 53.387 tokens de
mensagem em vez de 59.532.

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
piso dessa diferença é o schema: 4.468 tokens no núcleo, 10.613 com
`expose_all`. Não somar `tool_tokens` com `estimate_tokens` para "prever"
`prompt_tokens` — há também o template, que não é medido localmente.

## Turno de referência: `prompt_tokens` × `estimate_scaled` por passo (T1.2)

O turno medido é o de 12 passos citado acima: run `1794848745295-332087`,
requisição 1, `deepseek-flash`, `max_tool_steps` 200 na época, `tool_count` 31
(`expose_all`), `max_tokens` 6000, arquivo
`.agents/tupi/debug/telemetry.jsonl`. Comando reproduzível (a sonda
`crates/tupi/tests/turn_baseline.rs` dá o split, o `jq` dá o passo a passo):

```text
cargo test -p tupi --test turn_baseline -- --nocapture   # split + top tools do run
jq -r 'select(.event=="provider_response" and .run=="1794848745295-332087" and .request==1)
  | "\(.step)\t\(.prompt_tokens)\t\(.estimate_scaled)\t\(.prompt_tokens-.estimate_scaled)\t\(.cached_tokens)\t\(.scale_ppm)"' \
  .agents/tupi/debug/telemetry.jsonl
```

Linha do turno (as colunas de `tupi telemetry`, `render`,
`crates/tupi/src/telemetry_report.rs:169-281`): passos **12** · `re-read`
**7** · `provider_us` 64.786.859 (~5,4 s por passo, 48% da parede) ·
`tool_us` 61.777.028 (46%) · `parede_us` 135.566.153 · `prompt` 293.108 ·
`cached` 261.504 · `completion` 9.095.

| passo | `prompt_tokens` | `estimate_scaled` | conta | `cached` | `scale_ppm` |
| --- | --- | --- | --- | --- | --- |
| 1 | 6.328 | 8.550 | −2.222 | 4.480 | 100.000 |
| 2 | 10.239 | 11.097 | −858 | 6.400 | 87.000 |
| 3 | 11.753 | 12.101 | −348 | 10.624 | 83.000 |
| 4 | 15.257 | 15.062 | 195 | 12.288 | 82.000 |
| 5 | 18.955 | 18.615 | 340 | 15.744 | 82.000 |
| 6 | 25.241 | 25.545 | −304 | 19.328 | 83.000 |
| 7 | 27.722 | 27.699 | 23 | 25.728 | 83.000 |
| 8 | 30.012 | 29.814 | 198 | 27.904 | 83.000 |
| 9 | 32.285 | 32.129 | 156 | 30.464 | 83.000 |
| 10 | 35.291 | 35.067 | 224 | 32.512 | 83.000 |
| 11 | 38.776 | 38.687 | 89 | 36.736 | 84.000 |
| 12 | 41.249 | 41.413 | −164 | 39.296 | 84.000 |
| soma | 293.108 | 295.779 | −2.671 | 261.504 | — |

Leitura: a conta `prompt_tokens − estimate_scaled` **não** é o schema neste run.
Ela fica entre −2.222 e +340 e soma −2.671 no turno: com `scale_ppm` em
0,82–0,84 o estimador local já erra para cima mais que o payload de tools, então
o schema (10.613 com `expose_all`) **não** é o piso dessa diferença — o piso é o
que sobra depois da correção de escala, e ela é aplicada passo a passo. Atribuir
o schema pela diferença bruta superestima o custo fixo; o custo que de fato
viaja é `prompt_tokens`, e ele cresce ~3,2 k por passo neste turno
(6.328 → 41.249), dos quais ~22 k são prefixo cacheado na última requisição.
O `re-read` de 7 em 12 passos é o denominador de p(recall) para as podas
posteriores ([re-leituras](#re-leituras-por-turno-t62)).

## Escala e `cached_tokens` por passo (T4.2)

`tupi telemetry` passou a imprimir, para cada turno exibido, uma seção `passos`
com `escala` (o `provider_response.scale_ppm` **em vigor no envio**, em partes
por milhão) e `cached` (a fatia cacheada do `prompt_tokens` daquele passo), mais
`cache` = cached/prompt (`StepRow`, `render`,
`crates/tupi/src/telemetry_report.rs:41-59`). Antes isso só saía por conta
própria com `jq`; agora é coluna do relatório e o `--limit` recorta o turno que
se quer comparar.

Turno de referência (`1794848745295-332087`, turno 1, 12 passos) reimpresso com
as colunas novas — mesmos números da tabela de T1.2, nenhuma requisição nova:

```text
cargo run -p tupi -- telemetry --limit 1
```

| passo | escala | prompt | cached | cache |
| --- | --- | --- | --- | --- |
| 1 | 1,0000 | 6.328 | 4.480 | 71% |
| 2 | 0,8700 | 10.239 | 6.400 | 63% |
| 3 | 0,8300 | 11.753 | 10.624 | 90% |
| 4 | 0,8200 | 15.257 | 12.288 | 81% |
| 5 | 0,8200 | 18.955 | 15.744 | 83% |
| 6 | 0,8300 | 25.241 | 19.328 | 77% |
| 7 | 0,8300 | 27.722 | 25.728 | 93% |
| 8 | 0,8300 | 30.012 | 27.904 | 93% |
| 9 | 0,8300 | 32.285 | 30.464 | 94% |
| 10 | 0,8300 | 35.291 | 32.512 | 92% |
| 11 | 0,8400 | 38.776 | 36.736 | 95% |
| 12 | 0,8400 | 41.249 | 39.296 | 95% |
| soma | — | 293.108 | 261.504 | 89% |

Leitura antes × depois: **antes** o total do turno só dizia `cached` 261.504
(89%); a oscilação da escala — 0,8200 → 0,8300 no passo 6 e 0,8300 → 0,8400 no
passo 11 — ficava invisível, junto com a pergunta que ela responde: quantas
vezes o request mudou dentro do turno. **Depois** a coluna `escala` mostra a
subida em dois passos, que é exatamente o que a política assimétrica de T4.1
remove (dentro do turno a escala só desce, `next_scale_in_turn`,
`crates/tupi-app/src/app.rs:162-167`), e a coluna `cache` mostra que neste turno
o prefixo sobreviveu — 89% do prompt cacheado apesar da oscilação de 1%.

O critério de aceitação de T4.1 é, nesta tela, uma série **não crescente** de
`escala` dentro do turno, com `cache` acompanhando. A coluna é medida; a série
pós-T4.1 exige um turno novo (API) e não foi medida nesta subtarefa — a tabela
acima é o turno gravado, reimpresso com as colunas novas. O que fica travado
por teste é o formato: `each_step_prints_its_scale_and_cached_tokens`
(`crates/tupi/src/telemetry_report.rs`) falha se a escala, o cache ou a razão
por passo saírem da saída.

## Re-leituras por turno (T6.2)

`tupi telemetry` conta, por turno, as chamadas de leitura — `read`,
`lookup` e `peek` — no campo `re-read` da tabela e no total
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

## Ganho do dedup por turno (T3.3)

Dedup por `(path, blake3)` (`dedup_identical_results`,
`crates/tupi-domain/src/context.rs:485-527`) é a única poda com **p(recall) = 0
por construção**: o corpo do resultado repetido continua no contexto e o
segundo vira ponteiro para o primeiro, sem depender do `ResponseStore`. O
break-even da regra de poda (`tokens_poupados >= p_recall × prompt_tokens`) vale
para qualquer token poupado justamente porque `p_recall` não é estimado: nada
precisa ser relido, então o campo `re-read` de `tupi telemetry` não entra nesta
conta.

Sonda `crates/tupi-app/tests/dedup_gain.rs`, sem instrumentação nova: lê
`tool_finished.result` do JSONL já gravado, reconstrói o histórico de cada passo
e roda o dedup real.

```text
cargo test -p tupi-app --test dedup_gain -- --nocapture
# outro arquivo:
TUPI_TELEMETRY_PATH=/caminho/telemetry.jsonl \
  cargo test -p tupi-app --test dedup_gain -- --nocapture
```

Arquivo `.agents/tupi/debug/telemetry.jsonl`: **132 turnos** com resultado de
tool, **5 com duplicata**, 133 duplicatas somadas — **todas de `read`**. `exec`,
`patch`, `write`, `write_many` e `relocate` nunca entram: repetição ali é efeito
colateral, não economia.

| run · turno | orçamento | duplicatas | `tokens_poupados_por_passo` | duplicatas por tool |
| --- | --- | --- | --- | --- |
| 1794712224896-376076 · 2 | 200.000 | 6 | 9.960 | read 6 |
| 1794760400983-9825 · 1 | 200.000 | 19 | 1.007 | read 19 |
| 1794839753761-90692 · 1 | 200.000 | 50 | 98.000 | read 50 |
| 1794840261706-110773 · 3 | 200.000 | 6 | 1.128 | read 6 |
| 1794842285268-175772 · 2 | 200.000 | 52 | 26.052 | read 52 |

O turno de referência (`1794848745295-332087`, turno 1, 12 passos) **não tem
duplicata**: 0 tokens deduplicados por turno ali, e o mesmo vale para 127 dos
132 turnos. Nas 5 sessões que repetem leitura o ganho é real mas pontual: até
98.000 tokens/passo num turno de 50 re-leituras do mesmo arquivo.

**Ressalva medida, não estimada:** os 5 turnos com duplicata rodaram com
`context_budget` 200.000 e nenhum passou do orçamento, então `plan_with` nem
chegou ao dedup — o caminho rápido devolve o histórico intacto enquanto a
estimativa local couber no orçamento (`crates/tupi-domain/src/context.rs:114-116`).
A coluna da tabela é o ganho **disponível** no histórico daquele passo; ele só
vira token não cobrado quando o turno cruza o orçamento, que é a condição de
qualquer poda. Comparar com `context_budget` antes de somar dedup a uma
economia por turno.

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
`read`, `peek`, `lookup`, `docs`, `assert` — e que não declara
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
