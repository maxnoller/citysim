#!/usr/bin/env bash
# Run on this machine what CI (.github/workflows/ci.yml) checks, in the same
# order, stopping at the first failure. Keep the two in step.
#
#   scripts/ci-local.sh          # both CI jobs: sim, then game
#   scripts/ci-local.sh sim      # only the sim job (format, clippy, sim tests)
#   scripts/ci-local.sh game     # only the game job (three feature sets, game tests)
#   scripts/ci-local.sh --full   # CI, then the local rules CI does not run:
#                                #   clippy on the game in all three feature sets
#
# Builds with this checkout's profile settings (CI turns debug info and
# incremental builds off to keep its cache small; the results are the same).
set -euo pipefail
cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"

jobs=${1:-all}
run() {
    echo "+ $*"
    "$@"
}
start=$(date +%s)
if [[ $jobs == all || $jobs == sim || $jobs == --full ]]; then
    run cargo fmt --all --check
    run cargo clippy -p citysim_sim --all-targets -- -D warnings
    run cargo nextest run -p citysim_sim
fi
if [[ $jobs == all || $jobs == game || $jobs == --full ]]; then
    run cargo check -p citysim --all-targets
    run cargo check -p citysim --no-default-features --features gfx
    run cargo check -p citysim --no-default-features --features ui
    run cargo test -p citysim
fi
if [[ $jobs == --full ]]; then
    run cargo clippy -p citysim --all-targets -- -D warnings
    run cargo clippy -p citysim --no-default-features --features gfx -- -D warnings
    run cargo clippy -p citysim --no-default-features --features ui -- -D warnings
fi
echo "ci-local: all passed in $(($(date +%s) - start)) s"
