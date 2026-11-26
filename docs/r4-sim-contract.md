# Simulation contract: free-form roads and zoning that follows them

Branches `r4-roads` and `r4-people`, merged on `r4-sim` (crate `citysim_sim`): one simulation of free-form roads and
individual citizens. This page is the contract between the simulation and the graphics / interface work. Additions
are appended freely; changes to anything already listed are recorded at the bottom under a dated heading. The people
side of the model is summarised in "One model: roads and people" below.

## The model in short

- **Roads** are a graph of nodes joined by segments. Every segment has a **centre line** `RoadSegment::curve`, a
  cubic Bézier in sim metres from its `a` node to its `b` node (a straight road is a straight `Curve`). Lengths are
  arc lengths. Segments still split when another road joins or crosses them (the `a` half keeps the id), so segment
  ids can still vanish on edits.
- A road can start and end anywhere and run at any angle or curve (`Command::BuildCurve`). Ends near a node join it,
  ends on a road split it into a T, crossings become junctions. Pieces of road are priced and kept up by the metre.
- **Junctions** at any angle: every node's arms get stop lines from the real geometry of their neighbours; vehicles
  take turn paths (`Movement::curve`) through the box; `junction_shape` publishes the outline polygon and the cut line
  of every arm for meshing.
- **Zoning** is no longer one world grid. 8 m cells live on **lattices** (grids at some origin and angle); each side of
  each segment owns **blocks** (rectangles of cells, `BLOCK_DEPTH` = 2 deep, front row along the kerb). Roads whose
  lattices line up share a lattice and its cells, so an ordinary right-angle grid city is exactly lattice 0, the
  world grid you know (cell `(i, j)` of lattice 0 is `Cell (i, j)`). Curved roads get several straight blocks.
  Where blocks of different lattices overlap, the older road keeps the cells.
- **Buildings** stand on a rectangle of cells of one lattice: centre in metres, rotation, size in cells, the cells.
  The front always faces the road it grew for.

Coordinates are unchanged: sim `(x, y)` metres → Bevy `(x, 0, y)`; the right of a unit direction `d` is
`(-d.y, d.x)` (traffic drives on the right).

## New items and how to use them

### Curves (`citysim_sim::curve`, re-exported `Curve`, `Pose`)
- `Curve { p: [[f32; 2]; 4] }`: start, two handles, end. `Curve::straight(a, b)`, `Curve::new(..)`.
- Tool constructors: `Curve::from_tangents(start, start_dir, end, end_dir) -> Option<Curve>` (arc fit; `None` for an
  S-bend), `Curve::through_corner(start, corner, end)` (simple 3-point curve), `Curve::continuing(start, dir, end)`
  (continue a road's direction), complex curve = `Curve::new` with the 4 points.
- `length()`, `pose_at(d, offset) -> Pose { pos, dir }` (`d` metres along, `offset` metres to the right),
  `point_at(d, offset)`, `nearest(p) -> (d, distance, point)`, `split_at(d)`, `sub(d0, d1)`, `reversed()`,
  `intersections(&other) -> Vec<(d_self, d_other, point)>`, `self_intersection()`, `min_radius() -> (radius, d)`,
  `total_turn()`, `start_dir()`, `end_dir()`, `is_straight()`, `bounds()`.
- **Renderers:** `flatten(tol) -> Vec<[f32; 2]>` (centre line polyline within `tol` metres) and
  `flatten_offset(offset, tol) -> Vec<(d, point)>` (kerbs, lane lines, sidewalks).

### Roads (`roads`)
- `RoadSegment::curve` (centre line `a -> b`), `RoadSegment::serial` (build order; lower = older; split halves keep
  it). `RoadNetwork::next_serial`.
- `RoadSegment::pose(from_a, offset)`, `pose_dir(forward, progress, offset)` (along travel), `nearest(p)`,
  `dir_from(node)` (tangent away from a node). `RoadNetwork::curve(s)`, `nearest_segment(pos, max_dist) ->
  Option<(segment, from_a, distance)>`, `add_curve(a, b, kind, curve)`, `split_at(s, from_a, pos)`.
- `RoadKind::cost_per_m()`, `upkeep_per_m()`, `min_radius_m()` (street 12 m, avenue 24 m),
  `reserve_half_width_m()` (4 m: what zoning keeps clear; drawn widths are still `half_width_m()`).
- `roads::curve_speed_limit(kind, curve)`: `RoadSegment::speed_limit` is now lower on tight curves.

### Commands (`command`)
- `Command::BuildCurve { curve, kind }` — a road anywhere (see the model). Use `World::preview` while drawing.
- `Command::SetRoadKind { segment, from_m, to_m, kind }` — upgrade / downgrade part or all (`0..=length`) of a
  segment.
- `Command::RemoveRoad { segment, from_m, to_m }` — remove part or all of a segment.
- `Command::BulldozeArea { quad }` — road parts inside a convex quad (sim metres) and buildings overlapping it.
- `Command::ZoneArea { quad, zone }` — zone / unzone the eligible cells whose centre is inside a convex quad.
- `Command::ZoneCells { cells: Vec<CellRef>, zone }` — zone / unzone a list (frontage and paint tools).
- `Command::SetNodeControl { node, control }` — junction control for a node anywhere.
- `Preview` (= `Outcome`) new fields: `pieces: Vec<PlannedPiece { curve, kind, change: PieceChange::{New, Upgrade,
  Downgrade} }>` (the road as it will be, split at its nodes), `nodes: Vec<PlannedNode { pos, change:
  NodeChange::{End, Join, Split, Crossing} }>`, `length_m` (new metres), `removed: Vec<Curve>`, `buildings:
  Vec<BuildingId>` (demolished), `zone_cells: Vec<CellRef>`.
- New `CommandError`s, each with the place: `TooShort { at, length, min }`, `TooTight { at, radius, min }`,
  `SharpAngle { at, degrees, min }`, `TooClose { at }` (along / beside another road), `SelfCrossing { at }`,
  `BuildingInTheWay { at, building }`, `OffMap { at }`, `NoSuchRoad`, `NotANode`.
  `CommandError::at() -> Option<[f32; 2]>` gives the position for any error that has one (draw it); `Display` gives
  the reason.

### Zoning (`zoning`, re-exported `Zoning`, `Lattice`, `LatticeId`, `CellRef`, `ZoneCell`, `Blocked`, `Block`,
`Side`)
- `world.zoning: Zoning { lattices: Vec<Option<Lattice>>, blocks: Vec<Block> }`. `LatticeId::WORLD` (0) is the map
  grid.
