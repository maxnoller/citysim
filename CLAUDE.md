# CitySim — notes for agents

Original 3D city-builder (roads, RCI zoning, agent traffic). Rust 2024 + Bevy 0.20.1.

## Build / test / run
- `cargo test --workspace` · `cargo clippy --workspace --all-targets` (warning-free) · `cargo fmt --all --check` (width 120)
- CI: `.github/workflows/ci.yml` (fmt, clippy `-D warnings` + tests for the sim; `cargo check` of all three feature sets +
  tests for the game, with `Swatinem/rust-cache`). CI's clippy is the latest stable: keep the sim lint-clean on it.
- Run: `cargo run -p citysim`. Fast loop (≈3 s/edit vs ≈13 s): `--features bevy/dynamic_linking` on the command
  line only, never as a default. First Bevy build ≈ 9–10 min. Never commit a `.cargo/config.toml` naming a linker.
- Ownership check (must stay warning-free): `cargo clippy -p citysim --no-default-features --features gfx`
  and `... --features ui`. Both must also run.

## Layout and ownership (sim, gfx and ui are owned separately; keep the boundaries)
- `crates/sim` (`citysim_sim`) — SIM only. ALL state and rules. No Bevy, deterministic (only `world.rng`), serde.
  Mutate via `World::apply(Command)` / `World::step()`. `preview(cmd)` is Ok exactly when `apply(cmd)` would be.
- `crates/game/src/gfx/` — GFX only: scene/ground/lights, camera, roads, zones, buildings, vehicles, hovered-cell +
  tool-ghost drawing, overlays. One `GfxPlugin`. Assets in `crates/game/assets/gfx/`.
- `crates/game/src/ui/` — UI only: HUD, tool state machine, all input producing `Command`s, pause/speed/overlay/
  quality keys, save/load. One `UiPlugin`. Assets in `crates/game/assets/ui/`.
- `crates/game/src/shared.rs` — the ONLY contract between gfx and ui; change it only by agreement. gfx and ui
  never import each other. `main.rs` just builds the App (features `gfx`, `ui`, both default).
- `.github/workflows/ci.yml` — CI. `docs/` — README images (`screenshot.png`, `night.png`, `road-tool.png`, `traffic-overlay.png`).
- `crates/game/src/dev/` — development tooling, not owned by gfx/ui: `scenario` (real scripted towns), `autoshot`
  (screenshot and exit), `control` (file control channel for scripted play). `scripts/play.py` plays the real game with
  OS input (xdotool) through that channel.
- `docs/screenshot.png` (README), `THIRD_PARTY_NOTICES.md` (bundled font licences; licence files stay next to the fonts).
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
- Keys: gfx = WASD/arrows, Q/E, wheel, middle drag. ui = I R Z B Esc/right-click, Space 1 2 3, O, G, F5/F9,
  J, Tab, Alt, Enter, Backspace, [ ], Ctrl+Z.
