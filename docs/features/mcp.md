# Cliente MCP declarado em `ai.project.json`

Servidor MCP não vira N tools no registry: vira **uma** tool `mcp` com união de
ações, alimentada pela seção `mcp` do `ai.project.json` versionado no repo. O
objetivo é config de repositório, no mesmo espírito de `docs_folders`, `models` e
`space` — quem clona o repo herda os servidores do projeto, não os da máquina.

```json
"mcp": {
  "filesystem": {
    "type": "local",
    "command": ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."],
    "read_only": true
  },
  "sentry": {
    "type": "remote",
    "url": "https://mcp.sentry.dev/sse"
  }
}
```

## Shape do config

`McpServerConfig` é um enum marcado por `type`
(`crates/tupi/src/project_config.rs:52-81`), lido pela seção `mcp` de
`ProjectConfig` (`crates/tupi/src/project_config.rs:127-130`):

| campo | transporte | default |
| --- | --- | --- |
| `type` | ambos | obrigatório (`local` ou `remote`) |
| `command` | `local` | obrigatório, primeiro item é o executável |
| `env` | `local` | vazio, nunca ecoado |
| `url` | `remote` | obrigatório, `http(s)://` |
| `headers` | `remote` | vazio, nunca ecoado |
| `enabled` | ambos | `true` |
| `read_only` | `local` | `false` |

- **`mcp` ausente = feature desligada**: zero tool extra e zero latência de
  startup (`crates/tupi-tools/src/lib.rs:203-205` só registra `mcp` quando a
  lista de specs não está vazia).
- **`enabled: false`** desliga um servidor sem apagar a configuração
  (`crates/tupi/src/project_config.rs:59-67`); ele nem vira spec, logo não é
  spawnado (`mcp_specs`, `crates/tupi/src/main.rs:535`).
- **Validação recusa citando o caminho do arquivo**
  (`crates/tupi/src/project_config.rs:405-451`): nome vazio ou fora de
  `[a-z0-9_-]`, `command` vazio, `url` que não é `http(s)`, e `type: "remote"`
  — que é recusado com mensagem que ensina e remete à fase seguinte
  (`crates/tupi/src/project_config.rs:443-446`).
- **Credencial não vaza**: `env`/`headers` nunca entram na mensagem de erro nem
  no `Debug` do spec (`crates/tupi-tools/src/mcp/mod.rs:26-48`); há teste que
  trava isso (`crates/tupi/src/project_config.rs:870-876`).
- Campo desconhecido dentro do servidor é ignorado, como no resto do arquivo
  (`crates/tupi/src/project_config.rs:805-818`, espelho do teste de
  `project_config.rs:524-528`).

A fronteira é de mão única: `tupi-tools` **não** conhece `ProjectConfig`. O tipo
neutro é `ServerSpec` (`crates/tupi-tools/src/mcp/mod.rs:21-31`) e a conversão
config → spec acontece na composição, em `mcp_specs`
(`crates/tupi/src/main.rs:535`, ligada em `crates/tupi/src/main.rs:298`).

## As três ações

`McpTool` expõe uma tool só, com `action` default `servers`
(`crates/tupi-tools/src/mcp/tool.rs:15` e `:52-59`):

- **`action=servers`** — nome, transporte e **status** de cada servidor. Conecta
  na primeira vez e resume `ok · N tool(s)` ou `failed · <erro>`
  (`crates/tupi-tools/src/mcp/tool.rs:120-138`). Servidor quebrado não derruba os
  outros nem o binário.
- **`action=tools`, `server=…`** — nome e descrição curta das tools anunciadas
  (`crates/tupi-tools/src/mcp/tool.rs:140-160`); a descrição é cortada em 120
  caracteres (`crates/tupi-tools/src/mcp/tool.rs:17-18` e `:69-76`), porque o
  schema completo de um servidor grande custaria dezenas de milhares de tokens
  por passo.
- **`action=call`, `server=…`, `tool=…`, `arguments={…}`** — chama a tool e
  devolve `content[]` renderizado (`crates/tupi-tools/src/mcp/tool.rs:206-320`).
  Conteúdo `text` é concatenado; `image`/`resource` viram marcadores curtos e
  tipo desconhecido vira `[tipo]` — nunca erro fatal
  (`crates/tupi-tools/src/mcp/tool.rs:226-254`). Saída grande é cortada com
  cabeça e cauda anunciadas (`crates/tupi-tools/src/mcp/tool.rs:207-224`).

Na TUI a linha da tool mostra `sentry · list_issues`, não o JSON cru
(`describe_call`, `crates/tupi-tools/src/mcp/tool.rs:330-344`).

## Protocolo e limites

Transporte `local` = processo filho, JSON-RPC 2.0 sobre stdin/stdout
(`crates/tupi-tools/src/mcp/client.rs:98-156`). Sequência do handshake:
`initialize` (`protocolVersion` `2024-11-05`, `clientInfo` `tupi`) →
`notifications/initialized` → `tools/list`, com paginação por `nextCursor`
(`crates/tupi-tools/src/mcp/client.rs:21-24`, `:174-195`).

