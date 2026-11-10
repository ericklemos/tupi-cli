# Tupi

Chat de terminal em Rust, com TUI (ratatui), streaming e tools de leitura,
busca, edição, escrita, execução e grafo de chamadas. O modelo trabalha no
projeto sem receber o projeto inteiro.

## Ferramentas

- **Leitura e busca** — `read` (esqueleto do arquivo ou corpo de um símbolo,
  com blake3 e leitura de `rev:path`), `read_many` (lote atômico com orçamento
  compartilhado), `search` (descrição natural ou nome, com filtro de caminho),
  `map` (arquivos/símbolos por centralidade ou resumo por diretório) e
  `peek` (recupera por handle o que não coube no orçamento).
- **Edição** — `edit` troca o corpo `{ … }` de um símbolo Rust com
  `expected_hash`; `write` e `write_many` gravam arquivos inteiros (com hash,
  `create_only` e rollback no lote); `move` move ou copia arquivo/diretório e
  lista quem importava o nome antigo; `patch` aplica diff unificado validado
  com `git apply --check`. Caminho que escapa do root é recusado.
- **Execução** — `exec` roda `cargo check/build/test/clippy/xtask` com
  `package`, `target` e `filter`, agrupa erros de compilação, guarda resultado
  em cache e respeita timeout; `test_many` roda testes específicos em lote;
  `shell` aceita apenas formatador (`cargo fmt`, `rustfmt`), sem composição.
- **Grafo** — `assert` (existe/é público/é usado fora), `impact_check`
  (chamadores reversos), `context_graph` (chamadores e consumidores com
  profundidade), `trace` (caminho A → … → B), `diff` (símbolos alterados,
  chamadores e testes afetados) e `why` (commits que tocaram um símbolo).
  `diff` e `why` precisam de um repositório git.
- **Web** — `fetch` (baixa uma URL e devolve os blocos relevantes com âncoras),
  `websearch` (busca sem provedor pago) e `research` (busca, baixa as melhores
  fontes e devolve a evidência com URL).
- **Sessão** — `ledger` (fatos duráveis por repositório), `state` (notas,
  tarefas e retomada), `env` (allowlist estrita) e `setup` (gera a documentação
  das tools em `AGENTS.md` e na skill).

O root das leituras é o diretório onde o Tupi foi aberto. A v1 entende AST de
Rust; arquivos de outras linguagens caem no fallback de conteúdo inteiro,
limitado pelo mesmo orçamento.

## Requisitos

- Rust 1.99+
- Uma chave da plataforma DeepSeek
- `git` para `diff`, `why` e `patch`; `curl` para as tools de web

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
tupi-tools        AST, orçamento, store/peek, ledger, grafo e as tools
tupi-app          handlers de comando/query, loop de tools + eventos
tupi-tui          ratatui: envia comandos, reage a eventos
tupi              composição: env, registry, tools, app, TUI
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
