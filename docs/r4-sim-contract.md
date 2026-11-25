# Simulation contract: free-form roads and zoning that follows them

Branch `r4-roads` (crate `citysim_sim`). This page is the contract between the simulation and the graphics /
interface work. Additions are appended freely; changes to anything already listed are recorded at the bottom under a
dated heading.

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
