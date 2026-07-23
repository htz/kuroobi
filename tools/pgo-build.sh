#!/bin/bash
# Profile-guided build of the endgame solver.
#
# The search is one deeply nested call graph whose branches are heavily
# skewed — a cutoff is far more likely than a full move loop, and the
# leaf routines dominate the instruction stream. Letting the optimizer see
# those frequencies is worth several percent of wall-clock, and it changes
# no search behaviour at all: the tree, the node counts, and the answers
# are bit-identical to a normal release build.
#
# Training positions matter. Profiling only shallow problems overfits to
# them: a run trained on FFO40-44 plus FFO56 was 3-6% faster on exactly
# those and 0.1% faster on the full FFO40-59 set. Include at least one
# deep problem (30 empties) so the deep-search paths are represented.
#
#   tools/pgo-build.sh [<obf files to train on>...]
#
# Leaves the optimized binary at target/<host>/release/solve_obf.
set -euo pipefail

cd "$(dirname "$0")/.."
PROFDATA_DIR="${PGO_DIR:-target/pgo}"
TARGET="${PGO_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
PROFDATA_TOOL="${LLVM_PROFDATA:-$(xcrun -f llvm-profdata 2>/dev/null || command -v llvm-profdata)}"

if [ -z "$PROFDATA_TOOL" ]; then
    echo "llvm-profdata not found; install the Xcode tools or set LLVM_PROFDATA" >&2
    exit 1
fi

TRAIN=("$@")
if [ ${#TRAIN[@]} -eq 0 ]; then
    echo "usage: $0 <deep.obf> [more.obf...]" >&2
    echo "  train on a spread of depths; a 30-empty position is essential" >&2
    exit 1
fi

rm -rf "$PROFDATA_DIR"
mkdir -p "$PROFDATA_DIR"

echo "==> instrumented build"
RUSTFLAGS="-Cprofile-generate=$PWD/$PROFDATA_DIR" \
    cargo build --release --bin solve_obf --target "$TARGET"

for f in "${TRAIN[@]}"; do
    echo "==> training on $f"
    "./target/$TARGET/release/solve_obf" --hash-bits 25 "$f" > /dev/null
done

echo "==> merging profile"
"$PROFDATA_TOOL" merge -o "$PROFDATA_DIR/merged.profdata" "$PROFDATA_DIR"/*.profraw

echo "==> optimized build"
RUSTFLAGS="-Cprofile-use=$PWD/$PROFDATA_DIR/merged.profdata" \
    cargo build --release --bin solve_obf --target "$TARGET"

echo "==> target/$TARGET/release/solve_obf"
