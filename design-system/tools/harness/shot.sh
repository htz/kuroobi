#!/bin/bash
# shot.sh <Comp> [width] [theme]: render components/<Comp>/preview.html like the design system does, and screenshot it.
C=$1; W=${2:-760}; T=${3:-dark}
D=$(cd "$(dirname "$0")/../.." && pwd)
P=$D/project/components/$C/preview.html
H=$(head -1 $P | sed -n 's/.*height=\([0-9]*\).*/\1/p'); H=${H:-300}
O=$D/tools/harness/out; mkdir -p $O
{
  echo "<!doctype html><html data-theme=\"$T\"><head><meta charset=utf-8>"
  echo "<style>"; cat $D/../gui/src-ui/tokens.css; echo "body{margin:0;background:var(--bg);color:var(--text)}</style>"
  echo "<style>"; cat $D/project/components/bundle.css; echo "</style>"
  echo "<script src=\"../react.js\"></script><script src=\"../../project/components/bundle.js\"></script></head><body>"
  tail -n +2 $P
  echo "</body></html>"
} > $O/$C.html
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=1 \
  --window-size=$W,$H --screenshot=$O/$C-$T.png --virtual-time-budget=2000 file://$O/$C.html >/dev/null 2>&1
echo $O/$C-$T.png
