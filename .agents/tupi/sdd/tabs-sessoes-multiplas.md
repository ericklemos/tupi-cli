<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Tabs de sessão estilo navegador na TUI do Tupi (`tabs-sessoes-multiplas`)

**Status:** closed

## Objetivo

Permitir múltiplas sessões de conversa vivas e evoluindo em paralelo no Tupi (TUI), onde cada aba é uma sessão apartada: histórico próprio (`SessionStore`), `Memory` próprio (session id, dívida de recall e cache próprios), `ChatState` próprio e request em voo próprio. O ledger de fatos do repositório (`.agents/tupi/facts.jsonl`) permanece compartilhado por design. Arquitetura decidida: (B) `Sessions` em `tupi-app` — `App` vira roteador e cada sessão tem seu `SessionRuntime` — com `session: SessionId` no protocolo `Command`/`Query`/`Event`.

## Restrições

- Economia de tokens: nenhuma mudança pode duplicar payload de evento por aba; `EventBus` é broadcast com capacidade 256 (`crates/tupi-kernel/src/event.rs:10-13`).
- Edições cirúrgicas com `edit`; não reescrever arquivos inteiros para mudar trechos.
- Um único dono do mapa de abas (sem estado global espalhado entre TUI, `main.rs` e `App`).
- Sessão de aba é in-memory (histórico em `SessionStore`), mas memória de fatos é do repositório e continua compartilhada — não fragmentar `facts.jsonl` por aba.
- Não quebrar contratos existentes de `Command`/`CommandOutcome`/`Event` sem contar o custo em `tupi-web/src/events.rs` e nos testes de `tupi-app/tests`.
- A TUI não pode bloquear o desenho esperando provider: turnos já são tarefas assíncronas com eventos por `request_id`.
- A fila de pendências passa a ser append-only (decisão tomada): nada de read-modify-write do arquivo inteiro.
- Dedup de leitura (`DeliveryLedger`/`ResponseStore`/`ParseCache`) é única por processo e repositório — compartilhada entre abas, nunca por aba.

## Nuances

