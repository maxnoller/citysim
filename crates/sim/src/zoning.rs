//! Zoning that follows the roads: 8 m cells in rectangular *blocks* along
//! each side of every road segment, on *lattices* that roads share where
//! they line up.
//!
//! # Model
//!
//! * A [`Lattice`] is an infinite grid of `CELL_SIZE` cells with an origin
//!   (the corner of cell `(0, 0)`) and a unit direction `u` of its `+i`
//!   axis; `+j` runs along `v = (-u.y, u.x)`. Lattice 0
//!   ([`LatticeId::WORLD`]) is the map's own grid: its cell `(i, j)` is
//!   [`crate::Cell`] `(i, j)`.
//! * A [`Block`] belongs to one side of one road segment: a rectangle of
//!   cells of one lattice whose front row runs along the kerb
//!   (`balance::BLOCK_DEPTH` cells deep). A straight segment has one block
//!   per side; a curved one several straight blocks along the kerb (thin
//!   wedge gaps outside the bend, overlaps trimmed inside).
//! * Blocks of different roads whose lattices coincide (parallel or
//!   perpendicular within `LATTICE_ANGLE_TOL_DEG`, offset a whole number of
//!   cells within `LATTICE_OFFSET_TOL`) use the same lattice and *share*
//!   cells: a right-angle grid city is one lattice, its corner cells front
//!   either road. Where blocks of different lattices overlap, the older
//!   road's block (lower `RoadSegment::serial`) keeps the cells and the
//!   other block's overlapping cells are blocked. Cells under roads or
//!   under another lattice's buildings are blocked.
//! * A [`ZoneCell`] holds the zone and the building on it. It stays when
//!   its blocks go away while it holds a zone or a building (it is then
//!   not `covered`: not zoneable and not drawn as zoned land), so zoning
//!   survives road edits; new cells take the zone of an uncovered cell of
//!   another lattice at the same place.
//!
//! Lots and buildings live in one lattice: see `growth` and
//! [`crate::Footprint`].
//!
//! Renderers key on `world.versions.grid` (any cell or block changed).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::curve::right_of;
use crate::geom::{self, Cell};
use crate::roads::SegmentId;
use crate::{BuildingId, CELL_SIZE, Facing, World};

/// Zone type of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Zone {
    Residential,
    Commercial,
    Industrial,
}

impl Zone {
    pub const ALL: [Zone; 3] = [Zone::Residential, Zone::Commercial, Zone::Industrial];
}

/// Index into `Zoning::lattices`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct LatticeId(pub u16);

impl LatticeId {
    /// The map's own grid (cell `(i, j)` is `Cell (i, j)`).
    pub const WORLD: LatticeId = LatticeId(0);
}

/// One zone cell: lattice and integer cell coordinates in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CellRef {
    pub lattice: LatticeId,
    pub i: i32,
    pub j: i32,
}

impl CellRef {
    pub const fn new(lattice: LatticeId, i: i32, j: i32) -> CellRef {
        CellRef { lattice, i, j }
    }

    /// The world-grid cell `c` (lattice 0).
    pub const fn world(c: Cell) -> CellRef {
        CellRef::new(LatticeId::WORLD, c.x, c.y)
    }

    /// The neighbour one cell towards `f` (lattice axes: `PlusX` = `+i`,
    /// `PlusY` = `+j`).
    pub fn step(self, f: Facing) -> CellRef {
        let (di, dj) = f.offset();
        CellRef::new(self.lattice, self.i + di, self.j + dj)
    }

    pub fn offset(self, di: i32, dj: i32) -> CellRef {
        CellRef::new(self.lattice, self.i + di, self.j + dj)
    }
}

/// An infinite grid of cells at some position and angle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lattice {
    /// Corner of cell `(0, 0)`, sim metres.
    pub origin: [f32; 2],
    /// Unit direction of `+i`. `+j` is [`Lattice::v`].
    pub u: [f32; 2],
    /// Cells that exist (covered by a block, or holding a zone or
    /// building), by `(i, j)`.
    pub cells: BTreeMap<(i32, i32), ZoneCell>,
}

