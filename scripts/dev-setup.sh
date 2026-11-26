#!/usr/bin/env bash
# Prepare this checkout for fast builds, in one step. Safe to run again.
#
#   scripts/dev-setup.sh              # cargo config + warm target dir from the seed
#   scripts/dev-setup.sh --build      # ... then build the game and every test (and save
#                                     #     a seed if this machine has none for these Cargo files)
#   scripts/dev-setup.sh --build-release   # the same, plus the release game
#   scripts/dev-setup.sh --save-seed  # save this checkout's compiled dependencies as the seed
#   scripts/dev-setup.sh --check      # print the config it would write, change nothing
#
# 1. Linker: finds rustc flags that link on this machine (some toolchains ship
#    a broken `rust-lld` wrapper; then it disables lld with
#    `-C linker-features=-lld`), uses mold when it is installed (a dev build
#    links in ~1 s instead of ~20 s), and writes them, with `build.jobs`, into
#    `.cargo/config.toml` of this checkout. That file is git-ignored and never
#    committed: CI and other machines build exactly as before without it.
# 2. Seed: the compiled dependencies (Bevy and everything under it, ~400
#    crates, the 10-25 minute part of a cold build) are kept once per machine
#    in ~/.cache/citysim/seed/<key> and copied into a fresh `target/`. Cargo
#    checks every copied artifact by its fingerprint, so a stale or partial
#    seed only costs rebuilding what differs; it can never produce a wrong
#    build. Each checkout gets its own copy: no shared lock, no build writes
#    into another checkout's files.
#
# Environment: DEV_JOBS (default: half the cores) sets `build.jobs`;
# CITYSIM_SEED_DIR moves the seeds (default ~/.cache/citysim/seed).
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$root"
mode=${1:-setup}
seeds=${CITYSIM_SEED_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/citysim/seed}
say() { echo "dev-setup: $*"; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# --- 1. linker flags -------------------------------------------------------
echo 'fn main() { println!("ok"); }' > "$tmp/main.rs"
links() { # do binaries link (and run) with these rustc flags?
    # shellcheck disable=SC2086
    env -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS rustc --edition 2024 $1 "$tmp/main.rs" -o "$tmp/probe" \
        >/dev/null 2>&1 && [[ "$("$tmp/probe")" == ok ]]
}
if links ""; then
    flags=""
elif links "-C linker-features=-lld"; then
    flags="-C linker-features=-lld"
else
    say "no linker setup works; try 'rustc main.rs' by hand" >&2
    exit 1
fi
# mold links the 270 MB dev binary in about 1 s instead of 17-20 s (GNU ld).
if command -v mold >/dev/null && links "$flags -C link-arg=-fuse-ld=mold"; then
    flags="${flags:+$flags }-C link-arg=-fuse-ld=mold"
fi
cores=$(nproc 2>/dev/null || echo 4)
jobs=${DEV_JOBS:-$((cores / 2 > 2 ? cores / 2 : 2))}
toml_flags=""
for f in $flags; do toml_flags+="\"$f\", "; done
toml_flags=${toml_flags%, }
config="# Written by scripts/dev-setup.sh for this checkout only (git-ignored).
# Re-run the script to refresh it; delete it to build without it.
[build]
jobs = $jobs
rustflags = [$toml_flags]"
if [[ $mode == --check ]]; then
    echo "$config"
    exit 0
fi
mkdir -p .cargo
echo "$config" > .cargo/config.toml
say "wrote .cargo/config.toml (jobs $jobs, rustflags [$toml_flags])"
if [[ -n "${RUSTFLAGS:-}" && "$RUSTFLAGS" != "$flags" ]]; then
    say "note: RUSTFLAGS=\"$RUSTFLAGS\" is set and replaces the file's flags; unset it" >&2
fi

# --- 2. seed ---------------------------------------------------------------
# The key: what decides the compiled dependencies. Same key = same artifacts.
rustc_id=$(rustc -vV | sha256sum | cut -c1-8)
key="$rustc_id-$(cat Cargo.lock Cargo.toml crates/*/Cargo.toml <(echo "$flags") | sha256sum | cut -c1-12)"

# The newest complete seed of a profile: this exact key, else any for this
# compiler (cargo then rebuilds only what differs).
find_seed() {
    local p=$1
    if [[ -d "$seeds/$key/$p" ]]; then
        echo "$seeds/$key/$p"
        return
    fi
    { find "$seeds" -mindepth 2 -maxdepth 2 -type d -path "$seeds/$rustc_id-*/$p" -printf '%T@ %p\n' 2>/dev/null ||
        true; } | sort -rn | head -1 | cut -d' ' -f2-
}

# Save target/<profile> without this workspace's own crates as the seed.
save_profile() {
    local p=$1 dest="$seeds/$key/$1"
    [[ -d target/$p/deps ]] || return 0
    if [[ -d "$dest" ]]; then
        say "seed $key/$p exists already"
        return 0
    fi
    local free_gb
    free_gb=$(df -BG --output=avail "$seeds" | tail -1 | tr -dc 0-9)
    if ((free_gb < 20)); then
        say "only ${free_gb} GB free: not saving the $p seed" >&2
        return 0
    fi
    local part="$seeds/$key/.part-$p-$$"
    rm -rf "$part" && mkdir -p "$part"
    tar -C "target/$p" -cf - --exclude='./incremental' --exclude='./examples' --exclude='*citysim*' \
        --exclude='./.cargo-lock' --exclude='./tmp' . | tar -C "$part" -xf -
    # Atomic: other checkouts only ever see complete seeds.
    if mv -T "$part" "$dest" 2>/dev/null; then
        say "saved seed $key/$p ($(du -sh "$dest" | cut -f1))"
    else
        rm -rf "$part"
    fi
}

save_seed() {
    mkdir -p "$seeds/$key"
    save_profile debug
    save_profile release
    # Keep the seeds of the two newest keys; drop parts left by killed runs.
    find "$seeds" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' | sort -rn | tail -n +3 | cut -d' ' -f2- |
        while read -r old; do rm -rf "$old"; done
    find "$seeds" -mindepth 2 -maxdepth 2 -name '.part-*' -mmin +120 -exec rm -rf {} + 2>/dev/null || true
}

if [[ $mode == --save-seed ]]; then
    save_seed
    exit 0
fi

mkdir -p target
[[ -f target/CACHEDIR.TAG ]] || echo "Signature: 8a477f597d28d172789f06886806bc55" > target/CACHEDIR.TAG
for p in debug release; do
    seed=$(find_seed $p)
    if [[ -d target/$p/deps ]]; then
        say "target/$p is already built: seed not needed"
    elif [[ -n "$seed" ]]; then
        start=$(date +%s)
        if cp -a "$seed" "target/$p"; then
            say "copied the $p seed into target/$p ($(du -sh "target/$p" | cut -f1), $(($(date +%s) - start)) s)"
            [[ "$seed" == "$seeds/$key/$p" ]] || say "(a seed for other Cargo files: cargo rebuilds what changed)"
        else
            say "copying the $p seed failed; that profile builds cold" >&2
            rm -rf "target/$p"
        fi
    elif [[ $p == debug ]]; then
        say "no seed on this machine yet: the first build is cold (10-25 min)."
        say "use --build (or run --save-seed after building) so the next checkout starts warm."
    fi
done

case $mode in
--build | --build-release)
    say "building the game and all tests (dev profile)"
    cargo build -p citysim
    cargo test --workspace --no-run
    if [[ $mode == --build-release ]]; then
        say "building the release game"
        cargo build -p citysim --release
    fi
    save_seed
    ;;
esac
