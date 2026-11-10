# Tupi CLI — Design (v1)

**Data:** 2026-11-10
**Status:** aguardando aprovação

## Intenção e por quê

Tupi é um cliente de chat de terminal escrito em Rust, no espírito do OpenCode, mas
com base de código própria. A v1 não é um agente: é uma conversa com um modelo,
renderizada em TUI, com streaming. O objetivo é ter a fundação — contratos de
provedor, frontend desacoplado e um núcleo de comandos/queries — para crescer para
múltiplos provedores, ferramentas e agentes sem reescrever o chat.

O porquê de separar em crates desde o início é a experiência do autor: extrair
fronteiras depois, com código já acoplado, custa mais do que pagar o boilerplate agora.

## Objetivos

- Chat de sessão única em TUI (ratatui + crossterm), com histórico rolável.
- Streaming de tokens token a token.
- Provedor DeepSeek via API compatível com OpenAI, com a chave em
  `TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY`.
- Arquitetura microkernel: núcleo define contratos, providers e frontend são
  componentes ligados na composição.
- CQRS: escrita (comandos) separada de leitura (queries), com eventos de streaming.

## Não-objetivos (v1)

- Agentes, ferramentas, leitura/edição de arquivos, execução de comandos.
- Persistência de sessão em disco.
- Mais de um provedor implementado.
- Markdown renderizado, syntax highlight, temas.
- Múltiplas sessões simultâneas.
- Carregamento dinâmico de plugins (dylib).
- Modo headless (`tupi ask "..."`).

## Anti-objetivos

- Um chat que não compila em pedaços independentes: o frontend não pode importar
  o provider, e o provider não pode importar a UI.
- Um "CQRS" de fachada: os comandos precisam passar pelo bus e os eventos precisam
  ser a única saída do handler para a UI.
- Configuração espalhada: chave e modelo vêm do ambiente, lidos só na composição.

## Restrições

- Rust 1.99, edition 2021, resolver 2 (mesmo padrão dos projetos existentes).
- tokio, ratatui, crossterm, reqwest (rustls, json, stream), serde, thiserror/anyhow,
  async-trait; `futures-core`/`futures-util` para stream e EventStream.
- Dependências de workspace declaradas em `[workspace.dependencies]`.
- Sem shell/sistema externo na v1: o binário só fala HTTP com a DeepSeek.

## Abordagens consideradas

1. **Crate único com módulos.** Menor atrito, mas adia fronteiras e refatoração.
   Rejeitado pelo autor: prefere impor as fronteiras desde o começo.
2. **Workspace com um crate por provider.** Isola cada provedor, porém multiplica
   crates para algo que é um arquivo. Rejeitado: `tupi-providers` com um módulo por
   provedor resolve com menos peso.
3. **Workspace + microkernel + CQRS (escolhido).** Crates separados, kernel com
   contratos e bus, providers em um crate com um arquivo cada, TUI consumindo só
   comandos e eventos. Custo aceito: mais estrutura do que o fluxo mínimo pede.

Para o transporte HTTP: **reqwest + parser SSE manual** (escolhido) em vez do SDK
`async-openai`, porque os campos extras da DeepSeek (`thinking`, `reasoning_content`)
precisam caber sem gambiarra.

## Arquitetura

### Crates

```text
tupi-domain        modelos puros: Session, Message, Role, eventos. Sem I/O.
     ^
tupi-kernel        contratos: Provider, SessionStore, CommandBus, QueryBus,
                   EventBus, ProviderRegistry, erros. Depende de domain.
     ^
tupi-providers     um módulo por provedor (deepseek.rs) + registro estático.
                   Depende de kernel.
tupi-app           handlers de comando e query, sessão em memória, orquestração
                   do streaming. Depende de kernel.
tupi-tui           ratatui/crossterm; envia comandos e consome eventos.
                   Depende de kernel.
     ^
tupi (bin)         composition root: lê env, monta registry, app, TUI.
```

Regra de dependência: `tupi-tui` não conhece `tupi-providers`; `tupi-providers` não
conhece `tupi-app`. Só o binário conhece todos.

### Microkernel

O kernel é um contrato pequeno:

- `Provider`: `id()`, `stream_chat(ChatRequest) -> Stream<Item = Result<ProviderEvent>>`.
- `ProviderRegistry`: lookup por id, populado no binário.
- `Command` / `Query` / `Event`: enums fechados na v1.
- `CommandBus`: despacha `Command` para um handler registrado.
- `QueryBus`: despacha `Query` e devolve o resultado tipado.
- `EventBus`: `tokio::sync::broadcast` com eventos de streaming.

Providers não são comandos: são plugins do kernel, registrados na composição.

### CQRS

**Comandos (escrita)**
- `SendMessage { text }` — grava a mensagem do usuário, inicia o streaming e
  devolve `RequestId` imediatamente; a conclusão chega por evento.
- `CancelStream { request_id }` — aborta a task em andamento.

