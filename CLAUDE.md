# CitySim — notes for agents

Original 3D city-builder (roads, RCI zoning, agent traffic). Rust 2024 + Bevy 0.20.1.

## Build / test / run (fastest correct way first; times on the owner's 12-core desktop)
- **New checkout, first thing:** `scripts/dev-setup.sh --build`. It writes a git-ignored `.cargo/config.toml` for
  this checkout (linker flags that work here: mold, and lld off where the toolchain's wrapper is broken; `jobs` = half
  the cores), copies the machine's compiled dependencies (the "seed", `~/.cache/citysim/seed`) into `target/`, builds
  the game and every test, and saves a seed if there was none. Seeded: ≈1.5 min; without a seed 11–27 min (by load).
  Each checkout builds in its own `target/` (the script refuses a shared `CARGO_TARGET_DIR`: no waiting on another
  checkout's build lock, never another checkout's build of the workspace crates; seeds hold dependencies only).
  Then plain `cargo ...`: no `RUSTFLAGS`, no `-j` (an exported `RUSTFLAGS` replaces the file's flags and rebuilds
  everything).
  `--build-release` also builds the release game. Never commit a `.cargo/config.toml`: CI and other machines build
  without one.
- **Profiles:** `dev` for everything — tests, demo cities, screenshots, play-testing (game crate opt-level 0 and
  incremental; Bevy and the simulation at opt-level 3; same average frame time as opt-level 1, higher spikes).
  `--release` only to play at full speed or to measure performance (`CITYSIM_BENCH`); a release rebuild of the game
  crate takes about a minute (≈3 min under load).
- **Sim edit → tests:** `cargo nextest run -p citysim_sim -P quick` after each edit (≈9 s: every test except the
  ones that grow cities for 10 s or more, list in `.config/nextest.toml`); one test: `cargo nextest run -p citysim_sim
  NAME` (≈8 s); everything before pushing: `cargo nextest run -p citysim_sim` (≈90 s, all tests at once, the timing
  test alone; `cargo test` runs the test files one after another, ≈2× longer). A test that grows several independent
  cities grows them side by side (`common::par_map`). Simulation-only agents never build the game: `-p citysim_sim`.
- **Game edit → picture:** `cargo build -p citysim` (≈5 s for a one-line change in `ui/` or `gfx/`), then a
  windowless still: `CITYSIM_DEMO=organic CITYSIM_STILLS=/tmp/s CITYSIM_STILLS_LIST="a=wide;b=street"
  target/debug/citysim` (≈7 s with the demo cached, +8 s once after a sim change; no display, see Dev flags).
- **Balance edit → scenario numbers:** `cargo run -p citysim_sim --release --example headless -- --seeds 10 hierarchy`
  (one seed per core, `CITYSIM_HEADLESS_THREADS=n` for fewer; ≈12 s to rebuild the sim in release, then ≈85 s for ten
  10,000-resident cities).
- **Before pushing:** `scripts/ci-local.sh` runs what CI runs (≈3 min; sim tests through nextest, `sim` / `game` for
  one job); `--full` adds clippy on the game in all three feature sets. `cargo fmt --all --check` (width 120) and
  clippy warning-free.
- Git: `main` changes only through pull requests, squash-merged (the PR title becomes the commit: one plain sentence
  about what changes); both CI jobs must pass; merged branches are deleted. The repository is public: nothing private
  (paths, names, keys) in commits, pull requests or docs. Dependabot opens dependency updates daily.
- CI (on pull requests and on `main`): `.github/workflows/ci.yml` (fmt, clippy `-D warnings` + `cargo test` for the sim; `cargo check` of all three
  feature sets + tests for the game, with `Swatinem/rust-cache`; about 6 minutes, the sim tests on a two-core runner).
  CI's clippy is the latest stable: keep the sim lint-clean on it.
- Ownership check (must stay warning-free): `cargo clippy -p citysim --no-default-features --features gfx`
  and `... --features ui`. Both must also run.
- `--features bevy/dynamic_linking` (command line only, never a default) builds a second copy of Bevy; with mold the
  static dev build relinks in about a second, so it buys little.

