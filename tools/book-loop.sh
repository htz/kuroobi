#!/bin/bash
# Keep evaluating book entries, most frequent first, until stopped.
#
# One bookgen call per batch, saving the book each time. Stopping midway keeps
# the evaluations already saved, and the next run resumes from the unevaluated
# positions (bookgen only targets entries whose recorded depth is below
# --depth).
#
# Stop: touch <repo>/tools/book-loop.stop  or kill
#
# Usage: tools/book-loop.sh [positions per batch] [depth] [solve] [band]

set -u
cd "$(dirname "$0")/.." || exit 1

BATCH=${1:-1000}
DEPTH=${2:-26}
SOLVE=${3:-30}
BAND=${4:-8}
BOOK=weights/book.txt
LOG=/tmp/book-loop.log
STOP=tools/book-loop.stop

rm -f "$STOP"
echo "=== book evaluation loop started $(date '+%Y-%m-%d %H:%M:%S') ===" >> "$LOG"
echo "batch $BATCH positions / depth $DEPTH / solve $SOLVE / band $BAND" >> "$LOG"

round=0
while [ ! -f "$STOP" ]; do
  round=$((round + 1))
  # count the entries still unevaluated (depth 0)
  remain=$(awk 'NR>1 && $3=="0"' "$BOOK" 2>/dev/null | wc -l | tr -d ' ')
  done_n=$(awk 'NR>1 && $3!="0"' "$BOOK" 2>/dev/null | wc -l | tr -d ' ')
  total=$((remain + done_n))
  if [ "$remain" -eq 0 ]; then
    echo "$(date '+%H:%M:%S') all $total positions evaluated" >> "$LOG"
    break
  fi
  pct=$(awk -v d="$done_n" -v t="$total" 'BEGIN{printf "%.1f", t?100*d/t:0}')
  echo "$(date '+%H:%M:%S') round $round: evaluated $done_n / $total ($pct%) - next $BATCH" >> "$LOG"

  ./target/release/bookgen --deepen --out "$BOOK" \
      --depth "$DEPTH" --solve "$SOLVE" --band "$BAND" \
      --threads 1 --hash-bits 22 --max-cands 4 --limit "$BATCH" >> "$LOG" 2>&1

  # if bookgen exits non-zero, wait a bit and retry (power loss, transient failures)
  [ $? -ne 0 ] && sleep 30
done

echo "=== stopped $(date '+%Y-%m-%d %H:%M:%S') ===" >> "$LOG"