- `Lattice { origin, u, cells: BTreeMap<(i, j), ZoneCell> }`: `v()`, `angle()`, `cell_center(i, j)`,
  `cell_corners(i, j)`, `cell_of(pos)`, `local(pos)`, `world_at(x, y)`, `dir(facing)`.
- `CellRef { lattice, i, j }` names a cell; `CellRef::world(Cell)`.
- `ZoneCell { zone, building, covered, blocked: Option<Blocked::{Road, Contested, Building}>, fronts }`:
  `usable()` (covered and not blocked: draw these as zoneable land), `free()`, `fronts_on(facing)`.
- `Block { segment, side: Side::{Left, Right}, lattice, min, max, front: Facing, priority }`: `cells()`,
  `in_front_row(c)`, `depth_of(c)`; `Zoning::block_corners(&block)` (world corners), `Zoning::blocks_of(segment)`.
- Queries, cheap for one target per frame: `zoning::cell_at(world, pos)` (or `world.zoning.cell_at(pos)`),
  `world.zoning.cell(c)`, `cell_center(c)`, `cell_corners(c)`, `cells()`; `zoning::fill_area(world, pos, limit)`
  (connected area for a fill tool), `zoning::frontage(world, segment, side, depth)` (front cells of one road side
  between two junctions), `zoning::cells_in_quad(world, quad)`, `zoning::building_at(world, pos)`,
  `zoning::in_quad(quad, p)`.
- Draw zones from `world.zoning.cells()` (usable cells with a zone), keyed on `world.versions.grid`.

### Buildings
- `Building::footprint: Footprint { lattice, min, max, center, rotation, size: [width along the front, depth] }`.
  Use `Building::center()`, `rotation()` (radians, direction the front faces, `atan2` in sim axes), `front_dir()`,
  `size()` (now `(width along the front, depth)`), `corners()` (front-left, front-right, back-right, back-left),
  `cells()`. `Footprint::world(CellRect, Facing)` builds a lattice-0 footprint.

### Junctions
- `junction::junction_shape(world, node, extra) -> Option<JunctionShape { node, pos, arms: Vec<ArmCut>, outline }>`:
  `ArmCut { segment, setback, center, dir, left, right, half_width }` is where that arm's road surface ends (draw the
  segment between its two cuts); `outline` is the junction surface polygon (counter-clockwise). `extra` widens the
  kerbs beyond the carriageway (0 for asphalt, sidewalk width for the pavement). `None` for dead ends and smooth
  continuations (draw the road straight through the node).
- `Movement::curve` is the turn path (stop line to box edge, lane offsets included).

