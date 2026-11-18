# Provedor `openai-plan` (assinatura ChatGPT)

Provedor `openai-plan` em `crates/tupi-providers/src/openai_plan.rs:29`: fala a
Responses API do backend de assinatura ChatGPT (Plus/Pro/Team), não Chat
Completions, e autentica com OAuth PKCE público — sem `client_secret` no binário
nem no repositório (`:8-10`). Como todo provedor por assinatura, dispensa
`DEEPSEEK_API_KEY` (`crates/tupi/src/config.rs:33-39`).

## Login e credenciais

`/connect openai-plan` abre o navegador (`run_login_flow`,
`crates/tupi-providers/src/openai_plan.rs:817`), recebe o retorno em
`127.0.0.1:1455/auth/callback` (`:36-39`) e troca o `code` + `code_verifier` no
endpoint de token (`:263-270`). As credenciais vão para o redb em
`provider_credentials("openai-plan")` (`crates/tupi-app/src/app.rs:2144-2148`),
com arquivo `openai-plan.json` em `~/.tupi` só como fallback
(`crates/tupi-providers/src/openai_plan.rs:75`, `:463-465`). O access token é
renovado 5 min antes do vencimento (`REFRESH_SKEW_SECS`, `:72`) e o token novo é
regravado pelo hook `with_on_refresh` (`crates/tupi-app/src/app.rs:2140-2170`).

Cada requisição leva `Authorization: Bearer <access_token>` e
`chatgpt-account-id: <account_id>`
(`crates/tupi-providers/src/openai_plan.rs:1344`); o account id sai do claim
`https://api.openai.com/auth` do `id_token` (`:69`, `:883-900`), com fallback
para `sub` (`:1589`).

## Modelo padrão e catálogo

Padrão `gpt-5.2-codex` (`DEFAULT_OPENAI_PLAN_MODEL`,
`crates/tupi/src/config.rs:13`), cabeça do catálogo fixo do módulo
(`DEFAULT_MODELS`, `crates/tupi-providers/src/openai_plan.rs:44-52`). O catálogo
é literal e `list_models` não bate na rede: não há endpoint de modelos verificado
neste backend e uma falha ali viraria `ModelsFailed` em toda abertura da TUI
(`:42-43`).

## Variáveis de ambiente

Todas as constantes de rede nascem de `network_constant(env, default)`
(`crates/tupi-providers/src/openai_plan.rs:84-86`) e podem ser sobrescritas sem
recompilar (`:54-66`):

| Env | Default |
| --- | --- |
| `TUPI_OPENAI_CLIENT_ID` | `app_EMoamEEZ73f0CkXaXp7hrann` (`:32`) |
| `TUPI_OPENAI_AUTHORIZE_URL` | `https://auth.openai.com/oauth/authorize` (`:33`) |
| `TUPI_OPENAI_TOKEN_URL` | `https://auth.openai.com/oauth/token` (`:34`) |
| `TUPI_OPENAI_BASE_URL` | `https://chatgpt.com/backend-api/codex` (`:35`) |
| `TUPI_OPENAI_REDIRECT_URI` | `http://localhost:1455/auth/callback` (`:36`) |
| `TUPI_OPENAI_CALLBACK_ADDR` | `127.0.0.1:1455` (`:39`) |
| `TUPI_OPENAI_ORIGINATOR` | `codex_cli_rs` (`:40`) |
| `TUPI_OPENAI_MODELS` | lista separada por vírgula; vazio cai no catálogo (`:112-128`) |
| `TUPI_OPENAI_SCOPES` | `openid profile email offline_access` (`:65`) |

**Por que são substituíveis**: os valores do cliente oficial da OpenAI não são
documentados para terceiros e não puderam ser verificados na implementação
(`:8-10`). Manter os defaults num bloco único no topo do módulo e expô-los por
env deixa corrigir client id, URLs ou catálogo por configuração, sem patch no
binário.

## Descoberta: autocomplete de `/connect `

Digitar `/connect ` abre a lista de ids conhecidos, filtrável por prefixo, e Tab
completa para `/connect <id>` — mesmo molde de `/workflows/` e `/sdd `
(`crates/tupi-tui/src/state.rs:592-641`). A lista **não** é literal na TUI:
`ChatState.providers` (`state.rs:108-111`) é preenchido no boot por
`Query::ListProviders` (`crates/tupi-tui/src/run.rs:67-72`), respondido com
`registry.ids()` (`crates/tupi-app/src/app.rs:2641`). Duas consequências do
contrato: provedor sem registro não aparece (lista fixa inventaria id sem
credencial nem modelo), e com espaço depois do id o prefixo não casa mais nada —
a lista fecha em vez de reabrir (`state.rs:620-632`). Coberto por
`crates/tupi-tui/tests/connect_command.rs:14-57`.

## Limitações

- Os defaults de rede são palpite informado, não contrato publicado: se a OpenAI
  mudar client id, redirect ou base, o login falha até o env ser ajustado.
- Só Responses API (`{base}/responses`, corpo com `input`, `instructions`, `tools`
  sem envelope `function` e `store: false`,
  `crates/tupi-providers/src/openai_plan.rs:951-980`); Chat Completions não é
  aceito neste backend.
- O SSE tem vocabulário próprio (`response.output_text.delta`,
  `response.function_call_arguments.delta`, `response.output_item.done`,
  `response.completed`), tratado por um `extract_event` local — o de
  `crates/tupi-providers/src/sse.rs:86-153` só entende `choices[0].delta`.
- Exige plano ChatGPT ativo; `DEEPSEEK_API_KEY` ausente não é erro aqui, mas
  credencial vencida sem `refresh_token` obriga novo `/connect`
  (`crates/tupi-providers/src/openai_plan.rs:346`).

## Verificação

Sem rede nos testes: fixture SSE servida por `TcpListener` local
(`crates/tupi-providers/tests/openai_plan_stream.rs:1-40`) e credenciais no redb
temporário (`crates/tupi-app/tests/openai_plan_credentials.rs`). O autocomplete do
`/connect ` fecha em `crates/tupi-tui/tests/connect_command.rs`. Comandos:
`cargo test -p tupi-providers`, `cargo test -p tupi-app`,
`cargo test -p tupi-tui`, `cargo fmt --check` e `cargo clippy`.