impl Lattice {
    pub fn new(origin: [f32; 2], u: [f32; 2]) -> Lattice {
        Lattice {
            origin,
            u,
            cells: BTreeMap::new(),
        }
    }

    /// The map's grid.
    pub fn world() -> Lattice {
        Lattice::new([0.0, 0.0], [1.0, 0.0])
    }

    /// Unit direction of `+j`.
    pub fn v(&self) -> [f32; 2] {
        right_of(self.u)
    }

    /// Angle of `+i`, radians (`atan2` in sim axes).
    pub fn angle(&self) -> f32 {
        libm::atan2f(self.u[1], self.u[0])
    }

    /// World direction of a lattice-relative facing.
    pub fn dir(&self, f: Facing) -> [f32; 2] {
        let (di, dj) = f.offset();
        let v = self.v();
        [
            self.u[0] * di as f32 + v[0] * dj as f32,
            self.u[1] * di as f32 + v[1] * dj as f32,
        ]
    }

    /// Lattice coordinates (in cells, fractional) of a world point.
    pub fn local(&self, p: [f32; 2]) -> [f32; 2] {
        let d = geom::sub(p, self.origin);
        [geom::dot(d, self.u) / CELL_SIZE, geom::dot(d, self.v()) / CELL_SIZE]
    }

    /// World point at lattice coordinates (in cells).
    pub fn world_at(&self, x: f32, y: f32) -> [f32; 2] {
        let v = self.v();
        [
            self.origin[0] + (self.u[0] * x + v[0] * y) * CELL_SIZE,
            self.origin[1] + (self.u[1] * x + v[1] * y) * CELL_SIZE,
        ]
    }

    /// The cell containing a world point.
    pub fn cell_of(&self, p: [f32; 2]) -> (i32, i32) {
        let l = self.local(p);
        (libm::floorf(l[0]) as i32, libm::floorf(l[1]) as i32)
    }

    /// Centre of cell `(i, j)`.
    pub fn cell_center(&self, i: i32, j: i32) -> [f32; 2] {
        self.world_at(i as f32 + 0.5, j as f32 + 0.5)
    }

    /// Corners of cell `(i, j)`, counter-clockwise in lattice axes:
    /// `(i, j)`, `(i+1, j)`, `(i+1, j+1)`, `(i, j+1)`.
    pub fn cell_corners(&self, i: i32, j: i32) -> [[f32; 2]; 4] {
        let (x, y) = (i as f32, j as f32);
        [
            self.world_at(x, y),
            self.world_at(x + 1.0, y),
            self.world_at(x + 1.0, y + 1.0),
            self.world_at(x, y + 1.0),
        ]
    }
}

/// Why a cell of a block cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Blocked {
    /// A road runs over it.
    Road,
    /// It overlaps a cell of an older road's block in another lattice.
    Contested,
    /// It overlaps a building standing in another lattice.
    Building,
}

/// State of one zone cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ZoneCell {
    pub zone: Option<Zone>,
    pub building: Option<BuildingId>,
    /// Inside at least one block.
    pub covered: bool,
    /// Set when the cell cannot be used (only meaningful while `covered`).
    pub blocked: Option<Blocked>,
    /// Bit `1 << (facing as u8)` for each lattice side with a road right in
    /// front (the cell is in the front row of a block on that side). Corner
    /// cells of a shared lattice front two roads.
    pub fronts: u8,
}

impl ZoneCell {
    /// Usable zone land: covered, not blocked.
    pub fn usable(&self) -> bool {
        self.covered && self.blocked.is_none()
    }

    /// Usable and empty (no building).
    pub fn free(&self) -> bool {
        self.usable() && self.building.is_none()
    }

    /// Is there a road in front on side `f`?
    pub fn fronts_on(&self, f: Facing) -> bool {
        self.fronts & facing_bit(f) != 0
    }
}

