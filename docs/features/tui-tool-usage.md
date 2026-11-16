# Uso das ferramentas na TUI — linha viva, duração e agrupamento

Como a TUI (`crates/tupi-tui`) mostra cada chamada de tool: spinner e cronômetro
enquanto roda, duração medida ao terminar, glifo/cor por categoria, sub-chamadas
de `pipeline` indentadas sob o pai e um cabeçalho de placar quando o turno tem um
lote de tools. Este arquivo guarda o **contrato interno** — onde está o código,
quais invariantes não podem ser quebradas e o que ficou fora de escopo. O
dashboard web continua sem exibir duração, de propósito.

## Formato da linha

```text
  {lead}{glifo} {nome} · {resumo}{preenchimento}{tempo} {marcador}
```

- `lead`: 2 colunas de base, +2 quando um cabeçalho de grupo está aberto, +2 por
  nível de profundidade (`#` no `call_id`) e `└ ` quando `depth > 0`.
- `glifo` por categoria (`tool_glyph`, `crates/tupi-tui/src/run.rs:1585`), sempre
  largura 1: leitura (`◆` ciano), escrita (`▪` amarelo), execução (`▸` magenta),
  `agent` (`◇` ciano), resto `⚙` cinza-escuro. A lista de nomes é fechada:
  categoria nova que não entre em `tool_glyph` cai em `⚙`.
- `tempo`: `format_duration` (o mesmo do header de métricas), alinhado à direita
  e em cinza-escuro; **só aparece se sobrar coluna** (`slack > time_width`), e
  some por inteiro em largura < 60 (`compact`). O nome da tool nunca é truncado
  para caber o tempo.
- `marcador`: em voo, frame de braille do spinner em amarelo; concluída, `✓`
  verde ou `✗` vermelho.

Lotes: uma "run" é a sequência de `ChatEntry::Tool` consecutivas. Runs de 3+ ganham
um cabeçalho `  ⚙ {n} tools · {placar}`, com placar `N de M` enquanto houver tool
em voo, senão `N✓` / `N✓ k✗` — contando **só chamadas de topo**, porque as de
pipeline (id com `#`) já estão representadas pelo pai. O painel de detalhes mostra
o placar do turno ao lado do contexto (`placar_spans`,
`crates/tupi-tui/src/run.rs:1215`) e o larga antes do contexto em tela estreita.

## Onde mora

| peça | arquivo |
|---|---|
| `duration_us` no evento de domínio | `crates/tupi-domain/src/lib.rs` (`Event::ToolFinished`) |
| medição e publicação no laço de tools | `crates/tupi-app/src/app.rs` (`send_message`) |
| medição e publicação nas sub-chamadas | `crates/tupi-app/src/pipeline.rs` (`publish_finished`, `:270`) |
| relógio e duração por tool | `crates/tupi-tui/src/state.rs` (`ChatEntry::Tool.started/.duration`, `push_tool_started`, `finish_tool`) |
| helpers de leitura | `crates/tupi-tui/src/state.rs` (`tools_in_flight`, `turn_tool_stats`) |
| render da linha, glifo, spinner, grupos | `crates/tupi-tui/src/run.rs` (`transcript_layout_at`, `:1576`) |
| tick que anima | `crates/tupi-tui/src/run.rs` (`run_with_dashboard`, `:68`) |
| formatação da duração | `crates/tupi-tui/src/metrics.rs` (`format_duration`, `:114`) |
| duração que sobrevive à sessão | `crates/tupi-kernel/src/command.rs` (`TranscriptEntry::Tool.duration_us`) |

## Fluxo do dado

1. Quem executa mede: o laço de tools de `send_message` e `pipeline::execute`
   cronometram em volta da chamada e publicam
   `Event::ToolFinished { …, duration_us }` com `micros(duration).max(1)`.
2. A TUI lê o bus (não a telemetria): `push_tool_started` grava `Instant::now()`
   e `finish_tool` grava `Some(Duration::from_micros(duration_us))`.
3. O render usa `duration` quando existe e `now - started` enquanto não existe —
   e nunca reinicia o relógio: `now` chega por parâmetro em
   `transcript_layout_at`.
4. Sessão retomada: `TranscriptEntry::Tool` carrega `duration_us`, então
   `load_transcript` devolve a linha já com a duração gravada, não com o tempo
   desde a retomada.

## Invariantes

- **A duração é medida onde a tool roda.** Derivar `Instant` no consumidor daria
  errado: o evento atravessa o bus assíncrono e pode ser lido bem depois.
- **`duration_us > 0` sempre em produção.** O piso de 1µs mantém `0` livre para
  significar "não medido"; `finish_tool` recebe `Option<Duration>` e um evento sem
  medição não vira tempo inventado na tela.
- **O relógio nasce no estado** (`push_tool_started`), nunca no render: um
  `Instant` criado por frame zeraria o cronômetro a cada desenho.
- **Toda row do transcript nasce em `transcript_layout_at` com `owners.push(...)`.**
  Texto, tempo e marcador da tool são **uma única `Line` com um único owner** —
  é o que mantém o hit-test de seleção e a cópia por arraste coerentes.
- **Nada de tick em estado ocioso**: o `sleep` de 100 ms no `tokio::select!` só
  existe com `animating = tools_in_flight() > 0 || processing()`; a TUI não
  redesenha por tempo quando não há o que animar.
- **O web não muda**: `map_event` (`crates/tupi-web/src/events.rs:124`) consome
  `duration_us: _` e `WebEvent::ToolFinished` segue sem duração.
- **Placar e render usam a mesma fonte** (`tools_in_flight`/`turn_tool_stats`),
  para cabeçalho e linhas não divergirem.

## Testes que travam o formato

- `crates/tupi-tui/src/run.rs`: `tool_entries_render_as_a_compact_line`,
  `tool_in_flight_shows_spinner_and_elapsed_time`,
  `narrow_width_drops_the_time_but_keeps_the_tool_name`,
  `tool_run_of_three_gains_a_header_with_live_placar`,
  `short_tool_run_has_no_group_header`,
  `subagent_tools_render_indented_under_the_agent_line`,
  `copying_a_tool_line_keeps_the_displayed_duration`,
  `visual_review_of_a_multi_tool_turn`.
- `crates/tupi-tui/src/state.rs`: `finish_tool_updates_only_the_matching_call`,
  `tool_stats_count_failures_and_ignore_pipeline_children`,
  `load_transcript_rebuilds_every_entry`.

## Limitações conhecidas

- Dashboard web não exibe duração: o dado chega no evento e é descartado lá.
- Sub-chamadas irmãs de um passo de `pipeline` rodam em paralelo e chegam
  intercaladas; o agrupamento é por "execuções consecutivas de `Tool`", não por
  execução lógica, e a indentação é puramente visual (sem colapsar o grupo).
- Profundidade vem da contagem de `#` no `call_id`; um produtor novo de
  sub-chamada precisa manter o padrão `pai#passo.idx`, senão a linha aparece como
  irmã.
- O tempo é arredondado por `format_duration` (décimos até 60 s, `1m02s` depois):
  a linha não mostra microssegundos, mesmo os tendo.
