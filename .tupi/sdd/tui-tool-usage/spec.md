<!-- Gerado automaticamente pelo tupi. Não edite; a fonte é meta.json/tasks.json. -->

# Exibição viva do uso das ferramentas na TUI (`tui-tool-usage`)

**Status:** failed

## Objetivo

Tornar o uso das ferramentas visível e vivo apenas na TUI (crates/tupi-tui): cada chamada de tool passa a mostrar tempo decorrido ao vivo, resultado final com duração, ícone/cor por categoria e agrupamento por turno, com sub-chamadas de `pipeline` indentadas sob o pai em vez de competirem em pé de igualdade.

Estado atual (fontes):
- `crates/tupi-tui/src/run.rs:1549-1580` — cada tool é uma linha `⚙ nome · resumo ✓/✗/…`, sem tempo, sem categoria, sem hierarquia.
- `crates/tupi-tui/src/state.rs:27-49` — `ChatEntry::Tool { call_id, name, summary, ok }`: única informação disponível.
- `crates/tupi-tui/src/state.rs:147-163` / `:188-197` — `push_tool_started` e `finish_tool` apenas gravam `ok`.
- `crates/tupi-tui/src/run.rs:50-518` — `run_with_dashboard`: desenha no topo do loop e bloqueia em `tokio::select!` (chaves + eventos), então nada anima sem evento novo.
- `crates/tupi-domain/src/lib.rs:172-179` — `Event::ToolFinished { request_id, call_id, name, ok, summary }`, sem duração.
- `crates/tupi-app/src/pipeline.rs:275-292` — `publish_finished` já tem `duration` em mãos (medida em `:170-200`) e a descarta ao publicar.
- `crates/tupi-tui/src/metrics.rs:114-121` — `format_duration` já formata segundos/minutos com vírgula decimal: reutilizar.
Escopo: só TUI como superfície final; alteração mínima e aditiva no evento de domínio para carregar a duração; dashboard web continua funcionando sem exibir duração.

## Restrições

- Superfície de saída é apenas a TUI (crates/tupi-tui); nenhuma mudança visual no dashboard web (crates/tupi-web).
- Dashboard web deve continuar compilando: `map_event` desestrutura `Event::ToolFinished` campo a campo (crates/tupi-web/src/events.rs:193-210), então o campo novo exigirá `duration_us: _` lá.
- Nenhuma dependência nova em Cargo.toml; nenhum crate novo.
- Sem emoji/glifos de largura ambígua no terminal: usar apenas caracteres de largura 1 já usados no código (⚙ ✓ ✗ … ◆ ┃ │) ou braille de spinner se validado na largura.
- Qualquer texto novo no transcript precisa nascer como row + owner dentro de transcript_layout_at (crates/tupi-tui/src/run.rs:1530); rows desenhadas fora quebram a seleção por mouse (state.view).
- Nada de tick em estado ocioso: o redesenho periódico só existe enquanto há tool em voo ou turno processando.
- Não alterar lógica de execução de tools, nem telemetria, nem contadores existentes (StatsDto, tool_calls/tool_failures).
- Testes existentes que travam o formato atual precisam ser atualizados, não contornados: `tool_entries_render_as_a_compact_line` (crates/tupi-tui/src/run.rs:2255) exige `⚙`, `read · src/a.rs` e `✓`.

## Nuances

- A TUI não redesenha por tempo: `terminal.draw` (crates/tupi-tui/src/run.rs:66) só roda quando chega tecla ou evento. Sem uma branch de tick no `tokio::select!` (:68-77), duração e spinner ficam parados até o próximo evento — ou seja, 'ao vivo' é impossível só mexendo no render.
- A duração já é medida duas vezes e jogada fora: `pipeline.rs` mede `started.elapsed()` por chamada interna (:204, :232) e `app.rs` mede a chamada externa no laço de tools; ambas publicam `Event::ToolFinished` sem o número.
- `TelemetryEvent::ToolFinished` (crates/tupi-app/src/telemetry.rs) já carrega `duration_us` — o dado existe no canal de telemetria, mas a TUI não lê telemetria, e sim o EventBus.
- Sub-chamadas de pipeline publicam `ToolStarted`/`ToolFinished` próprios com `call_id` no formato `pai#passo.idx` (crates/tupi-app/src/pipeline.rs:158-160), além do `ToolFinished` do pai: um pipeline de N passos gera 2N+2 linhas indistinguíveis no transcript.
- `compact_summary` (crates/tupi-tui/src/state.rs:472-479) trunca o resumo antes de guardar; uma linha de tool com tempo+glifo precisa de orçamento de largura para o resumo, senão o tempo empurra o resumo para fora.
- `format_duration` usa vírgula decimal ("1,2s") e minutos a partir de 60s ("1m02s"): reutilizar mantém o transcript coerente com o header de métricas.
- O transcript suporta `compact` (largura < 60 em run.rs:1544): em tela estreita o tempo deve encolher ou sumir por precedência, nunca truncar o nome da tool.
- `TranscriptView`/selection guardam `width` e usam transcript_rows para hit-test: se o tempo for desenhado em coluna alinhada à direita, ele deve fazer parte da mesma `Line` do texto da tool (mesma owner) para não dessincronizar a cópia por arraste.
- Chamadas irmãs do mesmo passo de pipeline saem em paralelo (`buffer_unordered`, pipeline.rs:225-250) e chegam intercaladas: agrupar por execução exigiria estado novo; agrupar por 'execuções consecutivas de Tool no transcript' é determinístico e sem estado extra.
- `ChatEntry::Tool` não guarda `Instant`: o relógio tem que nascer em `push_tool_started` (fonte única no estado), nunca no render, senão cada frame reinicia o cronômetro.

