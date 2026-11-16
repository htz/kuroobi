#!/bin/bash
# Builds Kuroobi.app for one Mac architecture with the weights inside, and zips it.
#   usage: crates/gui/tools/bundle-app.sh <arm64|x86_64> [weights dir] [out dir]
set -euo pipefail
cd "$(dirname "$0")/../../.."

arch=${1:?usage: bundle-app.sh <arm64|x86_64> [weights dir] [out dir]}
weights=${2:-weights}
out=${3:-target/bundle/$arch}
case $arch in
  arm64) triple=aarch64-apple-darwin ;;
  # AVX2 and BMI2: every Intel Mac since 2013 has them, and the vector kernels need them.
  x86_64)
    triple=x86_64-apple-darwin
    export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C target-cpu=x86-64-v3"
    ;;
  *) echo "unknown architecture: $arch (arm64 or x86_64)" >&2; exit 1 ;;
esac
for f in nnue.bin linear.bin book.txt; do
  [[ -f $weights/$f ]] || { echo "$weights/$f is missing" >&2; exit 1; }
done

(cd crates/gui && npm run build)
cargo build --release -p gui --target "$triple"

conf=crates/gui/tauri.conf.json
field() { python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$conf" "$1"; }
name=$(field productName)
version=$(field version)
identifier=$(field identifier)

app=$out/$name.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/weights"
cp "target/$triple/release/kuroobi-gui" "$app/Contents/MacOS/kuroobi-gui"
cp crates/gui/icons/icon.icns "$app/Contents/Resources/icon.icns"
for f in nnue.bin linear.bin book.txt book_learn.txt; do
  [[ -f $weights/$f ]] || continue
  # A clone on APFS: the 1.2 GB of weights cost no space until either copy changes.
  cp -c "$weights/$f" "$app/Contents/Resources/weights/" 2>/dev/null ||
    cp "$weights/$f" "$app/Contents/Resources/weights/"
done
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleDisplayName</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>$identifier</string>
  <key>CFBundleExecutable</key><string>kuroobi-gui</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>10.13</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.board-games</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --deep --sign - "$app"

zip=$out/$name-$arch.zip
rm -f "$zip"
ditto -c -k --keepParent "$app" "$zip"
echo "$app"
echo "$zip"