### Version counters
- `versions.roads` (graph, curves, kinds), `versions.grid` (any zone cell or block), `versions.buildings` — same
  names, wider meaning.

## Removed or changed meaning (and what to use instead)

| Item | Now |
|---|---|
| `Command` is `Copy` | `Clone` only (`ZoneCells` holds a list). |
| `Outcome` struct | `type Outcome = Preview` (same fields plus the new ones). |
| `Preview::new_pieces / upgraded / downgraded` | Road length in cells (metres / 8, rounded), not unit pieces. |
| `Preview::cells` | Still world cells; for zoning use `zone_cells` (cells may be on rotated lattices). |
| `CommandError::DiagonalCrossing` | Never produced: crossings at any angle make junctions. |
| `CommandError::NotStraight` | Only from the `BuildRoad` shorthand. |
| `RoadSegment::length` | Arc length of `curve` (was the chord). |
| `RoadSegment::new(a, b, kind, length, load)` | Placeholder curve; use `RoadNetwork::add_curve` / `add_segment`. |
| `RoadSegment::speed_limit` | Kind's limit, lower on tight curves. |
| `segment_ends(s)` | Still the node positions; draw `curve`, not the chord. |
| `Movement::{p0, p1, p2}` (quadratic) | `Movement::curve` (cubic). |
| `Arm::dir` | Tangent of the arm's curve at the node (was the chord). |
| `world.grid`, `Grid`, `CellData`, `Occupant` | Going away (still present while zoning moves onto blocks): use `world.zoning` and its queries; `Occupant::Road` → `ZoneCell::blocked == Some(Blocked::Road)`, `Occupant::Building(b)` → `ZoneCell::building`. |
| `Building::footprint: CellRect` | `Footprint` (lattice rectangle + centre, rotation, size). `footprint.rect()` is the lattice rectangle (world cells only for lattice 0). |
| `Building::facing` | Relative to the building's lattice; use `rotation()` / `front_dir()` for drawing. |
| `Building::size()` | `(width along the front, depth)` (was `(x, y)`). |
| `growth::front_cells`, `growth::has_frontage` (`CellRect`) | Lattice-0 only; frontage is `ZoneCell::fronts`. |
| `junction::junction_at(world, cell)` | Only finds junctions at cell centres; use node ids (`junction_nodes`, `world.roads.node_at`) and `SetNodeControl`. |
| `derived::rect_center(CellRect)` | Takes a `Footprint` (returns its centre); `cell_rect_center(CellRect)` for rectangles. |
| `Zone` | Lives in `zoning` (still re-exported as `citysim_sim::Zone` and `grid::Zone`). |

## Status of the implementation

See the dated entries below; the newest says what works.

## Changes after the first push

### 2026-11-25 (second push: builder, blocks, saves)

Behaviour now behind the contract:
- `BuildCurve` builds any curve: ends snap to nodes (`SNAP_NODE_M` 1.5 m) or onto roads (`SNAP_ROAD_M` 0.75 m, a T),
  every crossing splits both roads, a straight road along a collinear straight road changes its kind. Refusals:
  `TooShort` (any piece or split leftover under `MIN_SEGMENT_M` 4 m), `TooTight` (street 12 m, avenue 24 m),
  `SharpAngle` (arms under 25°), `TooManyArms` (new: more than 6 arms at a node), `TooClose` (closer than 8 m beside
  another road away from where they meet), `SelfCrossing`, `BuildingInTheWay`, `OffMap`, `InsufficientFunds`.
  `BuildRoad` is shorthand for a straight `BuildCurve` (keeps `NotStraight`, `ZeroLength`, `OutOfBounds`,
  `BlockedByBuilding(Cell)`). Diagonals may cross between cell centres (a junction at the cell corner).
- `SetRoadKind`, `RemoveRoad`, `BulldozeArea`, `Bulldoze` work on parts of any segment.
- Zoning is real blocks and lattices (`world.zoning`, saved). Lattice 0 is the world grid; a right-angle grid on
  the cell lattice is exactly lattice 0 as before. Zoning commands act on `zoning` cells of any lattice.
- Save format 6. Versions 5, 4 and 3 load (their zoning and buildings move onto lattice 0 / the blocks).

Changes to items listed above:
- `world.grid` is removed (as announced). New `World::grid_view() -> GridView` answers world-cell questions
  (`get(Cell) -> Option<CellData { zone, occupant }>`, `is_road(Cell)`); build it once per frame, not per cell.
  `CellData`, `Occupant` stay for it; `Grid` is gone.
- `Building::footprint` is `Footprint` (was announced); `Footprint` has `cells()`, `contains(CellRef)`, `area()`,
  `corners()`, `rect()` (lattice rectangle), `Footprint::new(lattice, &Lattice, min, max, facing)`.
