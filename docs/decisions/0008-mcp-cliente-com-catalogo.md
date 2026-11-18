# ADR 0008 — MCP é cliente com catálogo: uma tool `mcp`, config no repo

## Contexto

Servidores MCP são a forma padrão de plugar ferramentas de terceiros, e a
alternativa óbvia — uma tool do registry por tool do servidor — colide com duas
decisões já tomadas:

- O registry é **estático no startup** (`crates/tupi-tools/src/lib.rs:176-413`):
  a lista de tools nasce de `register_with`, e tool flat de servidor MCP exigiria
  spawnar todo servidor antes de montar o registry — ou seja, o startup passaria
  a esperar processo de terceiro.
- Schema flat custa tokens em **todo** passo. A medição de `expose_all` mostra
  10.651 tokens de schema por passo contra 4.468 no núcleo
  ([`0005-expose-all.md`](0005-expose-all.md)); um servidor grande como o
  `filesystem` sozinho tem dezenas de tools com schema de objeto grande, e o custo
  entraria como prefixo de cada requisição (ver também
  [`0006-teto-de-passos.md`](0006-teto-de-passos.md)).

Ao mesmo tempo, a config precisa ser **do repositório**, não da máquina: quem
clona herda os servidores do projeto, no mesmo espírito de `docs_folders`,
`models` e `space` (`crates/tupi/src/project_config.rs:102-131`).

A restrição arquitetural que fecha o desenho: a dependência só aponta para baixo
— `tupi` conhece `tupi-tools`, o contrário não — e `Tool::call` é síncrono,
rodando dentro de `spawn_blocking` (`crates/tupi-app/src/app.rs:20-57`).

## Decisão

**Um cliente MCP bloqueante, stdio, com catálogo, atrás de uma única tool `mcp`
e de uma seção `mcp` versionada no `ai.project.json`.**

- **Config no repo, com validação que ensina.** A seção `mcp` é um mapa por nome
  (`crates/tupi/src/project_config.rs:127-130`), com transporte escolhido por
  `type` (`crates/tupi/src/project_config.rs:52-81`). Nome, `command` e `url` são
  validados antes de qualquer spawn, e a recusa cita o caminho do arquivo
  (`crates/tupi/src/project_config.rs:405-451`). Ausente no arquivo = feature
  desligada, zero tool extra e zero latência.
- **`type: "remote"` é recusado com mensagem que ensina**
  (`crates/tupi/src/project_config.rs:443-446`), nunca aceito em silêncio:
  transporte remoto é a task T6.
- **Uma tool com união de ações** — `servers` / `tools` / `call` — em vez de N
  tools flat (`crates/tupi-tools/src/mcp/tool.rs:15`). O schema viaja uma vez, e
  o schema real da tool do servidor só aparece em `call`.
- **`tupi-tools` não conhece `ProjectConfig`.** O tipo neutro é `ServerSpec`
  (`crates/tupi-tools/src/mcp/mod.rs:21-31`), com `Debug` redigido porque `env`
  pode carregar credencial (`crates/tupi-tools/src/mcp/mod.rs:35-48`); a
  conversão acontece na composição, em `mcp_specs`
  (`crates/tupi/src/main.rs:535`, ligada em `crates/tupi/src/main.rs:298`).
- **JSON-RPC 2.0 bloqueante, com timeout por leitura** (20 s,
  `crates/tupi-tools/src/mcp/client.rs:25-26`), sem tokio e sem reqwest: o
  handshake é `initialize` → `notifications/initialized` → `tools/list` com
  paginação por `nextCursor` e teto de 64 páginas
  (`crates/tupi-tools/src/mcp/client.rs:174-195`). Zero tokio em `tupi-tools`
  porque `call` é síncrona e já roda em `spawn_blocking`.
- **Falha de servidor não derruba o binário.** O TUI sobe antes; o spawn é lazy,
  na primeira call (`crates/tupi-tools/src/mcp/tool.rs:108-118`), e
  initialize/`tools/list` que falham viram `status` com a cauda do stderr em
  `mcp(action="servers")` (`crates/tupi-tools/src/mcp/tool.rs:120-138`,
  `crates/tupi-tools/src/mcp/client.rs:341-352`).
- **Processo filho não vira zumbi**: `Drop` fecha o stdin, mata e faz `wait`
  (`crates/tupi-tools/src/mcp/client.rs:355-365`).
- **Efeito declarado**: `servers`/`tools` são `ReadOnly`, `call` é `Opaque` a
  menos que o servidor declare `read_only: true`
  (`crates/tupi-tools/src/mcp/tool.rs:322-328`), e `mcp` só existe em `Execution`
  pelo default fail-closed de `mode_mask`.
- **Fora do núcleo**: `mcp` não entra em `CORE_TOOLS`
  (`crates/tupi-tools/src/lib.rs:442-452`) nem no catálogo dos subagentes SDD;
  aparece só com `expose_all` ou lista explícita de `expose`
  (`crates/tupi/src/tool_config.rs:29-37`).

## Alternativas rejeitadas

- **Uma tool flat por tool do servidor.** O registry é estático no startup
  (`crates/tupi-tools/src/lib.rs:176-413`): montar a lista exigiria spawnar todo
  servidor antes de abrir a TUI, e o schema entraria em todo passo — exatamente o
  custo que a ADR 0005 mediu em +6.145 tokens. `expose: "flat"` fica como opt-in
  posterior para quem aceitar o preço.
- **Aceitar `type: "remote"` e falhar em runtime.** Config aceita em silêncio é
  config quebrada descoberta no meio do turno; recusar na carga, com mensagem que
  diz "ainda não suportado", é mais barato que um erro de transporte.
- **Tokio/reqwest dentro de `tupi-tools`.** `call` é síncrona e roda em
  `spawn_blocking` (`crates/tupi-app/src/app.rs:20-57`); um runtime async no
  crate de tools seria dependência nova sem uso, e o `Cargo.toml` de
  `tupi-tools` não tem nenhum dos dois.
- **Spawnar e fazer handshake no startup (catálogo eager).** Daria `tools/list`
  de graça na primeira consulta, mas faria o binário esperar servidor de terceiro
  para abrir — e um servidor travado viraria timeout no boot.
- **Credencial visível em erro/diagnóstico.** `env` e `headers` fora de log,
  `Debug`, mensagem de erro e `describe_call`: o modo de falha mais comum de
  servidor MCP é justamente cuspir config no stderr.

## Consequências

- Um servidor novo é uma entrada no `ai.project.json` versionado; nada de config
  por máquina.
- O modelo descobre tool em dois passos (`tools`, depois `call`), não em um. O
  primeiro passo é barato e o schema grande nunca viaja no turno.
- Servidor quebrado aparece como `failed · …` em `mcp(action="servers")`, com
  stderr — não como panic nem como travamento de boot.
- `mcp` só aparece com `expose_all` ou lista explícita; sem isso a tool existe no
  registry mas não é exposta, e `CORE_TOOLS` continua fechado
  (`crates/tupi-tools/src/lib.rs:442-452`).
- **Revisar quando** (a) a T6 trouxer `type: "remote"` e o transporte deixar de
  ser stdio puro; (b) um servidor real precisar de mais de 64 páginas em
  `tools/list` ou de mais de 20 s por resposta; (c) o custo de duas chamadas
  (`tools` + `call`) aparecer na telemetria como passo extra recorrente — aí
  `expose: "flat"` volta à mesa com número.
