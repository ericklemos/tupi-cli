# Campos reais do JSONL de telemetria (insumo da task T0)

Medição feita sobre o arquivo vivo do próprio repo,
`.agents/tupi/debug/telemetry.jsonl` (11 972 linhas, `schema: 3`), no dia da
subtarefa T0.1. Todo campo listado abaixo foi lido literalmente do arquivo; o
que não aparece aqui foi conferido como **ausente**, não presumido.

## Envelope comum a toda linha

Inserido no ato da escrita por `write_line` (`crates/tupi-app/src/telemetry.rs:1037-1054`),
além da tag `event` do enum `TelemetryEvent`
(`crates/tupi-app/src/telemetry.rs:265-282`):

`schema` · `ts_ms` · `seq` · `run` · `event`

## `tool_started`

Campos: `request`, `call`, `name`, `arguments`, `arguments_chars`,
`arguments_bytes`, `truncated`, `summary` (`ToolStarted`,
`crates/tupi-app/src/telemetry.rs:525-534`), preenchidos em
`crates/tupi-app/src/app.rs:1555-1568`.

Exemplo real (linha 11972 do arquivo citado, íntegra):

```json
{"arguments":"{\"action\": \"close\", \"slug\": \"tupi-escrita-global\", \"budget_tokens\": 800}","arguments_bytes":72,"arguments_chars":72,"call":"call_00_wmp1mYFG7bhRxFt20fjX3308","event":"tool_started","name":"sdd","request":5,"run":"1794949142099-150668","schema":3,"seq":103,"summary":"close tupi-escrita-global","truncated":false,"ts_ms":1794951474697}
```

## `tool_finished`

Campos: `request`, `call`, `name`, `duration_us`, `ok`, `result`, `result_chars`,
`result_bytes`, `truncated`, `summary`, `error` (`ToolFinished`,
`crates/tupi-app/src/telemetry.rs:536-549`), preenchidos em
`crates/tupi-app/src/app.rs:1686-1708` e em
`crates/tupi-app/src/pipeline.rs:136-150`.

Exemplo real (linha 11963, `result` abreviado por orçamento; o campo existe
inteiro no arquivo, cortado em 512 KiB por `telemetry.rs:23`):

```json
{"call":"call_00_8AafcPzXNu3T0zf1nt357268","duration_us":3569104,"error":null,"event":"tool_finished","name":"peek","ok":true,"request":4,"result":"## memória do projeto …","result_bytes":2922,"result_chars":2850,"run":"1794949142099-150668","schema":3,"seq":94,"summary":"#### T0 — Medir a escrita real por tool e por extensão","truncated":false,"ts_ms":1794950927515}
```

`error` é `null` quando `ok: true`; numa recusa, `result` fica `null` e a
mensagem de recusa inteira vai em `error` (`app.rs:1688-1692`).

## Resposta à pergunta "tool e caminho tocado"

- **Tool**: campo `name`, idêntico em `tool_started` e `tool_finished`. É o nome
  curto da tool (`write`, `write_many`, `edit`, `patch`, `peek`, `sdd`, …), sem
  prefixo de pacote.
- **Caminho tocado: não é campo.** Nem `tool_started` nem `tool_finished`
  carregam `path`, `file`, `ext` ou equivalente. O único lugar em que o caminho
  existe é dentro da string `arguments` do `tool_started`, que é o JSON cru da
  call (`call.arguments`, `crates/tupi-app/src/app.rs:1556-1562`). Para
  escrever, o caminho tem de ser extraído desse JSON pelo relatório; o corte de
  512 KiB (`crates/tupi-app/src/telemetry.rs:23`) só o atinge se o payload vier
  antes da chave de caminho no JSON do modelo.
- **Correlação**: `started`×`finished` se dá por `call` (id da call), com
  `request` e `run` para desambiguar entre turnos. Atenção: dentro de
  `pipeline`, o id é reescrito como `"{id}#{step}.{index}"`
  (`crates/tupi-app/src/pipeline.rs:166-168`), então o `call` do finish não é
  igual ao do start da call externa.
- **Recusa**: só `ok: false` + texto livre em `error`. Não existe campo de
  classe de recusa, logo "hash velho (`StaleHash`)" × "símbolo não é Rust" só
  distingue por casamento de texto na mensagem — nada de enum gravado.
- **Tamanho do arquivo e linhas mudadas: não existem no JSONL.** `result_chars`
  e `result_bytes` medem a *resposta da tool*, não o arquivo; não há contagem de
  linhas alteradas nem diff por evento.

## Consequências para T0.2 / T0.3

1. A quebra por extensão exige derivar a extensão de `tool_started.arguments`
   (chave de caminho do JSON), não de um campo pronto.
2. A razão "linhas mudadas / tamanho do arquivo" não é calculável só com o
   JSONL; o relatório precisa abrir o arquivo no disco (ou cruzar
   `arguments.content` do `write` com o conteúdo atual) para obter o delta.
3. A classificação de recusas de `edit` depende de casar o texto de `error`;
   fica frágil por natureza e deve ser declarada como heurística no relatório.
