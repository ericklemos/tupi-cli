<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Detalhe inline da invocação de tool no transcript da TUI (`tool-detail-inline`)

**Status:** completed

## Objetivo

Permitir abrir, no próprio transcript da TUI, os detalhes internos de uma chamada de tool (argumentos da invocação e conteúdo do resultado), com a linha atual de uma linha por chamada virando cabeçalho de um bloco expansível. O gatilho é o clique/hover na linha (já há mouse capture e hit-testing de linhas) mais uma tecla equivalente para teclado puro. Hoje a TUI mostra `name · summary ✓` e descarta tudo o mais: `arguments` chega em `Event::ToolStarted` (crates/tupi-domain/src/lib.rs:143-219) e é jogado fora por `ChatState::push_tool_started` (crates/tupi-tui/src/state.rs:172-215), e o corpo do resultado nunca sai do executor — `publish_finished` publica só o resumo truncado (crates/tupi-app/src/pipeline.rs:270-293).

## Restrições

- Fechado, o transcript continua uma linha por chamada: o layout vive em `transcript_layout_at` (crates/tupi-tui/src/run.rs:1576-1879) e o detalhe entra como linhas extras, nunca como substituição do cabeçalho.
- O dado de output não existe hoje na TUI: `Event::ToolFinished` (crates/tupi-domain/src/lib.rs:143-219) carrega só `summary` + `duration_us`. Ou o evento ganha um campo de detalhe limitado, ou a TUI pede sob demanda ao app — decidir antes de codar, sem inventar conteúdo no render.
- Teto rígido de linhas/bytes por bloco expandido (o transcript é reconstruído a cada frame e rola por linhas): nada de despejar um output de 40 KB em `rows`.
- Expandir não pode custar frame: nenhuma leitura de disco, store ou telemetria dentro de `transcript_layout_at` (crates/tupi-tui/src/run.rs:1576-1879).
- Não quebrar o agrupamento de runs nem o recuo de subchamadas (`call_id` com `#`, crates/tupi-tui/src/state.rs:172-215): o detalhe de uma subchamada aparece indentado sob o pai.
- Estado só de UI: expandir/recolher não publica evento no bus nem altera o histórico enviado ao provedor.
- Não prometer streaming do output: `ToolFinished` é um evento único e o detalhe chega inteiro (ou truncado) de uma vez.

## Nuances

- O gancho de clique já existe e está morto: `hit_test` (crates/tupi-tui/src/run.rs:1882-1902) recebe `owners` + `pointer` e não tem chamador; `Transcript { rows, owners }` (run.rs:1569-1573) já devolve o mapa linha→índice de `ChatEntry`.
- `owners` hoje só é consumido para seleção de texto (`transcript_position`, run.rs:849-865) e a view é publicada em `TranscriptView` por `draw_transcript` (run.rs:896-980).
- Mouse capture já está ligado no TerminalGuard (crates/tupi-tui/src/terminal.rs:13-79), mas não há tratamento de `MouseEventKind` para linhas do transcript — o clique precisa ser roteado até o `ChatEntry` sob o cursor.
- `arguments` já trafega pronto em `Event::ToolStarted` dentro de `pipeline::execute` (crates/tupi-app/src/pipeline.rs:103-268); copiá-lo para `ChatEntry::Tool` é mudança de estado, não de execução.
- Cliping de output já tem critério pronto: `crate::telemetry::clip` é usado em `record_finished` (crates/tupi-app/src/pipeline.rs:113-151) com chars/bytes/truncated — reusar em vez de criar outro truncador.
- Sessão retomada perde o detalhe: `TranscriptEntry::Tool` guarda call_id/name/summary/ok/duration_us (reconstrução em crates/tupi-tui/src/state.rs:508-536); acrescentar argumentos/detalhe ao transcript salvo é uma decisão de produto separada.
- `summary` já vem descrito pela tool (`path=a.rs · symbol=new`) e é compactado por `compact_summary` (crates/tupi-tui/src/state.rs:656-663); o detalhe expansível é o que o resumo não cabe.
- Linhas de detalhe precisam herdar o recuo do bloco (`indent`, run.rs:896-980) e a mesma largura usada por `plain_rows`/`wrap_text` para não desalinhar o transcript.

## Tasks

- [x] **T1** Estado guarda os argumentos da invocação
  - Em crates/tupi-tui/src/state.rs:41-70, `ChatEntry::Tool` ganha `arguments: String` e `detail: Option<String>`. `push_tool_started` (state.rs:172-215) passa a receber `arguments` e o grava; `finish_tool` (state.rs:240-254) passa a receber `detail: Option<String>` e o grava no mesmo `call_id`. Ajustar os construtores de `ChatEntry::Tool` em `load_transcript` (state.rs:508-536) com `arguments` vazio e `detail: None` (detalhe efêmero: transcript salvo não muda). Atualizar os testes de state.rs que chamam `push_tool_started` (ex.: `finish_tool_updates_only_the_matching_call`, `subagent_tools_are_inserted_under_their_own_agent`).
  - [x] **T1.1** Estender ChatEntry::Tool e os setters de estado — `cargo test -p tupi-tui` compila e os testes de state.rs passam com as novas assinaturas.
  - [x] **T1.2** Repassar `arguments` do event ao estado no run loop — O braço de `Event::ToolStarted` em `run_with_dashboard` (crates/tupi-tui/src/run.rs:51-677) usa o campo `arguments` já publicado por `pipeline::execute` (crates/tupi-app/src/pipeline.rs:103-268) em vez de descartá-lo.
