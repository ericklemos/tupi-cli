# Detalhe inline da chamada de tool — bloco expansível

Como a TUI (`crates/tupi-tui`) abre, dentro do próprio transcript, os argumentos
de uma invocação de tool e o corpo do resultado: a linha
`{glifo} {nome} · {resumo} … {marcador}` continua sendo o **cabeçalho**, e o
detalhe entra como **linhas extras logo abaixo dele**. O gatilho é o clique na
linha ou `ctrl+o`. Fechado, o transcript segue exatamente o contrato de
`docs/features/tui-tool-usage.md` (uma linha por chamada, spinner, duração,
agrupamento de runs); este arquivo guarda só o que a expansão acrescenta.

## Formato aberto

```text
  {lead}{glifo} {nome} · {resumo}{preenchimento}{tempo} {marcador}
  {lead}args · {json pretty dos argumentos}
  {lead}out · {excerto do resultado}
```

- O cabeçalho é **inalterado**: mesmo `lead`, mesmo glifo, mesmo tempo e
  marcador. Abrir não reescreve a linha da chamada, só empilha linhas depois
  dela.
- `args · ` (`ARGS_MARKER`) só aparece com argumentos: o JSON é reformatado
  (`serde_json::to_string_pretty`) e quebrado na largura do bloco.
- `out · ` (`DETAIL_MARKER`) só aparece com resultado. `detail` vazio cai no
  `summary`; sem nenhum dos dois o bloco não ganha linha de resultado.
- Marcador **pendurado**: quando `width > marker + 8` o marcador divide a linha
  com o conteúdo e as continuações alinham sob ele; abaixo disso o marcador fica
  na própria linha e o texto usa a largura inteira. Nunca espreme o conteúdo em
  duas colunas para caber o marcador.
- Tool **em voo** mostra só `args · ` — não há `out · ` antes de `ToolFinished`.
- As linhas do bloco herdam o `lead`/recuo do cabeçalho e o recuo extra de
  subchamada (`#` no `call_id`), então o detalhe de um passo de `pipeline`
  aparece indentado sob o pai.
- Nada de spinner, duração ou marcador repetidos nas linhas extras: o bloco é
  só conteúdo.

## Onde mora

| peça | arquivo |
|---|---|
| teto de caracteres e recorte cabeça+cauda | `crates/tupi-domain/src/lib.rs` (`TOOL_DETAIL_MAX_CHARS`, `tool_detail_excerpt`) |
| campo `detail` no evento de fim | `crates/tupi-domain/src/lib.rs` (`Event::ToolFinished`) |
| publicação do excerto nas sub-chamadas | `crates/tupi-app/src/pipeline.rs` (`publish_finished`) |
| publicação do excerto no laço de tools | `crates/tupi-app/src/app.rs` (`send_message`) |
| `arguments`/`detail` no estado, conjunto de abertos | `crates/tupi-tui/src/state.rs` (`ChatEntry::Tool`, `push_tool_started`, `finish_tool`, `expanded`, `toggle_expanded`, `is_expanded`) |
| layout com blocos abertos | `crates/tupi-tui/src/run.rs` (`transcript_layout_expanded`) |
| linhas de argumentos e de resultado | `crates/tupi-tui/src/run.rs` (`args_rows`, `detail_rows`, `json_rows`) |
| roteamento do clique | `crates/tupi-tui/src/run.rs` (`entry_at_pointer`, `tool_entry_at`, `finish_click`) |
| atalho de teclado | `crates/tupi-tui/src/run.rs` (`shortcut_tool`, `toggle_tool_detail`) |
| anúncio no rodapé | `crates/tupi-tui/src/run.rs` (`draw_details`) |

## Fluxo do dado

1. **Argumentos.** `Event::ToolStarted` já traz `arguments` do executor; a TUI
   repassa em `push_tool_started(call_id, name, summary, arguments)` e o texto
   fica em `ChatEntry::Tool::arguments`. É mudança de estado, não de execução.
2. **Resultado.** Quem executa corta **uma vez**: `publish_finished` (e o laço de
   `send_message`) aplica `tupi_domain::tool_detail_excerpt` no corpo — sucesso
   ou erro — e publica `Event::ToolFinished { detail, .. }`.
3. **Consumo.** `finish_tool(call_id, ok, detail, elapsed)` grava o detalhe no
   mesmo `call_id`; detalhe vazio vira `None`, que no render significa "cai no
   `summary`".
4. **Render.** `transcript_layout_expanded` consulta `ChatState::expanded` e, para
   cada índice aberto, emite `args_rows` e `detail_rows`. Nenhuma leitura de
   disco, store ou telemetria acontece ali: o texto já chegou pronto.

## Gatilhos

- **Clique.** `MouseEventKind::Down(Left)` guarda `pressed_tool =
  tool_entry_at(...)` e inicia seleção; `Up` chama `finish_click`, que alterna o
  bloco **só** se soltou no mesmo índice e não houve arrasto. Com arrasto o mouse
  continua selecionando texto — inclusive sobre a linha da tool, que é o que
  permite copiá-la.
