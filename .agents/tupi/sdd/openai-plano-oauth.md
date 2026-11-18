<!-- Gerado automaticamente pelo tupi (somente leitura; fonte de verdade no banco local). -->

# Provedor OpenAI via plano ChatGPT (OAuth) (`openai-plano-oauth`)

**Status:** completed

## Objetivo

Adicionar o provedor `openai-plan` ao tupi: login OAuth no navegador com conta ChatGPT (Plus/Pro/Team), credenciais salvas no redb com refresh automático, e streaming via Responses API (`{base}/responses`), espelhando a arquitetura que o provedor `antigravity` já usa. Fonte: `crates/tupi-providers/src/antigravity.rs:1751-1854` (login), `crates/tupi-providers/src/antigravity.rs:780-930` (credenciais e refresh), `crates/tupi-kernel/src/provider.rs:65-77` (contrato `Provider`).

## Restrições

- Sem `client_secret`: o fluxo é PKCE público. Nenhum segredo vai para o binário nem para o repo.
- Todas as constantes de rede (client_id, authorize, token, base, redirect, originator, modelos) declaradas no topo do módulo novo E sobrescrevíveis por env (`TUPI_OPENAI_*`), porque os valores do cliente oficial da OpenAI não são documentados para terceiros e não puderam ser verificados nesta sessão (`research`/`websearch` voltaram vazios).
- Nada de tocar o caminho DeepSeek: `crates/tupi-providers/src/deepseek.rs` e `crates/tupi/src/config.rs:35-42` só ganham o ramo do provedor novo.
- Teste não acessa a rede: fixture SSE e servidor local, como em `crates/tupi-providers/tests/antigravity_last_payload.rs`.
- Sem editar o bloco gerado de `AGENTS.md` (`crates/tupi-tools/src/tools/setup/mod.rs:73`); doc do provedor entra fora dos marcadores, no máximo 1/5 do teto do arquivo.

## Nuances

- O contrato é `Provider` com `id`, `list_models`, `stream_chat` e `last_payload` — último payload guardado em `Mutex` como em `crates/tupi-providers/src/deepseek.rs:112-117`.
- O backend de assinatura fala Responses API, não Chat Completions: corpo com `input`, `instructions`, `tools` sem envelope `function` e `store: false`; o corpo novo não reusa `tupi_kernel::tools_payload` (`crates/tupi-kernel/src/tool.rs:17-31`, que monta o formato chat).
- Requisição exige `Authorization: Bearer <access_token>` e `chatgpt-account-id: <account_id>`; o account id sai do claim do `id_token` (JWT, `https://api.openai.com/auth`), com fallback para `sub`.
- Eventos SSE de resposta têm nome próprio (`response.output_text.delta`, `response.function_call_arguments.delta`, `response.output_item.done`, `response.completed`) — `crates/tupi-providers/src/sse.rs:86-153` só entende `choices[0].delta`, logo o módulo novo precisa do seu próprio `extract_event`.
- Reuso possível e desejado: `SseParser` (`crates/tupi-providers/src/sse.rs:8-50`) e `ToolCallAccumulator` (`crates/tupi-providers/src/deepseek.rs:55-93`, hoje `pub(crate)`, com `ToolCallDelta` de `:68-73`).
- Erros mapeiam para `ProviderError` (`crates/tupi-kernel/src/error.rs:1-29`): `Http { status, body }` para falha de token/stream, `Protocol` para evento fora do formato, `Network` para transporte e stall.
- PKCE reaproveita a técnica de `crates/tupi-providers/src/antigravity.rs:1496-1520` (`base64_url_encode` + `Sha256` via crate `sha2`, já em `crates/tupi-providers/Cargo.toml:16`); não há `base64` nem `rand` nas dependências, então o verifier nasce de `blake3` sobre tempo (como `antigravity.rs:1761-1770`).
- Credenciais seguem o padrão de persistência do redb: `store.provider_credentials(id)` e `save_provider_credentials` em `crates/tupi-app/src/app.rs:2082-2117`, com hook `with_on_refresh` para gravar token renovado (`app.rs:2140-2154`).
- `list_models` não bate na rede: catálogo fixo declarado no módulo (não há endpoint de modelos verificado neste backend), evitando `ModelsFailed` em toda abertura do TUI.
- Provider por assinatura não exige `DEEPSEEK_API_KEY`; hoje `crates/tupi/src/config.rs:35` testa `provider == "antigravity"` literal e precisa virar teste de assinatura.
- O comando `/connect <id>` só aceita `antigravity` (`crates/tupi-app/src/app.rs:2344-2438`), e a lista de comandos vive em `crates/tupi-tui/src/state.rs:8`; o parse do comando está em `crates/tupi-kernel/src/command.rs:66`.

