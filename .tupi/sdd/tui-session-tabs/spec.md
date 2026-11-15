<!-- Gerado automaticamente pelo tupi. Não edite; a fonte é meta.json/tasks.json. -->

# Abas de sessão paralelas na TUI (`tui-session-tabs`)

**Status:** draft

## Objetivo

Permitir várias conversas independentes na TUI (`crates/tupi-tui`), com uma barra de abas no topo, turnos rodando em paralelo e sessões que sobrevivem ao reinício. Hoje o app vive uma sessão só: `App` guarda um `SessionStore` (`crates/tupi-app/src/store.rs:5-7`) e `Command::NewSession` (`crates/tupi-kernel/src/command.rs:35-36`) descarta o histórico no lugar (`crates/tupi-app/src/app.rs:1557-1571`); a TUI tem um `ChatState` e um `request: Option<RequestId>` únicos (`crates/tupi-tui/src/run.rs:56-57`) e desenha 3 faixas sem topo (`crates/tupi-tui/src/run.rs:649-695`). Escopo: TUI + app + store + eventos. Fora de escopo: dashboard web (segue read-only em `crates/tupi-web/src/routes.rs:239-245`) e modo por aba.

## Restrições

- Decisões do usuário: alvo é a TUI; abas rodam em paralelo; sessões sobrevivem ao reinício; `/new` reinicia a aba atual e `/new-tab` abre uma aba nova.
- Fora de escopo: dashboard web. `/api/history` (`crates/tupi-web/src/routes.rs:239-245`) deve continuar respondendo como hoje, caindo na sessão corrente quando não há id.
- Não duplicar histórico: `SessionStore` continua sendo a única fonte das mensagens de uma sessão (`crates/tupi-app/src/store.rs:9-48`).
- Mudanças em `Command`/`Event` são aditivas: variantes e campos novos, sem remover nem renomear o que a web e os testes já consomem.
- O modo do agente (`mode_cell`, `crates/tupi-app/src/app.rs:369-416`) permanece global nesta entrega; Shift+Tab continua global.
- Persistência só no redb local via `crates/tupi-store`, com bump de `SCHEMA_VERSION` e criação de tabela em `ensure_schema` (`crates/tupi-store/src/lib.rs:275-333`); nada de arquivos paralelos.
- Gravação nunca no caminho quente do streaming: persistir em `StreamFinished`/`StreamFailed`, no fechamento de aba e no shutdown.
- Limite de abas simultâneas ativas (sugestão: 8) e teto de mensagens por sessão persistida precisam de valor explícito antes da implementação.

## Nuances

- Os eventos já carregam `request_id` (`crates/tupi-domain/src/lib.rs:125-201`) e cada turno ganha um id de `next_request`/`active` (`crates/tupi-app/src/app.rs:369-416`); abas paralelas significam N turnos vivos, então a TUI precisa de um mapa request_id → aba em vez do `request: Option<RequestId>` único (`crates/tupi-tui/src/run.rs:57`, filtros `Some(request_id) == request` no braço de eventos).
- `Event::Memory { text, after }` (`crates/tupi-domain/src/lib.rs:186-196`) e `Event::Notice` não têm sessão: sem um campo `session` a linha de memória de uma aba cai na aba ativa.
- O estado por conversa está espalhado em campos únicos do `App`: `session`, `active`, `pending`, `current`, `sdd_flow_started` (`crates/tupi-app/src/app.rs:369-416`). Paralelismo exige agrupá-los num `SessionRuntime` resolvido por sessão.
- A costura por sessão já existe no kernel e é ignorada: `CommandHandler::handle_in(…, session)` (`crates/tupi-kernel/src/command.rs:93-99`), `CommandBus::dispatch_in` (`:117-123`) e `QueryHandler::query_in` (`crates/tupi-kernel/src/query.rs:29-35`).
- `Memory::new_session` já raciocina por aba: fecha só a dívida de pendências do id anterior e gera id novo via contador de processo (`crates/tupi-app/src/memory.rs:145-179`).
- Comandos de barra são casados literalmente no braço de teclado (`/connect`, `/models`, `/new`, `crates/tupi-tui/src/run.rs:50-533`) e listados em `COMMANDS` (`crates/tupi-tui/src/state.rs:1`), com testes de autocomplete em `crates/tupi-tui/tests/`; `/new-tab` e `/tabs` precisam entrar nos dois lugares.
- `draw` monta `Layout::vertical([Min(1), composer, details])` (`crates/tupi-tui/src/run.rs:649-695`) e já tem modo `compact` (`height < 16 || width < 40`): a barra de abas é uma `Constraint::Length(1)` no topo e precisa degradar nesse modo.
- `open_overlay_with_ids` (`crates/tupi-tui/src/state.rs:388-400`) já permite um overlay com ids (usado por memória/modelos) — é o caminho natural para `/tabs`.
- `Query::History` é servido a partir do `SessionStore` (`crates/tupi-app/src/app.rs` e `crates/tupi-web/src/routes.rs:239-245`); sem sessão explícita deve resolver a sessão corrente para não quebrar web e testes.
- `tupi-domain` já tem `Session { id, messages }` (`crates/tupi-domain/src/lib.rs:83-86`) e `Message` com `role/content/tool_calls/tool_call_id` (`:42-47`), então a persistência pode serializar o `Session` inteiro; falta confirmar a definição de `SessionId` (importado em `crates/tupi-domain/src/lib.rs:83`, usado em `crates/tupi-kernel/src/command.rs:93`) antes de decidir a chave da tabela.
- `ensure_schema` recusa schema futuro (`FutureSchema`) e apenas reescreve a versão quando encontra uma anterior (`crates/tupi-store/src/lib.rs:275-333`): o bump não pode exigir migração de dados.
- `Settings` do store já serve para guardar a última aba ativa (`Store::setting`/`save_setting`, `crates/tupi-store/src/lib.rs:160-202`).