- Round 2 (ui writes, gfx draws): `RoadChoice`, `ToolModes { road, zone, bulldoze }`, `SnapFeedback` (snapped point,
  `SnapTarget`, `Guide` lines in sim metres, spacing), `PathPreview` (corners, legs with per-leg error and
  `upgrade_cells`, length, cost), `WorldHighlight` (hovered / pinned `WorldTarget`, doomed buildings and road cells),
  `DisplaySettings { edge_scroll, lot_markers }` (gfx's camera pans at the window edge when `edge_scroll`; default off).
  `ToolPreview::line` is the leg under the cursor only.
- `SceneReady(bool)`: gfx sets it once the render pipelines are built (`gfx/loading.rs` counts them in the render
  world); ui shows a loading card until then (`ui/loading.rs`), autoshot counts its delay from it.

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
- HUD data: `stats` (population, households, `workforce`, employed, unemployed, `unemployment` = unemployed / workforce,
  `vacant_jobs`, jobs, avg_commute_s, avg_speed m/s, congestion_share, buildings_by_kind); `employed + unemployed ==
  workforce` (use these for the jobless figure). `demand` (-1..=1 per zone).
- Budget: `economy.last_day` = yesterday's `Budget { residential_tax, commercial_tax, industrial_tax, road_upkeep,
  services }` with `income()`, `expenses()` (upkeep + services), `net()`; `economy::budget_now(world)` = today so far;
  `economy.history[i].budget` per finished day (`DaySummary`). City services cost 0.75 per resident per day.
- Money: start 30,000, road 65 per cell (`balance::ROAD_COST_PER_CELL`; ui prices follow it), upkeep 1 per road cell per
  day. Bulldozing is never refused for lack of money. Money may go negative.
- Abandonment: a building cut off from the network is abandoned after one full in-game day continuously unreachable
  (`growth` `BuildingMeta::unreachable_since`); reconnecting resets it. Empty for `ABANDON_EMPTY_DAYS` midnights too.
- Saves: format version 4 (round 2); version-3 (round-1) saves load (`save::v3`, fixture
  `crates/sim/tests/data/round1_town.citysave`); `World::load` rejects structurally damaged saves. Round-1 saves keep
  their old second-row buildings until they are rebuilt.
- Scenarios: `scenarios::{GridPlayer, two_districts, add_connections, overbuilt}`; `scenarios::prebuilt(w, f)` pays
  for a static layout (they cost more than a new game's money).
- Sim balance runs: `cargo run -p citysim_sim --release --example headless` (scripted cities in `citysim_sim::scenarios`).
- `IdMap` (slotmap-like, serialises its free list so loads stay in lockstep).
- Round 2 (sim): `RoadKind::{Street, Highway, Avenue}` (`lane_count`, `lane_offset_m(lane)` right of travel, lane 0 =
  kerb, `half_width_m`, `cost_per_cell`); `Command::BuildRoad { from, to, kind }` upgrades/downgrades built pieces in
  place (`Preview.{new_pieces, upgraded, downgraded}`); `Command::SetJunctionControl { cell, control:
  JunctionSetting::{Auto, Signal, Priority} }` (`Preview.junction`). `junction::{junction_view, signal_aspect,
  movement_aspect, junction_at, junction_nodes, setback}`: control, per-approach aspect / left arrow / time to change /
  queue / major, stop-line setback. Vehicles: `lane`, `lane_shift` (ease over `LANE_CHANGE_S`), `junction:
  Option<JunctionMove>` (in the box: `pos`/`heading` follow the turn curve), `braking`, `stopped()`;
  `drive::vehicle_pose(world, v)` = exact lane pose. Lots: frontage only, 1x1 / deep 1x2 / 2x2, `facing` to the road
  (`growth::has_frontage`). Money: `economy.fiscal` (`FiscalState`, `reason.text()`, `service_level`). Saves: format 4;
  version-3 saves load (`save::v3`). Scripted players: `GridPlayer::{new, hierarchy, single_artery}`; renders:
  `cargo run -p citysim_sim --release --example topdown -- signal|lots|video|watch out`.
- Junction rules (round-2 integration): a junction's stop lines sit `JUNCTION_MARGIN_M` (2.5 m) beyond the crossing
  carriageways (a bend: `BEND_MARGIN_M`), which leaves room for the zebra crossings gfx draws there; two movements
  conflict when their paths cross *or* come within `MOVEMENT_CLEARANCE_M` (car width plus room), so cars never
  touch in a box; a degree-2 node where the lane count changes (highway end, avenue -> street) gets a short merge box
  (`LANE_MERGE_TAPER_M`). Auto control turns a junction into signals from `AUTO_SIGNAL_PER_MINUTE` (16) vehicles in
  its busiest minute. New games start at 09:00 (`START_TICK_OF_DAY`). The pinned checksum in `tests/round2.rs` moves
  with any rule change: update it deliberately.

## Dev / headless flags (need `DISPLAY=:0`; llvmpipe works)
- `CITYSIM_DEMO=town` (~1,500 residents) · `city` (~5,000, streets only) · `hierarchy` (`GridPlayer::hierarchy`,
  ~10,000 with avenues, ~50 s to build in release) · `jam` (bad layout, one overloaded junction): real towns
  built by `citysim_sim::scenarios` through commands and fast-forwarded before the first frame (< 1 s release, a few s
  debug). `CITYSIM_DEMO_POP=n` target, `CITYSIM_DEMO_HOUR=h` stop time (default 8 = morning rush).
- `CITYSIM_AUTOSHOT=out.png` (or `--autoshot`): screenshot then exit 0. `CITYSIM_SHOT_VIEW=wide|street|close`,
  `CITYSIM_SHOT_DELAY=25` (s, default 3), `CITYSIM_SHOT_OVERLAY=none|zones|traffic`. Without a demo it builds
  a small sample city. Example: `CITYSIM_DEMO=town CITYSIM_AUTOSHOT=/tmp/t.png CITYSIM_SHOT_DELAY=25 cargo run -p citysim`.
- `CITYSIM_LOG_FPS=1` logs FPS (llvmpipe, 4 vCPU: small town 5–8, 10,000 residents ≈ 4 Low / 2 High). `CITYSIM_SEED=n`
  world seed (the `hierarchy` demo reaches 10,000 fastest with seed 3). `CITYSIM_CONFIG_DIR` overrides the settings
  folder (`settings.cfg`; not written during autoshot / UI shots).
- `CITYSIM_CONTROL=dir` + `scripts/play.py` (see its docstring): `launch`, `start`, `grow --until-pop N`, `road`, `zone`,
  `bulldoze`, `look`, `shot`, `stats`, `counts`, `cmd 'until-day N'` (`launch` waits for the `ready` reply: shaders
  built). `launch --env K=V` passes extra variables. The channel only reads numbers, frames the camera,
  fast-forwards (`turbo`, `until-*`; they fail while paused) and takes screenshots; all play goes through real input.
- UI capture: `CITYSIM_UI_SHOTS=dir` (interface states; `CITYSIM_SAVE_DIR` for the save/load step), `CITYSIM_UI_TOUR=1`.
  `CITYSIM_UI_SHOTS_ONLY=road-path,junction-tool,...` picks states (names in `ui/shots.rs`).
- ui tools: every plan (path, block grid, frontage, fill, road bulldoze, undo) is a list of ordinary `Command`s checked
  in order on a scratch copy of the world (`ui/plan.rs`) and sent only when all pass. Snapping is pure lattice maths in
  `ui/snap.rs`. `scripts/play.py road` presses Enter to build its two-corner path.
- gfx dev aids: `CITYSIM_GFX_POSE="x,z,yaw,pitch,dist"` (exact camera, beats `CameraOverride`), `CITYSIM_GFX_TOUR=1`
  (scripted camera flight for videos), `CITYSIM_GFX_STATS=1` (entity/mesh/material/triangle/tree counts every 5 s),
  `CITYSIM_GFX_QUALITY=low|medium|high` (starting `GraphicsQuality`), `CITYSIM_GFX_HOUR=h` (pin the drawn time of
  day), `CITYSIM_GFX_TOOLDEMO=1|2|3` (gfx-only build: sample road-tool feedback), `CITYSIM_DEMO_AVENUES=1` (demo main
  road upgraded to an avenue).
- gfx shaders: `assets/gfx/shaders/city.wesl` (the shared `CityMaterial`: night glow by the vertex `uv.x` tag
  `mesh_util::Glow`, cloud shadows, ground detail) and `sky.wesl`. Day/night lives in `gfx/daynight.rs`.
- gfx look: one palette (`gfx/palette.rs`), one shared vertex-coloured material for buildings/roads/trees/cars; models
  are generated in code (`building_gen`, `vehicle_gen`, `props`) and cached; LOD via `VisibilityRange` (mesh origin
  must be near the geometry: the cross-fade dither is measured from the entity origin).

## Bevy 0.20 API facts (verify in `~/.cargo/registry/src/*/bevy*-0.20.1`)
- Events between systems are `Message`s (`#[derive(Message)]`, `add_message`, `MessageWriter::write`); `AppExit`
  too. Observers take `On<E>`. Screenshot: `spawn(Screenshot::primary_window()).observe(save_to_disk(p))`.
- `shadow_maps_enabled`; ambient = resource `bevy::light::GlobalAmbientLight`. `PerspectiveProjection` has
  `near_clip_plane` (use `..default()`). Text: `FontSize::Px(18.0)`, lengths `px(8)`. `WindowResolution::new(u32,u32)`.
- `single()` returns `Result`; `Single<D, F>` param. Mesh: `bevy::mesh::{Indices, PrimitiveTopology}`,
  `bevy::asset::RenderAssetUsages`. UI picking is on by default: `bevy::picking::hover::HoverMap`.
- Never `remove::<Mesh3d>()` to hide something and re-insert it later: such an entity was not drawn again (0.20.1).
  Toggle `Visibility` and keep the mesh (cursor ghost, zones, overlay, trees do this).
- `Window::set_cursor_position` also warps the OS cursor. Required components: `Mesh3d` brings `Visibility`,
  `MeshMaterial3d` alone does not — spawn `Visibility` explicitly if you query it before a mesh exists.
- Bevy features: `default-features = false, features = ["3d", "ui"]`. Workspace drops `rust_2018_idioms`.
