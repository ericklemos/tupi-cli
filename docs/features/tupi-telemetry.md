# `tupi telemetry` — o split provider × tool por turno

Subcomando de manutenção que lê o JSONL de telemetria já emitido e imprime uma
linha por turno. É o instrumento que transforma "o turno está lento" em número
([P5](../philosophy.md)): sem ele nenhum corte de latência se justifica.

## Onde mora

- `crates/tupi/src/telemetry_report.rs` — `subcommand` (reconhece o nome só na
  primeira posição do argv), `run`, `aggregate`, `render`, `USAGE` (:11-13),
  `DEFAULT_LIMIT` (:16) e `TurnRow` (:21-38).
- Despacho no `main` **antes** de env/provider, no mesmo padrão de
  [`tupi install`](tupi-install.md) ([convenções](../conventions.md)).
- Fonte: o JSONL que `crates/tupi-app/src/telemetry.rs` já grava. O relatório é
  leitura de arquivo — nada no caminho quente depende dele.

## Colunas por turno

`TurnRow` soma, por `(run, turn)`:

| coluna | origem | leitura |
| --- | --- | --- |
| `steps` | `TurnFinished.steps` | quantos round trips o turno pagou |
| `provider_us` | Σ `ProviderResponse.duration_us` | tempo de modelo |
| `tool_us` | Σ `ToolFinished.duration_us` | tempo de execução local |
| `re-read` | Σ chamadas de `read`/`read_many`/`peek` | re-leitura dentro do turno |
| prompt/cached/completion | Σ dos três campos do turno | custo de token |

`share` compara as parcelas com `TurnFinished.duration_us`: é a diferença entre
tempo de provedor e tempo de tool que decide onde cortar
([latência do turno](../perf/turn-latency.md)).

## Invariantes

- Turno sem `TurnFinished` fica com `steps` **desconhecido**, não zero
  (`a_turn_without_turn_finished_keeps_its_steps_unknown`).
- Turno sem `ProviderResponse` cai nos totais do `TurnFinished`, e isso é
  declarado — não inventa split (`a_turn_without_provider_responses_falls_back_to_the_finished_totals`).
- Linha malformada é **contada** e nunca escondida
  (`malformed_lines_are_counted_and_never_hidden`).
- Cada `run` mantém o próprio turno; dois runs não se somam
  (`each_run_keeps_its_own_turn`).
- `--limit` corta a exibição, mas os totais somam o arquivo inteiro
  (`the_limit_keeps_the_last_turns_and_the_totals_sum_the_file`).
- `re-read` conta só as tools de leitura; `exec`, `shell` e `edit` não inflam o
  número.

## Como usar o número

Comparar turnos da **mesma** configuração. Somar turnos de `configs/tools.ron`
diferentes (ou antes/depois do manifesto) mede a mudança de configuração, não a
latência. Quando o aceite de um plano cita "menos re-leituras", é este campo que
prova — e ele mora em `docs/perf/turn-latency.md`, não no chat.

## Limitações conhecidas

- É relatório de arquivo: não agrega em tempo real nem substitui `StatsSnapshot`
  (`crates/tupi-app/src/telemetry.rs`), que conta eventos e não tempo.
- O split cobre o turno inteiro; não separa passos individuais nem atribui token
  de schema por passo — isso é estimado à parte
  ([latência do turno](../perf/turn-latency.md)).
