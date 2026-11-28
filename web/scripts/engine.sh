#!/bin/sh
# Builds kuroobi_ai.wasm, single-threaded and threaded (wasm32-wasip1-threads, for a cross-origin isolated page),
# into public/engine/ under their hashes (the names the site asks for) and, unless --wasm,
# stages web/model/nnue.bin there as the deployed model is laid out, for `npm run dev` and `npm run preview`.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/public/engine
hash12() { if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -c1-12; }

cargo build --manifest-path "$here/../Cargo.toml" -p web --release --target wasm32-unknown-unknown
cargo build --manifest-path "$here/../Cargo.toml" -p web --release --target wasm32-wasip1-threads
rm -rf "$out"
mkdir -p "$out"
# astro.config.mjs hashes these copies; the site asks for the hashed names.
cp "$here/../target/wasm32-unknown-unknown/release/kuroobi_ai.wasm" "$out/kuroobi_ai.wasm"
cp "$out/kuroobi_ai.wasm" "$out/kuroobi_ai-$(hash12 "$out/kuroobi_ai.wasm").wasm"
cp "$here/../target/wasm32-wasip1-threads/release/kuroobi_ai.wasm" "$out/kuroobi_ai_mt.wasm"
cp "$out/kuroobi_ai_mt.wasm" "$out/kuroobi_ai-mt-$(hash12 "$out/kuroobi_ai_mt.wasm").wasm"

[ "${1:-}" = --wasm ] && exit 0
model=$here/model/nnue.bin
[ -s "$model" ] || { echo "no $model" >&2; exit 1; }
name=nnue-$(hash12 "$model").bin
cp "$model" "$out/$name"
printf '{"url":"/engine/%s","bytes":%s}\n' "$name" "$(wc -c < "$model" | tr -d ' ')" > "$out/model.json"
