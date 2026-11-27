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

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::curve::{right_of, unit};
use crate::geom::{self, Cell};
use crate::roads::{NodeId, SegmentId};
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

/// A grid of cells: either an infinite straight grid at some position and
/// angle, or a [`Strip`] bent along a curved kerb.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lattice {
    /// Corner of cell `(0, 0)`, sim metres (a strip: its first kerb point).
    pub origin: [f32; 2],
    /// Unit direction of `+i`. `+j` is [`Lattice::v`]. A strip's direction
    /// changes along it: use [`Lattice::axes_at`] / [`Lattice::dir_at`].
    pub u: [f32; 2],
    /// Cells that exist (covered by a block, or holding a zone or
    /// building), by `(i, j)`.
    pub cells: BTreeMap<(i32, i32), ZoneCell>,
    /// The bent form (saved in the save file's extension block).
    #[serde(skip)]
    pub strip: Option<Strip>,
}

/// A lattice bent along a curved kerb. Column boundary `i0 + k` is the
/// half-line from `frames[k].0` (on the line `j = 0`, the kerb) along the
/// unit direction `frames[k].1` (`+j`); cell `(i, j)` is the quadrilateral
/// between boundaries `i` and `i + 1`, from `j` to `j + 1` cell sizes
/// along them. Neighbouring cells share their edges exactly, so a row of
/// cells follows the kerb without gaps or overlaps (narrower on the inside
/// of a bend, wider outside).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Strip {
    /// Lattice `i` of the first boundary.
    pub i0: i32,
    pub frames: Vec<([f32; 2], [f32; 2])>,
    /// Bounds of the frame points.
    pub lo: [f32; 2],
    pub hi: [f32; 2],
}

/// How far from its kerb line a strip's cells can reach (and its inverse
/// mapping is trusted), metres.
const STRIP_REACH: f32 = (crate::balance::MAX_BLOCK_DEPTH as f32 + 1.5) * CELL_SIZE;

