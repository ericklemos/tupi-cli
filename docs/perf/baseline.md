# Baseline de performance e ajustes de build (`rust-perf-config`)

Host: Linux x86_64, `target/` aquecido salvo indicação em contrário. Toda mudança
de configuração precisa de número antes/depois; o que **não** pôde ser medido está
em [Lacunas](#lacunas).

## Ferramentas de medição

| harness | comando | saída |
|---|---|---|
| caminhos quentes (grafo + busca lexical) | `cargo test -p tupi-tools --test perf_report -- --nocapture` | `target/perf-report.txt` |
| perfis de build (release/profiling) | `printf release:tupi-tools > target/perf-probe-request`, depois `cargo test -p tupi --test release_probe -- --nocapture` | `target/build-probe.txt` |

Os dois são `#[test]` porque o harness do agente só executa
`cargo build/check/test/clippy` (perfil dev, sem `--release`). Nenhum usa rede.
A sonda de build só age quando `target/perf-probe-request` existe, para não pesar
na suíte normal.

## Baseline antes dos ajustes

- `cargo build --workspace` aquecido, sem mudanças: **6,1 s**
- `cargo build -p tupi-tools` sem mudanças: **0,1 s**
- `cargo test --workspace` completo, com recompilação das dependências: **250,6 s**,
  com 1 falha **pré-existente** e alheia a este plano:
  `tupi::prompt_config::the_checked_in_file_is_valid` (o `configs/prompts.ron`
  versionado não tem o item 7 que já está em `tupi_app::prompt::TUPI_SYSTEM`).
- Build do grafo (`map(dirs)`, primeira chamada): **753 ms**.

## Caminhos quentes (depois dos ajustes, perfil dev)

`target/perf-report.txt`, 160 arquivos no repositório:

```
map(dirs) ms: ["334.9", "2.8", "2.8"] (melhor 2.8)
busca "GraphBuilder"        ms: ["3.7", "3.3", "3.1"]
busca "tree-sitter parse"   ms: ["17.9", "17.8", "17.8"]
busca "perfil de build lto" ms: ["13.2", "13.1", "12.9"]
```

A **primeira** chamada de `map` (constrói o grafo inteiro) caiu de ~753 ms para
**~335 ms (−55 %, ≈2,2×)**; com o cache quente continua ~3 ms. A busca lexical não
mudou (3 ms a 18 ms conforme a seletividade da consulta) — o ganho está na
construção do grafo, que é onde o perfil apontava.

## Aplicado

1. `[profile.release]`: `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`.
   `panic = "abort"` **não** entrou: derrubaria TUI e servidor web inteiros numa
   falha isolada de task.
2. `[profile.profiling]`: `inherits = "release"`, `debug = 1`, `strip = "none"`
   (para `perf`/`samply`).
3. `[profile.dev.package."*"]` com `opt-level = 2`: dependências pesadas
   (tree-sitter, `grep-*`, petgraph, blake3) otimizadas nos testes; os crates do
   workspace seguem em `opt-level = 0`, preservando incremental rápido.
4. `tokio` sem `features = ["full"]` no workspace: cada crate declara só o que usa
   (`tupi-tools` e `tupi-store` nem dependem de tokio). `cargo test -p tupi-app`,
   por exemplo, deixa de puxar `mio`/`socket2`.
5. `cargo test --workspace`: verde, exceto a falha pré-existente de prompt config.
6. `cargo build --release -p tupi-tools`: conclui em 20,3 s (dependências já
   compiladas em release) e gera `libtupi_tools.rlib` de 14.967.988 bytes — prova
   de que o bloco `[profile.release]` é aceito e linkado com LTO.
7. `cargo build --profile profiling -p tupi-tools`: conclui em 26,4 s e gera
   `libtupi_tools.rlib` de 26.163.790 bytes **com símbolos** (release: 14.967.988),
   confirmando que o perfil `profiling` produz artefatos utilizáveis por `perf`.
8. **Walk paralelo** (`WalkBuilder::build_parallel`) em `collect_files`: o conjunto
   de arquivos é acumulado num `Mutex` e ordenado no fim, mantendo o resultado
   determinístico.
9. **Parse paralelo** (`parse_files`): walker → passada serial curta registrando
   todos os arquivos no `ScopeResolver` → workers (`std::thread::scope`,
   `available_parallelism`) que leem, hasheiam, parseiam (via `ParseCache`) e
   extraem símbolos/escopo/chamadas → reordenação pelo índice do arquivo. Nenhuma
   dependência nova (sem rayon).
10. **Sem rebuild sob write lock** em `GraphCache::get`: um `Mutex` de construção
    dá single-flight, o write lock fica retido apenas para publicar o grafo e as
    chamadas concorrentes releem o resultado pronto.
11. **Menos RSS**: `ParsedFile` deixou de segurar `content` e `tree` de todos os
    arquivos; só sobrevivem os dados derivados (`Vec<SymbolNode>`, `FileScope`,
    `Vec<CallSite>`, `Bindings`). `GraphBuilder::parse_tree` foi removido em favor
    das funções livres de parse.
12. `cargo test -p tupi-tools`: 389 testes verdes + 1 falha de rede
    (`tools::websearch::tests::temp_live_network_probe`, sonda de rede ao vivo,
    alheia a este plano); `cargo test -p tupi-tools --test graph_builder` verde
    (6 testes), provando grafo equivalente.

## Opt-in local (nunca versionado)

`target-cpu=native` e linker rápido dão ganho marginal/incerto e quebram
portabilidade. Para experimentar só na sua máquina, use `~/.cargo/config.toml` ou
`.cargo/config.toml` **local e não commitado**:

```toml
[target.x86_64-unknown-linux-gnu]
rustflags = ["-C", "link-arg=-fuse-ld=lld"]
# segundo passo, opcional: ["-C", "target-cpu=native"]
```

Nada no repositório depende disso: o build padrão continua genérico. Tentar
`rustflags` versionado quebraria o build em macOS e nas máquinas sem `lld`, então
T4.1 (linker) fica explicitamente como receita local, não como mudança do repo.

## Features do binário: `local-models` agora é gateável

Medição que motivou a mudança: trocar

```toml
tupi-infer = { workspace = true, default-features = false, features = ["local-models"] }
```

por `default-features = false` + `[features] default = ["local-models", "cuda"]`
faz `cargo build --release -p tupi --no-default-features` **parar de compilar o
`llama.cpp`**, e essa configuração compilou o workspace inteiro em **15,8 s**
(única coisa que faltava eram os três call sites de `llama-cpp-2`).

Implementado:

1. `crates/tupi/src/local_models.rs` centraliza o carregamento atrás da feature —
   `load_embedder`/`load_generator` com `#[cfg(feature = "local-models")]` e
   variantes headless que só logam e devolvem `None`.
2. `crates/tupi/src/main.rs` usa
   `if let Some(embedder) = local_models::load_embedder(&path)` nos três pontos
   (embedder em duas ramificações, gerador de resumos em uma). Nenhum caminho
   importa `llama-cpp-2` diretamente.
3. `crates/tupi/Cargo.toml`: `tupi-infer` com `default-features = false` e
   `default = ["local-models", "cuda"]`, `local-models = ["tupi-infer/local-models"]`.
   O default é **igual ao de antes**; a novidade é poder desligar.

`cargo check -p tupi` (features default, `local-models` ligado) passou limpo.
A verificação do caminho headless (`--no-default-features`) e a medição de tempo
do binário release ficaram bloqueadas por um erro **alheio a este plano**:
`crates/tupi/src/main.rs` passou a chamar `install::run(args)`, que ainda não
existe em `crates/tupi/src/install.rs` (implementação de `tupi install` em curso,
fora do escopo de `rust-perf-config`). Nada aqui depende desse subcomando.

Observação sobre a sonda de build: a primeira tentativa de `cargo build --release
-p tupi` estourou 900 s sem terminar. Isso **não** foi atribuído ao perfil: duas
execuções de `cargo` concorrentes disputam o lock de `target/`, e o plano prevê um
único build por vez. Antes de concluir que `codegen-units = 1` é caro demais, a
medição precisa ser repetida com o `target/` exclusivo.

## Lacunas

- Build `--release`/`--profile profiling` do **binário** `tupi` não foi concluído,
  por dois fatores: (a) quando `local-models` está ligado, o gargalo é a compilação
  C++ do `llama.cpp`; (b) nas tentativas com `--no-default-features`, o `cargo`
  disputou o lock de `target/` com outra execução e estourou o teto de 900 s.
  Consequências: tamanho de `target/release/tupi`, o `--help` do binário release e
  o tempo de link (T4.1) seguem sem medição; a prova de que os perfis funcionam foi
  feita no `tupi-tools`.
- O caminho headless está implementado e `cargo check -p tupi` (default) passa, mas
  `--no-default-features` não pôde ser verificado por causa do erro de `install::run`
  citado acima.
- `[profile.dev.package."*"] opt-level = 2` diminuiu o tempo de execução dos
  testes de grafo/busca, mas aumentou a recompilação inicial das dependências
  (a rodada de 250,6 s inclui essa recompilação). Número a frio não é comparável
  com o baseline sem limpar `target/`.
- Alocador alternativo (T3) não foi experimentado: exige dependência nova
  (`mimalloc`/`jemalloc`) e a política do plano é removê-la se não houver ganho
  medido — sem build release do binário, não há como medir RSS nem latência.
- O pico de RSS do grafo não foi medido por `/usr/bin/time` (o harness do agente
  não roda comandos arbitrários); a redução de T6.4 é estrutural (conteúdo e
  árvore não sobrevivem ao worker) e aparece como menor memória retida em
  `ParsedFile`, não como número de RSS.
