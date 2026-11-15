#!/usr/bin/env bash
# Baseline de build do workspace tupi (subtarefa T1.1).
#
# Mede tempo de build a frio e incremental (debug e release) e o tamanho do
# binário `tupi`, sem destruir o `target/` compartilhado: os builds a frio usam
# diretórios de target ISOLADOS em target/perf-baseline/ (nunca `cargo clean`),
# então quem está trabalhando no repo não perde cache nem é pego de surpresa.
#
# Uso:
#   scripts/perf/measure-build-baseline.sh
#   TUPI_PERF_SKIP_COLD=1 scripts/perf/measure-build-baseline.sh   # só incremental
#   TUPI_PERF_BASE=/tmp/tupi-perf scripts/perf/measure-build-baseline.sh
#
# Saída: bloco markdown pronto para colar em docs/perf/build-baseline.md e um
# aviso no stderr sobre as condições da medição.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BASE=${TUPI_PERF_BASE:-"$ROOT/target/perf-baseline"}
DEV_DIR="$BASE/dev"
REL_DIR="$BASE/release"
SKIP_COLD=${TUPI_PERF_SKIP_COLD:-0}

mkdir -p "$BASE"

# Precisão de milissegundos onde `date +%s%N` existe (GNU); cai para segundos
# em BSD/macOS, onde o sufixo %N não é expandido.
have_ns=0
if [[ $(date +%s%N 2>/dev/null || true) =~ ^[0-9]{19}$ ]]; then
    have_ns=1
fi

now_ms() {
    if (( have_ns )); then
        echo $(( $(date +%s%N) / 1000000 ))
    else
        echo $(( $(date +%s) * 1000 ))
    fi
}

secs() { # ms -> "12.3"
    awk -v ms="$1" 'BEGIN { printf "%.1f", ms / 1000 }'
}

mib() { # bytes -> "33.2"
    awk -v b="$1" 'BEGIN { printf "%.1f", b / 1048576 }'
}

size_bytes() { # caminho -> bytes (0 se ausente)
    if [[ -f $1 ]]; then wc -c < "$1" | tr -d ' '; else echo 0; fi
}

build() { # label, target-dir, [flags...]
    local label=$1 dir=$2
    shift 2
    local start end
    start=$(now_ms)
    cargo build -p tupi --target-dir "$dir" "$@" >/dev/null
    end=$(now_ms)
    printf '| %s | %s s |\n' "$label" "$(secs $(( end - start )))"
}

touch_bin_source() {
    # Só mexe no mtime: o conteúdo de crates/tupi/src/main.rs não muda.
    touch "$ROOT/crates/tupi/src/main.rs"
}

host_line=$(rustc -vV 2>/dev/null | awk -F': ' '/^host:/ { print $2 }')
cpus=$(getconf _NPROCESSORS_ONLN 2>/dev/null || nproc 2>/dev/null || echo '?')

cat <<EOF
## Medição bruta

- data: $(date -u '+%Y-%m-%d %H:%M:%SZ')
- máquina: ${cpus} cpus · ${host_line:-?}
- cargo: $(cargo --version)
- rustc: $(rustc --version)
EOF

if [[ $SKIP_COLD != 1 ]]; then
    rm -rf "$DEV_DIR" "$REL_DIR"

    echo
    echo '| medição (debug) | tempo |'
    echo '| --- | --- |'
    build 'build a frio (target virgem)' "$DEV_DIR"
    build 'sem mudanças (no-op)' "$DEV_DIR"
    touch_bin_source
    build 'incremental (1 arquivo do binário)' "$DEV_DIR"
    dev_bin=$(size_bytes "$DEV_DIR/debug/tupi")
    dev_total=$(du -sm "$DEV_DIR" | cut -f1)

    rm -rf "$DEV_DIR"

    echo
    echo '| medição (release) | tempo |'
    echo '| --- | --- |'
    build 'build a frio (target virgem)' "$REL_DIR" --release
    build 'sem mudanças (no-op)' "$REL_DIR" --release
    touch_bin_source
    build 'incremental (1 arquivo do binário)' "$REL_DIR" --release
    rel_bin=$(size_bytes "$REL_DIR/release/tupi")
    rel_total=$(du -sm "$REL_DIR" | cut -f1)

    rm -rf "$DEV_DIR" "$REL_DIR"

    echo
    echo '| tamanho | valor |'
    echo '| --- | --- |'
    echo "| binário debug | $(mib "$dev_bin") MiB |"
    echo "| binário release | $(mib "$rel_bin") MiB ($rel_bin bytes) |"
    echo "| target de debug (build a frio) | ${dev_total} MiB |"
    echo "| target de release (build a frio) | ${rel_total} MiB |"
else
    echo
    touch_bin_source
    echo '| medição (target compartilhado) | tempo |'
    echo '| --- | --- |'
    start=$(now_ms)
    cargo build -p tupi >/dev/null
    end=$(now_ms)
    printf '| incremental debug | %s s |\n' "$(secs $(( end - start )))"
    start=$(now_ms)
    cargo build -p tupi --release >/dev/null
    end=$(now_ms)
    printf '| incremental release | %s s |\n' "$(secs $(( end - start )))"
fi

echo
echo '## Artefato canônico'

start=$(now_ms)
cargo build -p tupi --release >/dev/null
end=$(now_ms)
canon=$(size_bytes "$ROOT/target/release/tupi")
echo
echo "- \`cargo build --release -p tupi\` no target compartilhado: $(secs $(( end - start ))) s"
echo "- \`target/release/tupi\`: $canon bytes ($(mib "$canon") MiB)"

cat >&2 <<'EOF'

Atenção: as medições não isolam CPU nem I/O. Rode com a máquina ociosa (sem
outros cargo/rustc em paralelo) e compare sempre com o mesmo número de cpus,
rustc e flags de perfil. Para avaliar um `[profile.*]` novo, rode este script
antes e depois da mudança e compare as colunas de tempo a frio.
EOF