## Tasks

- [x] **t1** Módulo de credenciais e OAuth de assinatura
  - Novo `crates/tupi-providers/src/openai_plan.rs` registrado em `crates/tupi-providers/src/lib.rs`; constantes `TUPI_OPENAI_*` num bloco só no topo; credenciais, JWT, PKCE, exchange e refresh.
  - [x] **t1.1** Credenciais, JWT e persistência — Claim `https://api.openai.com/auth`, fallback `sub`, token inválido → None, round-trip serde, `is_expired` com skew, arquivo ausente → None.
  - [x] **t1.2** PKCE, authorize URL e token exchange — URL com client_id, redirect, `code_challenge_method=S256`, `state` e scopes; `expires_in` vira `expires_at`; erro → `ProviderError::Http`.
  - [x] **t1.3** Refresh automático — Token vencido renova antes do uso; falha vira `Http`/`Network`; hook persiste credencial nova.
- [x] **t2** Provider Responses API
  - `OpenAiPlanProvider` sobre Responses API (`build_body`, `extract_event`, `stream_chat`, `list_models` estático, `register_provider`).
  - [x] **t2.1** Corpo da requisição — Sistema em `instructions`; `tool_calls` viram `function_call`, resultado vira `function_call_output`; `store == false`; sem teto não há `max_output_tokens`.
  - [x] **t2.2** Parser de eventos SSE — Texto, reasoning summary, início/argumentos/fim de function call, usage, `response.failed`/`incomplete`; JSON inválido → `Protocol`.
  - [x] **t2.3** Stream e erros HTTP — Servidor local devolve `Started` → `Text` → `ToolCalls` → `Usage` → `Finished`; 401 repassa `Http`; stream parado estoura stall.
- [x] **t3** Fluxo de login no navegador
  - `run_login_flow` com listener `127.0.0.1:1455` em `/auth/callback`, abertura do navegador por SO, validação de `state`, troca do código.
  - [x] **t3.1** Callback e navegador — Página de sucesso no navegador; `state` divergente ou sem `code` → `Protocol`; porta ocupada → `Network` com a porta na mensagem.
- [x] **t4** Ligação no CLI, app e TUI
  - `crates/tupi/src/config.rs:35-42` deixa de exigir `DEEPSEEK_API_KEY` para provedor de assinatura; boot em `crates/tupi/src/main.rs:110-176`; credenciais e registro em `crates/tupi-app/src/app.rs:2082-2179`; ramo em `Command::Connect` (`app.rs:2344-2438`).
  - [x] **t4.1** Config e boot — `TUPI_PROVIDER=openai-plan` sem chave sobe com o modelo padrão; deepseek segue exigindo chave.
  - [x] **t4.2** Credenciais no redb — Login grava em `provider_credentials("openai-plan")`; refresh regrava; `sync_connected_providers` reconecta após reinício.
  - [x] **t4.3** Comando /connect — `/connect openai-plan` publica a URL como `Event::Notice`, registra o provider e lista modelos; id desconhecido mantém a mensagem atual.
- [x] **t6** Autocomplete de provedores no /connect
  - Autocomplete de provedores em `/connect `, no mesmo molde de `/workflows/` e `/sdd `: lista de ids conhecidos no `ChatState` (`crates/tupi-tui/src/state.rs:105` é o precedente de `sdd_specs`), novo ramo em `refresh_suggestions` (`state.rs:586-634`) e população no boot (`crates/tupi-tui/src/run.rs:65-66`). Ids vêm do registry (`crates/tupi-app/src/app.rs:2263`); se nenhum evento expõe a lista, criar consulta no barramento de queries e testar como em `crates/tupi-tui/tests/connect_command.rs`.
  - [x] **t6.1** Lista de provedores no ChatState — Digitar `/connect ` lista `deepseek` e `antigravity` e, quando o provedor novo estiver registrado, `openai-plan`; prefixo filtra; Tab completa para `/connect <id>`; espaço depois do id não reabre a lista.
  - [x] **t6.2** Testes de autocomplete — Teste de TUI cobre `/connect `, `/connect open`, Tab e a garantia de que texto comum continua sem sugestão.
- [x] **t5** Verificação e documentação
  - `cargo fmt --check`, `cargo clippy` e testes dos crates tocados; documentar envs, modelo padrão e limitações fora do bloco gerado de `AGENTS.md`.
  - [x] **t5.1** Testes e lints verdes — `cargo test -p tupi-providers -p tupi-app -p tupi -p tupi-tui` passa sem rede e sem warning novo de clippy.
  - [x] **t5.2** Documentação do provedor — `TUPI_OPENAI_*`, modelo padrão e o motivo das constantes serem substituíveis em prosa curta com fonte `caminho:linha`.
