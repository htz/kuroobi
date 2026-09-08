#!/bin/sh
# Train the stages that are furthest from Egaroucid, eight at a time, and
# fold every stage that improved back into one model.
#
# A stage owns its own weight tables, so eight of them can be optimised in
# eight processes at once with no coordination -- and must be, because a run
# aimed at one stage only ever wakes one worker: measured here, a single
# stage held the machine at 39% of one core out of ten. Each process reads
# only its own stage's examples (`train_data/by_stage/stage_NN.data`, split
# out beforehand), which fits in RAM and drops an epoch from 22 minutes to
# 80 seconds.
#
# Each process writes into its own directory because `--weights` is read,
# written and re-saved as `.stagebest` every epoch; sharing a path would
# corrupt it. Nothing is lost by that: `stage_merge` takes every stage from
# whichever input scores best on the held-out set, so pointing it at all
# eight outputs plus the start point keeps each stage's own improvement and
# leaves the untouched stages exactly as they were.
#
# Extra train flags come from TRAIN_OPTS, replacing the rate settings below;
# that is how a run switches to per-cell rates without editing this file:
#
#   TRAIN_OPTS='--cell-lr --lr 1 --min-appear 4' sh stage_sweep.sh ...
#
# Usage: sh stage_sweep.sh <start.bin> <out.bin> <stage>...
set -e
cd "$(dirname "$0")" || exit 1
START=$1; OUT=$2; shift 2
RATE_OPTS=${TRAIN_OPTS:---lr 0.0001 --plateau 2 --plateau-frac 0.2 --plateau-factor 0.5 --plateau-min 1e-9}
[ -n "$START" ] && [ -n "$OUT" ] && [ $# -gt 0 ] || {
  echo "usage: sh stage_sweep.sh <start.bin> <out.bin> <stage>..." >&2; exit 1; }
[ -f "$START" ] || { echo "no start model at $START" >&2; exit 1; }

RUN=weights/exp/sweep-$(date +%Y%m%d-%H%M%S)
mkdir -p "$RUN"
echo "sweep $RUN: stages $*"

for st in "$@"; do
  D=$RUN/s$st
  DATA=train_data/by_stage/stage_$(printf '%02d' "$st").data
  [ -f "$DATA" ] || { echo "no data for stage $st at $DATA" >&2; continue; }
  mkdir -p "$D"
  cp "$START" "$D/linear.bin"
  # One thread each: a single stage is one worker's worth of work, so eight
  # processes fill the eight performance cores and nothing contends.
  # shellcheck disable=SC2086  # RATE_OPTS is a flag list, split on purpose
  nohup ./target/release/train --epochs 200 --optimizer sgd \
    --stage "$st" --patience 6 \
    $RATE_OPTS \
    --weights "$D/linear.bin" --per-stage-best \
    --val bench/val61.data \
    --patterns egaroucid --max-examples 0 \
    "$DATA" > "$D/train.log" 2>&1 &
done

echo "waiting for $# runs"
wait

# Fold in. The start point is an input too, so a stage that never beat it
# keeps the weights it began with and the merge can only move forward.
INPUTS="$START"
for st in "$@"; do
  F=$RUN/s$st/linear.bin.stagebest
  [ -f "$F" ] && INPUTS="$INPUTS $F"
done
# shellcheck disable=SC2086
./target/release/stage_merge --val bench/val61.data --out "$OUT" $INPUTS
echo "merged into $OUT"