- `App` é `Arc<Self>` com estado de turno único — `active`, `current` (StreamHandle), `pending` (`crates/tupi-app/src/app.rs:362-401`); `send_message` recusa concorrência com `CommandError::Busy`. Turnos simultâneos exigem um `App` por aba ou tornar `active` um mapa por sessão (escolhido: estado de turno migra para `SessionRuntime`).
- `CommandBus`/`QueryBus` guardam um único handler (`crates/tupi-kernel/src/command.rs:92-94`, `crates/tupi-kernel/src/query.rs:30-32`) e `Command` não carrega id de sessão (`crates/tupi-kernel/src/command.rs:19-52`): a rota escolhida é `dispatch_in(session, …)` mantendo `dispatch(…)` no foco.
- A TUI já filtra eventos por `request_id` (`Some(request_id) == request`, `crates/tupi-tui/src/run.rs:52-485`): tokens, tools, contexto e falha já chegam rotulados. Os eventos sem `request_id` são os ambíguos com N abas: `ModelsLoaded`, `ModelsFailed`, `Notice`, `Memory`, `ModeChanged`, `ContextBudget` (`crates/tupi-domain/src/lib.rs:125-194`).
- Memória tem três camadas com donos diferentes: (1) ledger de fatos `.agents/tupi/facts.jsonl`, escopo do repositório, dedup por `fact_id` FNV-1a e tombstones (`crates/tupi-domain/src/memory.rs:29-173`); (2) fila de pendências, hoje arquivo único regravado inteiro por `save_pendings` (`crates/tupi-domain/src/memory.rs:220-231`); (3) estado in-memory por instância — `session`, `first_turn`, `sequence`, `cache` (`crates/tupi-app/src/memory.rs:93-100`).
- 'Memória própria por aba' = uma instância de `Memory` por aba (session id, dívida de recall e cache próprios), sobre o mesmo ledger do repositório. Compartilhar uma única `Memory` faria `/new` numa aba zerar a dívida de recall e o cache de todas (`Memory::new_session`, `crates/tupi-app/src/memory.rs:154-166`).
- O ledger de fatos tolera escritores concorrentes: `append` usa `OpenOptions::append(true)` (`crates/tupi-domain/src/memory.rs:123-133`). A fila de pendências NÃO tolerava: `save_pendings` regrava o arquivo inteiro (`crates/tupi-domain/src/memory.rs:220-231`) e `Memory::new_session` faz read-modify-write nela (`crates/tupi-app/src/memory.rs:154-166`). Decisão: migrar para append-only (`open_pending`/`close_pending` como linhas de log, leitor reconciliando + compactação), eliminando a necessidade de lock entre abas.
- `Memory::next_session` é `{pid}-{now}-{sequence}` com `sequence` começando em 1 por instância (`crates/tupi-app/src/memory.rs:144-147`): duas instâncias de `Memory` criadas no mesmo segundo, no mesmo processo, geram o MESMO session id — colisão real com uma instância por aba. O id passa a ser gerado fora (contador global ou `Sessions`).
- `ChatState` concentra transcript, input, scroll, seleção, overlay, contexto e cache de render (`crates/tupi-tui/src/state.rs:58-78`): é a unidade que vira por aba. `model`/`provider` hoje são variáveis locais de `run_with_dashboard` (`crates/tupi-tui/src/run.rs:52`).
- `SessionStore::new(SESSION_ID)` usa id constante por `App` (`crates/tupi-app/src/app.rs:449-506`); `/new` faz `replace_messages(Vec::new())` + `memory.new_session()` (`App::handle`, `crates/tupi-app/src/app.rs:1497-1510`).
- MODO POR SESSÃO (decidido). O gate é lido em tempo de chamada — `require_sdd_mode`/`require_execution_mode` (`crates/tupi-tools/src/sdd.rs:231-247`) — e o catálogo de specs vem de `ToolRegistry::specs_for_mode`, que só filtra por `Tool::sdd_allowed` (`crates/tupi-kernel/src/tool.rs:131-141`). Com célula compartilhada, alternar para planning na aba B bloquearia a edição de um turno JÁ EM CURSO na aba A: contaminação de comportamento, não só de UI.
- A via barata para modo por aba: `ToolRegistry` é apenas `Vec<Arc<dyn Tool>>` sem `Clone` (`crates/tupi-kernel/src/tool.rs:114-116`). Construir o catálogo UMA vez com `register_with` e derivar por sessão um registry com os mesmos `Arc` + um `SddTool` próprio (com a sua `AtomicU8`) compartilha de graça `ResponseStore`, `DeliveryLedger`, `ParseCache` e `BacktrackGraph` — que `register_with` hoje recria por chave própria (`crates/tupi-tools/src/lib.rs:124-160`). Chamar `register_with` N vezes é exatamente o que fragmenta a dedup.
- O `DeliveryLedger` é o mecanismo que evita re-entrega: `check` devolve `Seen`/`Changed` em vez do corpo quando o arquivo não mudou (`crates/tupi-tools/src/ledger.rs:110-200`). Fragmentá-lo por aba viola a regra de economia de tokens do AGENTS.md (cada corpo relido é pago de novo em todo passo).
- `App` guarda a célula de modo em `mode` com `with_mode_cell`/`mode()` (`crates/tupi-app/src/app.rs:419-429`): migra para `SessionRuntime`, e `Event::ModeChanged` passa a identificar a sessão.
- `main.rs` monta um `App` único + um `BusSet` e `DashboardContext` guarda `model`/`provider` fixos (`crates/tupi/src/main.rs:17-295`); o dashboard web é inspeção read-only (`crates/tupi-web/src/routes.rs:26-29`) e não tem conceito de sessão.
- `ChatState` já tem fila de mensagens por turno (`enqueue`/`dequeue`, `crates/tupi-tui/src/state.rs:205-222`) e métricas de reply por streaming — ambos naturalmente por aba.

## Tasks