- **Bloqueante com timeout por leitura**: 20 s por resposta
  (`crates/tupi-tools/src/mcp/client.rs:25-26`); a tool roda dentro de
  `spawn_blocking` no app, então nada de tokio no `tupi-tools` de hoje. São
  **duas** noções de timeout a manter distintas: 20 s **por leitura** no stdio e,
  quando o transporte remoto entrar, 20 s **por requisição** — em HTTP não existe
  leitura isolada depois que o stream abre
  ([`decisions/0009-mcp-http-cliente.md`](../decisions/0009-mcp-http-cliente.md)).
- **Teto de páginas**: `tools/list` para em 64 páginas, com erro em vez de loop
  infinito (`crates/tupi-tools/src/mcp/client.rs:34-35` e `:186-194`).
- **Startup não espera servidor terceiro**: o TUI sobe antes; o spawn e o
  handshake acontecem na primeira call (`connect`,
  `crates/tupi-tools/src/mcp/tool.rs:108-118`), e a conexão fica num pool por
  spec (`crates/tupi-tools/src/mcp/tool.rs:20-27`).
- **Falha de servidor vira status**: initialize/`tools/list` que falham viram
  `failed · …` na saída de `servers`, com a cauda do stderr (8 linhas de até 200
  caracteres, `crates/tupi-tools/src/mcp/client.rs:27-33` e `:341-352`).
  Requisição vinda do servidor (sampling, roots) é recusada com `-32601` em vez
  de travar (`crates/tupi-tools/src/mcp/client.rs:299-315`).
- **Sem zumbi**: o `Drop` da conexão fecha o stdin, mata o filho e faz `wait`
  (`crates/tupi-tools/src/mcp/client.rs:355-365`).

## Efeito e exposição

- `servers`/`tools` = `ReadOnly`; `call` = `Opaque`, **a menos** que o servidor
  declare `read_only: true` (`crates/tupi-tools/src/mcp/tool.rs:322-328`).
  Servidor remoto não tem como declarar `read_only` (o campo é do transporte
  `local`), então `call` remoto é `Opaque` sempre.
- `mode_mask` é o default fail-closed: `mcp` só existe em `Execution`.
- **`mcp` não entra em `CORE_TOOLS`** (`crates/tupi-tools/src/lib.rs:442-452`)
  nem no catálogo dos subagentes SDD; aparece só com `expose_all` ou lista
  explícita de `expose` (`crates/tupi/src/tool_config.rs:29-37`). Os testes
  travam isso em `crates/tupi-tools/src/lib.rs:653-795`.

## O que ficou fora desta fase

- **`type: "remote"` (SSE/HTTP)** — recusado no parse
  (`crates/tupi/src/project_config.rs:443-446`), com mensagem que ensina. O
  cliente HTTP já está **decidido**: `reqwest` do pin do workspace dirigido por
  um runtime `current_thread` por conexão, sem crate novo no `Cargo.lock`
  ([`decisions/0009-mcp-http-cliente.md`](../decisions/0009-mcp-http-cliente.md)).
  Falta a implementação.
- **`expose: "flat"`** (uma tool do registry por tool do servidor) — opt-in
  posterior: o registry é estático no startup e schema flat de servidor grande
  custa dezenas de milhares de tokens por request (ver
  [`decisions/0005-expose-all.md`](../decisions/0005-expose-all.md) e
  [`decisions/0006-teto-de-passos.md`](../decisions/0006-teto-de-passos.md)).
- **Sampling/roots/prompts do servidor** — o cliente só consome `tools/list` e
  `tools/call`.
- **Reconexão automática** — conexão morta é respawnada só na call seguinte,
  passando pelo mesmo timeout.

## Testes

- Cliente sem rede e sem `npx`: servidor fake em script de shell sob
  `#[cfg(unix)]`, com paginação em duas páginas, servidor que morre (stderr na
  mensagem, sem ecoar `env`), `Drop` que ceifa o filho e timeout
  (`crates/tupi-tools/src/mcp/client.rs:446-661`).
- Config: defaults, `enabled: false` preservando `env`, campo desconhecido
  ignorado e cada recusa com o caminho do arquivo
  (`crates/tupi/src/project_config.rs:757-954`).
- Registry: efeito por ação, `ReadOnly` só com `read_only: true`, ausência de
  `mcp` fora de `All`/lista explícita e startup que não spawna servidor
  (`crates/tupi-tools/src/lib.rs:653-795`).
- Conversão config → spec ignora servidor ausente e desligado
  (`crates/tupi/src/main.rs:721-745`).

A decisão e as alternativas rejeitadas estão em
[`decisions/0008-mcp-cliente-com-catalogo.md`](../decisions/0008-mcp-cliente-com-catalogo.md);
o transporte remoto, em
[`decisions/0009-mcp-http-cliente.md`](../decisions/0009-mcp-http-cliente.md).
