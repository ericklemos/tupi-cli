# Tupi

Chat de terminal em Rust, com TUI (ratatui), streaming e tools de leitura,
busca, edição, escrita, execução e grafo de chamadas. O modelo trabalha no
projeto sem receber o projeto inteiro.

A descoberta começa em `map`: o prompt não carrega o repositório.

## Como o Tupi pensa

- **Local para o que é mecânico** — embedding, resumo e transcrição rodam na
  máquina, sem sair do projeto.
- **API só onde há raciocínio** — o turno principal e o executor do SDD.
- **Nenhuma liberdade ao modelo** — tools fechadas, root confinado, edição com
  hash, shell de leitura e orçamentos de contexto e de passos.

O detalhamento de cada princípio, com o código que o sustenta hoje e o que
ainda não existe, está em [`docs/philosophy.md`](docs/philosophy.md).

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
- **Projeto** — `docs` consulta as pastas de `docs_folders` (`map`, `query`,
  `read`, `status` e `refresh`; `write` só dentro delas), funde o lexical com o
  vetorial quando o embedder está carregado e é core: só o modo SDD a vê. `sdd`
  mantém o plano em tasks e subtasks — `upsert_spec`, `set_tasks` e `close` no
  planejamento, `progress` na execução —, grava o estado em `.tupi/sdd` com
  espelho em `.agents/tupi/sdd/<slug>.md` e retoma com `/sdd <slug>`. `workflow`
  lista e lê procedimentos em três escopos: nativo (embutido no binário), global
  (`~/.agents/workflows`, `~/.tupi/workflows`) e local (`.agents/workflows`,
  `.tupi/workflows`); o usuário invoca com `/workflows/<nome>`. Ao fim da
  execução de um plano, um agente de docs escreve sob `docs_folders` e reindexa
  o que mudou.
- **Acervo** — `archive` (`status`, `index` e `aliases` do acervo) e
  `investigate` (`plan`, `run`, `status`, `report` e `cancel` para investigar o
  acervo por pessoa ou assunto) só entram no registry quando um acervo está
  configurado. A seção `archive` do `ai.project.json` já é validada, mas nada a
  converte em registry: as duas tools não aparecem no binário hoje.
- **Sessão** — `ledger` (fatos duráveis por repositório), `state` (notas,
  tarefas e retomada), `env` (allowlist estrita) e `setup` (gera a documentação
  das tools em `AGENTS.md` e na skill).

O root das leituras é o diretório onde o Tupi foi aberto. A v1 entende AST de
Rust; arquivos de outras linguagens caem no fallback de conteúdo inteiro,
limitado pelo mesmo orçamento.

## Transcrição

`tupi transcribe` transcreve áudio na máquina, sem provedor: whisper.cpp para o
ASR e dois modelos locais menores para segmentação e diarização, baixados sob
demanda no diretório de dados.

```bash
tupi transcribe palestra.m4a --timestamps
tupi transcribe a.wav b.wav --out transcricoes/ --speakers 2
```

- `--out` aponta o arquivo de saída e, com vários áudios, precisa ser
  diretório; sem ele o `.txt` fica ao lado do áudio. Dois áudios que gerariam o
  mesmo arquivo são recusados antes de qualquer trabalho.
- `--model` aceita `base`, `small`, `medium`, `turbo` e `auto` (o padrão) ou um
  caminho `.bin`. No `auto` o modelo vem da GPU detectada; se o backend CUDA não
  subir, a execução avisa e troca o modelo de GPU pelo de CPU.
- `--language` começa em `pt` (`auto` deixa o whisper decidir), `--speakers`
  fixa o número de vozes e `--timestamps` prefixa cada linha com o tempo.
- Os áudios rodam em paralelo até o número de núcleos (no máximo 8), cada um com
  o seu relatório de decode, ASR e diarização; o comando termina com o resumo e
  devolve código 1 quando algum arquivo falha.

O subcomando responde antes de abrir o store e a TUI, depende da feature
`transcribe` e exige `ffmpeg` no `PATH`.

## Dashboard

`tupi --web` sobe o painel no início da sessão e `/dashboard` o abre durante
ela. A porta vem de `--web-port`, depois de `dashboard_web_port` no
`ai.project.json`; sem nenhuma das duas o sistema escolhe uma porta livre. O
launcher sobe o servidor uma vez e abre o navegador.

O servidor é local: `Host`, `Origin` e `Sec-Fetch-Site` fora de
`127.0.0.1`/`localhost` recebem 403 e toda resposta sai com `no-store`. O painel
mostra visão geral, histórico, workflows, busca e vizinhança e caminhos do grafo,
memória, notas de `state`, telemetria recente e os eventos do turno por SSE.

## Requisitos

- Toolchain Rust com `edition = "2024"` (1.85 ou mais novo).
- Uma credencial de provedor, escolhida por `TUPI_PROVIDER`:
  - `deepseek` (padrão) — exige `DEEPSEEK_API_KEY`.
  - `antigravity` — dispensa a chave DeepSeek: usa as credenciais salvas no
    redb da sessão ou, na primeira execução, um arquivo do disco
    (`antigravity.json` no diretório de estado do Tupi ou
    `antigravity-accounts.json` em `$HOME/opencode`).
- `git` para `diff`, `why` e `patch`; `curl` para as tools de web; `ffmpeg`
  para `tupi transcribe`.
- Nada mais para os modelos locais: embedder (LFM2.5-Embedding-350M) e gerador
  de resumos (LFM3-1.7B) são baixados sob demanda com a feature `local-models`.

## Instalação

