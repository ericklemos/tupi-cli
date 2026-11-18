<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Cliente MCP configurado por ai.project.json (`mcp-client`)

**Status:** completed

## Objetivo

Ler servidores MCP declarados em `ai.project.json` (crates/tupi/src/project_config.rs:32-57) e expor as tools deles ao modelo por UMA tool `mcp` com ações `servers` / `tools` / `call`, transporte stdio (`type: "local"`). Config versionada no repo, no mesmo espírito de `docs_folders`/`models`/`space`, com validação que recusa citando o caminho do arquivo (padrão de project_config.rs:186-221) e teste de campo desconhecido ignorado (project_config.rs:524-528).

## Restrições

- Dependência só aponta para baixo: `tupi` -> `tupi-tools`; `tupi-tools` NÃO conhece `ProjectConfig` (crates/tupi/src/project_config.rs:32-57). O tipo neutro de servidor mora em `tupi-tools` e o mapa config->spec é feito em crates/tupi/src/main.rs:290-294.
- `Tool::call` é síncrono (crates/tupi-kernel/src/tool.rs:147-177) e roda dentro de `spawn_blocking` em crates/tupi-app/src/app.rs:20-57. O cliente JSON-RPC é bloqueante com timeout por leitura; nada de tokio em `tupi-tools` (Cargo.toml:7-28 não tem tokio nem reqwest).
- Escopo 1 = somente `type: "local"` (stdio). `type: "remote"` é recusado com mensagem que ensina ("ainda não suportado"), nunca aceito em silêncio; implementação remota fica na task T6.
- A tool `mcp` não entra em CORE_TOOLS (crates/tupi-tools/src/lib.rs:434-444) nem no conjunto dos subagentes SDD (register_agent, crates/tupi-tools/src/agent_tools.rs:27). Aparece apenas com expose_all ou lista explícita (crates/tupi/src/tool_config.rs:29-37).
- Falha de servidor não derruba o binário: initialize/tools/list que falham viram `status` na saída de `mcp(action="servers")` com a cauda do stderr; o TUI sobe antes e o startup nunca espera servidor terceiro.
- `mcp` ausente no arquivo = feature desligada, zero tool extra, zero latência de startup.
- Processo filho é morto no drop da conexão (kill + wait) e stdin é fechado: sem zumbi.
- Nenhuma credencial vai para log/telemetria: `env` e `headers` nunca aparecem em mensagem de erro nem em describe_call.

## Nuances

- Shape no `ai.project.json`, espelhando o `mcp` do opencode mas versionado no repo:
"mcp": {
  "filesystem": { "type": "local", "command": ["npx","-y","@modelcontextprotocol/server-filesystem","."], "read_only": true },
  "sentry": { "type": "remote", "url": "https://mcp.sentry.dev/sse" }
}
Com `enabled` default true e `env` default vazio.
- Protocolo: `initialize` (protocolVersion + clientInfo) -> `notifications/initialized` -> `tools/list` (paginação por `nextCursor`) -> `tools/call` devolvendo `content[]`. Conteúdo `text` é concatenado; tipo desconhecido vira marcador curto, nunca erro fatal.
- `effect()`: `servers`/`tools` = ReadOnly; `call` = Opaque, a menos que o servidor declare `read_only: true` (aí ReadOnly). `mode_mask` = EXECUTION (o default fail-closed de tool.rs:170-172 já cobre).
- Uma tool com schema de união de ações em vez de N tools flat: o registry é estático no startup (crates/tupi-tools/src/lib.rs:404-409) e schema flat de servidor grande custa dezenas de milhares de tokens por request (precedente: docs/decisions/0005-expose-all.md e 0006-teto-de-passo.md). `expose: "flat"` fica como opt-in posterior, não no escopo 1.
- `describe_call` curto e útil na TUI: "sentry · list_issues" em vez do JSON cru (padrão de tool.rs:160-167).
- Testes herméticos: `crates/tupi-tools/tests/mcp.rs` spawna um servidor fake em shell sob `#[cfg(unix)]` (mesmo padrão de fixture já usado em exec/shell), sem rede e sem npx.
- Documentação: `docs/features/mcp.md` + ADR `docs/decisions/0008-mcp-*.md` + uma linha em "Componentes" de docs/architecture.md (:34-51), conforme docs/conventions.md.

## Tasks

