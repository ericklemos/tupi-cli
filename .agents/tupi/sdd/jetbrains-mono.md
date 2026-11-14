<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# JetBrains Mono + ligaduras na interface do tupi (`jetbrains-mono`)

**Status:** draft

## Objetivo

Fazer a interface do tupi usar JetBrains Mono com ligaduras habilitadas nas superfícies onde o tupi controla a tipografia — hoje apenas o dashboard web (`crates/tupi-web/assets/styles.css`, servido por `crates/tupi-web/src/assets.rs`). Para a TUI (`crates/tupi-tui`) o limite é o emulador de terminal: no máximo documentar/recomendar a configuração, sem código de fonte no app.

## Restrições

- TUI: ratatui não tem noção de família de fonte nem de ligadura; `ChatStyleSheet` (crates/tupi-tui/src/markdown.rs:19) só manipula cor/`Modifier`. Nada de fonte pode ser imposto pelo app na TUI.
- Sem peso novo no repositório além do estritamente necessário: qualquer woff2 embutido precisa justificar os bytes (e o `guard` em routes.rs:70 marca `Cache-Control: no-store` em toda resposta, então fonte servida por rota é rebaixada a cada page load).
- Não introduzir dependência JS/CSS de CDN: o dashboard é local/loopback e precisa funcionar offline.
- Não quebrar alinhamento colunar nem a cópia de texto: a transcrição da TUI depende de célula de largura fixa.
- Escopo restrito a tipografia; não redesenhar layout nem paleta (as cores de `:root` e `ChatStyleSheet` ficam como estão).

## Nuances

- Superfície web: `crates/tupi-web/assets/styles.css:22` define `font: 14px/1.5 ui-sans-serif, system-ui, sans-serif` no `body`; só `#timeline li` usa `ui-monospace, monospace`. Ou seja, hoje o dashboard é sans-serif e não referencia JetBrains Mono em nenhum ponto.
- Os assets são embutidos por `include_str!` (crates/tupi-web/src/assets.rs:5-9) e servidos por rotas `get` explícitas em routes.rs:66-69; não há rota coringa de estáticos. Servir woff2 exige rota nova + `include_bytes!` (não `str`).
- Ligaduras de JetBrains Mono (setas, `!=`, `=>`) vêm da feature OpenType `calt`, que o navegador já liga por padrão com `font-variant-ligatures: normal`; o trabalho real é (a) a família ser de fato JetBrains Mono e (b) nenhuma regra desligar ligaduras. `font-feature-settings: "calt" 1` é o que garante o resultado em navegadores/SO onde o default é conservador.
- Estética: trocar o `body` inteiro para monoespaçada muda a leitura de textos de prosa (memória, fatos, pendências). Alternativa cirúrgica: mono em dados técnicos (timeline, grafo, paths, ids) e JetBrains Mono como fonte do código.
- `crates/tupi-web/assets/graph.js` desenha rótulos com `ctx.fillText` sem nunca setar `ctx.font` — o canvas cai no default `10px sans-serif`. Se a tipografia importa, esse é o ponto onde o grafo fica visualmente fora do padrão.
- Se a fonte não estiver instalada no SO do usuário, `font-family: "JetBrains Mono", ...` degrada silenciosamente para o fallback e as ligaduras desaparecem — a única forma de garantir a fonte é embarcar woff2.
- Na TUI, ligaduras são config do terminal (ex.: WezTerm/Ghostty/Kitty com `"calt" 1`), não do tupi; comportamento correto é documentar, não tentar codar.

## Tasks