impl Strip {
    pub fn new(i0: i32, frames: Vec<([f32; 2], [f32; 2])>) -> Strip {
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for (p, _) in &frames {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        Strip { i0, frames, lo, hi }
    }

    /// A strip whose cells reach `rows` rows out from its kerb line (a
    /// block-wide strip, see `faces`): its bounds hold the far side too.
    pub fn spanning(i0: i32, frames: Vec<([f32; 2], [f32; 2])>, rows: f32) -> Strip {
        let mut st = Strip::new(i0, frames);
        let reach = rows * CELL_SIZE;
        for (p, d) in &st.frames {
            let q = [p[0] + d[0] * reach, p[1] + d[1] * reach];
            st.lo = [st.lo[0].min(q[0]), st.lo[1].min(q[1])];
            st.hi = [st.hi[0].max(q[0]), st.hi[1].max(q[1])];
        }
        st
    }

    /// Number of columns.
    pub fn columns(&self) -> i32 {
        self.frames.len() as i32 - 1
    }

    fn column(&self, x: f32) -> usize {
        (libm::floorf(x) as i32 - self.i0).clamp(0, self.columns() - 1) as usize
    }

    pub fn at(&self, x: f32, y: f32) -> [f32; 2] {
        let k = self.column(x);
        let t = x - (self.i0 + k as i32) as f32;
        let (o0, d0) = self.frames[k];
        let (o1, d1) = self.frames[k + 1];
        let s = y * CELL_SIZE;
        let p0 = [o0[0] + d0[0] * s, o0[1] + d0[1] * s];
        let p1 = [o1[0] + d1[0] * s, o1[1] + d1[1] * s];
        [p0[0] + (p1[0] - p0[0]) * t, p0[1] + (p1[1] - p0[1]) * t]
    }

    /// `(t, y)` of `p` in column `k` (t = 0..1 across it), solving the
    /// bilinear map.
    fn solve(&self, k: usize, p: [f32; 2]) -> (f32, f32) {
        let (o0, d0) = self.frames[k];
        let (o1, d1) = self.frames[k + 1];
        let a = geom::sub(o1, o0);
        let b = [d0[0] * CELL_SIZE, d0[1] * CELL_SIZE];
        let c = [(d1[0] - d0[0]) * CELL_SIZE, (d1[1] - d0[1]) * CELL_SIZE];
        let q = geom::sub(p, o0);
        let a2 = -geom::cross(a, c);
        let a1 = geom::cross(q, c) - geom::cross(a, b);
        let a0 = geom::cross(q, b);
        let t = if a2.abs() < 1e-6 * (a1.abs() + 1.0) {
            if a1.abs() < 1e-9 { 0.5 } else { -a0 / a1 }
        } else {
            let disc = (a1 * a1 - 4.0 * a2 * a0).max(0.0);
            let r = libm::sqrtf(disc);
            let t1 = (-a1 + r) / (2.0 * a2);
            let t2 = (-a1 - r) / (2.0 * a2);
            if (t1 - 0.5).abs() <= (t2 - 0.5).abs() { t1 } else { t2 }
        };
        let d = [b[0] + c[0] * t, b[1] + c[1] * t];
        let e = [q[0] - a[0] * t, q[1] - a[1] * t];
        let y = geom::dot(e, d) / geom::dot(d, d).max(1e-9);
        (t, y)
    }

    /// Lattice coordinates of a world point, or `None` far from the strip.
    pub fn local(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let r = STRIP_REACH;
        if p[0] < self.lo[0] - r || p[0] > self.hi[0] + r || p[1] < self.lo[1] - r || p[1] > self.hi[1] + r {
            return None;
        }
        // Nearest boundary start, then the column around it that holds p.
        let mut best = (f32::INFINITY, 0usize);
        for (k, (o, _)) in self.frames.iter().enumerate() {
            let d = geom::dist(*o, p);
            if d < best.0 {
                best = (d, k);
            }
        }
        let n = self.columns() as usize;
        let k0 = best.1.min(n - 1);
        let mut pick: Option<(f32, usize, f32, f32)> = None;
        let try_column = |pick: &mut Option<(f32, usize, f32, f32)>, k: usize| {
            let (t, y) = self.solve(k, p);
            // How far outside the column (0 inside).
            let out = (-t).max(t - 1.0).max(0.0);
            if pick.is_none_or(|b| out < b.0) {
                *pick = Some((out, k, t, y));
            }
        };
        for k in k0.saturating_sub(2)..(k0 + 2).min(n) {
            try_column(&mut pick, k);
        }
        // Deep in a block-wide strip (`faces`) whose columns lean, the
        // column holding the point can be far from the nearest kerb
        // point: look at every column.
        let inside = p[0] >= self.lo[0] && p[0] <= self.hi[0] && p[1] >= self.lo[1] && p[1] <= self.hi[1];
        if inside && pick.is_some_and(|b| b.0 > 0.0) {
            for k in 0..n {
                try_column(&mut pick, k);
            }
        }
        let (_, k, t, y) = pick?;
        // How far the strip reaches beyond its kerb line (a block-wide
        // strip's bounds hold its far side too).
        let span = (self.hi[0] - self.lo[0]).max(self.hi[1] - self.lo[1]);
        if y.abs() * CELL_SIZE > r + span {
            return None;
        }
        Some([(self.i0 + k as i32) as f32 + t, y])
    }

    /// Directions of `+i` (along the kerb chord) and `+j` in column `x`.
    pub fn axes(&self, x: f32) -> ([f32; 2], [f32; 2]) {
        let k = self.column(x);
        let (o0, d0) = self.frames[k];
        let (o1, d1) = self.frames[k + 1];
        (unit(geom::sub(o1, o0)), unit([d0[0] + d1[0], d0[1] + d1[1]]))
    }
}

impl Lattice {
    pub fn new(origin: [f32; 2], u: [f32; 2]) -> Lattice {
        Lattice {
            origin,
            u,
            cells: BTreeMap::new(),
            strip: None,
        }
    }

    /// A lattice bent along a kerb (see [`Strip`]).
    pub fn bent(strip: Strip) -> Lattice {
        let (o0, _) = strip.frames[0];
        let u = unit(geom::sub(strip.frames[1].0, o0));
        Lattice {
            origin: o0,
            u,
            cells: BTreeMap::new(),
            strip: Some(strip),
        }
    }

    /// Is this a strip bent along a curve?
    pub fn is_strip(&self) -> bool {
        self.strip.is_some()
    }

    /// Directions of `+i` and `+j` at lattice coordinate `x` (the same
    /// everywhere on a straight lattice).
    pub fn axes_at(&self, x: f32) -> ([f32; 2], [f32; 2]) {
        match &self.strip {
            Some(s) => s.axes(x),
            None => (self.u, self.v()),
        }
    }

