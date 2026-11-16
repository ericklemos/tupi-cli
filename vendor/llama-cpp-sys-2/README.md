# Fork próprio de `llama-cpp-sys-2`

Este diretório é o fork **nosso** de `llama-cpp-sys-2` (base:
`utilityai/llama-cpp-rs`, crate `llama-cpp-2` 0.1.161, que vendoriza o
llama.cpp **v0.6.0**). Ele existe para subir o llama.cpp vendorizado ao
**v0.8.0** sem depender do repositório de terceiros: o submódulo `llama.cpp`
fica sob nosso controle e a atualização vira commit nosso.

## Pin do llama.cpp

| Campo | Valor |
|---|---|
| Tag | **`v0.8.0`** (registrada em [`LLAMA_CPP_TAG`](LLAMA_CPP_TAG)) |
| Build | **`b11960`** |
| Submódulo | `llama.cpp` (path `llama.cpp`, ver [`.gitmodules`](.gitmodules)) |
| Contrato que exige o pin | `/v1/systemone` com `-b 8192 -ub 8192` (card do `LiquidAI/d1-2B-GGUF`) |

O v0.6.0 do crate 0.1.161 **não** tem o d1 nem o contrato system-one; o
`d1-2B` exige llama.cpp `b11960`/v0.8.0 ou mais novo.

## Versão do crate

Permanece **`0.1.161`**, igual ao upstream. É o que faz o `[patch.crates-io]`
casar com `llama-cpp-sys-2 = "^0.1.161"` (duas entradas: alvo
macOS-aarch64 com `metal` e a genérica) exigido por `llama-cpp-2` 0.1.161.
Subir a tag do llama.cpp **não** muda esse número.

## Build

- Default é **CPU puro** (`default = []`, `GGML_NATIVE=OFF`): nenhum backend
de GPU entra sem feature.
- `cuda`, `vulkan`, `metal` são features **opt-in**; o `tupi-infer` só as liga
  quando o binário pede (`cuda`/`metal`/`vulkan` em `crates/tupi/Cargo.toml`).
  O patch nunca força CUDA no build padrão.
- O script de build valida que o submódulo está presente e é o pin acima;
  sem ele, falha com a instrução de `git submodule update --init llama.cpp`.

## Soberania: como subir o llama.cpp

Tag imutável, nunca branch solta:

```bash
# no repositório do fork
git -C llama.cpp fetch --tags origin
git -C llama.cpp checkout v0.8.1        # tag, não branch
git add llama.cpp LLAMA_CPP_TAG Cargo.toml README.md
git commit -m 'llama.cpp: v0.8.1'
git tag -a tupi-0.1.161-llama-v0.8.1 -m 'llama.cpp v0.8.1'
```

`LLAMA_CPP_TAG`, o metadata `[package.metadata.tupi.fork]` do `Cargo.toml` e a
tabela acima mudam no **mesmo commit**. O `[patch.crates-io]` aponta para a tag
imutável (ou o `rev` do commit dela), nunca para `main`.

Se o llama.cpp novo quebrar `llama-embed`/`llama-generate`
(`crates/tupi-infer/src/llama.rs`, `llama_generate.rs`), o ajuste entra no mesmo
commit da subida.

## Materializar o fork (uma vez)

Este checkout é o esqueleto do fork; a árvore do upstream 0.1.161 entra por
cima para trazer `build.rs`/`wrapper.*` completos, e o submódulo é adicionado:

```bash
git clone --recurse-submodules https://github.com/tupi-ai/llama-cpp-rs
cd llama-cpp-rs && git checkout 0.1.161          # base
rsync -a --exclude .git --exclude llama.cpp upstream-0.1.161/ .
git submodule add https://github.com/tupi-ai/llama.cpp.git llama.cpp
git -C llama.cpp checkout v0.8.0                 # tag do pin
git add -A && git commit -m 'fork: llama.cpp v0.8.0 (b11960)'
```

## Consumo pelo workspace

O patch **não** vive no `Cargo.toml` do workspace, e sim em
[`scripts/infer/llama-cpp-fork.toml`](../../scripts/infer/llama-cpp-fork.toml),
consumido só pela borda e só quando alguém pede:

```bash
cargo --config scripts/infer/llama-cpp-fork.toml check -p tupi-infer --features local-models
```

Motivo medido: `[patch.crates-io]` no manifesto é resolvido pelo cargo **antes**
de olhar features — com o `git = ...` ali, `cargo check -p tupi-infer` sem
feature nenhuma já tentava clonar o fork (`failed to load source for dependency
llama-cpp-sys-2`). No `--config`, o build leve (CPU, sem `local-models`)
continua idêntico ao do crates.io e não depende de rede nem de carregar o
llama.cpp. É o mesmo padrão de override opt-in de
[`scripts/perf/mold.toml`](../../scripts/perf/mold.toml).

O pin é o `rev` do commit (ou a tag anotada `tupi-0.1.161-llama-v0.8.0`
enquanto o fork não estiver publicado), nunca branch. O `tupi-infer` continua
sendo o único crate que baixa, carrega e infere; o `d1` não é consumido nesta
entrega.

## Plano B

`llama-server`/vLLM como sidecar também servem o contrato system-one, mas
ficam descartados por padrão: só com aprovação explícita, porque furam
"sem processo externo".