- [~] **T1** Protocolo ciente de sessão (tupi-domain + tupi-kernel)
  - Objetivo: cada `Command`, `Query` e evento ambíguo passa a nomear a sessão, sem reescrever os ~56 call sites de teste em `crates/tupi-app/tests`.
  - [x] **T1.1** `CommandBus::dispatch_in(session, command)` mantendo `dispatch(command)` no foco — `CommandBus` ganha `dispatch_in`; `dispatch` injeta o foco atual. Testes existentes de `tupi-app/tests` compilam sem alteração.
  - [ ] **T1.2** `Event`: `session: SessionId` nos 6 variantes sem `request_id` — `Memory`, `Notice`, `ModelsLoaded`, `ModelsFailed`, `ModeChanged` e `ContextBudget` (`crates/tupi-domain/src/lib.rs:125-194`) carregam `session`. `Token`/`ToolStarted`/`ToolFinished`/`ContextUpdated`/`ContextTrimmed`/`StreamFinished`/`StreamFailed`/`PromptMessages` não mudam (já roteáveis por `request_id`).
  - [x] **T1.3** `Query::History` por sessão com foco como default — `Query::History` lê o histórico da sessão pedida; sem sessão explícita usa o foco, preservando `history` (`crates/tupi-app/tests/pipeline.rs:173`).
- [x] **T2** `Memory` independente por sessão sobre o ledger do repositório
  - Três camadas: ledger de fatos (repositório, compartilhado), fila de pendências (append-only, decisão tomada) e estado in-memory (por sessão).
  - [x] **T2.1** Injetar `SessionId` na `Memory` e eliminar a colisão de `next_session` — `Memory::next_session` (`crates/tupi-app/src/memory.rs:144-147`) deixa de derivar id de `{pid}-{now}-{sequence}` local; duas instâncias criadas no mesmo segundo no mesmo processo geram ids distintos (teste novo).
  - [x] **T2.2** Migrar a fila de pendências para log append-only — `open_pending`/`close_pending` viram linhas de log append-only; leitura reconcilia o log aplicando os fechamentos; compactação reescreve só quando o log passa de um limiar. Teste: 2 sessões fechando pendências ao mesmo tempo preservam os 2 fechamentos (sem lock global).
  - [x] **T2.3** Dividir o ledger de fatos entre sessões, não a dívida de recall — Sessão A e B enxergam o mesmo `facts.jsonl`; `/new` em A zera apenas `session`/`first_turn`/`cache` de A. Teste: `Memory::new_session` em A não altera `first_turn` nem o cache de B (`crates/tupi-app/src/memory.rs:93-100`).
- [ ] **T3** `Sessions` em tupi-app: runtime por sessão com turnos paralelos
  - Hoje `App` é Arc<Self> com turno único: `active` com `compare_exchange(0, u64::MAX)` (`crates/tupi-app/src/app.rs:673-680`), `current` e `pending` singulares (`crates/tupi-app/src/app.rs:362-401`).
  - [ ] **T3.1** Extrair o runtime de turno de `App` para `SessionRuntime` — `active`, `current`, `pending`, `next_request`, `token_scale`, `SessionStore`, `Memory` e a célula de modo migram de `App` para `SessionRuntime`; `App` mantém só o compartilhado (registry base, events, redb, state dirs, telemetry, workflows, prefix, context profile).
  - [ ] **T3.2** `App` vira roteador: `CommandHandler`/`QueryHandler` por `SessionId` — `App::handle` resolve a sessão e delega. Dois `SendMessage` em sessões distintas retornam `Started` sem `CommandError::Busy`; dois na MESMA sessão continuam dando `Busy`.
  - [ ] **T3.3** `NewSession` e `CloseSession` no protocolo — `NewSession` numa sessão cancela o turno dela, fecha pendências (`close_unanswered`, `crates/tupi-app/src/app.rs:70-75`) e rotaciona só aquela `Memory`; sessões irmãs seguem intactas.
  - [ ] **T3.4** Modelo/provider por sessão — `SetModel` numa sessão afeta só ela; `Event::ModelsLoaded` chega com `session` e o overlay de modelos não vaza entre abas.