    /// World direction of a lattice-relative facing at lattice coordinate
    /// `x`.
    pub fn dir_at(&self, x: f32, f: Facing) -> [f32; 2] {
        let (di, dj) = f.offset();
        let (u, v) = self.axes_at(x);
        [u[0] * di as f32 + v[0] * dj as f32, u[1] * di as f32 + v[1] * dj as f32]
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

    /// World direction of a lattice-relative facing (a strip: along its
    /// first column; see [`Lattice::dir_at`]).
    pub fn dir(&self, f: Facing) -> [f32; 2] {
        let (di, dj) = f.offset();
        let v = self.v();
        [
            self.u[0] * di as f32 + v[0] * dj as f32,
            self.u[1] * di as f32 + v[1] * dj as f32,
        ]
    }

    /// Lattice coordinates (in cells, fractional) of a world point. Far
    /// from a strip (beyond any cell it can hold) the answer is a point no
    /// cell is at.
    pub fn local(&self, p: [f32; 2]) -> [f32; 2] {
        if let Some(s) = &self.strip {
            return s.local(p).unwrap_or([-1.0e7, -1.0e7]);
        }
        let d = geom::sub(p, self.origin);
        [geom::dot(d, self.u) / CELL_SIZE, geom::dot(d, self.v()) / CELL_SIZE]
    }

    /// World point at lattice coordinates (in cells).
    pub fn world_at(&self, x: f32, y: f32) -> [f32; 2] {
        if let Some(s) = &self.strip {
            return s.at(x, y);
        }
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
    /// Every block, sorted by segment, side, then position along it.
    pub blocks: Vec<Block>,
    /// The centre line and serial each segment's blocks were made for.
    pub(crate) built_for: BTreeMap<SegmentId, (crate::Curve, u64)>,
    /// `versions.roads` the blocks were made for.
    pub(crate) roads_version: u64,
    /// Bumped whenever any cell's zone changes (not when buildings come
    /// and go): growth's candidate lists follow it.
    pub(crate) zones_version: u64,
    /// Strips that grid a whole block of land enclosed by straight roads
    /// (see `faces`; derived, not saved: recomputed on load).
    #[serde(skip)]
    pub(crate) face_lattices: BTreeSet<LatticeId>,
    /// How the bands of one block's grid join (derived like
    /// `face_lattices`).
    #[serde(skip)]
    pub(crate) band_links: BTreeMap<LatticeId, BandLink>,
}

/// Where one band of a block's grid meets the next: an L-shaped block's
/// grid is two bands of rows, the second starting where the first ends,
/// column for column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandLink {
    /// The band before (its last row meets this band's row 0).
    pub prev: Option<LatticeId>,
    /// The band after (its row 0 meets this band's last row).
    pub next: Option<LatticeId>,
    /// Rows of this band.
    pub rows: i32,
}

impl Default for Zoning {
    fn default() -> Self {
        Zoning {
            lattices: vec![Some(Lattice::world())],
            blocks: Vec::new(),
            built_for: BTreeMap::new(),
            roads_version: 0,
            zones_version: 0,
            face_lattices: BTreeSet::new(),
            band_links: BTreeMap::new(),
        }
    }
}

impl Zoning {
    /// Is this lattice the grid of a whole block of land enclosed by
    /// straight roads (one strip per band of rows between two of its
    /// kerbs; see `faces`)? Its columns run from kerb to kerb across the
    /// block, its rows are stretched or squeezed to whole cells.
    pub fn is_block_grid(&self, id: LatticeId) -> bool {
        self.face_lattices.contains(&id)
    }

    /// The cell next to `c` towards `f` (lattice axes): the same
    /// lattice's neighbour, or across the boundary of two bands of one
    /// block's grid the cell of the next band (see [`BandLink`]).
    pub fn cell_across(&self, c: CellRef, f: Facing) -> CellRef {
        let n = c.step(f);
        if let Some(link) = self.band_links.get(&c.lattice) {
            if n.j < 0
                && let Some(p) = link.prev
                && let Some(pl) = self.band_links.get(&p)
            {
                return CellRef::new(p, n.i, pl.rows - 1);
            }
            if n.j >= link.rows
                && let Some(x) = link.next
            {
                return CellRef::new(x, n.i, 0);
            }
        }
        n
    }

    pub fn lattice(&self, id: LatticeId) -> Option<&Lattice> {
        self.lattices.get(id.0 as usize).and_then(|l| l.as_ref())
    }

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

