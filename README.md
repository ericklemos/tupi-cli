# Tupi

Chat de terminal em Rust, com TUI (ratatui) e streaming. Esta é a v1: uma
conversa por sessão, sem agente, ferramentas ou edição de arquivos.

## Requisitos

- Rust 1.99+
- Uma chave da plataforma DeepSeek

## Configuração

| Variável | Obrigatória | Padrão |
|---|---|---|
| `TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY` | sim | — |
| `TUPI_MODEL` | não | `deepseek-flash` |
| `TUPI_LOG` | não | `warn` |

As variáveis podem vir do ambiente ou de um arquivo `.env` (o Tupi procura na
raiz do projeto e nos diretórios acima). **O ambiente tem precedência** sobre o
arquivo.

```bash
cp .env.example .env
# edite .env e preencha TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY
cargo run -p tupi
```

Ou direto no ambiente:

```bash
export TUPI_CLI_DEEPSEEK_PLATAFORM_APIKEY="sk-..."
cargo run -p tupi
```

O `.env` é ignorado pelo git; o `.env.example` é versionado. Sem a chave, o
programa falha antes de abrir a TUI, citando o nome da variável.

## Controles

- **Enter** envia a mensagem; no modal de modelos, troca o modelo ativo
- **/models** abre o modal com os modelos do provedor atual (consulta `GET /models`)
- **Tab** completa o comando sugerido (autocomplete a partir de `/`)
- **↑ / ↓** rolam o histórico; com sugestões ou modal abertos, navegam pela lista
- **Esc** cancela o stream; no modal, fecha
- **Ctrl+C** sai

## Arquitetura

Workspace microkernel + CQRS:

```text
tupi-domain       tipos puros (Session, Message, Event)
tupi-kernel       contratos (Provider, buses, registry)
tupi-providers    um módulo por provedor (deepseek.rs)
tupi-app          handlers de comando/query + eventos de streaming
tupi-tui          ratatui: envia comandos, reage a eventos
tupi              composição: env, registry, app, TUI
```

A TUI não conhece o provider; o provider não conhece a TUI. Só o binário
conhece todos.

## Testes

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
```

O parser SSE é testado com chunks partidos no meio de linhas e de caracteres
UTF-8. A integração real com a DeepSeek é manual.

## Logs

`$XDG_STATE_HOME/tupi/tupi.log` (ou `~/.local/state/tupi/tupi.log`). Chave e
conteúdo de mensagens não são logados.
