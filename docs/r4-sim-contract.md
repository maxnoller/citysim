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
  (before 0.43). Avenue x avenue crossings carry 2.6x a street crossing straight through, 1.9x with 20 % turning.

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
- Cars on the road per 1,000 residents (hourly mean, before -> after): 500–1,500 residents: streets town 49.6 -> 49.2,
  avenue town 32.5 -> 26.0 (every car trip is still a car there; trips are faster, so fewer are on the road at once);
  4,000–6,000: 59–75 -> 17–23; 9,000–11,000: 89–97 -> 23–31 (the car share; a 12,000-resident city still shows
  500–700 cars in its evening rush).

## 2026-11-26 (r5-sim: good roads pay, turn pockets, the day, people and jobs)

Goal: a road hierarchy pays for itself, congestion costs a city visibly, rush hours line up with the day, jammed
residents are unhappy and say so, and the demand bars tell the player what work land is missing. (The charts `sim-01`..`sim-08` and the
run data behind these figures are kept outside the repository.)

### Turn pockets (renderers: read this)

- **Avenue cross-section changed.** Still 13 m (`half_width_m` 6.5), now two 2.75 m lanes each way either side of
  a 2 m median: `RoadKind::Avenue.lane_offset_m(0)` = 5.125 (was 4.75), `lane_offset_m(1)` = 2.375 (was 1.75).
  New `RoadKind::median_half_width_m()` (avenue 1.0, else 0) and `RoadKind::pocket_lane()` (`Some(2)` for avenues).
  `lane_offset_m(2)` on an avenue is the pocket lane, `balance::POCKET_OFFSET_M` (0.0, the median's centre).
- **`junction::turn_pocket(world, segment, forward) -> Option<TurnPocket>`**: the left-turn pocket at the end of a
  segment for travel `a -> b` (`forward`) or `b -> a`. `TurnPocket { node, segment, forward, lane, start_m, stop_m,
  offset_m, half_width_m }`; `start_m..stop_m` are metres along travel (`stop_m` is the stop line),
  `TurnPocket::from_a(length)` gives the same range from the `a` end. Draw the median only between pockets (gfx:
  `SegLayout::pockets`, `median_span()`), the pocket's edges, a left-only arrow and a stop bar. Pockets depend on
  the road graph and stop lines only (rebuild with the roads).
- Which approaches get one: every avenue approach into a junction of three or more arms with a left exit, if the
  block leaves at least `POCKET_MIN_M` (11 m); at most `POCKET_MAX_M` (40 m). Two pockets of one segment share it
  (`POCKET_GAP_M` of median between them); when both do not fit, the end with more arterial arms gets one
  (`junction::pocket_lengths`).
- Vehicles: lane index 2 (`Vehicle::lane`) is the pocket; `drive::vehicle_pose` and `lane_offset` already place
  cars there (gfx's `vehicle_offset` works unchanged). Lane slots per direction are 3.
- Driving: left turners (and U-turns) move into the pocket from the inner lane once past `start_m`; cars bound for
  a pocket move over from twice `LANE_PREPARE_M`, and nobody overtakes or keeps right out of the lanes the next
  turn needs within that distance. A left turner that finds the pocket full waits in the inner lane at its entry
  (`POCKET_ENTRY_M` in): queues spill back. Movements from the pocket exist for left turns and U-turns
  (`Movement::from_lane == lanes_in`); the inner lane keeps its (fallback) left turn.
- Signals: a junction whose approaches have pockets gets a protected left phase per opposite pair before the
  through phase, called only once `POCKET_CALL_CARS` (3) stand in a pocket (fewer go permissively and clear out at
  the end of green), up to `POCKET_LEFT_MAX_GREEN_S` (14 s); its through phases may run to `POCKET_MAIN_MAX_GREEN_S`
  (30 s). The other road's right turns run beside a left-arrow phase (`RIGHT_TURN_OVERLAP`);
  `ApproachView::aspect` follows straight-on movements only, so the main head stays red then.
- Conflicts: movements from neighbouring lanes of one approach, both preferred, conflict only when their paths come
  within `NEIGHBOUR_CLEARANCE_M` (2.2 m).
- Routing: a left turn from a pocket costs `ROUTE_POCKET_LEFT_SHARE` of the usual.
- Saves: junctions saved without pocket lanes get today's geometry on load (`traffic::upgrade_junction_geometry`).

Bench (cars per minute, all approaches saturated, before -> after): avenue x avenue straight 145 -> 158, 20 %
turning 63 -> 116, 30 % turning 62 -> 103, 15 % left 61 -> 103; avenue x street 30 % turning 49 -> 67; street x
street signal 56 -> 46 (streets are slower now, below), with 20 % turning 33 -> 27. An avenue crossing with 20 %
turning carries 4.4 times a street crossing.

### Other road and driving changes

- Residential streets: `STREET_SPEED_LIMIT` 13.9 -> 11.1 m/s (40 km/h), `STREET_FOLLOW_HEADWAY_S` 1.5 -> 2.0 s.
- A path through a node without a box (a smooth join of two curves) is a 0.5 m straight hop; cars on it were drawn
  with an arbitrary heading. Two pieces meeting at a kink (more than ~1.8°, `BEND_KINK_COS`) get a bend box sized to
  it, so `junction_shape` returns a shape there.
- The scripted `GridPlayer` lays the `x = 20` spine to every block away from the trunk (a block could be
  unreachable until the one between was built, and a town could stall).

### Money

- **Traffic losses.** Every commute's time beyond `TRAFFIC_FREE_FACTOR` (1.6) times its free-flow estimate is
  business output lost at `TRAFFIC_OUTPUT_PER_S` ($0.9 per vehicle second); the tax on it is not paid, up to
  `TRAFFIC_LOSS_MAX_SHARE` (70 %) of the day's business tax. The commercial and industrial tax lines of `Budget`
  are net of it; the loss itself: `Economy::last_day_traffic` / `traffic_today` (`TrafficLoss { commutes, lost_s,
  business_tax }`, `per_commute_s()`), today so far `economy::traffic_loss_now(world)`. The interface should show
  "Business tax lost to traffic" beside the tax lines (the `Budget` struct is unchanged).
- Services: base `SERVICES_COST_PER_RESIDENT_PER_DAY` 3.0 -> 3.6, size term 3.0 at 6,000 -> 2.2 at 9,000
  (`SERVICES_CITY_SIZE_*`): a well-run bigger city nets more than a smaller one.
- Avenues `AVENUE_COST_PER_CELL` 140 -> 90, `AVENUE_UPKEEP_PER_CELL_PER_DAY` 10 -> 8; `SIGNAL_UPKEEP_PER_DAY`
  100 -> 80.
- Saves: extension block version 3 (`ExtensionV3`: version 2 plus the traffic losses, the journey times and the
  commute moves below). Older blocks load.

### The day

- Shifts start later (`INDUSTRY_START_H` 6.5-8.5, `OFFICE_START_H` 8.5-9.5, `SHOP_START_H` 8-9.5,
  `LATE_SHIFT_OFFSET_H` 4.5); shops close at 19:30 (`SHOP_CLOSE_H`), nobody sets off for a night out after 19:30
  (`LEISURE_CLOSE_H`), venues close at 21:30 (`LEISURE_LAST_H`), nights out 1.25 + up to 1 h.
- Off-screen trips take at most `OFFSCREEN_MAX_TRIP_H` (2.5 in-game hours) while traffic is fine, longer in
  proportion to `delay_factor / DELAY_OK_FACTOR` (`day::offscreen_s`).
- New statistics: `People::trip_times_today` / `trip_times_yesterday` (`TripTimes`: journeys door to door by
  duration bins `BINS_H`, commutes apart, the longest with its purpose, journeys over 4 h by purpose and on / off
  screen; `share_over(h)`).

### People

- Commute happiness is each worker's own last commute over its free-flow estimate plus
  `COMMUTE_RATIO_SLACK_S` (15 s): full up to `COMMUTE_RATIO_OK` (1.3), nothing at `COMMUTE_RATIO_BAD` (3.0), or
  the old absolute measure if worse (`life::commute_ratio(world, citizen)`). `HAPPINESS_WEIGHTS` [0.22, 0.30, 0.16,
  0.12, 0.08, 0.08, 0.04] (the commute weighs as much as the job).
- `inspect::happiness_reasons` says "Stuck in traffic every day" when a long commute is slow for its distance (the
  roads) and "Long commute" when it is long because of the distance.
- A household whose workers all have commute happiness below `COMMUTE_FED_UP_BELOW` at midnight is fed up: with
  `COMMUTE_FED_UP_CHANCE` (0.2) a night it moves to a free home at most `COMMUTE_MOVE_NEARER` (0.7) times as far
  from the first worker's job, or else leaves. Counted in `People::commute_moves_today` / `_yesterday`
  (`CommuteMoves { moved_nearer, left }`; also in `moved_home` / `households_left`).

### Jobs and demand

- `demand::jobs_view(world) -> JobsView { workforce, unemployed, vacancies, vacancies_reachable, free_cells,
  short_of: Option<Zone>, reason: JoblessReason }`, `rate()`; `JoblessReason::{None, Searching, NoJobsWithinReach,
  NoLandZoned, BuildingsComing}` with `text()`. O(buildings + zone cells): every few frames at most.
- Unemployment pushes job demand harder (`UNEMPLOYMENT_JOB_BOOST` 1.5 -> 4.5), a quarter to each kind of work and
  half by their shortfall: a town short of industrial land shows industry near 1.0.
- A jobless citizen's employment reason names the cause ("Out of work: not enough jobs in the city", "... no jobs
  within reach", "Out of work until new workplaces open").
- `scenarios::short_of_industry` / `add_industry_land` (headless `short_industry`, `short_industry_fixed`):
  9-10 % jobless with industrial demand 0.97-1.0 and no industrial land free; zoned on day 3, nearly nobody is out
  of work by the next morning.

### Measured (release, 10 seeds per city; data in `sim-data/after`, `before`, `before2`)

Day 15 (money · net per day · business tax lost to traffic per day): avenue city $77k · $12.7k · $18.3k (before
$85k · $24.6k); streets city $65k · $4.5k · $27.1k (before $165k · $24.3k); streets with main streets upgraded on
day 8 $96k · $15.8k · $14.4k (before $75k); streets with relief roads $68k · $4.8k · $26.8k (before $163k). At 10,000
residents the avenue city holds $38-44k, the streets city about $55k; the avenue player's longest wait for money is
4-6 real minutes at 2.5x (before 5-6).

Commute over free flow · car trips given up, by size (10 seeds): avenue city 7-9k 1.9x · 0.43 %, 9-11k 2.2x ·
0.71 % (before 2.1x · 0.46 %, 2.3x · 0.60 %); streets city 5-7k 2.5x, 7-9k 2.8x · 0.94 %, 9-11k 3.3x · 2.1 %,
13-16k 3.6x · 3.9 % (before 1.9x, 2.2x · 0.48 %, 3.1x · 1.5 %, 3.5x · 3.5 %); upgraded 9-11k 1.9x · 0.45 %;
relief roads cut give-ups by a quarter to a third (2.1 -> 1.5 %, 3.9 -> 2.9 %) but not commutes. Targets missed:
avenue city at most ~2x with well under 0.5 % (2.2x, 0.7 % at 10,000); streets city 3-4x from 6,000-8,000 (2.5-2.8x).

Cars on the road per 1,000 residents, days 8-10 (avenue city / streets city): morning peak 06:00-08:00, evening
peak 18:00-19:00 at 60 / 65 (before 17:00-18:00, 50 / 57); 22:00 27 / 43 (before 19 / 31); 23:00 13 / 27 (14 /
22); 02:00 1.5 / 5 (3.4 / 7); noon 11 / 15. Journeys: in a sensible player's town at day 4, under 0.1 % of
commutes take over 8 in-game hours and about 2 % over 4 (the longest journeys are newcomers queuing at the highway
gateway).

Car bodies: no overlaps in the organic town's morning and evening rush (was 4 and 31 pair-ticks) nor in the
10,000-resident avenue and streets cities. Step time (20,000 citizens, 2,290 cars at the peak): no change against
the previous build measured back to back on the same (heavily loaded) machine, mean 2.05 -> 1.99 ms, p99 8.4 ->
7.5 ms; the absolute figure needs a quiet machine.

### Tools

`examples/pocketdiag.rs` (an avenue crossing on the bench second by second, what each lane's front car does on
green, `CONFLICTS=1`), `happy.rs` (happiness by part and reason in a saved city), `capacity::run_traced`,
`traffic::debug_last_denial`, `overlap` with `OVERLAP_DETAIL=1`, `topdown` draws medians and pockets and `day`
takes `CITYSIM_TOPDOWN_LAYOUT=hierarchy`, `headless` writes journey times, commute happiness and traffic losses per
day and has the `short_industry` scenarios.

## 2026-11-26 (r5-zoning: zoning that follows curves, blocks filled edge to edge)

The zoning model keeps its parts (lattices, blocks, cells, lots in one lattice); three things change.

### Strips: lattices bent along curved roads

- Every side of a **curved** segment gets its own lattice, a **strip** (`Lattice::strip: Option<Strip>`, check with
  `Lattice::is_strip()`). `Strip { i0, frames: Vec<(point, dir)>, lo, hi }`: column boundary `i0 + k` is the half-line
  from `frames[k].0` (on the kerb, 4 m from the centre line) along the unit normal `frames[k].1` (`+j`). Cell `(i, j)`
  is the quadrilateral between boundaries `i` and `i + 1`, `j..j+1` cell sizes out along them. Neighbouring cells
  share their edges exactly: a row follows the kerb with no gaps and no overlaps, narrower on the inside of a bend and
  wider outside. Columns are fitted to the segment (about 8 m along the middle of the first row); a curve continuing
  a straight road smoothly starts its first column where the straight road's block ends.
- Rows stop where a cell would get narrower than 4.5 m (inside of a tight bend) or wider than 12.5 m (outside): a
  street of 24 m radius keeps one row inside, rings of 70 m radius three or four.
- **Use the lattice's own geometry, never its `u` / `origin`, for strips**: `world_at`, `local`, `cell_of`,
  `cell_center`, `cell_corners` work for both kinds. New: `Lattice::axes_at(x) -> (u, v)` and `dir_at(x, facing)`
  (directions at lattice coordinate `x`; constant on a straight lattice), `Lattice::bent(Strip)`. `Lattice::dir(f)` on
  a strip answers for its first column. `local` far from a strip returns a point no cell is at.
- `Zoning::block_outline(&Block)` / `rect_outline(lattice, min, max)` (polyline outline; a strip's has every column
  boundary), `Zoning::lot_quad(&Footprint)` (the quad a lot covers: front-left, front-right, back-right, back-left; on
  a strip it narrows or widens). `block_corners` still exists (only the four corners on a strip).
- Straight roads keep straight lattices and share them as before, but only with a lattice they line up with at
  **both ends** (`LATTICE_ANGLE_TOL_DEG` is 0.2, was 1.0): a road half a degree off a long lattice no longer joins it
  and drifts across its rows. A right-angle grid on the cell lattice is still lattice 0, cell for cell.
- A split curve keeps its strips (the halves find the strip their kerb lies on), so zoning and buildings along it
  survive a T junction.

### Deeper blocks and who keeps overlapping land

- `balance::BLOCK_DEPTH` is **6** (was 2; `MAX_BLOCK_DEPTH` 6): land up to 48 m from a road can be zoned, so streets
  up to about 100 m apart can be zoned all the way in.
- Where cells of two lattices overlap, **the cell fewer rows from its own road keeps the land, then the older road**
  (was: the older road). Front rows always beat deep rows, so a new road never leaves a band of unzoneable land along
  itself, and rows of different roads meet along a tidy line (a diagonal at corners, a line between parallel roads).
- Land taken over that way keeps its zone: a newly usable cell takes the zone of the other lattice's cell under its
  middle (a contested one keeps its zone too, for when it is usable again).

### Lots with yards

- A lot is one or two cells wide and **as deep as the zoned free rows behind it allow** (up to the block depth), but
  never into rows nearer another road (those are left to that road's lots). `Footprint::size` is `[width, depth]`
  with depth up to 6; the building stands on the front `balance::BUILT_DEPTH` (2) rows, the rest is its yard.
  `Footprint::built_area()` (capacity is counted on it: unchanged per metre of street), `yard_depth()`.
- Renderers: draw the building on the front two rows of the lot (gfx: `buildings::lot_of`), inside `lot_quad` on a
  strip; the yard behind is grass.

### Zone tool queries

- `zoning::road_walk(world, (segment, m), (segment, m), max_m) -> Option<Vec<RoadPiece>>`: the shortest walk along the
  roads between two points (through bends, pieces and junctions). `RoadPiece { segment, forward, from_m, to_m }`.
- `zoning::frontage_walk(world, &walk, side, depth)`: cells along a walk on one side of it (side seen in the
  direction of travel), up to `depth` rows. `zoning::stretch_walk(world, segment)`: the stretch between junctions,
  dead ends or corners sharper than 45 degrees. `frontage(world, segment, side, depth)` is `frontage_walk` of it.
- `zoning::fill_area` links cells of different lattices when their outlines come within a metre, so it fills land
  enclosed by roads whatever lattices it is made of; clicked on bare land (the middle of a ring) it starts from the
  nearest land within 48 m that no road separates from the click.
- `zoning::cells_in_quad` finds strip cells however large the quad.

### Saves

- Format **7**: the body is laid out as version 6; strips travel in **extension block 4** (`ExtensionV4` = block 3 of
  r5-sim plus `(lattice id, Strip)` pairs). Version 6 loads with the same body; its blocks are made again the current
  way on load (`blocks::renew_all`): zones move onto the new cells, buildings on curves move onto the new lots under
  their front where those are free (`growth::reseat`), the rest stay where they stand. Fixture
  `tests/data/curves_v6.citysave` (a version-6 town with curves, written by the pre-r5 simulation).

### Measured (release, this machine)

- Zone-block regeneration after one road edit in the 10,000-resident hierarchy city (`examples/spikes.rs`): 0.43-0.48
  ms (was 0.22-0.34 ms with two-row blocks).
- Zoning test bed (`examples/topdown.rs zonebed`: gentle and tight curves, S-bend, ring, cul-de-sac, curve off a
  grid, curved avenue, rotated grids): 33 lattices instead of 194 for the same roads; the same zone-tool actions
  played in the game zone 2,044 cells instead of 490.

## 2026-11-27 (r6-traffic: traffic, the day and money hit their targets)

Goal: a good layout flows, a bad one hurts, the day looks alive, money rewards a growing well-run city. The charts,
pictures and run data of this work are kept outside the repository.

### Signals (renderers: aspects as before; timings changed)

- **Pressure gap-out.** A through green whose own standing queues have discharged ends (after its minimum) when the
  cars standing for another phase outnumber twice the cars within `SIGNAL_DETECT_M` of the line on its own lanes, unless
  a car near the line is still pulling away from a queue (`SIGNAL_DISCHARGE_SPEED`). A trickle of arrivals no longer
  holds a green against a standing queue (`SIGNAL_SWITCH_QUEUE_CARS`, `SIGNAL_SWITCH_RATIO`, `SIGNAL_DISCHARGED_CARS`).
  A coordinated avenue keeps its share of the cycle as before.
- **Queue order.** Signals that are not coordinated serve next the phase with the most cars standing for it (pocket
  cars count `SIGNAL_POCKET_WEIGHT` times); a phase whose front car has waited `SIGNAL_STARVED_S` goes first; roads take
  turns (after a through phase, its own road's phases wait while another road has demand); a left phase whose pocket
  holds `SIGNAL_LEFT_FIRST_CARS` runs before its own through phase (`SIGNAL_BY_QUEUE`). Two-phase plans are unchanged.
- Pocketed through phases may run `POCKET_MAIN_MAX_GREEN_S` (40 s), but only `POCKET_FULL_MAX_GREEN_S` (30 s) while one
  of their pockets is full. The all-red no longer waits for the box to empty (`SIGNAL_ALL_RED_WAITS_FOR_BOX`): cars let
  in yield to those still in it.
- `junction::ArmDemand` gained `discharging`, `near`, `pocket`, `pocket_full`, `wait_s` (crate-internal).

### Other road rules

- **Courtesy at give-ways** (`COURTESY_WAIT_S` 20 s, `COURTESY_SPEED` 4 m/s): at any priority junction a minor-road
  car that has stood at its line that long goes as if it had priority while every conflicting car with priority
  creeps (a slow queue on an avenue or a main street lets one in).
- **Residential streets** are driven with `STREET_FOLLOW_HEADWAY_S` 2.2 s (was 2.0): about 1,350 cars an hour per
  lane. (2.6 s was tried: it hardly slowed the streets-only city, but a small town of streets gave up trips at its T
  junctions, where queues backed up across them.)
- **Turn pockets** are up to `POCKET_LONG_M` (40 m) as before, and up to `POCKET_MAX_M` (60 m) only where they take at
  most half of a long block's drivable length. `junction::turn_pocket` reports the length; renderers need nothing new.
- **Routing:** the A* heuristic uses the fastest road that is not a highway, less what the highway stub can save
  (`path::Bound`); routes are unchanged, searches cheaper.

### The day (inspect panel: read this)

- **Lunch out gives way to slow roads:** the shares below are taken in full while commutes take at most
  `LUNCH_DELAY[0]` (2.0) times their free-flow time (`traffic::delay_factor`), not at all from `LUNCH_DELAY[1]` (3.0),
  linearly between (`day::lunch_room`); a lunch (a `Leisure` trip at midday, there or back) is a drawn car at most at
  the big city's share `LUNCH_CAR_SHARE` (0.4, `traffic::drawn_lunch`) whatever the town's size, the rest happen off
  screen. So a small town whose roads are already slow is not loaded further at midday.
- **Lunch breaks:** on `LUNCH_BREAK_SHARE` of days a worker leaves work at a personal time in `LUNCH_BREAK_H` for the
  nearest of a few venues within `LUNCH_MAX_TRIP_H`, stays `LUNCH_BREAK_VISIT_H` and drives back to work (trips of
  purpose `Leisure`; the shift's end and its attendance are unchanged).
- **Out for lunch:** someone at home with the day free (`MIDDAY_FREE_H`: a day off, or no job) goes out for lunch on
  `MIDDAY_OUT_SHARE` of days at a personal time in `MIDDAY_OUT_H`, to a venue nearby (`MIDDAY_DIST_SCALE_M`), for
  `MIDDAY_VISIT_H` plus up to `MIDDAY_VISIT_SPREAD_H`; children at home come along.
- **Evening:** the late shop shift starts `LATE_SHIFT_OFFSET_H` 4.0 h after the early one (after the lunch hour; 3.0 h
  put its commute into a busy small town's midday) with a narrow personal spread (`LATE_SHIFT_SPREAD_H`), ending soon
  after the shops close; nobody sets off for leisure after 19:00 (`LEISURE_CLOSE_H`), venues close at 21:00
  (`LEISURE_LAST_H`); nights out are planned to be home by `LEISURE_HOME_BY_H` (22:00) in today's traffic (the
  delay factor on the drive both ways) and end in time for it; after a day's work people go out only when their need
  is urgent (most go on days off); newcomers arrive 06:00-21:00; deliveries and exports peak between the rush hours.
  Shopping trips check the arrival time in today's traffic.
- **`inspect::TripView::reason: TripReason`** (`text()`): `ToWork`, `HomeFromWork`, `MovingIn`, `MovingOut`,
  `Groceries`, `ShoppingAfterWork`, `HomeFromShopping`, `LunchBreak`, `BackToWork`, `OutForLunch`, `HomeFromLunch`,
  `NightOut`, `HomeFromNightOut`, `Delivery`. A trip home counts as home first (someone off duty may lunch where they
  work). The same reason is on `inspect::VehicleView::reason` (judged from the owner's home and workplace) and on a
  planned trip, `inspect::NextStep::reason` (`Some` for a `Travelling` step: the way home from a lunch is
  `HomeFromLunch`, though its `purpose` says `ToHome`). The interface names every trip by its reason
  (`ui/people.rs` `reason_word`). `inspect::plan` while working shows the shift's end (a lunch break is not planned
  ahead), at lunch the next step is work, and the trip back from lunch ends in `Working`.

### Money

- Business tax is lost only on commute time beyond `TRAFFIC_FREE_FACTOR` 2.0 times free flow (was 1.6).
- Services: the city-size term is `SERVICES_CITY_SIZE_PER_RESIDENT` 1.5 (was 2.2), half of it at
  `SERVICES_CITY_SIZE_POP` 12,000 (was 9,000).
- Avenues cost `AVENUE_COST_PER_CELL` 75 a cell (was 90) and `AVENUE_UPKEEP_PER_CELL_PER_DAY` 6 (was 8).
- **Money flows through the day:** every in-game hour but midnight the city receives a 24th of yesterday's net
  (`economy::hourly`, `Economy::advanced_today`); midnight books the day's real `Budget` less what was advanced. The
  budget lines, `DaySummary` and `last_day` are unchanged; the money figure moves every in-game hour. The interface
  says so: the bar shows what each hour pays in ("+$208 / hour"), the budget page that midnight books the day and
  settles the rest.
- **`DaySummary::traffic: Option<TrafficLoss>`**: what traffic cost each booked day (`None` for days booked by saves
  written before it was kept; a version-4 block gives its last day the loss it kept). The budget page's loss chart reads
  it (`ui/city.rs` `LossHistory`).

### Saves

- Extension block **5** (`ExtensionV5`: block 4 plus the per-day traffic losses and `Economy::advanced_today`). Blocks
  1-4 load; format 7, version 6, 4 and 3 saves load as before.

### Measured (release, an 8-core cloud machine, 10 seeds per city)

As first measured (street headway 2.6 s, late shift at 3.0 h; the run data are kept outside the repository): avenue
city 9-11k 2.21x · 0.71 % -> 1.78x · 0.34 %
(1.70-2.2x across runs; the traffic rules alone at the old money: 2.07x · 0.52 %, part of the gain comes from the
scripted player building ahead now that cash flows hourly); streets city 6-8k 2.59x -> 2.53x, 7-9k 2.75x -> 2.93x
(target 3-4x missed); junction bench avenue x avenue 30 % turning 103 -> 117 cars a minute; cars per 1,000 residents
at 11:00-15:00 11 / 16 -> 24 / 32 (avenue / streets city), 22:00 27 / 43 -> 8 / 33, peaks 60 / 65 -> 50 / 59; net
income in the avenue city rises up to ~16,000 residents ($34k a day), traffic loss $12k at day 15 against a net of
$34k (streets city: $20k against $22k); the avenue player's longest wait for money 192-336 s -> 0-80 s real at 2.5x.

With the rules as they now stand (street headway 2.2 s, late shift at 4.0 h, lunch giving way to slow roads, courtesy
at every priority junction), 10 seeds: the small good town (`two_grid`, final day) 3.52x commute over free flow, 5.0 %
of trips given up, 84 cars per 1,000 at 11:00-15:00 (before this round 3.54x, 5.0 %, 72; with the first r6 rules 3.87x,
5.7 %, 101); the avenue city at 9-11k 1.48x, 0.28 % given up, 24 per 1,000 at midday and 20 at 22:00 on days 8-10; the
streets city 2.61x at 6-8k, 2.97x at 7-9k.

### Tools

`examples/dfsplit.rs` (what the delay factor is made of: waiting to pull out, the route at the speed limits, time lost
on the road; `BY_START=1` by departure hour), `sigcity.rs` (a signal of a saved city second by second, `ARM=n` front
cars), `dayprobe.rs` (who is where during the day), `midnight.rs` (the steps around midnight in a big city);
`simint day` takes `SIMINT_FROM=h`; `headless` hourly CSVs have cars on the road by purpose.

## 2026-11-27 (r6-ui: undo brings demolished buildings back)

- `World::building_record(id) -> Option<restore::BuildingRecord>`: everything needed to put a building back (the
  building, its lot's cells with their centres, its households with their members, its staff). The interface takes
  one for every building a plan will demolish, before sending it.
- `Command::RestoreBuilding(Box<BuildingRecord>)`: the same building (kind, level, footprint, facing, seed, age) under a
  new id, with its households (they had left the city) and its staff who are still jobless; members get their old jobs
  back where there is room; the demolition cost is refunded. Refused with `CommandError::CannotRestore(RestoreProblem)`:
  `LandChanged` (the land under the lot is gone or has another shape; a lattice made anew is matched by cell
  centres), `BuiltOn`, `Rezoned`, `NoRoad`. `preview` agrees with `apply`. Nothing is saved (undo is per session).
- Lots stay clear of road reserves, so a road that stood can always be put back while nothing else changed (undo
  rebuilds the roads first): lots are chosen by cell centres, and on a curved road side the straight rectangle over
  curved cells reached up to about a metre into the reserve on the inside of a bend, so the identical road was refused
  ("a building is in the way"). Growth and restore move such a footprint away from the road until it is clear
  (`growth::placed`, `build::reserve_intrusion`); rectangles on straight lattices are unchanged. Test: every road of a
  grown organic town can be removed and built again (`tests/restore.rs`); `examples/rebuildprobe.rs` counts refusals
  and overlapping buildings.

## 2026-11-27 (r6-zoning: one grid per block, greens, clean edges)

Nothing new is stored: the save format stays at 7 (and 6 loads as before). Everything below is derived from the roads
and blocks, found again on load.

### Block grids (`faces`)

- `faces` walks the faces of the road graph (the land roads enclose). A face whose sides are all **straight** roads in
  **two directions** within 30° (a square, a rectangle, a trapezoid, a block a few degrees off true, an L: sides
  alternate between the two directions) gets **one grid fitted between its kerbs**: column lines run from kerb to kerb
  across it, each stretch between two kerbs of one direction divided into whole cells (stretched or squeezed by under
  half a cell, cells stay 4-13 m a side), the rows likewise. Facing and meeting streets' rows are the same rows;
  corner cells front both roads. Faces already on whole cells at right angles keep the shared straight lattices (the
  grid town zones exactly as before); triangles, faces with curved sides and faces wider than 720 m keep per-road
  lattices.
- The grid is stored as strips (`Lattice::strip`), **one per band of rows** between two kerbs along the face's first
  direction (one band for four sides, two for an L). Use the strip's geometry as for any strip. New:
  `Zoning::is_block_grid(lattice)`, `Zoning::cell_across(cell, facing)` (the neighbour, across the boundary of two
  bands of one grid the next band's cell; `BandLink { prev, next, rows }`), `Strip::spanning(i0, frames, rows)`.
- A road edit that closes, opens or reshapes a face redoes the blocks of every road around it (the closure is
  computed in `blocks::sync`); zones move onto the new cells as on any re-zoning. Block regeneration after one edit in
  the 10,000-resident city: 0.24-0.70 ms (release, an 8-core cloud machine; the same run of the previous code:
  0.16-0.47 ms). Loading a version-7 save made before block grids remakes the faces' blocks (`blocks::derive_faces`): zones move,
  buildings move onto the lots under them where those are free, the rest stay. Fixture
  `tests/data/hand_blocks_v7.citysave`.

### Leftover land (`faces::leftovers`)

- `faces::leftover_faces(world) -> Vec<(face key, signature)>` (bounded faces up to 40,000 m²; the signature changes
  with the face's roads, blocks and lattices), `faces::leftover(world, key, signature) -> Option<Leftover>`,
  `faces::leftovers(world)`. `Leftover { face, signature, origin, step, size, inside, area_m2 }`: land inside the face
  that no block of its roads reaches, sampled every 2 m (`contains(p)`); at least 96 m². Nothing can be built there;
  gfx draws it as a small green (`gfx/greens.rs`).

### Land where rows meet (`zoning::cell_shape`)

- `zoning::cell_shape(world, cell) -> Option<Vec<[f32; 2]>>` / `cell_shapes(world)` / `LandShapes::new(world).shape(..)`:
  the land a usable cell holds where it differs from its quadrilateral. Where the rows of two roads meet at an angle,
  cells are cut along the line where the two roads' rows are equally deep, and a cell takes the land of the cells
  behind it that the other road's rows took, up to that line. A convex polygon (empty: nothing left). Renderers draw
  zones, lot markers, the hovered-cell frame and the zone tool's preview with it. Land of different lattices overlaps only along the line by rounding (property test:
  at most 1 in 50 cut cells over 4 m², never 32 m²). Growth still uses whole cells (a lot's building stands on its
  front rows, well inside the line).

### Zone paint

- (ui) The brush takes every cell the pointer's path since the last frame crosses and every cell within half a cell's
  diagonal of it, so a stroke is unbroken at any frame rate.

## 2026-11-27 (r7-roadclear: a road drawn over grown buildings takes them down)

Nothing new is stored: saves, scenarios and the pinned checksum are unchanged (plain road commands mean what they
meant).

- `Building::gives_way_to_roads() -> bool`: the one rule for which buildings make way for a road. Every building grown
  by itself on zoned land does, which is every building today; buildings the player places by hand will answer no
  and keep refusing roads. Anything else that takes buildings down for road land (a junction upgrade) asks this.
- `World::buildings_in_reserve(curve, kind) -> Vec<BuildingId>`: what stands in the reserve a road of `kind` needs
  along `curve`, measured exactly as a road command measures what is in its way (the lot's whole footprint closer than
  `reserve_half_width_m() - 0.05` to the centre line), in id order.
- `Command::Clearing(Box<Command>)` around `BuildRoad`, `BuildCurve` or `SetRoadKind`: the buildings in the new road's
  reserve that give way are taken down with it, as the bulldozer takes them (residents leave the city, staff lose the
  job, the lot's cells under the road lose their zone as every reserve does, the rest of the lot keeps it), and paid for
  at the bulldozer's price (`BULLDOZE_COST_PER_CELL` per lot cell). The command needs the money for road and
  demolitions together. Refused exactly as the inner command is for everything else; a building that does not give
  way still refuses it with `BuildingInTheWay`; any other inner command: `CommandError::NotARoadCommand`.
  `preview` agrees with `apply`; what it lists is exactly what goes (tests: `tests/clearing.rs`, on grid and curved
  lots). A change of kind clears only where the new kind needs a wider reserve: never today, every kind keeps 4 m (the
  avenue's 16 m drawn width is wider than its reserve; gfx fits lots clear of the drawn road).
- `Preview::buildings` (now also for roads clearing their way), `Preview::demolition_cost` (the part of `cost` for
  them), `Preview::road_cost` (the part of `cost` paid for road by any road command).
- `Command::Refund { tick, amount }`: gives back a payment of `amount` a road command made for road at `tick`, once
  (`economy.receipts`, the session's latest `RECEIPTS_KEPT` = 512 payments, never saved, ignored by `==`). Refused with
  `CommandError::NoSuchReceipt`. The interface's undo of a road sends it last, after taking the road back, putting the
  zoning the road's land took back (`ZoneCells`) and restoring the buildings (`RestoreBuilding`, which refunds the
  demolitions). Undoing a road still pays the bulldozer's dollar a cell to take it away.

## 2026-11-27 (r7-roundabout: a junction upgraded to a roundabout)

### Representation

A roundabout is a **setting of a junction node**, not road pieces: `JunctionState::ring: Option<RoundaboutSize>`
(`#[serde(skip)]`, saved in extension block 7). The road graph, the zoning lattices along the roads and the save body
stay as they are. With a ring the node's arms end at give-way lines just outside the ring
(`RING_GIVE_WAY_OUTSIDE_M` beyond the outer kerb, `Arm::setback`), and every movement is a **ring path**
(`roundabout::RingPath`): an entry curve from the give-way line onto a ring lane, an arc of that lane in the direction
of circulation, an exit curve onto the out-lane (`Movement::ring_path()`; `Movement::pose_at` and
`Movement::points(step)` follow it; `Movement::curve` then only joins its two ends: **draw ring movements with
`points`, never `curve`**). Traffic drives on the right, so the ring runs with the island on the driver's left: in sim
axes the angle round the centre decreases along the circulation (counter-clockwise on screen). One-way ring road
pieces were not used: they would need one-way roads, would grow zoning frontage on the ring and cut blocks.

`JunctionControl::Roundabout` is the control in force (signals, plans and auto control do not apply; `JunctionState::
setting` is what the junction returns to without the ring). In the save body a roundabout's control is written as
`Priority` (older builds read the save; the ring comes back from block 7 and the junction is rebuilt on load).

### Sizes (`roundabout::RoundaboutSize`, `dims()` -> `RingDims`)

| Size | Outer kerb radius | Lanes (width) | Island | Price / upkeep a day | Fits |
|---|---|---|---|---|---|
| Mini | 8.5 m | 1 (5.0 m) | 3.5 m, mountable painted dome | $1,200 / $15 | streets only |
| Small | 13 m | 1 (5.5 m) | 6 m + 1.5 m apron | $3,000 / $25 | streets; an avenue T, not two avenues |
| Medium | 19 m | 2 (4.5 m) | 8.5 m + 1.5 m apron | $6,500 / $40 | avenue meeting streets |
| Large | 27 m | 2 (4.75 m) | 16 m + 1.5 m apron | $11,000 / $60 | avenues crossing |

Land taken: a disc of `RingDims::reserve_r()` (outer kerb + `RING_PAVEMENT_M` 2.5 + `RING_VERGE_M` 0.5). Speed on a
ring lane: `sqrt(RING_LATERAL_ACCEL * r)` (mini 3.7 m/s ... large outer lane 7.7 m/s), trucks `RING_TRUCK_SPEED` of it.

### Which sizes fit (`roundabout::fit`, `roundabout::sizes(world, node)`, `roundabout::plan_ring`)

`RingProblem` (`Display` and `at()`): `TooFewArms` (a bend or road end), `TooManyArms` (> `RING_MAX_ARMS` 6),
`Highway`, `AvenueOnMini`, `ArmsTooClose { degrees, at }` (one arm's entry and the next arm's exit need room round the
ring), `NeighbourTooClose { at }` (an arm keeps `RING_MIN_ARM_M` 10 m of road between the give-way line and the next
junction's box), `RoadInTheWay { at }`, `OffMap`, `AlreadyThatSize`, `NotARoundabout`. A road edit that leaves a ring no
room (a new junction on an arm too near) makes it an ordinary junction again (its land goes back to zoning).

### Command

- `Command::SetRoundabout { node, size: Option<RoundaboutSize> }` (`None`: an ordinary junction again). Refused with
  `CommandError::Roundabout(RingProblem)`, `InsufficientFunds`, or `BuildingInTheWay` for a building on its land that
  does not `gives_way_to_roads()` (the roads' rule). `Preview::roundabout: Option<RoundaboutChange { node, before,
  after }>`, `buildings` (taken down, the bulldozer's way), `zone_cells` (lose their land), `road_cost` (the ring's
  price, refundable by `Command::Refund` as for roads: a receipt is issued), `demolition_cost`, `cost` (both).
  Removal refunds nothing and is never refused for money. `preview` agrees with `apply`.
- On apply: buildings on its land go, the junction is rebuilt (`versions.roads` bumps: renderers rebuild), zone cells
  under the disc are blocked (`Blocked::Road`, their zone cleared as under any road), cars standing past the new
  give-way line go back to it one behind another (turn-pocket cars into the inner lane; cars that no longer fit on a
  short arm are lifted off the road). Undo (the interface): `SetRoundabout(before)`, `SetNodeControl(before)`, zones by
  cell centre (`plan::Op::ZoneAt`), `RestoreBuilding` for each, `Refund`.
- Money: `economy::RunningCosts::{roundabouts, roundabout_count}` (part of `road_upkeep()`, saved in block 7).

### Rules of the ring (`roundabout` + `drive`)

- **Entry**: a front car gives way to every car whose path passes its merge point(s) (on the ring, or entering
  upstream, or let in this tick elsewhere): it goes only when that car reaches the point at least `RING_SAFETY_S`
  (0.8 s) after the entering car's rear has cleared it, judged from both cars' distance, speed and acceleration (about
  3.5 s critical gap from a stand). It needs exit room as at any junction (cars in the ring heading to the same exit
  count), the ring just beyond its merge not standing, and the start of its own path free.
- **On the ring**: cars follow whatever their path runs into, tested with oriented car boxes along the path
  (`RING_FRONT_GAP_M` 1 m ahead, `RING_SIDE_GAP_M` 0.3 m to the sides), headway `RING_HEADWAY_S` 1.0 s; oncoming cars
  on an arm's own lanes are ignored; two cars in each other's way: the one whose front is further ahead goes, as far as
  it can without touching. Stopping on the ring happens only behind a car (waiting at a blocked exit).
- **Lane by exit** (two-lane rings): the first exit and every exit onto a road with one lane out from the outer lane;
  from an avenue's inner lane, the inner ring lane for later exits (straight on from both lanes where the exit has two);
  an avenue exit takes both ring lanes side by side (outer -> kerb lane, inner -> inner lane). A car leaving from the
  inner lane across the outer one waits for a gap there (`RING_CROSS_SAFETY_S`), never cutting across a car beside it.
- Routing: `ROUTE_RING_S` (2.5 s) to enter, plus each turn's time round the ring beyond crossing the box.
- Diagnostics: `examples/ringprobe.rs` (one bench ring, overlaps, stuck cars), `examples/ringjam.rs` (a busy junction of
  the jam town or the rings demo with each size), `jbench rings` (the capacity table), `overlap rings|street_minis`,
  `topdown rings` (each size at a street and an avenue crossing), `headless` scenarios `rings`, `street_rings`,
  `minis`, `street_minis`.

### People on foot (with r7-walk)

- The walking network (`walk::build`) runs a pavement round each ring (from an arm's right-hand pavement in to the ring,
  round it at `outer_r + pavement / 2`, out along the next arm's left-hand one) and a zebra across each arm one car
  length out from the give-way line (`balance::RING_ZEBRA_M` 6.2 m to its middle; `walk::zebra_line_at(world, node,
  segment)` gives any arm's zebra line). Ring zebras are `Crossing { node: None, ring: Some(node), key.at: 0 | 1 }`:
  cars treat them as mid-block zebras (`walk::zebra_ahead`: an entering car stops short of it for people, and keeps it
  clear), and a car leaving the ring by that arm stops between the ring and the zebra
  (`walk::ring_exit_held`; it blocks the ring by at most a car length). People waiting there see the cars in the ring
  that will leave by their arm (`eval_crossing`) and never step in front of one that cannot stop.
- `capacity::Bench::walkers(per_min)` puts people on the bench's pavements (`walk::bench_walk`: walkers with no
  citizen behind them, `Walker::bench`, never saved, gone on arrival); `jbench rings` with `JBENCH_WALKERS=n`,
  `ringprobe` with `RING_WALKERS=n` (also counts car bodies over people).
- Test: `tests/walking.rs` `people_walk_round_roundabouts_and_cars_wait_for_them` (the rings demo town at the morning
  rush: a zebra on every ring arm set back from the ring, a way round the large ring on foot, people crossing, cars
  held, no car body over a person).

### Renderers

- `roundabout::ring(world, node) -> Option<&Ring>`: `size`, `center`, `dims`, `arms: Vec<RingArm { segment, kind,
  setback, angle, splitter }>`. `junction::junction_shape` of a ring node: the arm cuts joined by kerb fillets onto the
  outer kerb circle (the island is not cut out: draw it over).
- `capacity::Bench::roundabout(size)`, `BenchResult::mean_wait_s`.

### Saves

Extension block **7** (`ExtensionV7`: block 6 (people on foot), the roundabouts `(node, size)`, yesterday's
roundabout upkeep); the rings are restored before the walking network is made for them. Blocks 1-6 load; format 7,
versions 6, 4 and 3 load as before. The pinned checksum moved only because block 7 is written (with block 6 written the
state hashes as on main: `0x69ea187edd9a577e`; now `0x21260e95e52f11b1`).

### Measured (release, this 8-core machine)

Capacity (`jbench rings`, cars a minute with every approach queued / seconds a car stands at a moderate demand of 6
cars a minute per street approach, 18-20 per avenue approach; 10 / 30 / 50 % turning):

| | give-way | signals | mini | small | medium | large |
|---|---|---|---|---|---|---|
| street x street, sat. | 28 / 25 / 25 | 27 / 25 / 24 | 33 / 34 / 34 | 39 / 39 / 39 | 43 / 44 / 44 | 46 / 47 / 47 |
| street x street, wait | 18.6 / 6.7 / 5.8 | 3.0 / 4.8 / 5.7 | 1.8 / 1.8 / 1.4 | 0.7 / 0.8 / 1.0 | 0.5 / 0.5 / 0.5 | 0.4 / 0.4 / 0.4 |
| avenue x street, sat. | 106 / 71 / 65 * | 101 / 69 / 53 | - | 45 / 42 / 40 | 55 / 57 / 56 | 64 / 63 / 60 |
| avenue x street, wait | 7.0 / 8.0 / 11.3 | 4.7 / 11.5 / 17.7 | - | 19.5 / 12.3 / 19.7 | 1.2 / 2.1 / 4.8 | 0.8 / 1.1 / 1.4 |
| avenue x avenue, sat. | 28 / 28 / 30 | 140 / 117 / 104 | - | - | 72 / 72 / 72 | 90 / 82 / 84 |
| avenue x avenue, wait | 175 / 163 / 186 | 6.2 / 11.2 / 14.1 | - | - | 19.7 / 16.3 / 19.2 | 4.2 / 4.8 / 6.8 |

\* give-way on an avenue lets 0-4 % of the cars in from the side street; rings serve every arm (22-51 %).

With 12 people a minute walking past every junction (`JBENCH_WALKERS=12`; saturated cars a minute / moderate wait in
seconds, 10 / 30 / 50 % turning): street x street give-way 24 / 21 / 20 (13.6 / 37.1 / 33.2 s), signals 27 / 23 / 23
(3.1 / 7.5 / 15.0 s), mini 32 / 32 / 32 (2.0 / 1.7 / 2.0 s), small 36 / 37 / 35 (0.9 / 1.2 / 1.1 s), medium 40 / 41 / 41,
large 43 / 43 / 44 (0.4-0.5 s); avenue x street signals 103 / 69 / 52 (10.4 / 13.7 / 25.7 s), medium 60 / 54 / 56
(2.3 / 3.3 / 9.7 s), large 72 / 60 / 58 (1.7 / 2.5 / 5.0 s); avenue x avenue signals 132 / 109 / 104 (7.0 / 15.7 /
19.7 s), large 91 / 86 / 81 (9.8 / 16.9 / 9.8 s). Car bodies over people at the bench rings: none.

Scripted cities (10 seeds, 15 days; commute over free flow / car trips given up, by residents): avenue city 9-11k
1.47x / 0.25 %, with two-lane rings on its avenue junctions 1.71x / 0.28 %, with mini rings at its street junctions
1.59x / 0.40 %; streets city 9-11k 3.22x / 2.18 %, with small/mini rings everywhere 3.34x / 1.97 %, with mini rings
everywhere 2.82x / 1.58 %. On these 80 x 40 m blocks a two-lane ring leaves 10 m of road to the next junction and takes
23 % of the zoned land (final residents 16.3k vs 18.2k); mini rings take 1-9 %. Step time (`bench`, 20,000 citizens,
no rings): unchanged (808-851 us mean against 815-846 us). Overlapping car bodies over a 07:00-09:00 rush: avenue city
with rings 1 pair-tick (0.05 m), streets city with minis 7 (at most 0.09 m).

With people walking (r7-walk merged; 10 seeds, 15 days; commute over free flow / trips given up at 9-11k residents):
avenue city 1.90x / 0.43 %, with two-lane rings 2.13x / 0.52 %, with mini rings 1.89x / 0.42 %; streets city 3.43x /
2.25 %, with small/mini rings 3.56x / 2.67 %, with mini rings everywhere 2.86x / 2.10 %.

Incremental junction sync (plan-preview-fast): a junction whose own roads are unchanged (`JunctionInputs`) only has
its control and queues reset; a ring junction is always built again (its paths and land depend on the arms' far ends
and its neighbours), and `roundabout::set` drops the node's inputs, so setting or removing a ring rebuilds it.
`RemoveRoads` taking an arm, and an edit that leaves a ring no room, go through the same `sync_junctions` as any road
edit (the ring is dropped, its reserve's cars and land handed back). Checked in lockstep against rebuilding every
junction: `traffic::junction_sync_tests::incremental_junction_sync_with_roundabouts`.

## 2026-11-27 (r7-walk: people walk)

Citizens walk on the map: they leave a door, walk the pavements, wait at the kerb, cross on the zebras and go in at
their destination. Module `citysim_sim::walk`; everything about people on foot is in `world.walking` (`Walking`).

### The walking network (`walk::WalkNet`, derived, not saved)

- Built from the roads when `versions.roads` changes (`walk::network(world)` once the simulation has built it for the
  current roads): a pavement along both sides of every street and avenue at `half_width + pavement / 2` (
  `balance::STREET_PAVEMENT_M` 1.25, `AVENUE_PAVEMENT_M` 1.5; none along the highway), kerb corners round every node,
  a zebra across each junction arm where renderers draw one (stop line at least `ZEBRA_WIDTH_M + 1` from the node, not
  across the highway, not overlapping another arm's zebra; centre line at `walk::zebra_line(setback)` = setback minus
  `ZEBRA_INSET_M` 0.4 minus half of `ZEBRA_WIDTH_M` 1.75), and mid-block zebras on stretches of at least
  `MIDBLOCK_MIN_M` 150 m between stop lines, one per `MIDBLOCK_SPACING_M` (`walk::midblock_positions(world, s)`,
  metres from `a`: renderers draw these; gfx does in `road_geom::mesh_segment_with`).
- `Crossing { key: CrossingKey { segment, at }, node, arm, from_a, center, dir, kind, half_width, ends, points }`,
  `outline()` for drawing; `WalkNet::crossing(key)`, `on_segment(s)`, `at_arm(node, arm)`.
- Doors: `walk::door(building)` = the middle of the lot's front edge, `DOOR_INSET_M` 1.5 inside; routes snap to the
  nearest pavement within 45 m. `WalkNet::route(from, to, limit) -> Route { pts, marks: [(leg, CrossingKey)], len }`
  (A*, crossings cost `WALK_CROSSING_COST_M` 8 m extra).

### Walkers (`walk::Walker`, saved in extension block 6)

- `world.walking.walkers: Walkers` (dense list keyed by the leading citizen: `get`, `iter`, `len`...). A `Walker` has
  its `party` (children walking with the adult), `from`, `to`, `route`, `leg` / `along`, `speed`, a sideways
  `offset`, `seed`, the pose `pos` / `heading` / `state: WalkState::{Walking, Waiting, Crossing}`, `walked_m` (the
  walk cycle's phase), `done_m()`, `left_m()`, `ahead()` (the rest of the way from `pos`), `crossing()`.
- `walk::walker_of(world, citizen) -> Option<(leader, &Walker)>` for a leader or a party member. A walking citizen's
  `Journey` has `mode: Walking`, `arrive_tick: None` while walked on the map (`Some` when timed off screen as before:
  over the walker budget `walk::walker_budget` = `WALKERS_PER_1000` 300 per 1,000 residents, 200-8,000); party
  members carry `driver: Some(leader)`.
- Pace `walk::pace(world, citizen)`: `WALK_SPEED` 1.75 m/s, ±`WALK_SPEED_SPREAD` 10 % by person, by stage
  `WALK_STAGE_FACTOR` (children 0.82, teens 1.04, adults 1, seniors 0.78); a party walks at its slowest member's pace.
- A road edit makes every walker find their way again from where they stand (`Walking::routes_for`, saved); without a
  way they finish off screen.

### Who walks

- A trip from a building is walked when a route along the pavements exists no longer than `WALK_MAX_M` 650 m and than
  their pace covers in `WALK_MAX_H` 4 h (to work `WALK_COMMUTE_MAX_H` 3 h, to lunch `LUNCH_WALK_MAX_TRIP_H` 1.5 h), and
  the person's draw is below `walk::walk_share(len, senior)`: `WALK_SHARE_NEAR` 0.95 up to `WALK_NEAR_M` 120 m, falling
  by 1/e every `WALK_DECAY_M` 200 m (seniors: `WALK_SENIOR_RANGE` 0.7 of the distances), times `WALK_COMMUTE_FACTOR` (1)
  for commutes. The draw is a habit for commutes (both ways alike), daily otherwise. Children walk with the adult; a
  worker may walk to lunch though the car is at work. The old rule (every trip within 100 m straight walked off screen)
  is gone.
- Commuters who walk plan by the walking time; a walked commute counts as a fine commute (happiness) and loses no
  business tax to traffic.
- Counts: `walking.stats` (`WalkStats`: trips started on foot and otherwise today / yesterday and by hour; `share()`).

### Crossing rules

- **Cars give way.** A car does not enter a junction while someone is on (or stepping onto) the zebra of its own arm
  or of the arm it turns onto, on its half of the road (`walk::blocks_entry`); a car in a box stops short of the zebra
  it turns onto (`walk::held_at`); a car stops short of a mid-block zebra with someone on it and keeps it clear (does not
  enter it without room beyond) (`walk::zebra_ahead`). Hooks in `drive.rs` are marked "(walk.rs)".
- **People never step in front of a car that cannot stop**: braking distance plus two ticks plus `PED_SAFETY_M` 1.5 m,
  or a car over the zebra (`CrossNow::hazard`). A standing car can stop.
- **Signals**: people cross an arm during a through phase that sends no traffic from that arm (not in protected-left
  phases), stepping on only in the first `WALK_INTERVAL_S` 6 s of that green and only with time to cross before its
  maximum (`walk::signal_walk`; `walk::walk_signal(world, node, arm)` for renderers). People waiting call the phase
  (`junction::ArmDemand::walk_call`, `phase_demand`). Turning cars give way to them; there is no separate pedestrian
  phase or extra all-red.
- **Without signals** (give-way and all-way junctions, mid-block): people wait for a gap long enough to cross before the
  next moving car (or a crawling queue within 8 m, or a car just let into the junction) arrives, for up to
  `PED_PATIENCE_S` 40 s (on a main road: the major road of a give-way junction or a mid-block zebra, `PED_MAIN_PATIENCE_S`
  40 s), then step out and the cars stop. Later arrivals join a group within `PED_GROUP_S` 2 s; once a car has stood
  waiting `CAR_PATIENCE_S` 3 s, people let the cars go for `CAR_TURN_S` 6 s (`CrossTimers`, saved by `CrossingKey`).
- Per tick: `walk::prepare` before the cars (who holds which half of which zebra, signal calls; gathered at the end of
  the previous tick), `walk::tick` after them (cars' hazards at crossings people want to use, then the walkers move).

### Saves

- Extension block **6** (`ExtensionV6`: block 5 plus the walkers, crossing timers by `CrossingKey`, the day's walking
  counts and `routes_for`). Blocks 1-5 load; their walks stay timed off screen.

### Diagnostics and tools

`walk::debug_holds(world)` (car ticks held at zebras by kind), `walk::people_on_crossings(world)` (bodies for overlap
tests), `examples/walkprobe.rs` (`STREETS=1`, `SEED=n`: where people hold up cars in a grown city), `walkspots.rs`
(busy squares, signals and give-ways with people, mid-block zebras of a saved city: camera poses), `headless` hourly
CSVs gained `walkers_mean,walked,other`, daily `walked,other_trips`.
