# Baseline de build — T1.1

Medição de referência do build do workspace **antes** de qualquer mudança de
perfil (`[profile.*]`) ou de `.cargo/config.toml`. Toda alteração de configuração
de build só conta como ganho se for comparada com estes números; se não houver
ganho medido, a mudança é revertida em um único commit.

## Condições da medição

| item | valor |
| --- | --- |
| data | 2026-11-14 (UTC-3) |
| máquina | 28 cpus · `x86_64-unknown-linux-gnu` · NVMe |
| toolchain | `cargo 1.99.1` / `rustc 1.99.1` |
| alvo medido | `cargo build -p tupi` (único binário do workspace) |
| features | padrão do binário: `local-models` + TUI + web |
| perfil release | padrão do Cargo: `opt-level = 3`, `codegen-units = 16`, `lto = off`, `panic = unwind`, `strip = none` (não há `[profile.*]` no `Cargo.toml` raiz) |
| disco livre na hora | ~5.4 GiB (99% usado) |

Método: builds a frio em diretórios de target **isolados**
(`target/perf-baseline/{dev,release}`), porque `target/` neste ambiente é
compartilhado e um `cargo clean` destruiria o cache de todo mundo. Incremental =
apenas o mtime de `crates/tupi/src/main.rs` adiantado (conteúdo **inalterado**),
que força recompilar só o crate do binário e religar; no-op = build imediatamente
após o anterior, sem tocar em nada.

Reprodução: `bash scripts/perf/measure-build-baseline.sh` (as tabelas abaixo saem
no mesmo formato).

## Tempos

Build a frio (target virgem, todas as dependências incluindo o build nativo de
`llama-cpp-2`):

| perfil | a frio | sem mudanças (no-op) | incremental (1 arquivo do binário) |
| --- | --- | --- | --- |
| debug | **74.8 s** | 0.7 s | **0.7 s** |
| release | **82.3 s** | 0.4 s | **3.2 s** |

Referência complementar: com as dependências já quentes no `target/`
compartilhado, um rebuild de release do crate do binário + link ficou em 14.2 s —
número inflado por edições de outros arquivos no momento da medição, registrado
só para ordem de grandeza.

## Tamanhos

| item | valor |
| --- | --- |
| `target/release/tupi` | **34 845 560 bytes (33.2 MiB)**, sem strip |
| `target/debug/tupi` | 343 800 128 bytes (327.9 MiB) |
| `target/` de um build debug a frio | 4496 MiB |
| `target/` de um build release a frio | 1020 MiB |

O build a frio em target isolado produziu um binário release com **exatamente** o
mesmo tamanho do artefato canônico em `target/release/tupi` (34 845 560 bytes),
ou seja: o tamanho é reprodutível e serve como número de comparação.

## Como usar este baseline

- **T1.2 (perfis)**: `lto = "fat"` + `codegen-units = 1` deve *aumentar* o tempo a
  frio de release e *reduzir* o tamanho/consumo; registrar os dois lados. Comparar
  sempre com 82.3 s e 33.2 MiB.
- **`strip`**: o release atual não é stripado (33.2 MiB); qualquer ganho de
  tamanho via `strip = "symbols"`/`"debuginfo"` deve ser medido contra esse valor.
- **`opt-level` de deps em dev** (`[profile.dev.package."*"]`): afeta o tempo a
  frio de debug (74.8 s) e a latência das tools em debug; medir os dois.
- **`panic = "abort"`**: não muda nenhum desses números de forma relevante e
  permanece fora do default por derrubar TUI/servidor a cada panic.

## Limitações

- Máquina compartilhada, sem isolamento de CPU/I/O: ruído de ±10% é esperado;
  compare execuções na mesma toolchain e com número de cpus igual.
- O incremental mede apenas a recompilação do crate do binário; mudanças em
  `tupi-tools` (onde vive o grafo/`search`/`map`) custam mais que 0.7 s em debug.
- O ambiente está com o disco praticamente cheio; os 4496 MiB de um build debug a
  frio são parte do custo real de rebuildar do zero aqui.