/// Bit of a facing in `ZoneCell::fronts`.
pub fn facing_bit(f: Facing) -> u8 {
    1 << (f as u8)
}

/// Side of a road segment, seen travelling from its `a` node to `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
}

/// A rectangle of cells along one side of a road segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub segment: SegmentId,
    pub side: Side,
    pub lattice: LatticeId,
    /// Inclusive cell rectangle in the lattice.
    pub min: [i32; 2],
    pub max: [i32; 2],
    /// Lattice side of the block that faces the road.
    pub front: Facing,
    /// Overlap priority: lower wins (`(segment serial, index along the
    /// segment)`).
    pub priority: (u64, u32),
}

impl Block {
    pub fn contains(&self, c: CellRef) -> bool {
        c.lattice == self.lattice
            && (self.min[0]..=self.max[0]).contains(&c.i)
            && (self.min[1]..=self.max[1]).contains(&c.j)
    }

    /// All cells, row by row.
    pub fn cells(&self) -> impl Iterator<Item = CellRef> + '_ {
        (self.min[1]..=self.max[1])
            .flat_map(move |j| (self.min[0]..=self.max[0]).map(move |i| CellRef::new(self.lattice, i, j)))
    }

    /// Is `c` in the row along the road?
    pub fn in_front_row(&self, c: CellRef) -> bool {
        self.contains(c)
            && match self.front {
                Facing::PlusX => c.i == self.max[0],
                Facing::MinusX => c.i == self.min[0],
                Facing::PlusY => c.j == self.max[1],
                Facing::MinusY => c.j == self.min[1],
            }
    }

    /// Cells counted from the road (0 = front row).
    pub fn depth_of(&self, c: CellRef) -> i32 {
        match self.front {
            Facing::PlusX => self.max[0] - c.i,
            Facing::MinusX => c.i - self.min[0],
            Facing::PlusY => self.max[1] - c.j,
            Facing::MinusY => c.j - self.min[1],
        }
    }
}

/// All blocks, lattices and cells.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zoning {
    /// By `LatticeId`; `None` for a freed slot (reused lowest first).
    /// Slot 0 is always the world grid.
    pub lattices: Vec<Option<Lattice>>,
    /// Every block, sorted by segment slot, side, then position along it.
    pub blocks: Vec<Block>,
}

impl Default for Zoning {
    fn default() -> Self {
        Zoning {
            lattices: vec![Some(Lattice::world())],
            blocks: Vec::new(),
        }
    }
}

impl Zoning {
    pub fn lattice(&self, id: LatticeId) -> Option<&Lattice> {
        self.lattices.get(id.0 as usize).and_then(|l| l.as_ref())
    }

    #[allow(dead_code)]
    pub(crate) fn lattice_mut(&mut self, id: LatticeId) -> Option<&mut Lattice> {
        self.lattices.get_mut(id.0 as usize).and_then(|l| l.as_mut())
    }