- `growth::front_cells(&Footprint, Facing) -> Vec<CellRef>` and `growth::has_frontage(&World, &Footprint, Facing)`
  take a footprint now. New `growth::place_cells(&mut World, BuildingId)` marks a hand-inserted building's cells.
- New `World::roads_edited()`: call after changing `world.roads` directly (tools, tests); regenerates blocks.
- `CommandError::TooManyArms { at, arms }` added.
- `Stats::road_cells` / `avenue_cells` and so road upkeep are counted by the metre (`length / CELL_SIZE`), so a grid
  pays a little more upkeep than when junction cells were counted once (about +7 % on a block grid).
- Zone cells under a road are `blocked: Some(Blocked::Road)` and lose their zone; cells beyond a road's end are no
  longer zoneable (they never fronted a road).

### 2026-11-25 (third push: traffic, organic town, car bodies)

Additions:
- `scenarios::organic(&mut World) -> Vec<(Curve, CommandError)>`: the free-form demo town (curved avenue, a grid
  turned by 17°, a ring, a cul-de-sac estate, two works quarters), laid out with `BuildCurve` / `ZoneArea` only; call
  it inside `scenarios::prebuilt` (it costs more than a new game's money). It grows past 3,000 residents in about 12
  days and reaches about 3,300–3,500 by day 25. Also `scenarios::curve_road`, `zone_quad`, `first_hit`.
- `junction::Movement::preferred`: turn paths now exist from every in-lane; `preferred` marks the lanes drivers
  should use (kerb lane right, inner lane left, any lane straight). Draw only preferred ones if you show turn arrows.
- `balance::CAR_BODY_M` = 4.4 × 1.9 m and `balance::TRUCK_BODY_M` = 5.3 × 2.2 m (length, width): draw vehicles within
  these (queues keep 5.5 m between centres, so a longer truck would overlap the car behind).
- `traffic::car_bodies(&World)` and `traffic::overlapping_bodies(..)`: vehicle rectangles at their drawing poses.
- `TrafficState::stopped_share`: share of cars on the road standing (at a light, in a queue, giving way).
- Debug (not saved): `traffic::debug_node_denials`, `debug_gave_up`, `debug_gave_up_reasons`,
  `debug_last_block_sync_ns`.

Changed meaning:
- `Stats::avg_speed` / `TrafficState::avg_speed`: mean speed of the cars that are moving (was: of all cars, standing
  ones included, which read 5–10 km/h at rush hour because most cars wait at lights). Show `stopped_share` beside it
  if you want the waiting picture.
- Nodes with fewer than three arms are never signalled (`junction_control` is `Priority`, no lights to draw), even
  when busy. A node where the road kind changes (highway → avenue, avenue → street) has a short merge box like a
  lane-count change, so `junction_shape` returns a shape there.

### 2026-11-25 (final)

- Give-up rule: a car waiting at a red light needs twice `STUCK_REMOVE_AFTER_S` (120 s, longer than any signal
  cycle) before it gives up; other waiting counts as before.
- Departing cars are checked for a clear driveway where they really start (outside the junction box).
- `zoning::frontage` stops at bends sharper than 45° (not only at junctions and dead ends).
- Growth picks lots from candidate lists that change only with zoning or roads (no visible API change).
- Measured (release, this machine): zone-block regeneration after one road edit 0.35–0.47 ms in a 10,000-resident
  city; 20,000 citizens with 2,830 cars at the rush peak: mean step 1.35 ms, max 3.9 ms (r4 before: 1.25 ms, 3.8 ms);
  10,000-resident hierarchy city: no step over 5 ms in the spikes run.

## One model: roads and people (r4-sim, 2026-11-26)

`r4-people` (citizens bound to their cars, the 16-minute day, money sinks, the extended `inspect` API) is merged onto
the free-form roads. What a renderer or the interface needs from the combination:

- **Time.** A day lasts 16 real minutes at 1x: `balance::DAY_MINUTES = 16`, `TICKS_PER_DAY = 19_200`,
  `TICKS_PER_HOUR = 800` (40 real seconds), new games start at 09:00 (`START_TICK_OF_DAY`). Cars still move in real
  time (1 tick = 1/20 s), so a 40–90 s drive is 1–2¼ game hours. Speeds to offer: 1x, 3x, 6x, 16x (16 steps per
  frame at 16x; see the measurements below). One day is one year of age (`DAYS_PER_YEAR = 1`).
- **People in cars.** Every car trip is a car on the road while the vehicle budget allows
  (`traffic::vehicle_budget`: 125 per 1,000 residents, 150–2,500); beyond it, and for walks of up to
  `WALK_MAX_M` (100 m), trips happen off screen. A citizen is in their car from departure to arrival
  (`Location::Travelling(Journey)`, `Journey::vehicle`); `Trip`/`Departure` name their buildings (`from_building`,
  `to_building`), carry `party` (people in the car) and `cargo` (trucks); a household moving out drives out.
  `TravelMode::Walking` is new. Trucks are `TripPurpose::Goods`.
- **Traffic on every commute.** `TrafficState::delay_factor` (`traffic::delay_factor(world)`, at least 1): a moving
  average of commuting car trips' door-to-door time over their free-flow estimate. Commutes made off screen take that
  long too, so a jammed town makes every commuter late, not only the drivers you see. `avg_commute_s` stays the
  average of commutes driven on the road (residential demand follows it).
- **Money.** New sinks (people side): avenue upkeep 10 per cell per day, `$100` per signalled junction per day, city
  services growing with size and density; the breakdown is `economy::running_costs(world)` /
  `economy.last_day_costs` (`RunningCosts`). New (r4-sim): `Economy::attendance` (`Attendance { scheduled_ticks,
  worked_ticks, last_day_share }`, `share()`): commercial and industrial tax is paid on the share of scheduled shift
  time workers actually got in (late arrivals work less).
- **Saves.** Format 6 holds both sides' data. Formats 4 and 3 load (decoded through the frozen world-grid types in
  `save::v5`; their clocks keep the time of day on the longer day). Format 5 (the unreleased first citizens layout)
  is refused (`LoadError::UnsupportedVersion(5)`); `save::v5::save_as_v5` and `WorldV5` are gone. Format-6 files
  written before 2026-11-26 do not load (new fields in `TrafficState` and `Economy`; nothing was released).