## Layout and ownership (sim, gfx and ui are owned separately; keep the boundaries)
- `crates/sim` (`citysim_sim`) — SIM only. ALL state and rules. No Bevy, deterministic (only `world.rng`), serde.
  Mutate via `World::apply(Command)` / `World::step()`. `preview(cmd)` is Ok exactly when `apply(cmd)` would be.
- `crates/game/src/gfx/` — GFX only: scene/ground/lights, camera, roads, zones, buildings, vehicles, hovered-cell +
  tool-ghost drawing, overlays. One `GfxPlugin`. Assets in `crates/game/assets/gfx/`.
- `crates/game/src/ui/` — UI only: HUD, tool state machine, all input producing `Command`s, pause/speed/overlay/
  quality keys, save/load. One `UiPlugin`. Assets in `crates/game/assets/ui/`.
- `crates/game/src/shared.rs` — the ONLY contract between gfx and ui; change it only by agreement. gfx and ui
  never import each other. `main.rs` just builds the App (features `gfx`, `ui`, both default).
- `.github/workflows/ci.yml` — CI. `docs/` — README images (`screenshot.png`, `night.png`, `road-tool.png`, `people.png`, `traffic-overlay.png`), all from the real game.
- `crates/game/src/dev/` — development tooling, not owned by gfx/ui: `scenario` (real scripted towns, cached),
  `stills` (windowless screenshots), `autoshot` (screenshot and exit, in a window), `control` (file control channel for
  scripted play); `crates/game/build.rs` (the sim fingerprint for the demo cache). `scripts/play.py` plays the real game
  with OS input (xdotool) through that channel. `scripts/dev-setup.sh` (checkout setup, seeds), `scripts/ci-local.sh`
  (CI's checks), `.config/nextest.toml` (test tiers).
- `docs/screenshot.png` (README), `THIRD_PARTY_NOTICES.md` (bundled font licences; licence files stay next to the fonts).
- Asset root resolves to `crates/game/assets` for `cargo run -p citysim` and for the binary run directly.

## shared.rs contract (summary)
- `SimWorld { world, generation }` (`key(version)`, `replace(world)`), `SimClock { paused, speed }`, `SPEEDS = [1, 3,
  6, 12]` (a day is 16 min at 1x, 80 s at 12x), `SimCommand(Command)` message → applied in `GameSet::Sim`, result in
  `LastCommand`. `World::step` runs in `FixedUpdate` (`SimStep` set): the fixed rate is `TICKS_PER_SECOND` times
  `SimClock::fixed_per_tick()` (fast speeds split a tick's steps over up to `MAX_FIXED_HZ` = 120 updates a second, so
  no frame carries all 12), `steps_per_tick()` steps each. gfx interpolates cars across however many updates ran.
- Frame order: `GameSet::{Input, Hover, Tools, Sim, Render}`.
- `ActiveTool`, `OverlayMode`, `GraphicsQuality`, `PointerOverUi` (ui writes), `CameraInputBlocked` (ui: menu open or
  keyboard in a drawer), `HoveredCell` / `HoveredPoint` (shared computes from the `WorldCamera` ray).
- `WorldCamera` marker (gfx's 3D camera), `CameraPose`, `CameraOverride` (when Some, gfx must show that pose; beats
  `CameraFollow`), `CameraFollow { focus }` (gfx keeps the focus there), `CameraGoTo` message (move / zoom once).
- Coordinates: `sim_to_world`, `world_to_sim`, `cell_center`, `world_to_cell`, `zone_cell_corners`.
- Keys: gfx = WASD/arrows, Q/E, wheel, middle drag. ui = I R Z B (bulldozer on / off) J Esc/right-click, Space 1 2 3
  4, O, T, G, F5/F9, F3 (performance panel), F6 (keyboard into the city window or the open drawer: arrows, Enter, Esc),
  Tab (tool mode; city window page), Alt, Enter, Backspace, [ ], Ctrl+Z, P, U, F, arrows + Enter in menus.
- Tool feedback (ui writes with `set_if_neq`, gfx draws): `RoadChoice`, `ToolModes { road, zone, bulldoze }`
  (`RoadMode::{Straight, Curve, Continuous, Grid, Upgrade, Junction}`, `ZoneMode::{Paint, Frontage, Fill, Marquee}`,
  `BulldozeMode::{Road, Area}`), `SnapFeedback` (snapped point, `SnapTarget`, `Guide`s in sim metres, spacing),
  `CurveToolView` (the free-form road being drawn: pieces with `PieceChange`, refused curves, `error_at`, control
  points, handles, tightest bend, planned nodes; written only when it changes), `ToolPreview::{kind, area,
  zone_cells, error, cost}`, `WorldHighlight` (hovered / pinned `WorldTarget` incl. `Vehicle`, doomed buildings,
  `doomed_roads`). (`PathPreview`, `ToolPreview::{line, rect, cells}`, `WorldHighlight::doomed_road_cells` and
  `shared::cell_corners` are gone: nothing wrote or used them any more.)
- `RouteView { state: RouteState::{None, Driving, Planned, Unseen}, points, origin, destination }` (ui writes while a
  car or person is selected, only when it changes meaningfully: a new route, the car 2.5 m further on; gfx draws it in
  `gfx/route.rs`): `points` in sim metres along the lanes (lane offsets, left-turn pockets and junction turn paths included) from the car
  (or the trip's start when `Planned` / `Unseen`) to where it stops; `RouteEnd { pos, building }` at both ends (gfx
  rings both, glows the destination building and floats a pin over it). Traced in `ui/route.rs`
  (`vehicle_path`, `planned_path`, `person_trip`).
- `DisplaySettings { edge_scroll, lot_markers }`: edge panning (default off); empty zoned lots are marked only while
  zoning unless the player picks "Always" (persisted as `lot_markers_always`).
- `PerfProbe { detail: PerfDetail::{Off, Small, Full}, render: Option<RenderStats> }`: ui writes `detail` (the
  performance panel's size), gfx (`gfx/perf.rs`) fills `render` four times a second only while `Full` (GPU ms per
  top-level pass, render-thread CPU ms, draw calls, main-pass triangles, mesh / car entities and how many are on
  screen, the game's GPU memory, card / backend / driver). Closed, gfx measures nothing: Bevy's render-diagnostics
  recorder is taken out of the render world while the graph runs.
- `SceneReady(bool)`: gfx sets it once the render pipelines are built (`gfx/loading.rs`); ui shows a loading card until
  then, autoshot counts its delay from it.

## Sim facts for renderers
- `MAP_CELLS = 256`, `CELL_SIZE = 8` m. Cell `(x, y)` centre = `((x+.5)*8, (y+.5)*8)` m; sim `(x, y)` → Bevy
  `(x, 0, y)`. Highway stub cells `(0,128)..=(11,128)`; `roads.entry` is its edge node.
- Draw roads from `world.roads` (graph) only. Rebuild on `SimWorld::key(world.versions.{roads,grid,buildings})`.
- `Building { kind, footprint, level, capacity, occupied, facing: Facing, seed, created_tick }`.
- `Vehicle { pos (m, centre line), heading (rad, atan2 in sim xy), segment, forward (a→b), progress (m), seed }`.
- `RoadSegment { load: [f32; 2], capacity: [f32; 2] }` indexed `[a→b, b→a]`; `congestion_in(forward)`.
- Segment ids can vanish on road edits (split keeps the id for the `a` half): always re-check.
- Time: vehicles move in real time (1 tick = 1/20 s), the calendar is compressed: a day = 16 min at 1x
  (`balance::DAY_MINUTES`, `TICKS_PER_DAY` 19,200, an hour 40 real s). The interface shows every duration in game
  time (`ui/people.rs` `span`, `bar::commute_text`: 40 s of driving reads "1 h").
- Trip kind per car: `world.traffic.trips.get(vehicle_id).map(|t| t.purpose)` (`TripPurpose::Goods` = draw a truck).
- HUD data: `stats` (population, households, `workforce`, employed, unemployed, `unemployment` = unemployed / workforce,
  `vacant_jobs`, jobs, avg_commute_s, avg_speed m/s of moving cars, congestion_share, buildings_by_kind); `employed +
  unemployed == workforce`. `traffic.stopped_share` (cars standing this tick; ui eases it), `traffic::delay_factor`
  (shown as "trip delay"). `demand` (-1..=1 per zone).
- Budget: `economy.last_day` = yesterday's `Budget { residential_tax, commercial_tax, industrial_tax, road_upkeep,
  services }` with `income()`, `expenses()` (upkeep + services), `net()`; `economy::budget_now(world)` = today so far;
  `economy.history[i].budget` per finished day (`DaySummary`); cost breakdown `economy::running_costs(world)` /
  `economy.last_day_costs`. Shops and works pay tax on the shift time actually worked (`economy.attendance`).
- Money: start 30,000, street 65 / avenue 90 per cell to build (`balance::ROAD_COST_PER_CELL`,
  `AVENUE_COST_PER_CELL`; roads are priced by the metre), upkeep 4 / 8 per cell per day, $80 per signalled junction
  per day, services per resident growing with size and density. Business tax lost to traffic:
  `economy.last_day_traffic` / `economy::traffic_loss_now(world)` (`TrafficLoss`; the commercial and industrial tax
  lines are already net of it; the budget page shows it as its own line and, per day seen, as a chart). Why people
  are jobless: `demand::jobs_view(world)` (`JobsView`, `JoblessReason::text()`; shown under the statistics tiles). Bulldozing is never refused for lack of money. Money
  may go negative.
- People: individual citizens in households (`world.citizens`, `inspect::*`); every car trip is a car on the road up
  to `traffic::vehicle_budget` (125 per 1,000 residents), the rest happen off screen; commutes off screen take
  `traffic::delay_factor` times their free-flow time (what the roads do to real cars).
- Zoning is on lattices of blocks along each road side (`world.zoning`; `world.grid_view()` answers world-cell
  questions); buildings by centre and rotation (`Footprint`). Contract: `docs/r4-sim-contract.md`.
- Zoning r5 (`docs/r4-sim-contract.md`, "r5-zoning"): curved road sides are strip lattices (`Lattice::is_strip()`;
  use `cell_corners` / `axes_at` / `lot_quad`, never `u` on a strip); blocks are 6 rows (`BLOCK_DEPTH`), the cell
  nearer its own road keeps overlapping land; lots carry a yard behind the 2 built rows (`Footprint::built_area`).
  Zoning test bed: `cargo run -p citysim_sim --release --example topdown -- zonebed out.png [days]`;
  `scripts/zplay.py` plays the same layout in the game (`launch EXE load`, `zone`, `modes`, `hood`, `shots`).
- Abandonment: a building cut off from the network is abandoned after one full in-game day continuously unreachable
  (`growth` `BuildingMeta::unreachable_since`); reconnecting resets it. Empty for `ABANDON_EMPTY_DAYS` midnights too.
- Saves: format version 7 (strips in extension block 4; version 6 loads, its zoning is remade); versions 4 and 3 load (`save::v4`, `save::v3`, fixture
  `crates/sim/tests/data/round1_town.citysave`), version 5 is refused; `World::load` rejects structurally damaged
  saves. Round-1 saves keep their old second-row buildings until they are rebuilt.
- Scenarios: `scenarios::{GridPlayer, two_districts, add_connections, overbuilt, organic, upgrade_to_hierarchy}`;
  `scenarios::prebuilt(w, f)` pays for a static layout (they cost more than a new game's money).
- Traffic diagnostics: `--example simint` (save a grown city, replay a day: where trip time goes, worst junctions).
- Sim balance runs: `cargo run -p citysim_sim --release --example headless` (scripted cities in `citysim_sim::scenarios`).
- `IdMap` (slotmap-like, serialises its free list so loads stay in lockstep).
- Roads: `RoadKind::{Street, Highway, Avenue}` (`lane_count`, `lane_offset_m(lane)` right of travel, lane 0 = kerb,
  `half_width_m`, `upkeep_per_m`); `Command::BuildCurve { curve, kind }` (redrawing a built piece as the other kind
  upgrades / downgrades it); `Command::SetNodeControl { node, control: JunctionSetting::{Auto, Signal, Priority} }`.
  `junction::{junction_view, junction_shape, signal_aspect, movement_aspect, junction_nodes, setback}`. Vehicles:
  `lane`, `lane_shift`, `junction: Option<JunctionMove>`, `braking`, `stopped()`, `route`; `drive::vehicle_pose(world,
  v)` = exact lane pose. Money: `economy.fiscal` (`FiscalState`, `reason.text()`, `service_level`). Renders:
  `cargo run -p citysim_sim --release --example topdown -- signal|lots|video|watch out`.
- Junction rules: a junction's stop lines sit `JUNCTION_MARGIN_M` (2.5 m) beyond the crossing
  carriageways (a bend: `BEND_MARGIN_M`), which leaves room for the zebra crossings gfx draws there; two movements
  conflict when their paths cross *or* come within `MOVEMENT_CLEARANCE_M` (car width plus room), so cars never
  touch in a box; a degree-2 node where the lane count changes (highway end, avenue -> street) gets a short merge box
  (`LANE_MERGE_TAPER_M`). Waiting cars stand with the front bumper `STOP_LINE_M` behind the line
  (`drive::stop_back_m`). Auto control: avenues meeting get signals, streets joining or crossing an avenue give way
  until busy; a junction becomes signals from `AUTO_SIGNAL_PER_MINUTE` (16) vehicles let in plus still queued in its
  busiest minute. New games start at 09:00 (`START_TICK_OF_DAY`). The pinned checksum in `tests/round2.rs` moves
  with any rule change: update it deliberately.

## Dev / headless flags (the windowless modes need no display; the others run under Xvfb, see below)
- **Screenshots: `CITYSIM_STILLS=dir`** (windowless: no display, no Xvfb, no window can open). Renders off screen at
  `CITYSIM_SHOT_SIZE` (1920x1080) with the HUD, several shots from one start, then exits 0 once every file is written:
  `CITYSIM_STILLS_LIST="name=view;..."` saves `dir/name.png`; a view is `wide|street|close` (the autoshot presets of
  the demo) or an exact pose `x,z,yaw,pitch,dist` (sim metres, radians, as `CITYSIM_GFX_POSE`), `+zones` / `+traffic`
  shows that overlay. Time of day: the demo's (`CITYSIM_DEMO_HOUR`) or `CITYSIM_GFX_HOUR=h`. `CITYSIM_STILLS_SETTLE`
  (s between camera move and capture, 0.75), `CITYSIM_SHOT_DELAY` (s after the scene is ready, 1). Example:
  `CITYSIM_DEMO=organic CITYSIM_GFX_HOUR=21 CITYSIM_STILLS=/tmp/s CITYSIM_STILLS_LIST="w=wide;s=street;t=close+traffic"
  target/debug/citysim` (≈7 s for one shot, ≈10 s for four, demo cached; 4-5 s of it is building the render
  pipelines). Nothing is hovered (no cursor): interface states with hover or tools still need `CITYSIM_UI_SHOTS` under
  Xvfb (the demo cache makes those ≈10 s too).
- `CITYSIM_DEMO=town` (~1,500 residents) · `city` (~5,000, streets only) · `hierarchy` (`GridPlayer::hierarchy`,
  ~10,000 with avenues, ~50 s to build) · `jam` (bad layout, one overloaded junction): real towns built by
  `citysim_sim::scenarios` through commands and fast-forwarded before the first frame. `CITYSIM_DEMO_POP=n` target,
  `CITYSIM_DEMO_HOUR=h` stop time (default 8 = morning rush). `organic` = the sim's free-form town
  (`scenarios::organic`, ~4,300 residents, 8-13 s to build); `shapes` = gfx's road-shape test town (`dev/shapes.rs`:
  5-arm junction, a street meeting the avenue at 30°, a T on the outside of a bend, 25° grid, ring).
  Built demo worlds are cached as saves in `~/.cache/citysim/demos` (shared by all checkouts; `CITYSIM_DEMO_CACHE=dir`
  elsewhere, `=0` off) and load in well under a second. The key includes `CITYSIM_SIM_FINGERPRINT` (`crates/game/
  build.rs`: a hash of the sim's sources, `dev/scenario.rs`, `dev/shapes.rs` and `Cargo.lock`), so a sim edit
  rebuilds the demo once; unused files go after a week.
- `CITYSIM_BENCH=out.json`: windowless benchmark (no winit, no `DISPLAY` needed): renders off screen at
  `CITYSIM_BENCH_SIZE` (1920x1080) along a fixed camera path for `CITYSIM_BENCH_FRAMES` (1500) after
  `CITYSIM_BENCH_WARMUP` (240), sim running (`CITYSIM_BENCH_PAUSED=1` stops it), two GPU frames in flight; writes
  frame-time avg / median / p99 / worst 1 %, GPU ms per pass, triangles, entities, meshes, materials, draw calls, sim
  step time. Use with a demo and `CITYSIM_GFX_QUALITY`; `CITYSIM_BENCH_SPEED=3` runs the sim at the top speed.
  Hitches: every frame is recorded (main-world parts, render-thread parts, GPU wait, meshes / materials changed,
  pipelines created, tags for what happened); the report lists the 20 slowest frames with their tags, every frame goes
  to `<out>.frames.csv`. `CITYSIM_BENCH_SCRIPT=quality` switches the quality ten times (`CITYSIM_BENCH_SWITCHES=n`),
  `=actions` plays a fixed round of player actions through real commands, resources and keys during the camera path
  (build / zone / bulldoze a street, signals, overlays, day and night, city window, select a car, 12x, a fast zoom,
  save / load only with `CITYSIM_SAVE_DIR` set; `gfx/bench_script.rs`; `CITYSIM_BENCH_SCRIPT_SCALE=0.1` shortens it
  for a software renderer). **`scripts/perf-check.sh [dir]`** builds the release game and runs both scripts on the
  10,000- and 20,000-resident cities plus 1x / 12x: one table (quality switch <= 50 ms, any action <= 33 ms, 60 fps at
  20,000, 12x < 2 ms over 1x; about 4.5 min on the owner's desktop; `PERF_CHECK_ONLY=quality,actions10k`).
  `CITYSIM_BENCH_UI=1` draws the interface into the image too (else there is no UI without a window; with
  `CITYSIM_PERF=off|small|full` it measures the performance panel's cost); `CITYSIM_BENCH_GPU=0` leaves GPU pass timing
  to the panel, as in a game (on only while it is full). Tail
  figures are only meaningful on a quiet machine (other builds double them).
- `CITYSIM_RECORD=dir`: windowless video frames (`frame_00000.png`...) at `CITYSIM_RECORD_FPS` (30) with a fixed time
  step, camera on a Catmull-Rom path `CITYSIM_RECORD_PATH="t,x,z,yaw,pitch,dist;..."`; join with ffmpeg.
- Running the game on the owner's desktop: never open a window there; use the windowless modes (stills, bench,
  record) or `xvfb-run -a -s "-screen 0 1920x1080x24" env -u WAYLAND_DISPLAY -u XDG_SESSION_TYPE ...` (renders on the
  GPU, presents at 12-16 fps). `CITYSIM_SHOT_SIZE=1920x1080` sets the autoshot window size.
- `CITYSIM_AUTOSHOT=out.png` (or `--autoshot`, in a window: under Xvfb; prefer `CITYSIM_STILLS`): screenshot then
  exit 0. `CITYSIM_SHOT_VIEW=wide|street|close`,
  `CITYSIM_SHOT_DELAY=25` (s, default 3), `CITYSIM_SHOT_OVERLAY=none|zones|traffic`. Without a demo it builds
  a small sample city. Example: `CITYSIM_DEMO=town CITYSIM_AUTOSHOT=/tmp/t.png CITYSIM_SHOT_DELAY=25 cargo run -p citysim`.
- `CITYSIM_PERF=off|small|full`: the performance panel's starting size (over the saved one; for `CITYSIM_BENCH` runs
  of its cost). `ui/perf.rs` measures frames, the game update, sim steps and the process (`sysinfo`, a worker
  thread, once a second); its graph is a `UiMaterial` (`assets/ui/shaders/perf_graph.wesl`).
- `CITYSIM_LOG_FPS=1` logs FPS (meaningless under Xvfb: use `CITYSIM_BENCH`). `CITYSIM_SEED=n`
  world seed (the `hierarchy` demo reaches 10,000 fastest with seed 3). `CITYSIM_CONFIG_DIR` overrides the settings
  folder (`settings.cfg`; not written during autoshot / UI shots).
- `CITYSIM_CONTROL=dir` + `scripts/play.py` (see its docstring): `launch`, `start`, `grow --until-pop N`, `road`, `zone`,
  `bulldoze`, `curve`, `follow`, `checks`, `look`, `shot`, `stats`, `counts`, `cmd 'until-day N'` (`launch` waits for
  the `ready` reply: shaders built). `launch --env K=V` passes extra variables. The channel only reads numbers, frames
  the camera, fast-forwards (`turbo`, `until-*`; they fail while paused), finds things (`find-home`, `find-car [MIN]` = a slow car on screen with MIN m to go, `node NAME` = a
  named UI node's position) and takes screenshots; all play goes through real input. `stats` includes the tool modes
  (`road_mode=...`); `play.py` switches modes with Tab. `play.py checks` (right after `launch`, new game) runs eight
  awkward real-input cases (fast clicks, drag off the window, UI to map, two snaps, a self-crossing loop, zoom and
  rotate mid-draw, curve and undo, step back) and prints PASS / FAIL.
- UI capture: `CITYSIM_UI_SHOTS=dir` (interface states, best on `CITYSIM_DEMO=organic`; `CITYSIM_SAVE_DIR` for the
  save/load step; `CITYSIM_SHOT_SIZE=WxH` sets the window), `CITYSIM_UI_TOUR=1`.
  `CITYSIM_UI_SHOTS_ONLY=road-curve,people-person,...` picks states (names in `ui/shots.rs`).
- ui tools: plain modules know nothing of layout: `ui/fsnap.rs` (snapping: nodes, T onto roads, angles from the start
  road and 15° steps, whole-cell lengths, zoning lattices, parallel at a set distance, extensions, line meetings),
  `ui/shapes.rs` (`Draft` chains of straight / curve / continuous pieces, `GridFrame`, `Stretch` of road between
  junctions), `ui/plan.rs` (`Op`s checked in order on a scratch copy; road parts named by centre line and resolved
  when their turn comes; undo by serials / curves / zone cells), `ui/readout.rs` (the cursor card's words),
  `ui/people.rs` (names, portraits, person / place / car / city `Sheet`s with headline, trip bar (with lateness),
  mood reason, action links, `Section::folding` sections the panel lets the player fold, remembered in
  `panel::Folds`). One-line panel texts are cut with "…" by measured width (`panel::Ellipsis`). A plan is sent only when every step passes. `scripts/play.py road` presses Enter to build its
  two-point road.
- ui look (the HUD): docked to the screen edges, angular, gunmetal plates with one purple accent. `ui/theme.rs`
  is the only place for colours / sizes / fonts (accent = `ACCENT_SET`: base, fill gradient, ink-on-accent, text;
  swap those five to change the purple). Fonts: Barlow Semi Condensed (text) and Barlow Condensed (figures),
  static faces chosen by weight in `UiFonts`; Lucide for icons (it lacks arrows: no `→ ↑ ↓` in texts, use `›`).
  `ui/plate.rs` + `assets/ui/shaders/plate.wesl`: `PlateMaterial` (a `UiMaterial`) draws chamfered / slanted plates;
  the node's border widths are the edge line, its border radii are the corner cuts, slants are fractions of the
  height (Bevy 0.20 passes border widths in px ordered left, top, right, bottom). Static plates share materials via
  `PlateCache`; animated ones use `PlateAnim`. Widgets (`widgets.rs`): `ButtonKind::{Slot, Segment, Toggle, Item,
  Action, Primary, Ghost, Row, Danger, Speed, Stepper, Sys}` with hover / press / active / `Disabled` / `Focused`
  looks, `caps`, `figure`, `Justified` (text alignment; text bundles already carry a `TextLayout`). Layout:
  `bar.rs` (bottom bar; the bulldozer is a small switch after the build slots, `ButtonKind::Demolish`, red when on,
  with its two modes and undo on a small plate above it; B toggles it and switching it off returns to the tool before,
  `UiState::before_demolish`), `top.rs` (city plate, `Alert` ribbon, system buttons), `tray.rs` (drawer over the bar:
  roads / zones / junctions / maps pages only), `city.rs` (the city window: `UiState::window: Option<CityPage>` =
  statistics / people / budget pages, side list, header, tiles, charts from `economy.history` and `people.history`
  via `chart.rs`; `budget.rs` and `panel.rs` fill the budget and city people parts; `WindowArea`; the game keeps
  running behind it, a build tool closes it; new pages: add a `CityPage` and a `page(...)` body), `panel.rs`
  (right-edge selection panel, `PeopleView`, follow, leader line, `PanelArea`), `route.rs` (the selected trip's
  `RouteView`, "Going to" jump = `UiAction::JumpToDestination`),
  `context.rs` (cursor readout tag and the card builders), `menu.rs` (pause menu, `MenuFocus`, first-run plate),
  `settings.rs` (settings and keys plates). `UiState::tray: Option<InfoTray>` (maps only; a tool's tray replaces
  it); `tray::TrayExtent` is the drawer's height (the side panel and the cursor readout stay above it);
  `UiState::tray_focus` while F6 keyboard focus is in the window or a drawer. The cursor readout never overlaps the
  window or a drawer (it flips, else hides). The interface is designed at 1920x1080 (`UiScale` 1.0) and compacts below a logical
  width of 1660 (`UiState::compact`).
- gfx dev aids: `CITYSIM_GFX_POSE="x,z,yaw,pitch,dist"` (exact camera, beats `CameraOverride`), `CITYSIM_GFX_TOUR=1`
  (scripted camera flight for videos), `CITYSIM_GFX_STATS=1` (entity/mesh/material/triangle/tree counts every 5 s),
  `CITYSIM_GFX_QUALITY=low|medium|high` (starting `GraphicsQuality`), `CITYSIM_GFX_HOUR=h` (pin the drawn time of
  day), `CITYSIM_GFX_TOOLDEMO=1|2|3|7` (gfx-only build, on `CITYSIM_DEMO=shapes`
  round `CITYSIM_GFX_TOOLDEMO_AT=x,z`: 1 a hovered junction and a selected building, 2 the bulldozer's area with what
  it removes, 3 zoning a marquee, 7 de-zoning; `4|5|6` the free-form road tool from real previews), `CITYSIM_DEMO_AVENUES=1` (demo main road upgraded
  to an avenue), `CITYSIM_GFX_SHOWCASE_SITES=x,z` (building sites with cranes at a spot).
- gfx tool feedback: `gfx/cursor.rs` draws the hovered-cell frame and a faint cell grid around the cursor for the road
  and zone tools only (never for Inspect or the bulldozer, which marks what it would remove in red);
  `gfx/route.rs` draws `RouteView` (accent ribbon with a dark edge and white chevrons, dashed when planned / unseen,
  rings at both ends, a glowing destination building and a pin), rebuilt only when `RouteView` changes.
- gfx shaders: `assets/gfx/shaders/city.wesl` (the shared `CityMaterial`: night glow by the vertex `uv.x` tag
  `mesh_util::Glow`, cloud shadows, ground detail) and `sky.wesl`. Day/night lives in `gfx/daynight.rs`.
- gfx look: one palette (`gfx/palette.rs`), one shared vertex-coloured material for buildings/roads/trees/cars; models
  are generated in code (`building_gen`, `vehicle_gen`, `props`) and cached; LOD via `VisibilityRange` (mesh origin
  must be near the geometry: the cross-fade dither is measured from the entity origin).
- gfx performance rule: never change a material asset that many meshes use (Bevy re-extracts and re-specialises every
  mesh using it: 30-60 ms frames). No frame rebuilds the whole city: a quality switch applies at once (the other
  levels' pipelines compile during loading on hidden warm-up cameras, `scene.rs`) and what depends on it (tree density,
  road furniture, shadow cascades, cloud shadows) follows `scene::QualityApplied`, re-meshing a few chunks a frame
  (`QUALITY_REMESH_MS`); a road edit refits only buildings near the changed 64 m buckets of `RoadEdges`
  (`unchanged_in`); a loaded world's buildings spawn `FILL_PER_FRAME` a frame, nearest the camera first. Time-of-day inputs live in a shared `ShaderBuffer` (`WorldMaterials::day`,
  `DayParams`), brake lights are `MeshTag` bit 0.
- gfx roads: `road_geom::Frame` samples a segment's curve (two points when straight); everything along a road is a
  ribbon over it between the arm cuts of `junction::junction_shape`. `gfx/land.rs`: one index of drawn road edges per
  roads version; building lots, lot markers and zone cells are fitted clear of it in their own frame (`land::fit`).
  The free-form road tool is drawn from `shared::CurveToolView` (filled by ui's road tool).
  Trees under the road being drawn are left out of their chunks while `CurveToolView` is active (re-meshed at most
  every 0.2 s). A selected vehicle (`WorldHighlight::selected`) gets white + accent ground rings and a floating
  diamond above it that grows with camera distance (unlit, readable at night); gfx's accent is the ui purple.

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