    /// Live lattices with their ids.
    pub fn lattices(&self) -> impl Iterator<Item = (LatticeId, &Lattice)> + '_ {
        self.lattices
            .iter()
            .enumerate()
            .filter_map(|(k, l)| l.as_ref().map(|l| (LatticeId(k as u16), l)))
    }

    pub fn cell(&self, c: CellRef) -> Option<&ZoneCell> {
        self.lattice(c.lattice)?.cells.get(&(c.i, c.j))
    }

    #[allow(dead_code)]
    pub(crate) fn cell_mut(&mut self, c: CellRef) -> Option<&mut ZoneCell> {
        self.lattice_mut(c.lattice)?.cells.get_mut(&(c.i, c.j))
    }

    /// Every existing cell, lattice by lattice, `(i, j)` order within one.
    pub fn cells(&self) -> impl Iterator<Item = (CellRef, &ZoneCell)> + '_ {
        self.lattices()
            .flat_map(|(id, l)| l.cells.iter().map(move |(&(i, j), c)| (CellRef::new(id, i, j), c)))
    }

    /// Centre of a cell (it need not exist).
    pub fn cell_center(&self, c: CellRef) -> Option<[f32; 2]> {
        Some(self.lattice(c.lattice)?.cell_center(c.i, c.j))
    }

    /// Corners of a cell (see [`Lattice::cell_corners`]).
    pub fn cell_corners(&self, c: CellRef) -> Option<[[f32; 2]; 4]> {
        Some(self.lattice(c.lattice)?.cell_corners(c.i, c.j))
    }

    /// The cell under a world position: a usable one if any lattice has
    /// one there, else any existing cell; lower lattice ids first.
    pub fn cell_at(&self, pos: [f32; 2]) -> Option<CellRef> {
        let mut fallback = None;
        for (id, l) in self.lattices() {
            let (i, j) = l.cell_of(pos);
            if let Some(c) = l.cells.get(&(i, j)) {
                let r = CellRef::new(id, i, j);
                if c.usable() {
                    return Some(r);
                }
                fallback = fallback.or(Some(r));
            }
        }
        fallback
    }

    /// Blocks of one segment (both sides).
    pub fn blocks_of(&self, s: SegmentId) -> impl Iterator<Item = &Block> + '_ {
        self.blocks.iter().filter(move |b| b.segment == s)
    }

    /// World corners of a block (same order as cell corners).
    pub fn block_corners(&self, b: &Block) -> Option<[[f32; 2]; 4]> {
        let l = self.lattice(b.lattice)?;
        let (x0, y0, x1, y1) = (
            b.min[0] as f32,
            b.min[1] as f32,
            (b.max[0] + 1) as f32,
            (b.max[1] + 1) as f32,
        );
        Some([
            l.world_at(x0, y0),
            l.world_at(x1, y0),
            l.world_at(x1, y1),
            l.world_at(x0, y1),
        ])
    }
}

// ----------------------------------------------------------------- queries ----

/// Is `p` inside the convex quad `q` (either winding)?
pub fn in_quad(q: &[[f32; 2]; 4], p: [f32; 2]) -> bool {
    let mut sign = 0.0f32;
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let c = geom::cross(geom::sub(b, a), geom::sub(p, a));
        if c.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

/// Existing cells whose centre lies inside the convex quad, in
/// [`Zoning::cells`] order.
pub fn cells_in_quad(world: &World, quad: &[[f32; 2]; 4]) -> Vec<CellRef> {
    let mut out = Vec::new();
    for (id, l) in world.zoning.lattices() {
        // Bounds of the quad in lattice coordinates.
        let loc: Vec<[f32; 2]> = quad.iter().map(|&p| l.local(p)).collect();
        let lo = loc
            .iter()
            .fold([f32::INFINITY; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]);
        let hi = loc
            .iter()
            .fold([f32::NEG_INFINITY; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]);
        let (i0, i1) = (libm::floorf(lo[0]) as i32, libm::ceilf(hi[0]) as i32);
        let (j0, j1) = (libm::floorf(lo[1]) as i32, libm::ceilf(hi[1]) as i32);
        let area = (i1 - i0 + 1) as i64 * (j1 - j0 + 1) as i64;
        if area > l.cells.len() as i64 * 4 {
            for (&(i, j), _) in l.cells.iter() {
                if in_quad(quad, l.cell_center(i, j)) {
                    out.push(CellRef::new(id, i, j));
                }
            }
        } else {
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if l.cells.contains_key(&(i, j)) && in_quad(quad, l.cell_center(i, j)) {
                        out.push(CellRef::new(id, i, j));
                    }
                }
            }
        }
    }
    out
}

/// The cell under a world position (see [`Zoning::cell_at`]).
pub fn cell_at(world: &World, pos: [f32; 2]) -> Option<CellRef> {
    world.zoning.cell_at(pos)
}