- **Read API** (people side, unchanged here): `inspect::{plan, happiness_reasons, inside, citizen_pos}`,
  `CitizenView::{money, happiness}`, `TripView::expected_tick`, `VehicleView::{party, cargo, requested_tick,
  started_tick, on_road_s}`; `traffic::has_room`.

### 2026-11-26 (r4-sim: what changed in the combination)

Junctions and driving:
- **Stop lines.** A waiting car stands with its *front bumper* `STOP_LINE_M` (now 0.5 m) behind the line; its centre
  is `drive::stop_back_m(truck)` = half its body (`CAR_BODY_M` / `TRUCK_BODY_M`) + `STOP_LINE_M` before the line (was:
  centre 0.6 m before, nose in the box). Renderers need nothing new: draw cars where `vehicle_pose` puts them.
- **Committed entries.** A car let into a junction keeps its turn over cars of the same class until it is in; before,
  two conflicting cars could swap places tick after tick and both crawl at walking pace.
- **Turning lanes keep their order.** On multi-lane approaches lane `l` turns into lane `l` (double left and right
  turns): `Movement::to_lane` changed for left turns from the outer lane and right turns from the inner lane;
  `preferred` is unchanged (lefts from the inner lane, rights from the kerb lane). Two movements from one arm to
  another in their own lanes, side by side, never conflict (the highway's two lanes into town used to block each
  other through the merge box).
- **Lane changes** see cars that just entered a box as still in the lane they left; lane choices are made every
  other tick.
- **Signals.** A car whose exit lane is jammed (its last car standing with no room behind it) does not call for green.
  Two avenues crossing are served alike (both through phases up to `SIGNAL_MAIN_MAX_GREEN_S`, no coordination);
  avenue coordination (green waves) covers every junction where an avenue pair meets lesser roads.
- **Auto control.** Streets crossing an avenue give way to it (avenue arms `major`) until the junction is busy (was:
  always lights). "Busy" counts the cars let in during the busiest minute *plus the cars still queued* at its end (a
  jammed all-way stop let in fewer cars than the threshold however long its queues).
- **Departures.** A car does not pull out into a standing queue (`SPAWN_QUEUE_M`, 25 m ahead in its lane); new cars
  never start with their nose over a stop line.
- **All-way stops.** A car let go pulls up to the line and in without stopping again (it waits a little back from
  the line now); a car let go last tick keeps its turn over everyone at any junction.

People:
- Off-screen commutes use the delay factor (`day::road_s`); other off-screen trips the free-flow estimate.
- Newcomers arrive by day (`MOVE_IN_DAY_H` 06–22, `MOVE_IN_NIGHT_SHARE` 0.2 of the mean rate at night; the same
  number per day).

