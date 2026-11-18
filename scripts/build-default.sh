#!/usr/bin/env bash
# Build principal do tupi: release + GPU CUDA = "todo o poder da ferramenta".
#
#   scripts/build-default.sh                       # build completo
#   scripts/build-default.sh --locked              # argumentos extras vão para o cargo
#   TUPI_FEATURES=metal scripts/build-default.sh   # mesmo build, outro backend de GPU
#
# Por que `--features cuda` é o build cheio (`crates/tupi/Cargo.toml:57`):
#
#   cuda = ["local-models", "transcribe", "tupi-infer/cuda", "tupi-transcribe/cuda"]
#
# Isto é: embedder + gerador de resumos (llama.cpp), whisper/sherpa do
# `tupi transcribe` e o backend de GPU nos dois. O default (`local-models`) é o
# build leve de CPU — `cargo build` sozinho não tem GPU.
#
# O que deliberadamente NÃO entra neste comando:
#
# - `mimalloc`: opt-in só para o A/B de alocador (`docs/perf/allocator-and-linker.md`);
#   medido sem ganho e nunca no build padrão.
# - `--config scripts/infer/llama-cpp-fork.toml`: o patch para o fork com
#   llama.cpp v0.8.0 é receita, não configuração utilizável (o fork em
#   `vendor/llama-cpp-sys-2` é esqueleto e a tag do remoto não existe). Enquanto
#   isso não fechar, `llama-cpp-sys-2` vem do crates.io — é o que compila.
# - `target-cpu=native` e linker rápido: ficam em `~/.cargo/config.toml` local,
#   nunca no repositório (`docs/perf/baseline.md`, "Opt-in local").
#
# Requisitos: toolkit CUDA (`nvcc`; `.cargo/config.toml` já fixa `CUDACXX` com
# `force = false`) e paciência — `llama.cpp` em CUDA, somado a
# `codegen-units = 1` + ThinLTO do `[profile.release]`, faz deste o build mais
# caro do repositório. A seleção é `-p tupi` porque a raiz é workspace virtual
# (sem `[package]`): a feature é do pacote do binário, não do workspace.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

FEATURES=${TUPI_FEATURES:-cuda}
BIN="$ROOT/target/release/tupi"

echo "+ cargo build --release -p tupi --features $FEATURES $*"
cargo build --release -p tupi --features "$FEATURES" "$@"

if [[ ! -x $BIN ]]; then
    echo "erro: $BIN não foi gerado" >&2
    exit 1
fi

size=$(wc -c < "$BIN" | tr -d ' ')
echo "ok: $BIN ($size bytes)"

# Sanidade do backend. `strip = "symbols"` apaga os símbolos, então a checagem é
# por string e só avisa: binário sem CUDA continua válido (o `tupi transcribe`
# loga o fallback para CPU em `crates/tupi-transcribe/src/asr.rs:82`).
if [[ $FEATURES == *cuda* ]] && command -v strings >/dev/null 2>&1; then
    if ! strings "$BIN" | grep -qi 'ggml_backend_cuda\|cudart'; then
        echo "aviso: nenhuma string de CUDA no binário; confira se o toolkit entrou (CUDACXX)" >&2
    fi
fi