/// The connected area of usable cells around `pos` (for a fill tool):
/// cells reachable from the cell under `pos` through usable cells of the
/// same lattice sharing a side, or of another lattice whose centre is within
/// a cell's width; at most `limit` cells, nearest first in breadth-first
/// order. Empty when no usable cell is under `pos`.
pub fn fill_area(world: &World, pos: [f32; 2], limit: usize) -> Vec<CellRef> {
    let z = &world.zoning;
    let Some(start) = z.cell_at(pos).filter(|&c| z.cell(c).is_some_and(|c| c.usable())) else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = std::collections::VecDeque::new();
    let mut out = Vec::new();
    seen.insert(start);
    queue.push_back(start);
    while let Some(c) = queue.pop_front() {
        out.push(c);
        if out.len() >= limit {
            break;
        }
        let mut next: Vec<CellRef> = Facing::ALL.iter().map(|&f| c.step(f)).collect();
        if let Some(p) = z.cell_center(c) {
            for (id, l) in z.lattices() {
                if id == c.lattice {
                    continue;
                }
                let (i, j) = l.cell_of(p);
                for dj in -1..=1 {
                    for di in -1..=1 {
                        let (ni, nj) = (i + di, j + dj);
                        if l.cells.contains_key(&(ni, nj)) && geom::dist(l.cell_center(ni, nj), p) <= CELL_SIZE * 1.05 {
                            next.push(CellRef::new(id, ni, nj));
                        }
                    }
                }
            }
        }
        for n in next {
            if z.cell(n).is_some_and(|x| x.usable()) && seen.insert(n) {
                queue.push_back(n);
            }
        }
    }
    out
}