Scripted cities: `scenarios::upgrade_to_hierarchy(world)` upgrades a streets grid's main streets (the lines
`GridPlayer::hierarchy` builds as avenues) to avenues; the headless example has a `rescue` scenario using it.
`GridPlayer::layout` and `scenarios::Layout` are public.

Tools: `examples/simint.rs` (build a city to a population and save it; replay a day from a save and see where trip
time goes, the junctions where cars stand and give up; `node`, `cross`, `flow` junction benches), `topdown save` /
`savewatch` (views of a saved city), `headless --seeds` writes the day's give-ups, trips, happiness, commute, step time,
delay factor and attendance; `plot_sweep.py` charts seed sweeps.

Measured (release, this machine, 10 seeds per scripted city; charts `sim-05`..`sim-08` in the shots folder):
- Car-body overlaps over two rush hours: 10,000-resident avenue city 168 pair-ticks before, **0** after; organic
  town 16 before, 8 after (at most one pair at a time, all at degree-2 bend nodes with a sharp kink).
- Step time, 20,000 citizens with the car budget full, one day: mean 0.97 ms / median 0.96 / p99 1.30 / max 18 ms
  before, **0.84–0.86 ms / 0.83–0.85 / 1.19–1.24 / 2.2–2.3 ms** after; 16 steps (a frame at 16x): mean 15.5 ms
  before, **13.4–13.7 ms** after, p99 16.7–16.9 ms, max 17–25 ms.
- Cars given up at day 15 (share of car trips): avenue city 7.8 % before, 6.4 % after; streets city 9.3 % / 8.3 %;
  a good small town (`two_grid`) day 2 ≈ 0.6 %, both. Residents at day 15: avenue city 11,200 → 12,100, streets
  city 10,200 → 11,300; the single-artery town still stalls (3,400 → 3,900); the organic town reaches 4,900 by day
  7; the overbuilt network goes broke.
- Rescue (`rescue`): upgrading the streets city's main streets on day 8 halves its give-ups the next day (5.3 % →
  2.2 %) and keeps them below the streets city's (5.2 % vs 8.3 % at day 15); residents 12,200 vs 11,300 and taxes
  $98k vs $91k a day at day 15. Its commute improves only slightly, and the streets city stays far richer ($80k vs
  $19k at day 15): the avenue hierarchy is not yet clearly better on commutes (see the r4-sim report).

## 2026-11-26 (r4-traffic: junction capacity, car demand, layouts that matter)

Goal: the road layout decides the outcome. Junction capacity was measured in isolation, fixed, and car demand set
from it. Charts `traffic-02` (junction bench before/after), `traffic-03` (10 seeds of the avenue, streets and
single-artery cities before/after), `traffic-04` (the two rescues), `traffic-05`/`06` (evening rush pictures),
`traffic.mp4` (side-by-side evening rush) in the shots folder.

### Junction bench (`citysim_sim::capacity`, `examples/jbench.rs`)

- `capacity::run(&Bench, seed, warmup_s, measure_s) -> BenchResult`: one junction at cell (128, 64) on an empty map,
  approaches saturated (or a rate), cars per minute through it by arm, lane and turn, cars per second per lane on
  green. `Bench::crossing(main, cross, setting)`, `.turning(left, right)`, `.tee()`, `.loads(..)`;
  `capacity::build(&Bench)` gives the bench world. Real trips, nothing special-cased.
- Measured (cars per minute, all approaches saturated; before -> after): all-way stop of streets 41 (two arms
  starved) -> 31 (fair, every arm alike), with 30 % turning 29 -> 25; signalised street x street 57 -> 56;
  avenue x street 91 -> 128; avenue x avenue 100 -> 145 (with 20 % turning 50 -> 63, 30 % 44 -> 62); an avenue
  through a street that gives way 117 cars per minute. Per lane on green: street 0.49 cars/s, avenue 0.63
  (before 0.43). Avenue x avenue crossings carry 3.4x a street crossing straight through, 1.9x with 20 % turning.

### Rules changed (what renderers / the interface may notice)

- **Car following.** Time headway 1.1 s on avenues and the highway (`FOLLOW_HEADWAY_S`, was 0.5 s), 1.5 s on streets
  (`STREET_FOLLOW_HEADWAY_S`, new): queues discharge at about 0.6 / 0.45 cars per second per lane; moving cars keep
  visibly more distance. Standing queues are unchanged (`MIN_GAP_M`).
- **Room beyond the box.** A car may enter behind a moving out-lane if the room its last car frees in
  `EXIT_LOOKAHEAD_S` suffices and the out-lane still has storage for `EXIT_STORAGE_MARGIN_CARS` more (a discharging
  platoon keeps flowing; a short block that could fill up needs the room now).
