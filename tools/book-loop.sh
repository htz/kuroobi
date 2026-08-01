#!/bin/bash
# book の評価付けを、止めるまで頻出順に広げ続ける。
#
# 1 バッチごとに bookgen を呼び、そのつど book を保存する。途中で止めても
# 保存済みの評価は残り、次回は未評価の局面から再開される (bookgen は
# 「記録済みの深さが --depth 未満のエントリ」だけを対象にするため)。
#
# 停止: touch <repo>/tools/book-loop.stop  または kill
#
# 使い方: tools/book-loop.sh [バッチあたりの局面数] [深さ] [読切] [選択読み]

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
echo "=== book 評価付けループ開始 $(date '+%Y-%m-%d %H:%M:%S') ===" >> "$LOG"
echo "バッチ $BATCH 局面 / 深さ $DEPTH / 読切 $SOLVE / 選択読み $BAND" >> "$LOG"

round=0
while [ ! -f "$STOP" ]; do
  round=$((round + 1))
  # 未評価の残り数 (深さ 0 のエントリ) を数える
  remain=$(awk 'NR>1 && $3=="0"' "$BOOK" 2>/dev/null | wc -l | tr -d ' ')
  done_n=$(awk 'NR>1 && $3!="0"' "$BOOK" 2>/dev/null | wc -l | tr -d ' ')
  total=$((remain + done_n))
  if [ "$remain" -eq 0 ]; then
    echo "$(date '+%H:%M:%S') 全 $total 局面の評価が完了しました" >> "$LOG"
    break
  fi
  pct=$(awk -v d="$done_n" -v t="$total" 'BEGIN{printf "%.1f", t?100*d/t:0}')
  echo "$(date '+%H:%M:%S') ラウンド $round: 評価済み $done_n / $total ($pct%) — 次の $BATCH 件" >> "$LOG"

  ./target/release/bookgen --deepen --out "$BOOK" \
      --depth "$DEPTH" --solve "$SOLVE" --band "$BAND" \
      --threads 1 --hash-bits 22 --max-cands 4 --limit "$BATCH" >> "$LOG" 2>&1

  # bookgen が異常終了したら少し待って再試行 (電源断・一時的な失敗対策)
  [ $? -ne 0 ] && sleep 30
done

echo "=== 停止 $(date '+%Y-%m-%d %H:%M:%S') ===" >> "$LOG"
