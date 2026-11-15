# 🤨 Skeptic — resolução do falso-verde do `exec`

**Data:** 2026-11-11 · **Catástrofe:** `.agents/results/skeptic/2026-11-11-exec-cache-stale-green.md`
· **Veredito:** buraco principal fechado; três lacunas residuais continuam abertas e
nomeadas no fim.

## O que mudou

### 1. A chave do cache passou a ser o conteúdo, não o `mtime`

`crates/tupi-tools/src/tools/exec/render.rs:32-93`

- **Antes:** identidade de cada arquivo = `(path, len, mtime_nanos)`, apenas extensões
  `rs|toml|lock`, `hidden(true)`, `standard_filters(true)`.
- **Agora:** `blake3` do **conteúdo** de todo arquivo regular sob a raiz, com o caminho e o
  tamanho no hash. O walker roda com `hidden(false)`, então `.cargo/config.toml`
  (rustflags/linker/`[env]`), `rust-toolchain.toml`, `.sqlx/` e fixtures de **qualquer**
  extensão entram na chave. Continuam fora: `target/`, `.git/` e o que o git ignora.
- **Duas falhas fechadas (o cache admite não saber):** acima de `limit` arquivos ou de
  64 MiB de conteúdo, `fingerprint` devolve `None` e o cache simplesmente não é usado.
  Erro de IO em qualquer arquivo também devolve `None`. Nenhum caminho trunca em silêncio.
- Arquivos especiais (FIFO, socket) são ignorados por `is_file()`, evitando abrir um FIFO e
  travar a tool dentro do `spawn_blocking`.

### 2. Um hit não se disfarça mais de execução fresca

`crates/tupi-tools/src/tools/exec/diagnostics.rs:69-...` e `exec/mod.rs:84-205`

- `Cached` agora guarda `at: Instant`; `sections_for` recebe `cached_age: Option<Duration>`
  em vez de um `bool`.
- Linha de status passa de `status: ok · 3.4s · em cache` para
  `status: ok · em cache há 2m5s · execução original: 3.4s`. A duração antiga não é mais
  apresentada como se fosse deste run.

### 3. Quatro testes que falham se o buraco voltar

`crates/tupi-tools/src/tools/exec/tests.rs:131-192`

| Teste | Falsifica |
|---|---|
| `fingerprint_follows_content_when_length_and_mtime_are_unchanged` | reescreve o arquivo com o mesmo tamanho e **restaura o mtime original** (`File::set_modified`); exige fingerprint diferente |
| `fingerprint_covers_hidden_build_configuration` | altera `.cargo/config.toml` (mesmo tamanho, byte diferente); exige fingerprint diferente |
| `fingerprint_covers_fixtures_outside_the_source_extensions` | altera `case.json` (mesmo tamanho); exige fingerprint diferente |
| `a_cache_hit_reports_its_age_instead_of_the_original_runtime` | exige `em cache há 2m5s` e `execução original: 3.4s` no corpo |

## Prova

| Afirmação | Falsificador rodado | Resultado |
|---|---|---|
| Compila | `exec kind=check package=tupi-tools` | ok · 2.0s |
| Testes do módulo passam | `exec kind=test package=tupi-tools filter=exec::tests` | 16 passed; 0 failed |
| Sem lint novo | `exec kind=clippy package=tupi-tools` | ok; 4 avisos, todos pré-existentes em `sdd`, `docs/store`, `docs/index`, `lib.rs` |
| Suite do crate | `exec kind=test package=tupi-tools target=lib` | 320 passed; **6 failed**, todos em `tools::workflow::tests` |
| Cache reutilizado ponta a ponta nesta sessão | segundo `exec kind=check package=tupi-tools` | **não verificado** — o segundo run não imprimiu `em cache há`, então não há evidência de hit (instância de tool recriada por chamada ou fingerprint indisponível); o rótulo em si está coberto pelo teste unitário |

Os 6 `tools::workflow::tests` que falham (`an_agents_workflow_beats_the_native_one`,
`an_unknown_name_lists_what_exists`, `list_shows_source_and_read_returns_the_body`,
`the_closest_layer_wins`, `the_prompt_section_lists_names_without_bodies`,
`tupi_beats_agents_in_the_same_scope`) são **de outro módulo** e afirmam que os workflows
nativos (`WorkflowSource::Native`) deveriam estar no catálogo — o catálogo devolveu zero
nativos. Meu diff não toca `tools/workflow/**` nem o embed `NATIVE`. **Não verificada como
pré-existente:** sem `git` nesta sessão não há como diffar a árvore limpa; o falsificador
é rodar `cargo test -p tupi-tools --lib tools::workflow` num checkout limpo.

## Lacunas residuais (abertas, por escrito)

1. **Arquivos ignorados pelo git continuam fora da chave.** Trocar `hidden(true)` por
   `hidden(false)` trouxe dotfiles, mas `standard_filters(true)` mantém o respeito ao
   `.gitignore`. Um fixture em diretório ignorado (`.env`, `.brain`, testes que leem
   artefato gerado) segue invisível. Falsificador: teste que escreve em path gitignorado.
2. **Link simbólico de diretório não é seguido** (o walker não faz `follow_links`); fontes
   vendidas por symlink de diretório não entram na chave. Falsificador: symlink de dir com
   um `.rs` dentro.
3. **Ambiente não entra na chave.** `RUSTFLAGS`, `CARGO_BUILD_TARGET`, `CARGO_HOME` e a
   versão do toolchain via `rustup override` mudam o resultado do build e não mudam o hash.
   `rust-toolchain.toml` só cobre o caso em que a toolchain é fixada por arquivo.