- **Conflicts clear by distance.** `JunctionState::blocks(o, dist, m)`: a car on movement `o`, `dist` metres in,
  blocks `m` only until it is past where their paths come within `MOVEMENT_CLEARANCE_M` (`JunctionState::clear_after`,
  a cache rebuilt when empty, not saved). Two cars on conflicting movements may now share a box, never while both
  are still in each other's way.
- **Arrival times** count acceleration (a car pulling away from a queue arrives sooner than its speed says): signals
  no longer end a green on a discharging queue, and precedence at the line is right for starting cars.
- **Signals.** Actuated on the passage time: a green is held only for cars within `SIGNAL_PASSAGE_S` (2.5 s) of the
  line; cars within `SIGNAL_DETECT_M` call their phase. Minimum green 4 s (was 5), main / side maximum 20 / 15 s (was
  30 / 20). Protected left phases are off (`SIGNAL_LEFT_PHASES = false`; when on, a left phase is called only by a
  left turner stuck at the line): every plan has one phase per opposite pair, left turns are permissive and a left
  turner waiting at the line clears out on yellow. `ApproachView::left_protected` is therefore always false and
  `left_aspect` follows the main aspect.
- **Precedence.** A car of lower precedence never slips in ahead of a conflicting car with precedence that waits at
  its line only for the box to clear (a chain of permissive left turns starved the oncoming stream). Two cars both let
  go keep a stable order (the nearer one first).
- **All-way stops** are first come, first served: a car stops for `STOP_DWELL_S` (1 s), the one that has stood
  longest goes, nobody jumps a conflicting car's turn, and the next one sets off `ALL_WAY_REACTION_S` (1.2 s) after
  the box empties. (Two arms of a crossing used to be starved.)
- **Lane choice.** Cars move into their turning lane up to `LANE_PREPARE_M` (150 m, was 90) before the line; straight
  cars avoid a lane whose next car turns left; a slow car in the wrong lane for its turn asks the car behind in the
  lane it needs to leave a gap (zip merging: `MERGE_ASK_SPEED`, `MERGE_ROOM_M`, `MERGE_MIN_LINE_M`).
- **Auto control.** All-way junctions (equal roads) get signals once their busiest minute (let in plus still queued)
  reaches `AUTO_SIGNAL_PER_MINUTE` = 30 (was 16; back below 18), about what an all-way stop passes. Junctions with a
  major road (an avenue through streets, a T) are judged by their *minor* approaches only
  (`AUTO_SIGNAL_MINOR_PER_MINUTE` 30, back below 18): an avenue no longer gets a signal at every street because of its
  own traffic. New counters `JunctionState::minor_minute`, `minor_peak_today`, `minor_peak_yesterday` (saved in the
  extension block, see Saves).
- **Routing** charges turns (`Derived::route_turns`, `drive::build_route_turns`): a left turn per oncoming lane it
  must find gaps in (`ROUTE_LEFT_PER_LANE_S`), crossing or turning out of a minor road per major lane, a right turn
  `ROUTE_RIGHT_S`. Drivers prefer to turn where it is easy.
- **Parking** is modelled but off (`PARK_DWELL_S = 0`): a car standing at the kerb on arrival blocked the lane
  behind it more than it showed anything.

### Car demand (`traffic::car_share`, `traffic::truck_share`)

- Below `CAR_SHARE_FROM_POP` (1,500 residents) every car trip is a car on the road, as before (small towns keep
  their cars per 1,000 residents). Above it the share of citizens' car trips that are real cars falls to
  `CAR_SHARE_LARGE` (0.4) at `CAR_SHARE_TO_POP` (5,000). Which citizens drive on screen is fixed per citizen
  (`traffic::drawn_driver`): the same person always does. The share is fixed whatever the traffic: congestion is not
  hidden, the network carries this share and the layout decides how well. The other trips happen off screen and
  take the delay the drawn cars measure (`delay_factor`), as before.
- Newcomers moving in and households moving out follow the same share (an undrawn move-in drives in off screen in
  `road_s` time). Goods trucks: `TRUCK_SHARE` (0.5) of the car share; an undrawn delivery arrives at once.
- `vehicle_budget` is unchanged and now rarely binds.

### Consequences of traffic

- `delay_factor` (commutes over free-flow time) above `DELAY_OK_FACTOR` (2.5) lowers residential demand by
  `DELAY_DEMAND_PER_UNIT` (0.5) per unit, and a worker's commute happiness is the worse of the old absolute measure
  and a linear fall from 2.5x to `DELAY_HOPELESS_FACTOR` (5x). `COMMUTE_OK_S` / `HOPELESS_COMMUTE_S` are unchanged
  (the interface uses them).