- [ ] **T4** Tools por sessão sem duplicar o estado de deduplicação
  - Modo por aba exige um `SddTool` por sessão (a `mode_cell` é fixada em `SddTool::new`, `crates/tupi-tools/src/sdd.rs:204-217`). A via escolhida evita chamar `register_with` N vezes — isso recriaria `ResponseStore`, `DeliveryLedger`, `ParseCache` e `BacktrackGraph` por aba (`crates/tupi-tools/src/lib.rs:124-160`) e mataria a dedup de leitura (`check` → `Seen`/`Changed`, `crates/tupi-tools/src/ledger.rs:110-200`).
  - [ ] **T4.1** Snapshot de `ToolRegistry` para derivar catálogo por sessão — `ToolRegistry` ganha `#[derive(Clone)]` ou `pub fn snapshot(&self) -> ToolRegistry` clonando os `Arc<dyn Tool>` (`crates/tupi-kernel/src/tool.rs:114-116`); teste prova que dois snapshots compartilham o mesmo `Arc` de uma tool.
  - [ ] **T4.2** Fábrica de catálogo por sessão trocando apenas o `SddTool` — A sessão A lê um arquivo e a sessão B recebe `Seen` (não o corpo) ao ler o mesmo arquivo inalterado; teste em `tupi-tools` com dois registries provando dedup compartilhada.
  - [ ] **T4.3** `mode_cell` por sessão — `ToggleMode`/`SetMode` numa sessão não altera `specs_for_mode` nem bloqueia tool em curso na sessão irmã; `Event::ModeChanged` identifica a sessão. Teste: turno de edição vivo em A continua executando enquanto B entra em SDD.
- [ ] **T5** TUI com tabs estilo navegador
  - A TUI hoje tem um `ChatState` (`crates/tupi-tui/src/state.rs:58-78`) e `model`/`provider` como variáveis locais em `run_with_dashboard` (`crates/tupi-tui/src/run.rs:52`).
  - [ ] **T5.1** `Tab` e `Tabs`: vec de abas com índice ativo — Cada `Tab` guarda `ChatState`, `session: SessionId`, `request: Option<RequestId>`, `model`, `provider` e `title`. `run_with_dashboard` deixa de ter `state`/`request`/`model` soltos.
  - [ ] **T5.2** Roteamento de eventos por sessão em modo background — Tokens de uma aba não ativa atualizam o transcript dela; ao alternar, o texto já está lá. `Event::Memory`, `Notice`, `ModeChanged`, `ModelsLoaded` e `ContextBudget` vão para a aba da `session`.
  - [ ] **T5.3** Barra de tabs e atalhos — Barra de 1 linha só com ≥2 abas, com marcador na aba ativa. Atalhos: `Ctrl+T` nova, `Ctrl+W` fecha (com confirmação se o turno roda), `Alt+1..9`/`Ctrl+N`/`Ctrl+P` alterna. `Shift+Tab` continua sendo o toggle de modo (`crates/tupi-tui/src/run.rs:552-566`).
  - [ ] **T5.4** Título da aba e fechamento — Título = primeira mensagem truncada; fallback `modelo`; fallback `nova`. Fechar a última aba cria uma nova em vez de sair. `/new` reseta só a aba atual.
- [ ] **T6** Fiação em `main.rs` e no dashboard
  - `main.rs` monta um `App` único + um `BusSet` e `DashboardContext` guarda `model`/`provider` fixos (`crates/tupi/src/main.rs:17-295`).
  - [ ] **T6.1** `main.rs` cria as `Sessions` e a primeira aba — `main.rs` monta `App` como roteador, registra sessão inicial e passa `BusSet` único à TUI; `with_memory(root)` passa a criar uma `Memory` por sessão.
  - [ ] **T6.2** Dashboard reflete o foco sem quebrar o SSE — `tupi-web::events::map_event` compila com os novos campos e ignora sessão não focada; `Query::History` do dashboard resolve no foco (`crates/tupi-web/src/routes.rs:26-29`).
- [ ] **T7** Provas de isolamento e paralelismo
  - [ ] **T7.1** Duas sessões streamando no mesmo instante — Teste com provider scripted: dois `SendMessage` simultâneos, ambos `Started`, tokens de cada um no seu histórico, nenhum `Busy`.
  - [ ] **T7.2** Memória isolada com ledger compartilhado — O fato gravado na sessão A não injeta bloco de recall em B antes de B consultar; `/new` em A não agenda consulta em B. Inclui a pendência fechada por A aparecendo para B (mesmo arquivo append-only).
  - [ ] **T7.3** Cancelar uma sessão não toca a outra — `CancelStream` na sessão A deixa o turno de B vivo e sem `StreamFinished` cruzado (`App::cancel`, `crates/tupi-app/src/app.rs:1293-1354`).
