#!/usr/bin/env bash
# Hitch check of the release game, windowless (no display, no window opens):
# graphics quality switches, a scripted round of player actions in the
# 10,000- and the 20,000-resident city, the simulation at the top speed
# against 1x, and the average frame at 20,000 residents. Prints one table
# (figure, target, result, pass / fail) and writes every raw result (the
# benchmark's JSON reports, per-frame CSVs, logs, the table) to the folder.
#
#   scripts/perf-check.sh [out-dir]       # default: /tmp/citysim-perf-<time>
#   PERF_CHECK_ONLY=quality,actions10k scripts/perf-check.sh   # some runs only
#
# Runs: quality (CITYSIM_BENCH_SCRIPT=quality, ten switches), actions10k and
# actions20k (CITYSIM_BENCH_SCRIPT=actions, HUD drawn), speed1 / speed12 (the
# plain camera path at 1x and 12x), plain20k. All at 1920x1080 on High
# (PERF_CHECK_SIZE, PERF_CHECK_QUALITY override). The demo cities come from
# the shared demo cache (~/.cache/citysim/demos): the first run builds them
# (about a minute for 10,000 residents, a few for 20,000 on 12 cores).
# Settings and saves go to a temporary folder: the player's are never touched.
# Frame times only mean something on a quiet machine with a real GPU.
set -euo pipefail
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"

out=${1:-/tmp/citysim-perf-$(date +%Y%m%d-%H%M%S)}
mkdir -p "$out"
out=$(cd "$out" && pwd)
size=${PERF_CHECK_SIZE:-1920x1080}
quality=${PERF_CHECK_QUALITY:-high}
only=${PERF_CHECK_ONLY:-}
start=$(date +%s)

echo "+ cargo build --release -p citysim"
cargo build --release -p citysim
bin=target/release/citysim
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git rev-parse HEAD >"$out/commit.txt"
git status --short >>"$out/commit.txt"

run() {
    local name=$1
    shift
    if [[ -n $only && ",$only," != *",$name,"* ]]; then
        return
    fi
    local t0
    t0=$(date +%s)
    echo "+ $name: $*"
    mkdir -p "$tmp/save-$name" "$tmp/config-$name"
    if ! env CITYSIM_DEMO=hierarchy CITYSIM_SEED=3 CITYSIM_GFX_QUALITY="$quality" CITYSIM_BENCH_SIZE="$size" \
        CITYSIM_SAVE_DIR="$tmp/save-$name" CITYSIM_CONFIG_DIR="$tmp/config-$name" "$@" \
        CITYSIM_BENCH="$out/$name.json" "$bin" >"$out/$name.log" 2>&1; then
        echo "  $name FAILED (see $out/$name.log)"
    fi
    echo "  $name: $(($(date +%s) - t0)) s"
}

run quality CITYSIM_BENCH_SCRIPT=quality CITYSIM_BENCH_WARMUP=120
run actions10k CITYSIM_BENCH_SCRIPT=actions CITYSIM_BENCH_UI=1
run speed1 CITYSIM_BENCH_FRAMES=1200 CITYSIM_BENCH_SPEED=0
run speed12 CITYSIM_BENCH_FRAMES=1200 CITYSIM_BENCH_SPEED=3
run plain20k CITYSIM_DEMO_POP=20000 CITYSIM_BENCH_FRAMES=1200
run actions20k CITYSIM_DEMO_POP=20000 CITYSIM_BENCH_SCRIPT=actions CITYSIM_BENCH_UI=1

python3 scripts/perf-summary.py "$out" | tee "$out/table.txt"
echo "raw results: $out ($(($(date +%s) - start)) s)"
