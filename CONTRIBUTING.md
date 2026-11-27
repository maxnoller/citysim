# Contributing

Thanks for looking. CitySim is a hobby project; issues with a screenshot or a save file are the most useful kind of
report, and small pull requests are welcome.

## Building

Rust (stable) and the system packages Bevy needs on your platform (on Debian / Ubuntu: `libasound2-dev libudev-dev
libwayland-dev libxkbcommon-dev`). Then:

```
cargo run -p citysim --release      # play
cargo test -p citysim_sim           # the simulation's tests
scripts/ci-local.sh                 # everything CI runs
```

`scripts/dev-setup.sh --build` sets a checkout up for fast rebuilds; `CLAUDE.md` describes every development loop,
the windowless screenshot and benchmark modes, and the layout of the code.

## Ground rules

- `crates/sim` holds all state and rules: pure Rust without Bevy, deterministic, changed only through
  `World::apply(Command)` and `World::step()`. A rule change moves the pinned checksum in `tests/round2.rs`; update it
  on purpose and say so.
- In `crates/game`, `gfx/` draws and `ui/` owns the interface and input. They never import each other;
  `shared.rs` is the contract between them.
- `cargo fmt --all` (width 120) and clippy without warnings.
- Anything visible comes with a screenshot from the real game.

## Pull requests

`main` only changes through pull requests, merged by squashing: the pull request's title becomes the commit, so write
it as one plain sentence about what changes ("Zoning follows curved roads"), and put the reasoning and how it was
checked in the description. CI (formatting, clippy and tests for the simulation; checks and tests for the game) has to
pass.
