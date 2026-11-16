# Arquitetura

## Camadas

Microkernel + CQRS, em que o binário é o único ponto que conhece todos os
crates:

| crate | papel |
|---|---|
| `tupi-domain` | tipos puros (`Session`, `Message`, `Event`) |
| `tupi-kernel` | contratos (`Provider`, buses, registry) |
| `tupi-providers` | um módulo por provedor |
| `tupi-tools` | AST, orçamento, store/peek, ledger, grafo e as tools |
| `tupi-app` | handlers de comando/query, loop de tools e eventos |
| `tupi-tui` | ratatui: envia comandos, reage a eventos |
| `tupi` | composição: env, registry, tools, app, TUI |

## Fronteiras

- A TUI não conhece o provider; o provider não conhece a TUI.
- Efeito colateral (rede, arquivo, banco) fica na borda: `tupi` e `tupi-tools`.
- O root das leituras de tool é o diretório onde o Tupi foi aberto; caminho que
  escapa dele é recusado.
- Medição de tempo pertence a quem executa: `Event::ToolFinished` carrega
  `duration_us` medido por `tupi-app`; o consumidor formata o número, nunca o
  reinventa a partir de `Instant` (o evento atravessa o bus e chega tarde).
- Subcomando de manutenção não sobe a pilha da TUI: `tupi install` responde no
  `main` antes de env, redb e provider (ver
  [`features/tupi-install.md`](features/tupi-install.md)).
- Poder de execução só encolhe: o `shell` recusa antes de rodar o que está fora
  da allowlist e nenhuma correção dele amplia o que pode ser executado (ver
  [`features/shell-tool.md`](features/shell-tool.md)).

## Componentes

- **Grafo e busca** (`tupi-tools`): walk + parse paralelos, cache de grafo com
  single-flight, busca lexical sobre os símbolos; números em
  [`perf/baseline.md`](perf/baseline.md).
- **Tools** (`tupi-tools`): `read`/`search`/`map`/`peek`, edição com hash,
  `exec`/`test_many`, grafo e web. `AGENTS.md` é gerado por `setup`. O `shell` é
  inspeção crua: allowlist restrita, recusa que ensina a alternativa e nenhuma
  memória de projeto no resultado (ver
  [`features/shell-tool.md`](features/shell-tool.md)).
- **Transcript da TUI**: `ChatState` guarda o relógio e a duração de cada tool;
  `transcript_layout_at` desenha linha viva, glifo por categoria e indentação de
  sub-chamadas (ver [`features/tui-tool-usage.md`](features/tui-tool-usage.md)).
- **Sessão**: redb local (`$XDG_DATA_HOME/tupi/tupi.redb`), ledger e state por
  repositório.
- **Instalação**: `crates/tupi/src/install.rs` — `bin_dir`, `detect_shell`,
  `rc_path`, `block`, `apply`, `plan`, `run`; testes hermetizados com ambiente
  injetado.