**Queries (leitura)**
- `GetHistory` — mensagens da sessão.
- `ListProviders` — ids registrados.

**Eventos**
- `Token { request_id, text }`
- `StreamFinished { request_id }`
- `StreamFailed { request_id, message }`

Fluxo de `SendMessage`: handler → `SessionStore.append(user)` → `Provider::stream_chat`
→ publica `Token` a cada delta → ao fim, `SessionStore.append(assistant)` →
`StreamFinished`. A TUI só reage a eventos; nunca chama o provider.

Streaming e CQRS convivem por evento: o comando inicia, os deltas são eventos, a
query de histórico só é consistente depois de `StreamFinished`.

**Concorrência:** uma sessão aceita um stream por vez. `SendMessage` durante streaming
devolve `CommandError::Busy`; a TUI mostra o estado e mantém o texto no input.
`RequestId` é um `u64` atômico gerado pelo app e devolvido pelo comando, para que o
frontend possa cancelar sem esperar o primeiro evento.

### Sessão e estado

`SessionStore` em memória (`Mutex<HashMap<SessionId, Session>>`) na `tupi-app`.
Sessão única criada na inicialização, id fixo. Sem disco.

### Provedor DeepSeek

- `POST https://api.deepseek.com/chat/completions`, `stream: true`.
- Chave de `TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY`; ausência é erro de inicialização
  com mensagem acionável.
- Modelo padrão `deepseek-flash`; `TUPI_MODEL` sobrescreve.
- `thinking` desligado na v1 (campo omitido).
- Parser SSE próprio: acumula bytes, quebra em linhas, ignora `: keep-alive`,
  trata `data: [DONE]`, desserializa `choices[0].delta.content`, preserva UTF-8
  partido entre chunks.
- Timeout de conexão de 10 s; sem timeout total (stream pode ser longo). Se não
  chegar nenhum byte por 120 s, o stream é abortado como `StreamFailed`.
- Erros de HTTP, de rede e de formato viram `ProviderError` distintos.

### Interface (TUI)

- Layout: histórico (scroll) + input de uma linha + barra de status (modelo,
  provedor, estado: idle/streaming/busy).
- Enter envia; Ctrl+C sai; Esc cancela o stream em andamento.
- Texto puro, sem markdown.
- Renderização incremental dos tokens; rolagem automática enquanto o stream corre,
  rolagem manual quando o usuário sobe.
- A mensagem do assistente em andamento vive num buffer local da TUI; ela só entra
  no histórico persistido ao receber `StreamFinished`.
- Loop de eventos com `tokio::select!` entre `crossterm::event::EventStream` e o
  canal do `EventBus`.
- Guarda de terminal: restaurar modo raw, cursor e tela ao sair e em panic.

## Configuração

| Variável | Obrigatória | Padrão |
|---|---|---|
| `TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY` | sim | — |
| `TUPI_MODEL` | não | `deepseek-flash` |
| `TUPI_LOG` | não | `warn` |

Nenhum arquivo de config na v1.

## Observabilidade

`tracing`/tracing-subscriber gravando em arquivo no diretório de estado do usuário
(`$XDG_STATE_HOME/tupi/tupi.log`, fallback `~/.local/state/tupi/tupi.log`), porque
a TUI ocupa o stderr. Chave e corpo de mensagens nunca são logados.

## Erros

- `thiserror` nos crates de biblioteca; `anyhow` só no binário.
- Chave ausente: falha antes de abrir a TUI, com o nome exato da variável.
- Falha de rede durante o stream: `StreamFailed` na UI, texto já recebido preservado,
  input volta a aceitar.

## Testes

- Parser SSE: fixtures com chunks partidos no meio de linhas e de caracteres UTF-8,
  `[DONE]`, keep-alive e JSON inválido.
- `tupi-app`: handler de `SendMessage` com `FakeProvider` que emite N tokens;
  verifica eventos e histórico final; teste de `Busy`.
- `ProviderRegistry`: lookup de id desconhecido.
- TUI: sem snapshot na v1; um smoke test de montagem do estado.
- Integração real com a DeepSeek: manual, fora do CI.

## Marcos

1. Workspace, crates vazios, `tupi-domain` e contratos do kernel compilando.
2. `tupi-providers`: DeepSeek + parser SSE com testes.
3. `tupi-app`: comandos, queries, eventos e teste com fake.
4. `tupi-tui`: chat, streaming, scroll, cancelamento.
5. `tupi` (bin): env, composição, mensagens de erro.
6. Polimento: status bar, Esc, testes restantes.

## Decisões deixadas ao implementador

- `Command`/`Query` como enums fechados na v1; traits só quando houver extensão real.
- `EventBus` como `broadcast` com capacidade 256; lag vira aviso na UI.
- Cancelamento por `AbortHandle` guardado no handler.
- Sem `session_id` na v1: uma sessão implícita.
- `SessionStore` usa `std::sync::Mutex` (seções curtas, sem await dentro).
