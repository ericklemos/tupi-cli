# ADR 0009 — transporte remoto do MCP usa o `reqwest` já pinado, bloqueante por runtime próprio

## Contexto

O transporte `local` (stdio) já está implementado e a ADR
[0008](0008-mcp-cliente-com-catalogo.md) fechou o desenho: uma tool `mcp` com
catálogo, JSON-RPC 2.0 bloqueante, timeout por leitura de 20 s
(`crates/tupi-tools/src/mcp/client.rs:26`) e nenhum tokio dentro de
`tupi-tools`. Falta decidir com **qual cliente HTTP** o `type: "remote"`
(streamable HTTP/SSE) vai falar — a config já reserva o shape e recusa o
transporte com mensagem que ensina até a T6 entrar
(`crates/tupi/src/project_config.rs:69`, `:312`).

As restrições que a escolha precisa respeitar não são negociáveis:

- `Tool::call` é **síncrona** e roda dentro de `spawn_blocking`
  (`crates/tupi-app/src/app.rs:20-57`). Um cliente async só serve se houver
  runtime para ele ser dirigido, e quem dirige já é uma thread bloqueante.
- **Nada de tokio em `tupi-tools`** era a regra do stdio
  (`crates/tupi-tools/Cargo.toml` não tem tokio nem reqwest). Transporte remoto
  é o caso em que essa regra encosta no limite: HTTP de verdade precisa de
  cliente com TLS, SSE e timeout.
- A dependência só aponta para baixo: `tupi-tools` **não** conhece
  `ProjectConfig` (`crates/tupi-tools/src/mcp/mod.rs:1-8`); o mapa config → spec
  acontece na composição (`mcp_specs`, `crates/tupi/src/main.rs:535`, ligada em
  `:298`).
- O peso já pago importa: `reqwest = { version = "0.13.5", default-features =
  false, features = ["json", "stream", "rustls"] }` está pinado no workspace
  (`Cargo.toml`, `[workspace.dependencies]`) e **todo binário do `tupi` já o
  linka**, porque `crates/tupi` depende de `tupi-providers`
  (`crates/tupi/Cargo.toml:15`), que usa `reqwest::Client` com `connect_timeout`
  (`crates/tupi-providers/src/deepseek.rs:19-30`).

A pergunta da subtarefa é exatamente essa: pagar dependência nova (cliente
bloqueante próprio) ou reusar o stack de `reqwest` que já está no grafo.

## Decisão

**`reqwest` do pin do workspace, dentro de `tupi-tools::mcp`, dirigido por um
runtime `current_thread` por conexão e `block_on` por requisição.**

1. **Reuso, não dependência nova.** O transporte remoto entra com
   `reqwest.workspace = true` e `tokio = { workspace = true, features = ["rt",
   "time"] }` no `Cargo.toml` de `tupi-tools`. Nenhum crate novo entra no
   `Cargo.lock`, nenhum byte novo entra no binário (o `reqwest` já é linkado via
   `tupi-providers`) e **nenhuma feature nova entra no pin** — `json`, `stream` e
   `rustls` já estão ligadas, e `default-features = false` mantém o cliente
   bloqueante de fora. Cargo unifica features no workspace: mexer no pin mudaria
   o `reqwest` de todo mundo, inclusive do provider.
2. **A superfície síncrona não muda.** Cada conexão remota possui um
   `tokio::runtime::Runtime` `new_current_thread()` + `enable_all()`, e cada
   requisição JSON-RPC é `runtime.block_on(...)`. Isso é legal porque `call` já
   roda em `spawn_blocking` (`crates/tupi-app/src/app.rs:50-53`) — não há runtime
   ambiente para aninhar, e o runtime nasce sem thread trabalhadora, então não
   sobra thread para ceifar.
3. **O protocolo continua num lugar só.** O `tupi-tools::mcp` guarda framing,
   `initialize` → `notifications/initialized` → `tools/list` com paginação,
   redação de credencial e truncamento de erro; só o `Connection` ganha um
   segundo transporte ao lado do stdio. `ProjectConfig`,
   `command`/`env` e a validação seguem em `crates/tupi`.
4. **Tempo e falha.** Timeout por **requisição** (o `READ_TIMEOUT` de 20 s,
   `crates/tupi-tools/src/mcp/client.rs:26`, aplicado no builder do cliente), não
   por leitura: em HTTP não existe "leitura" isolada depois que o stream abre.
   Erro remoto vira status em `mcp(action="servers")`, como no stdio, com no
   máximo a primeira linha do corpo (200 caracteres, mesmo teto de
   `STDERR_LINE_CHARS`, `crates/tupi-tools/src/mcp/client.rs:33`).