    /// Outline of a block in world metres: along its `min[1]` edge from
    /// `min[0]` to `max[0] + 1`, then back along its far edge (4 corners on
    /// a straight lattice, every column boundary on a strip).
    pub fn block_outline(&self, b: &Block) -> Option<Vec<[f32; 2]>> {
        self.rect_outline(b.lattice, b.min, b.max)
    }

    /// Outline of the cell rectangle `min..=max` of a lattice (see
    /// [`Zoning::block_outline`]).
    pub fn rect_outline(&self, lattice: LatticeId, min: [i32; 2], max: [i32; 2]) -> Option<Vec<[f32; 2]>> {
        let l = self.lattice(lattice)?;
        let (y0, y1) = (min[1] as f32, (max[1] + 1) as f32);
        let xs: Vec<f32> = if l.is_strip() {
            (min[0]..=max[0] + 1).map(|x| x as f32).collect()
        } else {
            vec![min[0] as f32, (max[0] + 1) as f32]
        };
        let mut out: Vec<[f32; 2]> = xs.iter().map(|&x| l.world_at(x, y0)).collect();
        out.extend(xs.iter().rev().map(|&x| l.world_at(x, y1)));
        Some(out)
    }

    /// The quadrilateral a building's lot covers (its cell rectangle's four
    /// corners; on a strip the lot narrows or widens with the bend).
    /// Front-left, front-right, back-right, back-left seen from the road.
    pub fn lot_quad(&self, fp: &crate::Footprint) -> Option<[[f32; 2]; 4]> {
        let l = self.lattice(fp.lattice)?;
        let (x0, y0, x1, y1) = (
            fp.min[0] as f32,
            fp.min[1] as f32,
            (fp.max[0] + 1) as f32,
            (fp.max[1] + 1) as f32,
        );
        let c = [
            l.world_at(x0, y0),
            l.world_at(x1, y0),
            l.world_at(x1, y1),
            l.world_at(x0, y1),
        ];
        // Order the corners by the front direction.
        let f = [libm::cosf(fp.rotation), libm::sinf(fp.rotation)];
        let r = [-f[1], f[0]];
        let ctr = [(c[0][0] + c[2][0]) * 0.5, (c[0][1] + c[2][1]) * 0.5];
        let key = |p: [f32; 2]| {
            let d = geom::sub(p, ctr);
            (geom::dot(d, f) > 0.0, geom::dot(d, r) > 0.0)
        };
        let pick = |front: bool, right: bool| c.iter().copied().find(|p| key(*p) == (front, right)).unwrap_or(ctr);
        Some([
            pick(true, false),
            pick(true, true),
            pick(false, true),
            pick(false, false),
        ])
    }

