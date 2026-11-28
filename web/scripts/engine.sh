#!/bin/sh
# Builds kuroobi_ai.wasm into public/engine/ under its hash (the name the site asks for) and, unless --wasm,
# stages web/model/nnue.bin there as the deployed model is laid out, for `npm run dev` and `npm run preview`.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
out=$here/public/engine
hash12() { if command -v sha256sum > /dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -c1-12; }

cargo build --manifest-path "$here/../Cargo.toml" -p web --release --target wasm32-unknown-unknown
rm -rf "$out"
mkdir -p "$out"
# astro.config.mjs hashes this copy; the site asks for the hashed name.
cp "$here/../target/wasm32-unknown-unknown/release/kuroobi_ai.wasm" "$out/kuroobi_ai.wasm"
cp "$out/kuroobi_ai.wasm" "$out/kuroobi_ai-$(hash12 "$out/kuroobi_ai.wasm").wasm"

[ "${1:-}" = --wasm ] && exit 0
model=$here/model/nnue.bin
[ -s "$model" ] || { echo "no $model" >&2; exit 1; }
name=nnue-$(hash12 "$model").bin
cp "$model" "$out/$name"
printf '{"url":"/engine/%s","bytes":%s}\n' "$name" "$(wc -c < "$model" | tr -d ' ')" > "$out/model.json"