4. **Custo.** Toda chamada de `exec` agora paga o crawl + leitura integral do workspace
   (fora `target/`, `.git/` e gitignored). Em repos pequenos é ruído diante de um cargo
   run; em repos grandes o teto de 5.000 arquivos / 64 MiB desliga o cache — fail closed,
   como deve ser, mas o custo do crawl permanece. O que o cache compra continua sendo
   **latência, não tokens**: o resumo já está no histórico e o pruner vai substituí-lo por
   um ponteiro independentemente.

## Ordem em que eu atacaria o resto

1. Lacuna 1 (gitignored) — é a mesma classe de falso-verde que o buraco original; um teste
   com `.gitignore` + fixture fecharia o caso e decidiria entre "incluir ignorados" e
   "nunca cachear quando existir arquivo ignorado que possa ser entrada".
2. Lacuna 3 (ambiente) — incluir `RUSTFLAGS`/`CARGO_BUILD_TARGET` no hash é uma linha.
3. Lacuna 2 (symlink de dir) — resolve-se com `follow_links(true)` mais proteção de ciclo,
   ou documentando que fontes vetadas por symlink nunca são cacheadas.

## Segundo round — as três lacunas da primeira entrega

### 1. Arquivo ignorado pelo git entrou na chave — fechada

- Walker agora roda com `.ignore(false).git_ignore(false).git_global(false).git_exclude(false).parents(false)`.
- Fora da chave sobrou uma lista nomeada e justificada: `target/`, `.git/`, `.brain/`, `.agents/tupi/`.
  Não existe mais whitelist de extensão nem filtro de gitignore.
- Teste: `fingerprint_covers_files_the_gitignore_hides` (`.gitignore` com `*.json`, fixture
  alterada, exige hash novo).

### 2. Symlink de diretório passou a ser seguido — fechada, sem teste de ciclo

- `.follow_links(true)` em `fingerprint_with` (`crates/tupi-tools/src/tools/exec/render.rs`).
- Ciclo de link não trava o walk: o teto de arquivos converte o laço em `None`, e sem chave
  não há cache. **Não verificado por teste** — um teste de ciclo precisa criar mais de 5.000
  entradas; falsificador pendente.
- Teste: `fingerprint_follows_a_directory_symlink` (`#[cfg(unix)]`, diretório fora da raiz
  linkado para dentro dela).

### 3. Ambiente entrou na chave — fechada, com ressalva

- `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_TARGET`, `CARGO_BUILD_RUSTFLAGS` e
  `RUSTUP_TOOLCHAIN` entram no hash antes dos arquivos.
- A injeção é por `fingerprint_with(root, limit, environment)` para o teste ficar puro:
  `std::env::set_var` é `unsafe` em Rust 2024 e corre com testes paralelos.
- Teste: `fingerprint_changes_when_the_build_environment_changes`.
- **Ressalva:** a leitura real do ambiente pelo wrapper `fingerprint` não tem teste.
  Falsificador: teste serializado com `set_var` em bloco `unsafe`.

### 4. A única exclusão sem prova: `.agents/tupi/`

- É estado que a própria tool grava (`facts.jsonl`, `pending.jsonl`, `debug/`). Busca lexical
  não achou nenhum teste lendo esse caminho como entrada.
- O teste `fingerprint_ignores_the_agents_own_bookkeeping` fixa a exclusão para ela não ser
  ampliada em silêncio. Ele **não** detecta se um teste passar a ler `.agents/tupi/`: nesse
  dia a exclusão vira furo e nada falha.

## Achado operacional: o cache pode nunca acertar

- Cinco chamadas idênticas de `exec kind=check package=tupi-tools` nesta sessão; **nenhuma**
  imprimiu `em cache há`.
- O teto não é a causa: um teste-probe mediu `fingerprint(<raiz do workspace>, 5_001) =
  Some(64)` em 0.01s (probe removido depois da medição).
- Hipóteses abertas: (i) a instância de `ExecTool`/o processo é recriada por chamada e o cache
  em RAM morre; (ii) algum arquivo sob a raiz muda entre chamadas.
- Se (i) for verdade, o cache é **puro custo**: 0.01s de walk por chamada e zero reuso. O
  falsificador é um teste de integração que instancia `ExecTool` e chama `call` duas vezes.

## Vereditos do segundo round

| # | Afirmação | Falsificador | Resultado |
|---|---|---|---|
| 1 | Compila | `cargo check -p tupi-tools` | ok · 0.6s |
| 2 | Testes do módulo | `cargo test -p tupi-tools -- exec::tests` | **20 passed, 0 failed** |
| 3 | Sem lint novo | `cargo clippy -p tupi-tools` | ok; 5 avisos, nenhum em `exec/` |
| 4 | Fingerprint do workspace não estoura o teto | probe temporário | `Some(64)` em 0.01s |
| 5 | Cache acerta ponta a ponta | 2 chamadas idênticas | **refutada** — nunca imprimiu `em cache há` |

## Nota de estabilidade do workspace

Durante a sessão `cargo check -p tupi-tools` falhou uma vez compilando `tupi-infer`
(`cannot find backend in the crate root`) porque outra sessão adicionou `tupi-infer` com a
feature `local-models` a `crates/tupi-tools/Cargo.toml` no meio de um refactor. As medições
acima foram refeitas depois dessa mudança e passaram.