    /// World corners of a block (same order as cell corners; on a strip
    /// only its four corners: see [`Zoning::block_outline`]).
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
        if l.is_strip() || area > l.cells.len() as i64 * 4 {
            for &(i, j) in l.cells.keys() {
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

/// A cell and its corners.
type CellQuad = (CellRef, [[f32; 2]; 4]);

/// How far from a click on bare land [`fill_area`] looks for the land
/// around it, metres.
const FILL_REACH_M: f32 = 48.0;

/// The usable cell nearest to `pos` within `FILL_REACH_M` that no road
/// separates from it (the middle of a ring, beyond the deepest row).
fn nearest_land(world: &World, pos: [f32; 2]) -> Option<CellRef> {
    let r = FILL_REACH_M;
    let q = [
        [pos[0] - r, pos[1] - r],
        [pos[0] + r, pos[1] - r],
        [pos[0] + r, pos[1] + r],
        [pos[0] - r, pos[1] + r],
    ];
    let mut best: Option<(f32, CellRef)> = None;
    for c in cells_in_quad(world, &q) {
        if !world.zoning.cell(c).is_some_and(|x| x.usable()) {
            continue;
        }
        let Some(p) = world.zoning.cell_center(c) else { continue };
        let d = geom::dist(p, pos);
        if d > r || best.is_some_and(|b| d >= b.0) {
            continue;
        }
        let line = crate::Curve::straight(pos, p);
        let crosses = world.roads.segments.values().any(|s| {
            let (lo, hi) = s.curve.bounds();
            let (a, b) = (pos[0].min(p[0]), pos[0].max(p[0]));
            let (e, f) = (pos[1].min(p[1]), pos[1].max(p[1]));
            hi[0] + 4.0 >= a
                && lo[0] - 4.0 <= b
                && hi[1] + 4.0 >= e
                && lo[1] - 4.0 <= f
                && (!line.intersections(&s.curve).is_empty() || s.curve.nearest(pos).1 < 4.0)
        });
        if !crosses {
            best = Some((d, c));
        }
    }
    best.map(|b| b.1)
}

/// The connected area of usable cells around `pos` (for a fill tool):
/// cells reachable from the cell under `pos` through usable cells of the
/// same lattice sharing a side, or of another lattice whose outline comes
/// within a metre (rows of different roads meeting); at most `limit`
/// cells, nearest first in breadth-first order. A road between two cells
/// separates them, so this is the land enclosed by roads. Clicked on bare
/// land (the unzoned middle of a ring), it starts from the nearest land
/// within `FILL_REACH_M` that no road separates from the click. Empty when
/// there is none.
pub fn fill_area(world: &World, pos: [f32; 2], limit: usize) -> Vec<CellRef> {
    let z = &world.zoning;
    let usable = |c: CellRef| z.cell(c).is_some_and(|c| c.usable());
    let Some(start) = z
        .cell_at(pos)
        .filter(|&c| usable(c))
        .or_else(|| nearest_land(world, pos))
    else {
        return Vec::new();
    };
    // Usable cells of lattices other than the start's by 16 m buckets.
    const B: f32 = 16.0;
    let key = |p: [f32; 2]| (libm::floorf(p[0] / B) as i32, libm::floorf(p[1] / B) as i32);
    let mut index: std::collections::HashMap<(i32, i32), Vec<CellQuad>> = std::collections::HashMap::new();
    let multi = z.lattices().nth(1).is_some();
    if multi {
        for (c, x) in z.cells() {
            if x.usable()
                && let Some(q) = z.cell_corners(c)
            {
                let ctr = [(q[0][0] + q[2][0]) * 0.5, (q[0][1] + q[2][1]) * 0.5];
                index.entry(key(ctr)).or_default().push((c, q));
            }
        }
    }
    let grow = |q: [[f32; 2]; 4], m: f32| {
        let ctr = [(q[0][0] + q[2][0]) * 0.5, (q[0][1] + q[2][1]) * 0.5];
        q.map(|p| {
            let d = geom::sub(p, ctr);
            let l = geom::dot(d, d).sqrt().max(1e-3);
            [p[0] + d[0] / l * m, p[1] + d[1] / l * m]
        })
    };
    let mut seen = BTreeSet::new();
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
        if multi && let Some(q) = z.cell_corners(c) {
            let g = grow(q, 0.5);
            let (k0, k1) = (key(q[0]), key(q[2]));
            let (x0, x1) = (k0.0.min(k1.0) - 1, k0.0.max(k1.0) + 1);
            let (y0, y1) = (k0.1.min(k1.1) - 1, k0.1.max(k1.1) + 1);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    for (o, oq) in index.get(&(x, y)).map(|v| v.as_slice()).unwrap_or(&[]) {
                        if o.lattice != c.lattice && crate::blocks::quads_overlap(&g, &grow(*oq, 0.5)) {
                            next.push(*o);
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

/// One piece of a walk along the roads: part of a segment, travelled
/// forwards (`a` to `b`) or backwards, between two distances from its `a`
/// end (`from_m` where the walk enters, `to_m` where it leaves).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoadPiece {
    pub segment: SegmentId,
    pub forward: bool,
    pub from_m: f32,
    pub to_m: f32,
}

/// The shortest walk along the roads from a point on one segment (`from`
/// = segment and metres from its `a` end) to a point on another, through
/// any junctions; `None` when they are not connected within `max_m`.
pub fn road_walk(world: &World, from: (SegmentId, f32), to: (SegmentId, f32), max_m: f32) -> Option<Vec<RoadPiece>> {
    let roads = &world.roads;
    let s0 = roads.segments.get(from.0)?;
    let s1 = roads.segments.get(to.0)?;
    if from.0 == to.0 {
        return Some(vec![RoadPiece {
            segment: from.0,
            forward: to.1 >= from.1,
            from_m: from.1,
            to_m: to.1,
        }]);
    }
    // Dijkstra over nodes, seeded from both ends of the first segment.
    let mut dist: BTreeMap<NodeId, (f32, Option<(SegmentId, NodeId)>)> = BTreeMap::new();
    let mut open: Vec<(f32, NodeId)> = Vec::new();
    for (n, d) in [(s0.a, from.1), (s0.b, s0.length - from.1)] {
        if dist.get(&n).is_none_or(|x| d < x.0) {
            dist.insert(n, (d, None));
            open.push((d, n));
        }
    }
    let targets = [(s1.a, to.1), (s1.b, s1.length - to.1)];
    let mut done: BTreeSet<NodeId> = BTreeSet::new();
    let mut best: Option<(f32, NodeId)> = None;
    while let Some(k) = (0..open.len()).min_by(|&x, &y| open[x].0.total_cmp(&open[y].0).then(open[x].1.cmp(&open[y].1)))
    {
        let (d, n) = open.swap_remove(k);
        if !done.insert(n) || d > max_m || best.is_some_and(|b| d >= b.0) {
            continue;
        }
        for &(t, extra) in &targets {
            if t == n && best.is_none_or(|b| d + extra < b.0) {
                best = Some((d + extra, n));
            }
        }
        let Some(node) = roads.nodes.get(n) else { continue };
        for &s in &node.segments {
            if s == from.0 || s == to.0 {
                continue;
            }
            let Some(seg) = roads.segments.get(s) else { continue };
            let m = if seg.a == n { seg.b } else { seg.a };
            let nd = d + seg.length;
            if dist.get(&m).is_none_or(|x| nd < x.0) {
                dist.insert(m, (nd, Some((s, n))));
                open.push((nd, m));
            }
        }
    }
    let (_, end) = best?;
    // Walk back from the node where the last segment is entered.
    let mut mids: Vec<(SegmentId, NodeId)> = Vec::new();
    let mut n = end;
    while let Some(&(_, Some((s, prev)))) = dist.get(&n) {
        mids.push((s, prev));
        n = prev;
    }
    mids.reverse();
    let start = n;
    let mut out = Vec::new();
    out.push(RoadPiece {
        segment: from.0,
        forward: start == s0.b,
        from_m: from.1,
        to_m: if start == s0.b { s0.length } else { 0.0 },
    });
    for (s, enter) in mids {
        let seg = &roads.segments[s];
        let fwd = seg.a == enter;
        out.push(RoadPiece {
            segment: s,
            forward: fwd,
            from_m: if fwd { 0.0 } else { seg.length },
            to_m: if fwd { seg.length } else { 0.0 },
        });
    }
    let fwd = end == s1.a;
    out.push(RoadPiece {
        segment: to.0,
        forward: fwd,
        from_m: if fwd { 0.0 } else { s1.length },
        to_m: to.1,
    });
    Some(out)
}

/// The stretch of road around `segment` between two junctions, dead ends
/// or corners sharper than 45 degrees (through every smooth node where
/// just two segments meet), as a walk in `segment`'s direction.
pub fn stretch_walk(world: &World, segment: SegmentId) -> Vec<RoadPiece> {
    let roads = &world.roads;
    let Some(seg) = roads.segments.get(segment) else {
        return Vec::new();
    };
    // (segment, same direction as `segment`?), in order.
    let mut stretch: Vec<(SegmentId, bool)> = vec![(segment, true)];
    for forward in [true, false] {
        let mut end = if forward { seg.b } else { seg.a };
        loop {
            let node = &roads.nodes[end];
            if node.segments.len() != 2 {
                break;
            }
            let Some(&n) = node.segments.iter().find(|&&x| stretch.iter().all(|&(y, _)| y != x)) else {
                break;
            };
            // Through bends, not round a corner sharper than 45 degrees.
            let Some(&prev) = node.segments.iter().find(|&&x| x != n) else {
                break;
            };
            let (din, dout) = (roads.segments[prev].dir_from(end), roads.segments[n].dir_from(end));
            if geom::dot(din, dout) > -libm::cosf(45f32.to_radians()) {
                break;
            }
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
    stretch
        .into_iter()
        .map(|(s, fwd)| {
            let len = roads.segments[s].length;
            RoadPiece {
                segment: s,
                forward: fwd,
                from_m: if fwd { 0.0 } else { len },
                to_m: if fwd { len } else { 0.0 },
            }
        })
        .collect()
}

/// Cells along a walk on one side of it (`side` seen in the direction of
/// travel), up to `depth` cells from the road (1 = the front row): the
/// cells of the blocks of each piece's segment on that side whose middle
/// lies beside the travelled part (half a cell of slack at each end).
/// Usable cells only, without duplicates, in the order travelled.
pub fn frontage_walk(world: &World, walk: &[RoadPiece], side: Side, depth: u8) -> Vec<CellRef> {
    let mut out: Vec<CellRef> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let flip = |s: Side| if s == Side::Left { Side::Right } else { Side::Left };
    for p in walk {
        let Some(seg) = world.roads.segments.get(p.segment) else {
            continue;
        };
        let want = if p.forward { side } else { flip(side) };
        let (lo, hi) = (
            p.from_m.min(p.to_m) - CELL_SIZE * 0.5,
            p.from_m.max(p.to_m) + CELL_SIZE * 0.5,
        );
        let table = seg.curve.arc_table();
        let mut cells: Vec<(f32, CellRef)> = Vec::new();
        for b in world.zoning.blocks_of(p.segment).filter(|b| b.side == want) {
            for c in b.cells() {
                if b.depth_of(c) >= depth as i32 || !world.zoning.cell(c).is_some_and(|x| x.usable()) {
                    continue;
                }
                let Some(ctr) = world.zoning.cell_center(c) else {
                    continue;
                };
                let d = seg.curve.nearest_with(&table, ctr).0;
                if (lo..=hi).contains(&d) {
                    cells.push((if p.forward { d } else { -d }, c));
                }
            }
        }
        cells.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        for (_, c) in cells {
            if seen.insert(c) {
                out.push(c);
            }
        }
    }
    out
}

/// Cells of the blocks on one side of a road, along the stretch between
/// the two junctions, dead ends or sharp corners around `segment` (through
/// bends and smooth joins of pieces, see [`stretch_walk`]), up to `depth`
/// cells from the
/// road (1 = the front row). `side` is seen travelling along `segment`
/// from `a` to `b`. Usable cells only, without duplicates, ordered along
/// the road.
pub fn frontage(world: &World, segment: SegmentId, side: Side, depth: u8) -> Vec<CellRef> {
    frontage_walk(world, &stretch_walk(world, segment), side, depth)
}

/// The land a usable cell holds, where it differs from its quadrilateral:
/// where the rows of two roads meet at an angle, each cell is cut along the
/// line where the two roads' rows are equally deep (instead of a staircase
/// of whole cells), and a cell whose neighbour behind it lost the place to
/// the other road's rows takes that neighbour's land up to the line. The
/// land of all usable cells then tiles the zoned area up to clean edges.
/// A convex polygon (empty when nothing is left); `None` when the cell is
/// exactly its quadrilateral. Renderers draw zones, lot markers and yards
/// with it. See [`cell_shapes`] for all at once.
pub fn cell_shape(world: &World, c: CellRef) -> Option<Vec<[f32; 2]>> {
    LandShapes::new(world).shape(world, c)
}

/// [`cell_shape`] for many cells: the blocks indexed once (valid while the
/// roads, and so the blocks, stay as they are).
pub struct LandShapes {
    idx: crate::blocks::BlockIndex,
}

impl LandShapes {
    pub fn new(world: &World) -> LandShapes {
        LandShapes {
            idx: crate::blocks::BlockIndex::new(&world.zoning),
        }
    }

    pub fn shape(&self, world: &World, c: CellRef) -> Option<Vec<[f32; 2]>> {
        crate::blocks::cell_shape(&world.zoning, &self.idx, c, &mut Vec::new())
    }
}

/// [`cell_shape`] of every usable cell whose land differs from its
/// quadrilateral.
pub fn cell_shapes(world: &World) -> BTreeMap<CellRef, Vec<[f32; 2]>> {
    let z = &world.zoning;
    let mut out = BTreeMap::new();
    if z.lattices().nth(1).is_none() {
        return out;
    }
    let idx = crate::blocks::BlockIndex::new(z);
    let mut scratch = Vec::new();
    for (c, x) in z.cells() {
        if x.usable()
            && let Some(s) = crate::blocks::cell_shape(z, &idx, c, &mut scratch)
        {
            out.insert(c, s);
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

// ------------------------------------------------------------- invariants ----

/// Zoning invariants (`World::check_invariants`): blocks made for the
/// current roads, cells pointing at buildings that stand on them, no zone
/// under a road, every covered cell inside a block, buildings holding all
/// their cells, and usable
/// cells of different lattices never overlapping.
pub(crate) fn check(world: &World) -> Result<(), String> {
    let z = &world.zoning;
    if z.lattice(LatticeId::WORLD).is_none() {
        return Err("world lattice missing".into());
    }
    if z.roads_version != world.versions.roads {
        return Err(format!(
            "blocks made for roads version {} but roads are at {}",
            z.roads_version, world.versions.roads
        ));
    }
    for (s, _) in world.roads.segments.iter() {
        if !z.built_for.contains_key(&s) {
            return Err(format!("{s:?} has no blocks"));
        }
    }
    for b in &z.blocks {
        if !world.roads.segments.contains_key(b.segment) {
            return Err(format!("block of missing {:?}", b.segment));
        }
        if z.lattice(b.lattice).is_none() {
            return Err(format!("block on missing {:?}", b.lattice));
        }
    }
    let mut in_blocks: std::collections::BTreeSet<CellRef> = std::collections::BTreeSet::new();
    for b in &z.blocks {
        in_blocks.extend(b.cells());
    }
    for (c, cell) in z.cells() {
        if let Some(b) = cell.building {
            match world.buildings.get(b) {
                Some(bb) if bb.footprint.contains(c) => {}
                _ => return Err(format!("{c:?} references bad building {b:?}")),
            }
        }
        if cell.blocked == Some(Blocked::Road) && cell.zone.is_some() {
            return Err(format!("{c:?} is under a road and zoned"));
        }
        let in_block = in_blocks.contains(&c);
        if cell.covered != in_block {
            return Err(format!("{c:?} covered {} but in a block: {in_block}", cell.covered));
        }
        if !cell.covered && cell.zone.is_none() && cell.building.is_none() {
            return Err(format!("{c:?} is an empty leftover"));
        }
    }
    for (id, b) in world.buildings.iter() {
        let fp = &b.footprint;
        for c in fp.cells() {
            if z.cell(c).and_then(|x| x.building) != Some(id) {
                return Err(format!("{id:?} does not hold {c:?}"));
            }
        }
    }
    // Usable cells of different lattices do not overlap.
    let usable: Vec<(CellRef, [[f32; 2]; 4])> = z
        .cells()
        .filter(|(_, x)| x.usable() || x.building.is_some())
        .map(|(c, _)| {
            let l = z.lattice(c.lattice).expect("lattice");
            let k = 0.5 / CELL_SIZE;
            let (x, y) = (c.i as f32, c.j as f32);
            (
                c,
                [
                    l.world_at(x + k, y + k),
                    l.world_at(x + 1.0 - k, y + k),
                    l.world_at(x + 1.0 - k, y + 1.0 - k),
                    l.world_at(x + k, y + 1.0 - k),
                ],
            )
        })
        .collect();
    if z.lattices().nth(1).is_some() {
        let mut tiles: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
        for (k, (_, q)) in usable.iter().enumerate() {
            let t = Cell::from_pos(q[0]);
            tiles.entry((t.x, t.y)).or_default().push(k);
        }
        for (k, (c, q)) in usable.iter().enumerate() {
            let t = Cell::from_pos(q[0]);
            for dy in -2..=2 {
                for dx in -2..=2 {
                    for &o in tiles.get(&(t.x + dx, t.y + dy)).map(|v| v.as_slice()).unwrap_or(&[]) {
                        let (oc, oq) = &usable[o];
                        if o > k && oc.lattice != c.lattice && crate::blocks::quads_overlap(q, oq) {
                            return Err(format!("{c:?} overlaps {oc:?}"));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// After loading an older save: zones and buildings of its world grid move
/// onto the blocks of their roads (the world lattice; a zoned cell no block
/// covers stays as an uncovered cell, so zoning survives).
pub(crate) fn adopt_grid(world: &mut World, zones: &[(Cell, Zone)]) {
    world.zoning = Zoning::default();
    let doomed = crate::blocks::sync(world);
    debug_assert!(doomed.is_empty());
    for &(c, zone) in zones {
        let l = world.zoning.lattice_mut(LatticeId::WORLD).expect("world lattice");
        let e = l.cells.entry((c.x, c.y)).or_default();
        if e.blocked != Some(Blocked::Road) {
            e.zone = Some(zone);
        }
    }
    let ids: Vec<BuildingId> = world.buildings.keys().collect();
    for id in ids {
        let cells: Vec<CellRef> = world.buildings[id].footprint.cells().collect();
        crate::growth::claim(world, cells.into_iter(), Some(id));
    }
    // Other lattices (diagonal roads) give way to the buildings.
    if world.zoning.lattices().nth(1).is_some() {
        let doomed = crate::blocks::refresh_boxes(world, &[([0.0, 0.0], [crate::MAP_SIZE_M, crate::MAP_SIZE_M])]);
        debug_assert!(doomed.is_empty());
    }
    world.zoning.zones_version += 1;
    world.versions.grid += 1;
}
