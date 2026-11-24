# CitySim — notes for agents

Original 3D city-builder (roads, RCI zoning, agent traffic). Rust 2024 + Bevy 0.20.1.

## Build / test / run
- `cargo test --workspace` · `cargo clippy --workspace --all-targets` (must be warning-free) · `cargo fmt --all --check` (max_width 120)
- Run: `cargo run -p citysim`. Faster dev loop (≈3 s per edit instead of ≈13 s):
  `cargo run -p citysim --features bevy/dynamic_linking` (CLI flag only; never make it a default feature).
- First Bevy build ≈ 9–10 min on 4 vCPU. Never commit a `.cargo/config.toml` naming a linker; put mold/lld
  in your own `~/.cargo/config.toml`.
- Sim tests only: `cargo test -p citysim_sim` (fast, no Bevy).

## Headless visual checks (no human needed)
- `CITYSIM_AUTOSHOT=out.png cargo run -p citysim` (or `-- --autoshot out.png`): builds a sample city via
  `SimCommand`s, shows a road ghost, waits `CITYSIM_AUTOSHOT_DELAY` s (default 3), saves PNG, exits 0.
- `CITYSIM_DEMO=1 [CITYSIM_DEMO_DELAY=1.5] cargo run -p citysim`: ~15 s scripted demo driving the real tools
  through a scripted `Pointer`, moving the camera; exits when done. Good for screen recordings.
- Needs an X display: `DISPLAY=:0`. Software rendering (llvmpipe/lavapipe, Vulkan) works with defaults.
- `CITYSIM_SEED=n` sets the world seed. F5/F9 quick-save/load `citysim.citysave` in the cwd.

## The two-crate rule
- `crates/sim` (`citysim_sim`): ALL state and rules. No Bevy. Deterministic (only `world.rng`, no HashMap
  iteration, no wall clock), serde-serialisable. Public plain-data fields; mutate only via
  `World::apply(Command)` and `World::step()`. `preview(cmd)` is Ok exactly when `apply(cmd)` would succeed.
- `crates/game` (`citysim`): Bevy front-end, NO game rules. One plugin per concern in its own module
  (sim_driver, scene, camera, cursor, tools, road_render, zone_render, hud, script). Frame order:
  `GameSet::{Input, Tools, Sim, Render}`. Tools send `SimCommand` messages; `sim_driver` applies them.
- Renderers rebuild meshes only when `SimWorld::key(world.versions.X)` changes (covers wholesale reloads).
- Road pathfinding / vehicles / rendering use ONLY the road graph (`world.roads`), never the grid.
- Stateful collections use `citysim_sim::IdMap` (slotmap-like, keys from `slotmap::new_key_type!`), whose
  serialisation keeps the free list so a loaded save stays in lockstep with the original.
- Road edits may split/merge/remove segments (a split keeps the original id for the `a` half); anything
  holding a `SegmentId` must re-check `roads.segments.contains_key`.
- `// PHASE2:` marks the stubs still to fill (demand, growth, households, traffic, daily economy, stats).

## Coordinates
- `MAP_CELLS = 256`, `CELL_SIZE = 8.0` m. Cell `(x, y)` centre = `((x+0.5)*8, (y+0.5)*8)` in sim metres.
- Sim `(x, y)` → Bevy `(x, 0, y)` (`game/src/coords.rs`). Flat overlays use small y offsets (see coords).
- Highway stub: cells `(0,128)..=(11,128)` from the west edge; `roads.entry` is its map-edge node.
- All tunable numbers live in `crates/sim/src/balance.rs`.

## Bevy 0.20 API facts (differ from older versions — check `~/.cargo/registry/src/*/bevy*-0.20.1`)
- Events between systems are `Message`s: `#[derive(Message)]`, `app.add_message::<T>()`,
  `MessageWriter::write`, `MessageReader::read`. `AppExit` is written via `MessageWriter<AppExit>`.
- Observers take `On<E>`; screenshots: `commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p))`
  (`bevy::render::view::screenshot`), `ScreenshotCaptured` is an `EntityEvent`.
- Lights: `shadow_maps_enabled` (not `shadows_enabled`); ambient is the resource
  `bevy::light::GlobalAmbientLight` (`AmbientLight` is a per-camera component).
- Text: `TextFont { font_size: FontSize::Px(18.0), .. }`; UI lengths via `px(8)`; `Node` for layout.
- `PerspectiveProjection` has a new `near_clip_plane` field — build with `..default()`.
- `WindowResolution::new(u32, u32)`; mouse: `AccumulatedMouseMotion` / `AccumulatedMouseScroll` resources.
- Queries: `single()` returns `Result`; `Single<D, F>` system param exists. Mesh imports:
  `bevy::mesh::{Indices, PrimitiveTopology}`, `bevy::asset::RenderAssetUsages`.
- Bevy features used: `default-features = false, features = ["3d", "ui"]` (no audio/2d).
- Examples in the new `bsn_list!` scene macro style exist, but the classic `commands.spawn((..))` API works.
- The workspace drops `rust_2018_idioms`: it flags every Bevy system signature.
