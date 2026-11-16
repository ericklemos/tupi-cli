# Alocador global e linker rápido: medidos e descartados (T3, T4.1)

Complemento de [`baseline.md`](baseline.md). As duas subtasks restantes do plano
`rust-perf-config` foram **implementadas, medidas e descartadas**: nenhuma delas
mostrou ganho, e a regra do plano é não versionar mudança sem número.

## T3 — alocador `mimalloc` como feature opcional

Implementação testada: `mimalloc = { version = "0.1", optional = true }` em
`crates/tupi/Cargo.toml`, feature `mimalloc = ["dep:mimalloc"]` fora do `default`
e, em `crates/tupi/src/main.rs`:

```rust
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
```

Medição com `crates/tupi/tests/alloc_bench.rs` (mesmo workload usado em T2:
build do grafo, `map` quente e três buscas lexicais) em duas rodadas:

| métrica (perfil dev) | alocador do sistema | `mimalloc` |
|---|---|---|
| build do grafo | 433 / 437 ms | 614 / 530 ms |
| `map` com cache quente | 3,42 / 3,34 ms | 3,35 / 3,33 ms |
| busca `"GraphBuilder"` | 4,59 / 3,39 ms | 3,20 / 3,07 ms |
| busca `"tree-sitter parse"` | 22,1 / 20,8 ms | 20,5 / 20,1 ms |
| busca `"perfil de build lto"` | 18,7 / 17,1 ms | 16,8 / 16,7 ms |
| pico de RSS (`VmHWM`) | 124–126 MB | 136–249 MB |

O build do grafo — o custo que `map`, `search` e o grafo de código pagam na
primeira chamada — ficou **mais lento** (2 de 2 rodadas) e o pico de RSS subiu.
As buscas oscilaram dentro do ruído (ganho de 1–6 % alternando com empate).

Decisão (Q4 do plano: sem ganho medido, a dependência sai): dependência, feature e
`#[global_allocator]` **removidos**. `crates/tupi/tests/alloc_bench.rs` ficou como
harness permanente e grava `target/alloc-bench.txt`:

```text
cargo test -p tupi --test alloc_bench -- --nocapture
```

Ele também fecha a lacuna de T6.4: **pico de RSS de 124 MB** após construir o
grafo dos ~160 arquivos do repositório (126 MB ao final da rodada), com
`map` quente em 3,3 ms e buscas entre 3,3 ms e 20,3 ms.

## T4.1 — linker rápido (`mold`)

`mold` está instalado nesta máquina, mas nada foi versionado no
`.cargo/config.toml`: Linux sem `mold` e macOS quebrariam o build. A receita
opt-in ficou em `scripts/perf/mold.toml`:

```text
cargo --config scripts/perf/mold.toml build --release -p tupi
```

Medição com `crates/tupi/tests/link_probe.rs` (`cargo build --release -p tupi
--no-default-features`, perfil release com `lto = "thin"` e `codegen-units = 1`;
apaga `target/release/deps/tupi-*` para forçar o rebuild do binário — apagar só
`target/release/tupi` custa 0,12 s, porque é um hardlink):

| cenário | linker padrão | `mold` |
|---|---|---|
| rebuild do binário com deps em cache | 19,84 s | 19,89 s |
| build completo a frio | não medido (target/ aquecido) | 82,0 s |

Quem domina os ~20 s é ThinLTO + codegen do binário, não o link: trocar o linker
**não muda o tempo** (19,84 s → 19,89 s, dentro do ruído). macOS não foi testado
(sem host disponível) — mais um motivo para nada de linker entrar no build padrão.

Reprodução:

```text
printf plain > target/link-probe-request && cargo test -p tupi --test link_probe -- --nocapture
printf mold  > target/link-probe-request && cargo test -p tupi --test link_probe -- --nocapture
```

O resultado fica em `target/link-probe.txt`. A sonda só roda quando o arquivo
`target/link-probe-request` existe, para não pesar na suíte normal.
