# Provedor pergunta os modelos ao servidor

Regra dura: **todo `Provider::list_models` pergunta ao backend quais modelos a
conta tem.** Catálogo literal no código não é fonte de verdade, é no máximo
recurso de emergência quando a pergunta falha.

## Por quê

Um literal envelhece sozinho: modelo novo lançado depois da compilação
simplesmente não existe para o tupi, e o `/models` mente sobre a conta sem
avisar ninguém. Foi exatamente o que aconteceu com `openai-plan`, que listava
`gpt-5.x` fixo (`crates/tupi-providers/src/openai_plan.rs:44`) enquanto o
cliente oficial já trabalhava com `gpt-6.2-sol`, `gpt-6.2-astra`, `gpt-6.1-sol`
e `gpt-6-sol`.

## Como cada provedor cumpre

- `deepseek` — `GET {base}/models` e `parse_models`; falha vira `Http`/`Network`
  (`crates/tupi-providers/src/deepseek.rs:119`).
- `antigravity` — busca o catálogo disponível no endpoint e reautentica uma vez
  em 401 (`crates/tupi-providers/src/antigravity.rs:1218`). O catálogo estático
  filtrado por entitlement só entra depois de falha registrada.
- `openai-plan` — `GET {base}/models` com `chatgpt-account-id` e `originator`
  (`crates/tupi-providers/src/openai_plan.rs:1370`), id lido de `slug`/`id`/
  `model` por `extract_model_ids`.

## Falha

- **401/403** sobe como `ProviderError::Http`: credencial recusada precisa
  aparecer como erro, nunca como lista.
- **Rede/caminho/formato** pode cair no catálogo embutido, mas com
  `tracing::warn!` e sem apagar o último catálogo bom. Falhou a pergunta, não é
  catálogo vazio.
- Caminho de catálogo que não pôde ser verificado expõe override por env
  (`TUPI_OPENAI_MODELS_URL`) em vez de fingir que está certo.

## Teste

Provedor novo entra com teste de `list_models` contra `TcpListener` local, no
molde de `crates/tupi-providers/tests/openai_plan_stream.rs` — sem rede, e
afirmando que a requisição saiu (ex.: `GET /models`) e que 401 sobe como erro.
