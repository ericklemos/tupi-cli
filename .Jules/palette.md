# Palette — diário de UX

Apenas aprendizados críticos de UX/acessibilidade do dashboard (`crates/tupi-web/assets`) e da TUI (`crates/tupi-tui`).

## 2026-02-14 - Abas do dashboard eram só CSS, não semântica

**Learning:** a troca de abas em `crates/tupi-web/assets/app.js` era feita só alternando a classe `.active` em `#tabs button` e `main .panel`. Isso significa que qualquer trabalho de a11y nas abas precisa sincronizar TRÊS estados em JS — classe `.active` (visual), `aria-selected` (leitor de tela) e `tabindex` rotativo (ordem de Tab) — senão a interface "parece" certa e o leitor de tela anuncia outra coisa. Dois atalhos deste app: `.panel { display: none }` já esconde painéis inativos da árvore de acessibilidade (não precisa de `aria-hidden`), e a navegação por seta exige `role="tablist"` no contêiner — um `<nav>` sozinho não serve, porque seu papel implícito é `navigation`.

**Action:** ao mexer em qualquer widget de estado que use classes CSS (abas, filtros, toggles), centralize a transição numa única função (`selectTab`) que grava classe + ARIA no mesmo lugar; nunca duplique a lógica em handlers de `click` separados.

## 2026-02-14 - Na TUI, o rodapé de atalhos fica inerte quando um overlay abre

**Learning:** `draw_overlay` (`crates/tupi-tui/src/run.rs`) é modal: `run_with_dashboard` trata todas as teclas dentro do bloco do overlay e dá `continue`, então o rodapé que ensina "shift+tab: modo · ctrl+c copia · ctrl+q sai" continua visível enquanto nada dele funciona — a interface mentia sobre si mesma. Um modal nesta TUI precisa carregar seu próprio conjunto de teclas, e elas não podem ser genéricas: `enter` aplica o modelo, mas em memória/pendências apenas fecha, e `d` esquece um fato (destrutivo, sem confirmação).

**Action:** ao criar ou ajustar um modal, derive a legenda do MESMO estado que o handler consulta (título contendo "memória"/"pendências", `ids` vazios) para as dicas nunca divergirem do comportamento real. Pendente: confirmação dupla para `d` no overlay de memória.

## 2026-02-14 - Transcript vazio não ensinava nada; a orientação morava toda fora dele

**Learning:** numa sessão nova `ChatState::default()` deixa `lines` vazio e `transcript_layout_at` só devolvia preenchimento em branco — toda a orientação da primeira tela estava concentrada no placeholder do composer ("/modelos troca o modelo") e no rodapé de atalhos de `draw_details`. Os dois nunca somem, então qualquer dica nova no transcript precisa evitar repeti-los. E o transcript tem duas fontes de verdade: as `rows` que o usuário vê e `state.view` (`TranscriptView`), que é o que a seleção por mouse/hit-test usa. Um placeholder desenhado FORA de `transcript_layout_at` mantém a seleção coerente (não há texto real para copiar); se ele passar a ser selecionável, é obrigatório nascer como `rows` + `owners` dentro do layout.

**Action:** todo estado "zero itens" desta TUI deve renderizar a própria orientação em vez de confiar no composer; conteúdo decorativo no transcript não mexe em `state.view`/seleção, e ao torná-lo selecionável ele precisa ser gerado por `transcript_layout_at` com `owners`.

## 2026-02-15 - O placeholder do composer prometia um comando inexistente

**Learning:** o placeholder de `draw_composer` ensinava "/modelos troca o modelo", mas o handler de Enter só reconhece `/models` (`COMMANDS` em `crates/tupi-tui/src/state.rs:1`): quem seguia a dica recebia "comando desconhecido". O mesmo composer também escondia que Enter MUDA de significado com o turno rodando — ocioso ele abre um turno, ocupado ele enfileira (`state.enqueue`, que já aparece como "você · pendente" no transcript) e recusa comandos `/`. Na TUI não existe hover nem tooltip: as únicas superfícies de ensino são a cor do prompt, a borda/título do bloco e o placeholder, e todas são lidas de `state.status` no mesmo frame.

**Action:** nenhuma dica do composer pode ser uma lista escrita à mão — placeholder, título e cor devem sair da mesma condição que o handler consulta (`state.status != Status::Idle`, `COMMANDS`). Quando o efeito do Enter muda, o composer avisa antes da tecla; `notice` depois do erro é feedback tarde demais.

## 2026-11-14 - O diálogo no dashboard era o texto do modelo jogado no innerHTML