- An empty city forgets its delay factor (like its average commute).

### Scenarios

- `scenarios::add_gateway_bypass(world, kind) -> (roads built, money spent)`: the second fix for a jammed grid city,
  two relief roads from the highway stub at (5, 128) into the grid at (20, 118) and (20, 138), around its worst
  junction (the first crossing behind the gateway). `headless` scenario `bypass` (streets city, fix on day 8).
- `GridPlayer` keeps a reserve of $5,000 beyond the price of its next block (`PLAYER_RESERVE`).

### Saves

- Extension block version 2 (`ExtensionV2`: version 1 plus the junctions' minor-road counters); version-1 blocks and
  saves without a block still load. The body is unchanged (format 6).

### Tools

`examples/jbench.rs` (junction bench, CSV), `sigtrace.rs` (a bench signal second by second), `lanes.rs` (entries by
approach, lane and turn at one junction of a save; `LANES_USE`, `LANES_TRACE`), `follow.rs` (follow cars through a
save: time and standing per segment, detour factor), `simint` gains `controls <save>`, `bypass <save> <out>` and a
breakdown by junction type of standing time and refusals; `topdown rush` takes `CITYSIM_TOPDOWN_LAYOUT=streets`,
`CITYSIM_TOPDOWN_RED`, `CITYSIM_TOPDOWN_VIEW`; `overlap` takes `streets`.

### Measured (release, this machine; 10 seeds per city unless noted)

Commute over free-flow time (`delay_factor`) · car trips given up · residents · taxes per day:

| | ~9,000–10,000 residents | ~12,000 | day 15 |
|---|---|---|---|
| Avenue hierarchy, before | 4.6x · 4.3 % · 9,700 (day 10) | 4.8x · 5.2 % · 10,800 (day 12) | 5.0x · 6.5 % · 12,200 · $95k |
| Avenue hierarchy, after | 2.2x · 0.5 % · 9,600 (day 8) | 2.6x · 1.2 % · 12,000 (day 10) | 3.4x · 2.2 % · 16,800 · $144k |
| Streets only, before | 4.7x · 7.4 % · 9,200 (day 10) | 4.9x · 8.4 % · 10,100 (day 12) | 4.9x · 8.3 % · 11,300 · $91k |
| Streets only, after | 2.4x · 0.8 % · 8,700 (day 8) | 3.1x · 1.7 % · 10,700 (day 10) | 3.6x · 4.0 % · 14,700 · $123k |
| Streets, main streets upgraded on day 8 | 1.5x · 0.06 % · 8,900 (day 8) | 2.0x · 0.4 % · 11,500 (day 10) | 3.1x · 1.6 % · 17,500 · $148k |
| Streets, relief roads at the gateway on day 8 | 2.2x · 0.2 % · 8,700 (day 8) | 3.2x · 0.9 % · 10,800 (day 10) | 3.7x · 2.5 % · 15,200 · $128k |
| Single artery | stalls at 2,200 residents, 4.2x, 7 % given up | | |

- The avenue city now beats the streets city on every traffic measure at every size, grows 14 % larger by day 15,
  and earns 16 % more tax a day; the rescue (upgrade) drops commutes from 2.0x to 1.5x the same day and keeps
  give-ups down; relief roads halve give-ups but barely move commutes. Money at day 15: avenue city $85k, streets
  city $165k, upgraded $75k (after paying ~$35k), relief roads $163k: a bigger city costs more in services, so net
  income per day is about equal and the streets city still banks more (see the report).
- Junction bench, step time and overlaps: see above; step time (20,000+ citizens, car budget topped up to 2,500)
  mean 0.78 ms (before 0.87 on the same machine), p99 1.30 ms, max 3.1 ms; 16 steps 12.5 ms (13.9). Overlapping car
  bodies over the 07:00–09:00 rush: 0 in the 10,000-resident avenue and streets cities, 2 pair-ticks (one pair at most,
  0.24 m) in the organic town (8 before).
- Other scenarios (3 seeds): grid player 5,000 residents on day 5 (the balance test's player: 28.6 real minutes at
  2.5x), $30–36k in hand;
  two-district grid town 2,200 / 3,400 residents on days 2 / 3 with 1.4 % / 3.3 % of trips given up (before 0.7 % /
  2.6 %); one-junction town 1,600–2,000 residents, 20–24 % given up; fixed on day 2 2,800; organic town 8,700–9,300
  residents on day 7 with 2–5 % given up (before 4,700, 4 %); overbuilt mesh broke (-$74k).