- **Teclado.** `ctrl+o` → `toggle_tool_detail` → `shortcut_tool`: a âncora da
  seleção manda quando cai numa linha de tool; sem seleção, o alvo é a chamada
  mais recente do transcript (teclado puro nunca teve ponteiro). Sem tool, a tecla
  deixa um aviso (`ctrl+o: nenhuma tool no transcript`) em vez de parecer
  quebrada. Com overlay aberto a tecla não é consumida — o foco é do overlay.
- **Rodapé.** O `ctrl+o` só é anunciado em `draw_details` quando existe tool no
  transcript: atalho anunciado que não faz nada é pior do que atalho invisível.

## Invariantes

- **Fechado = uma linha por chamada.** O bloco só **acrescenta** linhas coladas
  no cabeçalho; nada é substituído, e as linhas vizinhas ficam idênticas.
- **Toda linha nova nasce com `owners.push(Some(index))`**, o mesmo índice do
  cabeçalho: é isso que faz clicar numa linha de detalhe recolher o bloco e
  arrastar sobre ele copiar o bloco inteiro.
- **Estado só de UI.** `expanded` é `BTreeSet<usize>` de índices de
  `ChatEntry::Tool`; abrir/recolher não publica no bus nem toca no histórico
  enviado ao provedor. `reset_for_new_session` esvazia o conjunto.
- **Expandir não custa frame.** O layout é reconstruído por frame: nada de I/O,
  de consulta ao store ou de recálculo do output por conta própria.
- **Sem promessa de streaming.** `ToolFinished` é evento único: o detalhe chega
  inteiro ou truncado de uma vez.
- **A profundidade vem do `call_id`** (`#`), não do estado novo: um produtor novo
  de sub-chamada mantém o padrão `pai#passo.idx` ou o detalhe aparece como irmão.

## Tetos e marcas de corte

Dois tetos, dois donos — e **todo corte é anunciado**:

| teto | valor | onde | marca |
|---|---|---|---|
| caracteres | `TOOL_DETAIL_MAX_CHARS` (4 000) | `tupi-domain::tool_detail_excerpt` | `... (N caracteres omitidos) ...` no próprio texto |
| linhas de `args` | `ARGS_MAX_ROWS` (12) | `args_rows` | `… (N linhas omitidas)` |
| linhas de `out` | `DETAIL_MAX_ROWS` (12) | `detail_rows` | `… (N linhas omitidas)`, em estilo apagado |

- O recorte de caracteres é **cabeça + cauda** (cauda = 1/4 do teto, e a cabeça
  absorve o custo do marcador): o total nunca passa do teto e o fim do output —
  onde mora o erro de build — não desaparece.
- O critério de caractere é o mesmo de `crate::telemetry::clip` (limite de
  caractere, nunca byte no meio de UTF-8); só o número muda, porque aqui o
  consumidor é uma linha de transcript, não o arquivo de telemetria.
- Argumentos vazios ou JSON inválido rendem uma linha `—` explícita em vez de um
  bloco aberto em branco, que pareceria bug de desenho.
- Marcas de corte saem em cinza apagado: não são saída da tool, não devem se
  passar por conteúdo do resultado.

## Testes que travam o comportamento

- `crates/tupi-tui/src/run.rs`: `expanded_tool_block_only_adds_rows_under_its_header`
  (aberto == fechado + bloco colado, sem buraco e sem tocar as outras chamadas),
  `expanded_tool_bounds_the_result_and_announces_the_cut` (12 linhas, corte
  anunciado, um owner só), `expanded_tool_in_flight_shows_arguments_only`,
  `ctrl_o_toggles_the_selected_tool_then_the_latest`,
  `ctrl_o_without_tools_only_leaves_a_notice`,
  `subagent_tools_render_indented_under_the_agent_line`, e o review visual.
- `crates/tupi-tui/src/state.rs`: `toggle_expanded_twice_returns_to_the_initial_state`,
  `new_session_clears_the_expanded_blocks`,
  `tool_entry_keeps_the_arguments_and_the_finished_detail`,
  `finish_tool_updates_only_the_matching_call`.
- `crates/tupi-app`: o detalhe publicado é limitado e anuncia o truncamento.

## Limitações conhecidas

- **Sessão retomada perde o detalhe.** `TranscriptEntry::Tool` continua guardando
  só `call_id/name/summary/ok/duration_us`; `load_transcript` reconstrói
  `ChatEntry::Tool` com `arguments` vazio e `detail: None`. Reabrir o bloco numa
  sessão antiga mostra `—` nos argumentos e o `summary` no resultado. Persistir
  argumentos/detalhe no transcript salvo é decisão de produto separada.
- **Não há hover.** O gatilho é clique (down+up no mesmo bloco, sem arrasto) ou
  `ctrl+o`; passar o mouse não abre nada.
- **`ctrl+o` não escolhe bloco por ponteiro**: sem seleção de texto ele abre a
  chamada mais recente, não a que está sob o cursor.
- O detalhe é **texto plano quebrado por largura** (`json_rows`), sem markdown,
  syntax highlight ou numeração de linhas.
- Output maior que o teto nunca chega inteiro: o meio do corpo se perde no
  recorte de 4 000 caracteres, antes do corte de linhas.
- O dashboard web não exibe argumentos nem detalhe: o campo novo do evento não
  entra no `WebEvent`.