5. **Credencial não viaja em diagnóstico.** `headers` do config vão para a
   requisição e para lugar nenhum mais: fora de `Debug`, de log, de mensagem de
   erro e de `describe_call`, exatamente como `env` no `ServerSpec`
   (`crates/tupi-tools/src/mcp/mod.rs:35-48`). Redirect para origem diferente é
   recusado justamente porque os headers carregam credencial.
6. **`call` remoto é `Opaque` sempre.** `read_only` é campo do transporte
   `local` no config; servidor remoto não tem como declarar que não escreve, e
   `Opaque` é o default seguro (`crates/tupi-tools/src/mcp/tool.rs:322-328`).
7. **SSE é parse, não framework.** `Accept: text/event-stream, application/json`
   e leitura incremental por `bytes_stream` (feature `stream` já ligada); sessão
   do servidor (`Mcp-Session-Id`) é ecoada quando vier. Sem reconexão com backoff
   nesta fase, como o stdio não tem retry.
8. **Teste hermético, sem rede.** O servidor fake é um `TcpListener` em loopback
   respondendo SSE fixo, no mesmo espírito do fake de stdio em
   `crates/tupi-tools/tests/mcp.rs`: nada de `npx`, nada de internet.

## Alternativas rejeitadas

- **`ureq` (ou qualquer cliente bloqueante nativo).** É o encaixe perfeito em
  `Tool::call` — e é por isso que foi o candidato sério. Perde na conta de peso:
  traz a própria árvore de TLS/HTTP (mais um `rustls`/webpki e suas versões para
  auditar), entra no `Cargo.lock` e no tempo de compilação do `tupi-tools`,
  enquanto o `reqwest` já está compilado, linkado e em uso no mesmo binário. Dois
  clientes HTTP no mesmo processo também significam duas políticas de proxy, de
  raízes de TLS e de redirect para manter iguais.
- **Ligar o cliente bloqueante do próprio `reqwest` no pin.** Uma feature a mais
  no `[workspace.dependencies]` vale para todos os crates (unificação), então o
  provider também pagaria; e ele esconde um runtime com thread própria por
  cliente, cujo tempo de vida não fica amarrado à conexão que o `Drop`
  (`crates/tupi-tools/src/mcp/client.rs:355-365`) precisa encerrar. Um
  `current_thread` explícito dá o mesmo conforto com dono claro.
- **Trait de transporte em `tupi-tools` com a implementação HTTP na composição
  (`crates/tupi`).** Manteria `reqwest`/tokio fora de `tupi-tools`, mas partiria
  o protocolo em dois: id de requisição, notificações, paginação por
  `nextCursor`, redação e truncamento teriam de atravessar a fronteira, e cada
  transporte novo exigiria conhecimento de JSON-RPC dentro do binário. A seta
  `tupi` → `tupi-tools` existe para o binário **compor**, não para reimplementar
  cliente.
- **Tornar `Tool::call` async / esperar a resposta no `tupi-app`.** Refatora o
  contrato do kernel (`crates/tupi-kernel/src/tool.rs:147-177`) inteiro por causa
  de uma tool, e o prazo que interessa é o passo do turno, não concorrência
  interna.
- **Chamar `curl` pelo `exec`/`shell`.** Não é cliente: não controla stream SSE,
  não tem timeout por requisição e o allowlist do `shell` é fronteira de
  segurança — pedir `curl` ali é ampliar poder de execução para resolver um
  problema de biblioteca.
- **Adiar o remoto e manter só stdio por mais uma fase.** A config já reserva o
  shape e a validação já recusa `remote` ensinando o caminho
  (`crates/tupi/src/project_config.rs:312`); o custo de agora é uma dependência
  já paga, não uma decisão nova.

## Consequências

- `tupi-tools` passa a listar `reqwest` e `tokio` no `Cargo.toml`, mas **nada
  novo é compilado**: os dois já estão no grafo por `tupi-providers`, e o binário
  não engorda.
- `cargo check -p tupi-tools` deixa de ser independente de rede de crates (espera
  o `reqwest` já em cache) — custo de build local, não de usuário.
- A regra "nada de tokio em `tupi-tools`" fica qualificada: tokio entra **sem
  async público**, só como motor interno de runtime `current_thread` para o
  transporte remoto; `Tool::call` continua síncrona e o stdio continua sem
  runtime.
- Há duas noções de timeout a documentar na
  [`features/mcp.md`](../features/mcp.md): 20 s **por leitura** no stdio, 20 s
  **por requisição** no remoto.
- Fica um seam pronto para as peças que a T6 não cobre: OAuth (renovação de
  `headers`), reconexão de SSE com backoff e `headers` por servidor.
- **Revisar quando** (a) o SSE mostrar reconexão frequente e exigir backoff com
  estado de sessão; (b) OAuth entrar e o `reqwest` precisar de middleware de
  renovação; (c) a medição mostrar o runtime por conexão como custo visível no
  primeiro passo — aí um runtime compartilhado por processo (ou o cliente
  bloqueante do pin) volta à mesa **com número**.
