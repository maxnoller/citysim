# CitySim — notes for agents

Original 3D city-builder (roads, RCI zoning, agent traffic). Rust 2024 + Bevy 0.20.1.

## Build / test / run
- `cargo test --workspace` · `cargo clippy --workspace --all-targets` (warning-free) · `cargo fmt --all --check` (width 120)
- Run: `cargo run -p citysim`. Fast loop (≈3 s/edit vs ≈13 s): `--features bevy/dynamic_linking` on the command
  line only, never as a default. First Bevy build ≈ 9–10 min. Never commit a `.cargo/config.toml` naming a linker.
- Ownership check (must stay warning-free): `cargo clippy -p citysim --no-default-features --features gfx`
  and `... --features ui`. Both must also run.

## Layout and ownership (three parallel agents: SIM, GFX, UI)
- `crates/sim` (`citysim_sim`) — SIM only. ALL state and rules. No Bevy, deterministic (only `world.rng`), serde.
  Mutate via `World::apply(Command)` / `World::step()`. `preview(cmd)` is Ok exactly when `apply(cmd)` would be.
- `crates/game/src/gfx/` — GFX only: scene/ground/lights, camera, roads, zones, buildings, vehicles, hovered-cell +
  tool-ghost drawing, overlays. One `GfxPlugin`. Assets in `crates/game/assets/gfx/`.
- `crates/game/src/ui/` — UI only: HUD, tool state machine, all input producing `Command`s, pause/speed/overlay/
  quality keys, save/load. One `UiPlugin`. Assets in `crates/game/assets/ui/`.
- `crates/game/src/shared.rs` — the ONLY contract between gfx and ui; change it only by agreement. gfx and ui
  never import each other. `main.rs` just builds the App (features `gfx`, `ui`, both default).
- `dev/` (script demo, autoshot) and `dev_demo.rs` (THROWAWAY fake town) — tooling, not owned by gfx/ui.
- Asset root resolves to `crates/game/assets` for `cargo run -p citysim` and for the binary run directly.

## shared.rs contract (summary)
- `SimWorld { world, generation }` (`key(version)`, `replace(world)`), `SimClock { paused, speed }`, `SPEEDS`,
  `SimCommand(Command)` message → applied in `GameSet::Sim`, result in `LastCommand`. `World::step` runs in
  `FixedUpdate` (`SimStep` set) at `TICKS_PER_SECOND`, `steps_per_tick()` times.
- Frame order: `GameSet::{Input, Hover, Tools, Sim, Render}`.
- `ActiveTool`, `ToolPreview` (ui writes with `set_if_neq`, gfx draws), `OverlayMode`, `GraphicsQuality`,
  `PointerOverUi` (ui writes), `HoveredCell` (shared computes from the `WorldCamera` ray).
- `WorldCamera` marker (gfx's 3D camera), `CameraPose`, `CameraOverride` (when Some, gfx must show that pose).
- Coordinates: `sim_to_world`, `world_to_sim`, `cell_center`, `world_to_cell`, `cell_corners`.
- Keys: gfx = WASD/arrows, Q/E, wheel, middle drag. ui = I R Z B Esc/right-click, Space 1 2 3, O, G, F5/F9.

## Sim facts for renderers
- `MAP_CELLS = 256`, `CELL_SIZE = 8` m. Cell `(x, y)` centre = `((x+.5)*8, (y+.5)*8)` m; sim `(x, y)` → Bevy
  `(x, 0, y)`. Highway stub cells `(0,128)..=(11,128)`; `roads.entry` is its edge node.
- Draw roads from `world.roads` (graph) only. Rebuild on `SimWorld::key(world.versions.{roads,grid,buildings})`.
- `Building { kind, footprint, level, capacity, occupied, facing: Facing, seed, created_tick }`.
- `Vehicle { pos (m, centre line), heading (rad, atan2 in sim xy), segment, forward (a→b), progress (m), seed }`.
- `RoadSegment { load: [f32; 2], capacity: [f32; 2] }` indexed `[a→b, b→a]`; `congestion_in(forward)`.
- Segment ids can vanish on road edits (split keeps the id for the `a` half): always re-check.
- Time: vehicles move in real time (1 tick = 1/20 s), the calendar is compressed: a day = 4 min at 1x (`balance`).
- Trip kind per car: `world.traffic.trips.get(vehicle_id).map(|t| t.purpose)` (`TripPurpose::Goods` = draw a truck).
- HUD data: `stats` (unemployment, avg_commute_s, avg_speed m/s, congestion_share, buildings_by_kind), `economy.last_day`
  (`Budget` by category), `economy.history` (one `DaySummary` per day), `demand` (-1..=1).
- Sim balance runs: `cargo run -p citysim_sim --release --example headless` (scripted cities in `citysim_sim::scenarios`).
- `IdMap` (slotmap-like, serialises its free list so loads stay in lockstep). `// PHASE2:` marks sim stubs.

## Dev / headless flags (need `DISPLAY=:0`; llvmpipe works)
- `CITYSIM_DEMO=town` (~350 buildings over 20 s, 300 cars, live loads/demand/money) · `stress` (6,000 / 3,000)
  · `script` (alias `1`; ~15 s tool demo through synthetic input; `CITYSIM_DEMO_DELAY=s`).
- `CITYSIM_AUTOSHOT=out.png` (or `--autoshot`): screenshot then exit 0. `CITYSIM_SHOT_VIEW=wide|street|close`,
  `CITYSIM_SHOT_DELAY=25` (s, default 3), `CITYSIM_SHOT_OVERLAY=none|zones|traffic`. Without a demo it builds
  a small sample city. Example: `CITYSIM_DEMO=town CITYSIM_AUTOSHOT=/tmp/t.png CITYSIM_SHOT_DELAY=25 cargo run -p citysim`.
- `CITYSIM_LOG_FPS=1` logs FPS (llvmpipe: town ≈ 12–24, stress ≈ 8). `CITYSIM_SEED=n` sets the world seed.

## Bevy 0.20 API facts (verify in `~/.cargo/registry/src/*/bevy*-0.20.1`)
- Events between systems are `Message`s (`#[derive(Message)]`, `add_message`, `MessageWriter::write`); `AppExit`
  too. Observers take `On<E>`. Screenshot: `spawn(Screenshot::primary_window()).observe(save_to_disk(p))`.
- `shadow_maps_enabled`; ambient = resource `bevy::light::GlobalAmbientLight`. `PerspectiveProjection` has
  `near_clip_plane` (use `..default()`). Text: `FontSize::Px(18.0)`, lengths `px(8)`. `WindowResolution::new(u32,u32)`.
- `single()` returns `Result`; `Single<D, F>` param. Mesh: `bevy::mesh::{Indices, PrimitiveTopology}`,
  `bevy::asset::RenderAssetUsages`. UI picking is on by default: `bevy::picking::hover::HoverMap`.
- `Window::set_cursor_position` also warps the OS cursor. Required components: `Mesh3d` brings `Visibility`,
  `MeshMaterial3d` alone does not — spawn `Visibility` explicitly if you query it before a mesh exists.
- Bevy features: `default-features = false, features = ["3d", "ui"]`. Workspace drops `rust_2018_idioms`.
