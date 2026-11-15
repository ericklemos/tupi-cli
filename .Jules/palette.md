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