**Learning:** a lista "Mensagens" do dashboard (`#history-list` em `crates/tupi-web/assets/app.js`) é a única cópia do diálogo IA ↔ usuário fora da TUI, e interpolava `message.role` (slug cru em inglês) + `message.content.slice(0, 160)` direto no `innerHTML`. Dois defeitos só visíveis com conteúdo real: qualquer `<`/`&` do modelo (código, HTML, tags) virava markup e sumia, e o corte de 160 caracteres achatava uma resposta longa em uma linha só, sem sinal de que havia mais. `MessageDto` (`crates/tupi-web/src/routes.rs:217`) não carrega timestamp, então horário no diálogo exige mexer no backend — fora do escopo do Palette.

**Action:** ao renderizar conteúdo do modelo no dashboard, escape antes de `innerHTML` (`escapeHtml`) e identifique o autor por rótulo textual + cor (nunca só cor), com `white-space: pre-wrap` no corpo para preservar quebras; o padrão `<li class="msg msg-<role>">` + coluna fixa de rótulo é reutilizável em qualquer lista de diálogo.

## 2026-11-14 - Na TUI, "ao vivo" não é problema de render: é de loop

**Learning:** `run_with_dashboard` (`crates/tupi-tui/src/run.rs`) desenha uma vez no topo do laço e então **bloqueia** em `tokio::select!` com apenas duas fontes: teclado e `EventBus`. Não existe relógio. Consequência prática: nenhuma animação, cronômetro ou spinner desta TUI pode ser implementada só no render — sem uma branch de `sleep` no select, o `now` passado a `transcript_layout_at` só muda quando chega evento, e a duração de uma tool congelaria exatamente enquanto ela roda, que é quando o usuário mais precisa do sinal. O `now: Instant` que `transcript_rows`/`metrics.header` já recebem é a costura pronta: quem anima é o tick, não o desenho. E o tick precisa ser condicional (`tools_in_flight() > 0` ou `processing()`), porque esta TUI roda também sobre SSH e redesenhar em tela ociosa é desperdício puro.

**Action:** antes de aceitar qualquer pedido de "tempo real" na TUI, verifique se o loop tem fonte de tempo; se não tiver, a task de tick é pré-requisito da de render, nunca o contrário. Duração de operação vem do executor (medida onde a tool roda) e viaja no evento — derivar `Instant` no consumidor dá errado porque o evento atravessa o bus e é lido tarde.

## 2026-11-14 - O render do transcript existia em duas cópias que podiam divergir

**Learning:** `transcript_layout_at` (a que `draw_transcript` usa) e `transcript_rows_at` (usada pela seleção por arraste e pelos testes) eram duas implementações paralelas do MESMO layout. Trocar o formato da linha de tool só na primeira deixou a TUI desenhando `◆ read · a.rs 0,2s ✓` enquanto o teste — e a cópia por mouse — seguia vendo `⚙ read · a.rs ✓`. Numa interface onde o usuário copia texto do transcript, duas cópias do render são um bug de correção, não de estética: o texto copiado não é o texto exibido. `transcript_rows_at` agora delega para `transcript_layout_at` e devolve `.rows`.

**Action:** qualquer mudança visual no transcript nasce em `transcript_layout_at` e `transcript_rows_at` deve continuar sendo um delegador magro. Se aparecer necessidade de variar algo entre desenho e seleção, isso é sinal de que a variação deve ser parâmetro (`now`, `width`), não uma segunda função.

## 2026-11-14 - Fechar spec SDD dispara execução automática que pode morrer no meio

**Learning:** `sdd action=close` não só arquiva a spec: ele **executa** as subtasks. No `tui-tool-usage` a execução parou em "limite de 24 passos" na T1.2, mas o relatório dizia "1/20 subtasks ok" enquanto o código real já tinha ido bem além (publishers preenchidos, `cargo check --workspace` limpo). O progresso reportado pelo SDD é grosseiro e **mente para baixo**; o único juiz é compilar e rodar testes das crates afetadas. O dano concreto que sobrou: `crates/tupi-web/src/events_tests.rs` continuava construindo `Event::ToolFinished` sem o campo novo, e isso só aparece em `cargo test` — `cargo check` não compila `cfg(test)`. A execução também apagou `.Jules/palette.md` da árvore de trabalho (recuperável em `git show HEAD:.Jules/palette.md`).

**Action:** depois de qualquer `sdd close` ou execução interrompida, rode `cargo test -p` nas crates cujos **testes** constroem os tipos alterados, e confira que arquivos de estado do agente (`.Jules/`) sobreviveram. Ao adicionar campo obrigatório em enum de domínio, procure também as construções dentro de `#[cfg(test)]`.
