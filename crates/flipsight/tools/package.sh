#!/bin/bash
# Zips kuroobi-flipsight for one Mac architecture, without the weights (pass them with --weights).
#   usage: crates/flipsight/tools/package.sh <arm64|x86_64> [out dir]
set -euo pipefail
cd "$(dirname "$0")/../../.."

arch=${1:?usage: package.sh <arm64|x86_64> [out dir]}
out=${2:-target/bundle/$arch}
case $arch in
  arm64) triple=aarch64-apple-darwin ;;
  # AVX2 and BMI2: every Intel Mac since 2013 has them, and the vector kernels need them.
  x86_64)
    triple=x86_64-apple-darwin
    export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C target-cpu=x86-64-v3"
    ;;
  *) echo "unknown architecture: $arch (arm64 or x86_64)" >&2; exit 1 ;;
esac
cargo build --release -p flipsight --target "$triple"

dir=$out/kuroobi-flipsight-$arch
rm -rf "$dir"
mkdir -p "$dir"
cp "target/$triple/release/kuroobi-flipsight" "$dir/"
codesign --force --sign - "$dir/kuroobi-flipsight"

zip=$dir.zip
rm -f "$zip"
ditto -c -k --keepParent "$dir" "$zip"
echo "$zip"