- [x] **T2** Evento de fim de tool carrega um detalhe limitado do output
  - Em `Event::ToolFinished` (crates/tupi-domain/src/lib.rs:143-219) acrescentar `detail: String` documentado como excerto com teto (head+tail, marcador explícito de corte). Preencher em `publish_finished` (crates/tupi-app/src/pipeline.rs:270-293) reusando o critério de `crate::telemetry::clip` já aplicado em `record_finished` (pipeline.rs:113-151). Constante do teto (ex.: `TOOL_DETAIL_MAX_CHARS`) junto do evento, não no render. Propagar o campo em todo consumidor do evento (grep `ToolFinished` em crates/, incluindo collectors de teste de tupi-app e tupi-web/crates/tupi-web/src/events.rs).
  - [x] **T2.1** Campo `detail` no variante e no publisher — O detalhe publicado é limitado por constante e nunca zero-linha sem marcador de truncamento; testes de pipeline e de eventos continuam verdes.
  - [x] **T2.2** Consumidor da TUI usa o detalhe — `finish_tool` recebe o detalhe no braço de `Event::ToolFinished` e o mantém só em memória.
- [x] **T3** Expansão por clique e por teclado
  - `ChatState` (state.rs:79-103) ganha `expanded: std::collections::BTreeSet<usize>` (índices de `ChatEntry::Tool`) com `toggle_expanded(index)`/`is_expanded(index)`, limpo em `reset_for_new_session` (state.rs:645-653). Clique: usar `hit_test` (run.rs:1882-1902) — hoje sem chamador — sobre `Transcript.owners` com o `pointer` salvo pela view publicada em `draw_transcript` (run.rs:896-980), tratando `MouseEventKind::Down/Up` no laço de eventos de `run_with_dashboard`. Tecla: `ctrl+o` alterna a tool sob o cursor/seleção quando houver, sempre com fallback documentado no rodapé de `draw_details` (run.rs:1193-1359) — sem prometer tecla que não faz nada.
  - [x] **T3.1** Conjunto de expandidos no estado — Alternar duas vezes volta ao estado inicial; sessão nova começa vazia; teste unitário em state.rs cobre toggle e reset.
  - [x] **T3.2** Roteamento do clique até o ChatEntry sob o cursor — Clique numa linha de detalhe recolhe o bloco (todas as linhas do bloco apontam para o mesmo índice em `owners`); clique fora do transcript abre seleção de texto como hoje.
  - [x] **T3.3** Atalho de teclado equivalente — `ctrl+o` alterna o bloco da tool selecionada e não interfere em `ctrl+letter` já mapeados (`is_shortcut_char`, run.rs:744-746).
- [x] **T4** Render do bloco expandido no transcript
  - Em `transcript_layout_at` (run.rs:1576-1879), quando o índice estiver em `expanded`, emitir linhas extras logo após o cabeçalho: (a) `args · ` com o JSON de argumentos quebrado por `plain_rows` na largura `content_width`; (b) o `detail` (ou `summary` quando ausente) limitado a 12 linhas, com marca explícita de corte e o mesmo teto em caracteres da constante de T2; (c) todas as linhas novas empurram `owners.push(Some(index))` para o clique funcionar. Herdar o recuo de bloco (`lead`/`indent`) e o recuo extra de subchamada (`depth`, `call_id` com `#`). Nenhum I/O dentro do laço; nada de reaproveitar `duration`/spinner nas linhas de detalhe.
  - [x] **T4.1** Argumentos formatados no bloco — JSON inválido ou vazio renderiza uma linha `—` em vez de sumir silenciosamente; largura estreita (`< 60`) não estoura a coluna do marcador.
  - [x] **T4.2** Excerto do resultado com teto e marca de corte — Bloco nunca passa de 12 linhas de corpo; corte sempre anunciado; tool em voo sem `detail` mostra só argumentos.
  - [x] **T4.3** Recolhido continua uma linha por chamada — Teste de layout existe hoje (`visual_review_of_a_multi_tool_turn`, run.rs:2761) e passa inalterado; expandido/recolhido produz exatamente as linhas do cabeçalho mais o detalhe.
- [x] **T5** Cobertura de teste e regressão
  - Testes em crates/tupi-tui/src (layout e estado) e crates/tupi-app (detalhe publicado). Cobrir: expansão de subchamada indentada sob o pai, clique em linha de detalhe recolhendo o bloco, teto de 12 linhas com marca de truncamento, e `load_transcript` sem detalhe (sessão retomada). Rodar `cargo test -p tupi-tui -p tupi-app -p tupi-domain` e o teste visual existente.
  - [x] **T5.1** Testes de layout expansível — Cobre aberto, fechado e subchamada; falha se o bloco passar do teto ou perder o recuo.
  - [x] **T5.2** Testes de estado e de evento — Toggle/reset em state.rs e campo `detail` preenchido com truncamento anunciado em tupi-app.