`tupi install` declara o diretório do executável em execução no `PATH` do
usuário, gravando um bloco marcado no arquivo de inicialização do shell. Não
copia o binário e nunca exige root/administrador.

```bash
# na raiz do workspace, sobre o binário recém-compilado
cargo build --release
./target/release/tupi install

# dry run: mostra o arquivo e o bloco sem escrever nada
./target/release/tupi install --print

# força um shell quando a detecção não for confiável
./target/release/tupi install --shell fish
```

Depois que o diretório estiver no `PATH` de um shell novo, a forma curta
`tupi install` funciona.

`--shell` aceita `auto` (padrão), `bash`, `zsh`, `fish`, `posix` e
`powershell`. No modo `auto`, o shell vem de `$SHELL`, com fallback para os
arquivos de inicialização que já existem:

| Shell | Arquivo | Linha gravada |
|---|---|---|
| `zsh` | `~/.zshrc` | `export PATH="<dir>:$PATH"` |
| `bash` | `~/.bashrc` | `export PATH="<dir>:$PATH"` |
| `fish` | `~/.config/fish/config.fish` | `set -gx PATH "<dir>" $PATH` |
| `posix` | `~/.profile` | `export PATH="<dir>:$PATH"` |
| `powershell` | `$PROFILE` (`Documents/PowerShell/...`) | `$env:Path = "<dir>;$env:Path"` |

O bloco é delimitado por dois marcadores fixos:

```text
# >>> tupi install >>>
export PATH="/home/ana/.cargo/bin:$PATH"
# <<< tupi install <<<
```

Rodar o comando de novo substitui o bloco no lugar, então ele nunca é
duplicado e tudo fora dos marcadores fica byte a byte igual. Uma segunda
execução com o mesmo diretório reporta `já-presente` e não escreve nada; só o
diretório muda quando o binário se move. O relatório lista o diretório
declarado, o arquivo tocado (`criado`, `atualizado` ou `já-presente`) e o
comando para reabrir o shell.

O `PATH` novo só vale em shells novos: reabra o terminal (ou dê `source` no
arquivo) antes de chamar `tupi` pelo nome. `tupi install` nunca pede a chave de
API, nunca abre o store de sessão e nunca sobe a TUI.

## Configuração

Toda variável abaixo é lida do ambiente ou de um `.env` (na raiz do projeto ou
nos diretórios acima — **o ambiente tem precedência**). Nenhuma delas amplia o
poder do modelo: orçamento de tokens, passos de tool e compactação de contexto
ficam nos arquivos do projeto (`configs/token.ron`, `configs/tools.ron` e
`configs/context.ron`).

| Variável | Padrão | Efeito |
|---|---|---|
| `TUPI_PROVIDER` | `deepseek` | provedor do turno principal e do executor do SDD (`deepseek`, `antigravity`) |
| `DEEPSEEK_API_KEY` | — | chave da plataforma DeepSeek; obrigatória só no provedor `deepseek` |
| `TUPI_MODEL` | `deepseek-flash` (`antigravity-gemini-3.8-flash` com `TUPI_PROVIDER=antigravity`) | modelo ativo; o escolhido em `/models` fica no redb e vence a variável |
| `DEEPSEEK_MODEL` | — | alternativa a `TUPI_MODEL` quando ele não está definido |
| `TUPI_LOG` | `info` | filtro do log (`tracing`/`EnvFilter`) |
| `TUPI_SPACE` | ligado | `off` desliga o índice vetorial local que o `search` usa |
| `TUPI_RAG_EMBEDDING` | ligado | `off`, `0` ou `false` desliga o embedder local e o download dele |
| `TUPI_RAG_SUMMARIES` | ligado | `off`, `0` ou `false` desliga o gerador de resumos e o download dele |
| `TUPI_TELEMETRY` | ligado | `0`, `off` ou `false` desliga a telemetria |
| `TUPI_TELEMETRY_PATH` | `.agents/tupi/debug/telemetry.jsonl` | arquivo de telemetria |
| `TUPI_TELEMETRY_SHELL_PATH` | derivado de `TUPI_TELEMETRY_PATH` | arquivo bifurcado das execuções da tool `shell` |
| `TUPI_TELEMETRY_BODY` | desligado | `1`, `on`, `true` ou `yes` grava o corpo do prompt na telemetria |

As variáveis de `space` e de RAG dependem da feature `local-models` compilada;
o diretório de estado (redb, log e `antigravity.json`) é `$XDG_STATE_HOME/tupi`,
com fallback para `~/.local/state/tupi`.

O prompt que configura o agente fica em `configs/prompts.ron`, no campo
`system`. Se o arquivo faltar ou estiver inválido, o Tupi usa o prompt padrão
do código.

```bash
# DeepSeek: a chave é a única variável obrigatória
printf 'DEEPSEEK_API_KEY=sk-...' > .env
cargo run -p tupi

# Antigravity: dispensa a chave DeepSeek e usa as credenciais salvas
TUPI_PROVIDER=antigravity cargo run -p tupi
```

Ou direto no ambiente:

```bash
export DEEPSEEK_API_KEY="sk-..."
cargo run -p tupi
```

O `.env` é ignorado pelo git. Sem chave no provedor `deepseek` e sem
credencial salva no `antigravity`, o programa falha antes de abrir a TUI,
citando o nome da variável.

## Controles

- **Enter** envia a mensagem; no modal de modelos, troca o modelo ativo
- **/models** abre o modal com os modelos do provedor atual (consulta `GET /models`)
- **/enable-show-prompts-messages** liga/desliga o debug que mostra no histórico
  o pacote exato de mensagens enviado ao provedor; a escolha fica no redb
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
