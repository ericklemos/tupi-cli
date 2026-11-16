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

## T3 de novo: release + concorrência (atualiza a decisão acima)

A medição original de T3 tinha dois furos de método, e esta seção os fecha:

1. rodou no **perfil dev**, onde os crates do workspace ficam em `opt-level = 0`
   (`Cargo.toml:86-91`) — o cenário mais generoso para o alocador do sistema;
2. o harness era `crates/tupi/tests/alloc_bench.rs`, um teste de integração, e a
   `#[global_allocator]` vivia em `crates/tupi/src/main.rs`. Como `crates/tupi`
   só tem `[[bin]]` (`crates/tupi/Cargo.toml:6-8`), a `static` do binário nunca
   entra no binário de teste: a rodada "mimalloc" não trocava o alocador de quem
   media.

O A/B correto agora é `crates/tupi/tests/alloc_ab.rs`, que declara a
`#[global_allocator]` no próprio teste (sob `cfg(feature = "mimalloc")`) e mede
três regimes: grafo frio, caminho quente single-thread e 8 threads × 12 buscas na
mesma tool (o regime de TUI + servidor web), com `VmRSS` amostrado a cada rodada
de um laço longo — o proxy de fragmentação que uma rodada única não enxerga.

```text
printf ab > target/alloc-ab-request
cargo test -p tupi --release --no-default-features --test alloc_ab -- --nocapture
printf ab > target/alloc-ab-request
cargo test -p tupi --release --no-default-features --features mimalloc \
    --test alloc_ab -- --nocapture
```

Três rodadas de cada lado, perfil release, `--no-default-features` (sem
`local-models`); números de `target/alloc-ab-{sistema,mimalloc}.txt`:

| métrica | alocador do sistema | `mimalloc` |
|---|---|---|
| grafo frio | 191 / 198 / 256 ms | 169 / 199 / 208 ms |
| `map` quente (melhor de 5) | 0,49 / 0,51 / 0,52 ms | 0,47 / 0,48 / 0,49 ms |
| busca `"GraphBuilder"` | 3,31 / 3,33 / 3,68 ms | 3,16 / 3,54 / 5,28 ms |
| busca `"tree-sitter parse"` | 11,01 / 11,04 / 11,21 ms | 10,81 / 11,15 / 12,03 ms |
| busca `"perfil de build lto"` | 10,40 / 10,45 / 10,82 ms | 10,16 / 10,63 / 11,03 ms |
| 8 threads × 12 buscas (total) | 145 / 160 / 162 ms | 124 / 160 / 198 ms |
| p95 por chamada concorrente | 18,20 / 18,28 / 18,58 ms | 16,00 / 21,12 / 21,97 ms |
| `VmHWM` depois do grafo | 124 MB | 260–273 MB |
| `VmHWM` final | 137 MB | 318–343 MB |
| `VmRSS` da 1ª à 10ª rodada longa | 137,3 → 137,3 MB | 318 / 333 / 343 MB, plano |

Leitura:

- **Latência: empate.** `map` quente é idêntico (0,5 ms); as três buscas ficam
  dentro do ruído, alternando quem ganha. O grafo frio pareceu favorecer
  `mimalloc` (169 ms contra 191 ms na melhor rodada), mas a rodada 3 do sistema
  saiu em 256 ms e a do `mimalloc` em 208 ms: a dispersão é maior que o delta.
- **Concorrência: empate com dispersão alta.** Médias 155 ms (sistema) e 161 ms
  (`mimalloc`); mediana por chamada 10,7 ms contra 10,4–14,2 ms. Não há ganho
  reproduzível, ao contrário do que a hipótese de "alocador multithread resolve"
  previa.
- **Memória: perda consistente e grande.** Pico e RSS estacionário ficam em
  ~2,3–2,5× o do alocador do sistema (137 MB → 318–343 MB), o mesmo sinal do
  teste em dev (124–126 MB → 136–249 MB). Isso não é ruído: aparece em todas as
  rodadas, inclusive nas de melhor latência.

Decisão: `mimalloc` continua **fora do default**. A dependência e a feature
`mimalloc` (`crates/tupi/Cargo.toml:33-35`, `:60-62`) e a `#[global_allocator]`
gateada (`crates/tupi/src/main.rs:20-24`) ficam versionadas como caminho opt-in
de medição, e `crates/tupi/tests/alloc_ab.rs` fica como harness permanente — o
portão `target/alloc-ab-request` evita peso na suíte normal.

