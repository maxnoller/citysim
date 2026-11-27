#!/usr/bin/env bash
# Follow measurements: runs the windowless recorder (CITYSIM_RECORD) on the
# organic demo following a car (CITYSIM_FOLLOW) at 1x and 12x with frame times
# of 1/60 s, 1/144 s and alternating 8 / 25 ms, plus a pause, a walker off the
# map and a car that arrives, and prints the largest marker-to-body distance
# and frame-to-frame movement of the followed body on a 1920x1080 screen
# (scripts/follow_stats.py).
#
#   scripts/follow-check.sh [BINARY] [OUT_DIR] [RUN...]
#
# BINARY defaults to target/debug/citysim (build it first), OUT_DIR to
# /tmp/follow-check; RUN names pick runs (default: all). Frames are rendered
# at 320x180 on Low and not saved: only positions matter. Two runs at a time.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=${1:-target/debug/citysim}
OUT=${2:-/tmp/follow-check}
shift $(( $# > 2 ? 2 : $# ))
mkdir -p "$OUT"
# name speed fps dt-list pause kind
RUNS=(
  "s1_60 1 60 - - car"
  "s1_144 1 144 - - car"
  "s1_uneven 1 60 8,25 - car"
  "s12_60 12 60 - - car"
  "s12_144 12 144 - - car"
  "s12_uneven 12 60 8,25 - car"
  "s1_pause 1 60 - 4,7 car"
  "s1_walker 1 60 8,25 - walker"
  "s1_arrive 1 60 8,25 - arrive"
)
want=("$@")
run() {
  read -r name speed fps dt pause kind <<<"$1"
  env CITYSIM_DEMO=organic CITYSIM_GFX_QUALITY=low CITYSIM_RECORD="$OUT/frames_$name" \
    CITYSIM_RECORD_SIZE=320x180 CITYSIM_RECORD_FPS="$fps" CITYSIM_RECORD_SAVE=0 \
    CITYSIM_RECORD_PATH="0,548,956,0.9,0.66,90" CITYSIM_FOLLOW="$kind" CITYSIM_RECORD_SPEED="$speed" \
    CITYSIM_RECORD_SECONDS=15 CITYSIM_RECORD_LOG="$OUT/$name.csv" \
    $( [ "$dt" != - ] && echo CITYSIM_RECORD_DT="$dt" ) \
    $( [ "$pause" != - ] && echo CITYSIM_RECORD_PAUSE="$pause" ) \
    timeout 1800 "$BIN" >"$OUT/$name.log" 2>&1 || echo "$name failed (see $OUT/$name.log)"
  grep -h "follow demo" "$OUT/$name.log" | sed "s/^.*follow demo/$name: follow demo/" || true
}
jobs_running=0
for r in "${RUNS[@]}"; do
  name=${r%% *}
  if [ ${#want[@]} -gt 0 ] && [[ ! " ${want[*]} " == *" $name "* ]]; then continue; fi
  run "$r" &
  jobs_running=$((jobs_running + 1))
  if [ $jobs_running -ge 2 ]; then wait -n; jobs_running=$((jobs_running - 1)); fi
done
wait
uv run -q --with numpy python scripts/follow_stats.py "$OUT"/*.csv