- [x] **T1** Config `mcp` em ProjectConfig
  - Adicionar `pub mcp: BTreeMap<String, McpServerConfig>` ao struct de crates/tupi/src/project_config.rs:32-57 com `#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]`. `McpServerConfig` = enum tagueado por `type` (`local` com `command: Vec<String>`, `env`, `enabled` default true, `read_only` default false; `remote` com `url`, `headers`, `enabled`). Validar em `parse_project` (project_config.rs:227-304) usando a mesma família de erros de `ProjectConfigError` (project_config.rs:186-221): nome do servidor restrito a [a-z0-9-_], `command` vazio recusado, `remote` recusado nesta fase com mensagem que ensina. `write_project` (project_config.rs:358-369) precisa preservar `mcp` no roundtrip.
  - [x] **T1.1** Struct + enum + defaults — `cargo test -p tupi project_config` verde com testes novos: ausente = mapa vazio; local completo parseia; campo desconhecido dentro do servidor é ignorado (padrão project_config.rs:524-528).
  - [x] **T1.2** Validação com recusa que nomeia o caminho — Testes recusam: `command: []`, nome com espaço/maiúscula, `type: "remote"`, url não http em remote. Mensagem contém o nome do servidor e o caminho do arquivo.
  - [x] **T1.3** Roundtrip de escrita — Teste: ler arquivo com `mcp` + `record_model_window` (project_config.rs:100-182) e conferir que `mcp` sobrevive intacto no arquivo reescrito.
- [x] **T2** Cliente JSON-RPC stdio em `tupi-tools`
  - Novo módulo `crates/tupi-tools/src/mcp/client.rs`: `Connection::spawn(spec)` com `std::process::Command` (stdin/stdout piped, stderr em thread que guarda a cauda), framing JSON-RPC por linha, `initialize` + `notifications/initialized`, `tools/list` com paginação por `nextCursor`, `tools/call`. Timeout por leitura com canal + `recv_timeout` (sem tokio). `Drop` mata o filho e fecha o stdin.
  - [x] **T2.1** Spawn + handshake + listagem — Contra servidor fake, `initialize` devolve serverInfo e `tools/list` com 2 páginas por cursor retorna todas as tools.
  - [x] **T2.2** Timeout e servidor morto — Servidor que não responde em N segundos devolve erro com o nome do servidor, sem travar; servidor que fecha stdout devolve erro, não panic.
  - [x] **T2.3** Sem zumbi e sem vazamento de env — Teste garante kill no drop; nenhum `env`/header aparece em mensagem de erro serializada.
- [x] **T3** Tool `mcp` (servers/tools/call)
  - `crates/tupi-tools/src/mcp/mod.rs`: `McpTool` imita o contrato de crates/tupi-kernel/src/tool.rs:147-177 com schema de união `action: servers | tools | call` (+ `server`, `tool`, `arguments`). `effect`: servers/tools ReadOnly, call Opaque salvo `read_only`. `describe_call` = "srv · tool". Pool de conexões por nome em `OnceLock<Mutex<HashMap<String, Arc<Connection>>>>`, espelhando WATCHERS (crates/tupi-tools/src/lib.rs:407-408). Registro em `register_with` (lib.rs:171-405), campo `pub mcp: Vec<McpServerSpec>` em `ToolsOptions` (lib.rs:51-81, default vazio).
  - [x] **T3.1** Actions servers/tools — `mcp(action="servers")` lista nome, tipo e status (ok/failed + cauda do stderr); `mcp(action="tools", server=..)` lista nome+descrição das tools do servidor.
  - [x] **T3.2** Action call + render de conteúdo — `call` devolve o texto concatenado de `content[]`, truncado no orçamento existente; `content` de tipo desconhecido vira marcador curto.
  - [x] **T3.3** Efeito e exposição — `mcp` fora de CORE_TOOLS (lib.rs:434-444): com `Exposure::Core` não aparece em `registry.specs()`; aparece com `All`/`Only(["mcp"])`. `effect` de `call` é Opaque e `mode_mask` não inclui SDD/Docs (teste análogo a lib.rs:563-602).
- [x] **T4** Wiring em `crates/tupi`
  - Em crates/tupi/src/main.rs:290-294, mapear `project.mcp` -> `Vec<McpServerSpec>` e preencher `tool_options.mcp`; nada mais muda em main.rs:404-409. Servidor `enabled: false` não é spawnado.
  - [x] **T4.1** Mapa config -> spec no boot — `cargo check -p tupi` verde e `tupi --web` sobe com `ai.project.json` contendo um servidor fake; `mcp(action="servers")` responde no turno.
  - [x] **T4.2** Ausente/desligado não custa nada — Sem chave `mcp` ou com `enabled:false`, `registry` não tem `mcp` e o startup não spawna processo nenhum.
- [x] **T5** Docs e ADR
  - `docs/features/mcp.md` (shape do config, as três ações, limites e o que ficou fora), ADR `docs/decisions/0008-mcp-cliente-com-catalogo.md`, e uma linha em "Componentes" de docs/architecture.md:34-51. Seguir docs/conventions.md.
  - [x] **T5.1** Feature doc + ADR — Arquivos existem, citam caminho:linha do código real e `docs(action="refresh")` reindexa sem erro.
- [x] **T6** Fora do escopo: transporte remoto
  - `type: "remote"` (streamable HTTP/SSE) com cliente bloqueante próprio (ureq ou reqwest com runtime dedicado), headers e OAuth. Registrado aqui só para não se perder; a config já reserva o shape e a validação recusa com mensagem que ensina até esta task entrar.
  - [x] **T6.1** Decidir cliente HTTP — ADR com a escolha (peso de dependência vs reuso do stack reqwest de tupi-providers).