/// Cells of the blocks on one side of a road, along the stretch between
/// the two junctions (or dead ends) around `segment` (continuing through
/// nodes where only two segments meet), up to `depth` cells from the road
/// (1 = the front row). `side` is seen travelling along `segment` from `a`
/// to `b`. Usable cells only, without duplicates, ordered along the road.
pub fn frontage(world: &World, segment: SegmentId, side: Side, depth: u8) -> Vec<CellRef> {
    let roads = &world.roads;
    if !roads.segments.contains_key(segment) {
        return Vec::new();
    }
    // The stretch: (segment, same direction as `segment`?), in order.
    let mut stretch: Vec<(SegmentId, bool)> = vec![(segment, true)];
    for forward in [true, false] {
        let seg = &roads.segments[segment];
        let mut end = if forward { seg.b } else { seg.a };
        loop {
            let node = &roads.nodes[end];
            if node.segments.len() != 2 {
                break;
            }
            let Some(&n) = node.segments.iter().find(|&&x| stretch.iter().all(|&(y, _)| y != x)) else {
                break;
            };
            let ns = &roads.segments[n];
            let (rel, next) = match (forward, ns.a == end) {
                (true, true) => (true, ns.b),
                (true, false) => (false, ns.a),
                (false, true) => (false, ns.b),
                (false, false) => (true, ns.a),
            };
            if forward {
                stretch.push((n, rel));
            } else {
                stretch.insert(0, (n, rel));
            }
            end = next;
        }
    }
    let mut out: Vec<CellRef> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (s, same) in stretch {
        let want = if same {
            side
        } else if side == Side::Left {
            Side::Right
        } else {
            Side::Left
        };
        let mut blocks: Vec<&Block> = world.zoning.blocks_of(s).filter(|b| b.side == want).collect();
        if !same {
            blocks.reverse();
        }
        for b in blocks {
            let mut cells: Vec<CellRef> = b
                .cells()
                .filter(|&c| b.depth_of(c) < depth as i32)
                .filter(|&c| world.zoning.cell(c).is_some_and(|x| x.usable()))
                .collect();
            // Order along the road.
            if let (Some(l), Some(curve)) = (world.zoning.lattice(b.lattice), roads.segments.get(s).map(|x| x.curve)) {
                let key = |c: &CellRef| {
                    let d = curve.nearest(l.cell_center(c.i, c.j)).0;
                    if same { d } else { -d }
                };
                cells.sort_by(|x, y| key(x).total_cmp(&key(y)).then(x.cmp(y)));
            }
            for c in cells {
                if seen.insert(c) {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// The building standing at a world position, if any.
pub fn building_at(world: &World, pos: [f32; 2]) -> Option<BuildingId> {
    for (_, l) in world.zoning.lattices() {
        let (i, j) = l.cell_of(pos);
        if let Some(b) = l.cells.get(&(i, j)).and_then(|c| c.building) {
            return Some(b);
        }
    }
    None
}

// ---------------------------------------------------- world-grid mirror ----

/// Interim: rebuild lattice 0 from the world grid (roads on the cell
/// lattice), when the grid or the roads changed.
pub(crate) fn mirror_grid(world: &mut World) {
    let key = (world.versions.grid, world.versions.roads, world.versions.buildings);
    if world.cache.zoning_key == Some(key) {
        return;
    }
    world.cache.zoning_key = Some(key);
    use crate::grid::Occupant;
    let reach = crate::balance::ZONE_ROAD_REACH;
    let mut cells = BTreeMap::new();
    let g = &world.grid;
    for y in 0..crate::MAP_CELLS {
        for x in 0..crate::MAP_CELLS {
            let c = Cell::new(x, y);
            let cd = g.get(c).expect("in bounds");
            let road = cd.occupant == Occupant::Road;
            let near = !road && g.near_road(c, reach);
            let building = match cd.occupant {
                Occupant::Building(b) => Some(b),
                _ => None,
            };
            if !near && !road && cd.zone.is_none() && building.is_none() {
                continue;
            }
            let mut fronts = 0;
            for f in Facing::ALL {
                let (dx, dy) = f.offset();
                if g.is_road(c.offset(dx, dy)) {
                    fronts |= facing_bit(f);
                }
            }
            cells.insert(
                (x, y),
                ZoneCell {
                    zone: cd.zone,
                    building,
                    covered: near || road,
                    blocked: road.then_some(Blocked::Road),
                    fronts: if road { 0 } else { fronts },
                },
            );
        }
    }
    let mut blocks = Vec::new();
    let mut segs: Vec<(SegmentId, &crate::RoadSegment)> = world.roads.segments.iter().collect();
    segs.sort_by_key(|(s, _)| crate::idmap::key_index(*s));
    let depth = crate::balance::BLOCK_DEPTH as i32;
    for (s, seg) in segs {
        let (pa, pb) = (seg.curve.start(), seg.curve.end());
        let (ca, cb) = (Cell::from_pos(pa), Cell::from_pos(pb));
        if !geom::same_pos(ca.center(), pa) || !geom::same_pos(cb.center(), pb) {
            continue;
        }
        let (dx, dy) = ((cb.x - ca.x).signum(), (cb.y - ca.y).signum());
        if dx != 0 && dy != 0 {
            continue;
        }
        let (lo, hi) = (
            Cell::new(ca.x.min(cb.x), ca.y.min(cb.y)),
            Cell::new(ca.x.max(cb.x), ca.y.max(cb.y)),
        );
        // Right of travel a -> b is (-dy, dx).
        for side in [Side::Left, Side::Right] {
            let (nx, ny) = if side == Side::Right { (-dy, dx) } else { (dy, -dx) };
            let (min, max) = if nx != 0 {
                let x0 = if nx > 0 { lo.x + 1 } else { lo.x - depth };
                ([x0, lo.y], [x0 + depth - 1, hi.y])
            } else {
                let y0 = if ny > 0 { lo.y + 1 } else { lo.y - depth };
                ([lo.x, y0], [hi.x, y0 + depth - 1])
            };
            let front = match (nx, ny) {
                (1, 0) => Facing::MinusX,
                (-1, 0) => Facing::PlusX,
                (0, 1) => Facing::MinusY,
                _ => Facing::PlusY,
            };
            blocks.push(Block {
                segment: s,
                side,
                lattice: LatticeId::WORLD,
                min,
                max,
                front,
                priority: (seg.serial, 0),
            });
        }
    }
    world.zoning = Zoning {
        lattices: vec![Some(Lattice {
            cells,
            ..Lattice::world()
        })],
        blocks,
    };
}
