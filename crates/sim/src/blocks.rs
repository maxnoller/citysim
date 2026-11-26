//! Generating zone blocks from the road network and resolving their cells
//! (see `zoning` for the model).
//!
//! [`sync`] runs after every road edit: blocks of new, changed or removed
//! segments are regenerated, then the cells in the boxes around them are
//! recomputed. A cell's state is a pure function of the blocks, roads and
//! buildings around it, so recomputing any region gives the same result as
//! recomputing the whole map.

use std::collections::BTreeSet;

use crate::curve::{self, Curve, right_of, unit};
use crate::roads::{RoadNetwork, SegmentId};
use crate::zoning::{Block, Blocked, CellRef, Lattice, LatticeId, Side, Strip, Zoning, facing_bit};
use crate::{BuildingId, CELL_SIZE, Facing, MAP_SIZE_M, World, balance, geom};

/// Side length of the tiles of the spatial indices, metres.
const TILE: f32 = 32.0;
/// Tiles kept beyond each map edge (blocks of roads along the edge reach
/// off the map).
const BORDER: i64 = 3;
const TILES: usize = (MAP_SIZE_M / TILE) as usize + 2 * BORDER as usize;
/// Cells are shrunk by this much per side before overlap tests (touching
/// or rounding-level overlaps do not count).
const SHRINK: f32 = 0.5;
/// Road reserve test: a cell is under a road when its square, shrunk by
/// this much per side, comes closer than the reserve to the centre line.
const ROAD_SHRINK: f32 = 0.75;

/// Geometry of a block before its lattice is matched.
struct Draft {
    side: Side,
    origin: [f32; 2],
    u: [f32; 2],
    min: [i32; 2],
    max: [i32; 2],
    front: Facing,
    sub: u32,
    /// Straight segments: the `a` end and the length. Such a block may
    /// slide along its road (less than half a cell) onto a lattice whose
    /// rows line up with it; it then covers the cells centred within half
    /// a cell beyond either end.
    span: Option<([f32; 2], f32)>,
}

pub(crate) fn opposite(f: Facing) -> Facing {
    match f {
        Facing::PlusX => Facing::MinusX,
        Facing::MinusX => Facing::PlusX,
        Facing::PlusY => Facing::MinusY,
        Facing::MinusY => Facing::PlusY,
    }
}

/// Blocks of a straight segment, in lattice-free form.
fn drafts(curve: &Curve, depth: i32) -> Vec<Draft> {
    let len = curve.length();
    let a = curve.start();
    let u = unit(geom::sub(curve.end(), a));
    let v = right_of(u);
    // Cell (0, 0) is centred on the `a` node; the road is row 0.
    let origin = [
        a[0] - (u[0] + v[0]) * CELL_SIZE * 0.5,
        a[1] - (u[1] + v[1]) * CELL_SIZE * 0.5,
    ];
    let n = libm::floorf(len / CELL_SIZE + balance::LATTICE_OFFSET_TOL) as i32;
    vec![
        Draft {
            side: Side::Right,
            origin,
            u,
            min: [0, 1],
            max: [n, depth],
            front: Facing::MinusY,
            sub: 0,
            span: Some((a, len)),
        },
        Draft {
            side: Side::Left,
            origin,
            u,
            min: [0, -depth],
            max: [n, -1],
            front: Facing::PlusY,
            sub: 0,
            span: Some((a, len)),
        },
    ]
}

/// Distance from the centre line to a curved road's kerb line (where its
/// first row of cells starts), metres.
const KERB_M: f32 = CELL_SIZE * 0.5;
/// Narrowest and widest a strip cell's edge may be before the rows stop
/// (the inside of a bend narrows them, the outside widens them), metres.
const STRIP_MIN_W: f32 = 4.5;
const STRIP_MAX_W: f32 = 12.5;

/// Kerb line frames of one side of a curved segment: columns of about one
/// cell measured along the middle of the first row, fitted to the length
/// between `s0` and the end minus `s1` (metres of that middle line; negative
/// values extend the strip straight on past the end).
fn strip_frames(curve: &Curve, side: Side, s0: f32, s1: f32) -> Vec<([f32; 2], [f32; 2])> {
    let sgn = if side == Side::Right { 1.0 } else { -1.0 };
    let table = curve.arc_table();
    let len = curve.length_with(&table);
    let mid = curve.flatten_offset(sgn * (KERB_M + CELL_SIZE * 0.5), 0.05);
    let mut cum = vec![0.0f32];
    for w in mid.windows(2) {
        let l = cum.last().copied().unwrap_or(0.0) + geom::dist(w[0].1, w[1].1);
        cum.push(l);
    }
    let lref = *cum.last().unwrap_or(&0.0);
    let (a, b) = (s0, lref - s1);
    let n = (libm::roundf((b - a) / CELL_SIZE) as i32).max(1);
    let w = (b - a) / n as f32;
    let start = curve.pose_at_with(&table, 0.0, 0.0);
    let end = curve.pose_at_with(&table, len, 0.0);
    (0..=n)
        .map(|k| {
            let s = a + k as f32 * w;
            let (pos, dir) = if s <= 0.0 {
                (
                    [start.pos[0] + start.dir[0] * s, start.pos[1] + start.dir[1] * s],
                    start.dir,
                )
            } else if s >= lref {
                let e = s - lref;
                ([end.pos[0] + end.dir[0] * e, end.pos[1] + end.dir[1] * e], end.dir)
            } else {
                let q = cum.partition_point(|&c| c < s).clamp(1, cum.len() - 1);
                let (c0, c1) = (cum[q - 1], cum[q]);
                let f = if c1 > c0 {
                    ((s - c0) / (c1 - c0)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let d = mid[q - 1].0 + (mid[q].0 - mid[q - 1].0) * f;
                let p = curve.pose_at_with(&table, d, 0.0);
                (p.pos, p.dir)
            };
            let n = right_of(dir);
            let off = sgn * KERB_M;
            ([pos[0] + n[0] * off, pos[1] + n[1] * off], n)
        })
        .collect()
}

/// Rows a strip column can hold away from its kerb (`out` = +1 when the
/// cells lie along `+j`, -1 along `-j`), at most `max`: each row's back
/// edge stays between `STRIP_MIN_W` and `STRIP_MAX_W` long and the cell
/// does not fold over.
fn column_depth(s: &Strip, k: usize, out: f32, max: i32) -> i32 {
    let (o0, d0) = s.frames[k];
    let (o1, d1) = s.frames[k + 1];
    let front = geom::sub(o1, o0);
    let fl = libm::sqrtf(geom::dot(front, front));
    if fl < STRIP_MIN_W * 0.5 {
        return 0;
    }
    for r in 0..max {
        let y = out * (r + 1) as f32 * CELL_SIZE;
        let p0 = [o0[0] + d0[0] * y, o0[1] + d0[1] * y];
        let p1 = [o1[0] + d1[0] * y, o1[1] + d1[1] * y];
        let e = geom::sub(p1, p0);
        let w = geom::dist(p0, p1);
        if !(STRIP_MIN_W..=STRIP_MAX_W).contains(&w) || geom::dot(e, front) <= 0.0 {
            return r;
        }
    }
    max
}

/// A block's cell rectangle and the side facing the road: `(min, max,
/// front)`.
type Rect = ([i32; 2], [i32; 2], Facing);

/// Blocks of strip columns `c0..=c1` (lattice `i`), each as deep as its
/// columns allow (runs of equal depth share a block): `(min, max, front)`.
fn strip_rects(st: &Strip, c0: i32, c1: i32, out: f32, max: i32) -> Vec<Rect> {
    let depth = |i: i32| column_depth(st, (i - st.i0) as usize, out, max);
    let mut rects = Vec::new();
    let mut k = c0;
    while k <= c1 {
        let d = depth(k);
        let mut e = k;
        while e < c1 && depth(e + 1) == d {
            e += 1;
        }
        if d > 0 {
            rects.push(if out > 0.0 {
                ([k, 0], [e, d - 1], Facing::MinusY)
            } else {
                ([k, -d], [e, -1], Facing::PlusY)
            });
        }
        k = e + 1;
    }
    rects
}

/// Sign of a side: +1 right of travel, -1 left.
fn side_sign(side: Side) -> f32 {
    if side == Side::Right { 1.0 } else { -1.0 }
}

/// An existing strip whose kerb line is this side's kerb (the curve was
/// split or rebuilt in place): its lattice, the columns whose middles lie
/// along the curve, and which way from the kerb its cells lie (+1: `+j`).
fn find_strip(z: &Zoning, curve: &Curve, side: Side) -> Option<(LatticeId, i32, i32, f32)> {
    let sgn = side_sign(side);
    let table = curve.arc_table();
    let len = curve.length_with(&table);
    let samples: Vec<([f32; 2], [f32; 2])> = [0.0, 0.25, 0.5, 0.75, 1.0]
        .iter()
        .map(|f| {
            let p = curve.pose_at_with(&table, len * f, 0.0);
            let n = right_of(p.dir);
            (
                [p.pos[0] + n[0] * sgn * KERB_M, p.pos[1] + n[1] * sgn * KERB_M],
                [n[0] * sgn, n[1] * sgn],
            )
        })
        .collect();
    let (lo, hi) = curve.bounds();
    'lattices: for (id, l) in z.lattices() {
        let Some(st) = &l.strip else { continue };
        let r = KERB_M + 1.0;
        if st.hi[0] < lo[0] - r || st.lo[0] > hi[0] + r || st.hi[1] < lo[1] - r || st.lo[1] > hi[1] + r {
            continue;
        }
        let mut xs = Vec::new();
        let mut out = 0.0;
        for &(p, n) in &samples {
            let Some(loc) = st.local(p) else { continue 'lattices };
            if loc[1].abs() > 0.03 || loc[0] < st.i0 as f32 - 0.5 || loc[0] > (st.i0 + st.columns()) as f32 + 0.5 {
                continue 'lattices;
            }
            let (_, v) = st.axes(loc[0]);
            let d = geom::dot(v, n);
            if d.abs() < 0.995 || (out != 0.0 && d.signum() != out) {
                continue 'lattices;
            }
            out = d.signum();
            xs.push(loc[0]);
        }
        let (x0, x1) = (xs[0].min(xs[4]), xs[0].max(xs[4]));
        let c0 = (libm::ceilf(x0 - 0.5) as i32).max(st.i0);
        let c1 = (libm::floorf(x1 - 0.5) as i32).min(st.i0 + st.columns() - 1);
        return Some((id, c0, c1, out));
    }
    None
}

/// Where a strip must start (metres along it, positive into the curve)
/// so that its first column meets the end of the block of a straight road
/// continuing it smoothly through `node` on the same side; 0 when there is
/// none. `t_in` points from the node into the curve, `out` away from the
/// road on this side.
fn joint_shift(
    z: &Zoning,
    roads: &RoadNetwork,
    s: SegmentId,
    node: crate::roads::NodeId,
    t_in: [f32; 2],
    out: [f32; 2],
) -> f32 {
    let Some(n) = roads.nodes.get(node) else { return 0.0 };
    if n.segments.len() != 2 {
        return 0.0;
    }
    let Some(&o) = n.segments.iter().find(|&&x| x != s) else {
        return 0.0;
    };
    let Some(other) = roads.segments.get(o) else { return 0.0 };
    if !other.curve.is_straight() || geom::dot(other.dir_from(node), t_in) > -0.999 {
        return 0.0;
    }
    for b in z.blocks_of(o) {
        let Some(c) = z.block_corners(b) else { continue };
        let mid = [(c[0][0] + c[2][0]) * 0.5, (c[0][1] + c[2][1]) * 0.5];
        if geom::dot(geom::sub(mid, n.pos), out) <= 0.0 {
            continue;
        }
        let e = c
            .iter()
            .map(|p| geom::dot(geom::sub(*p, n.pos), t_in))
            .fold(f32::NEG_INFINITY, f32::max);
        if e.abs() <= CELL_SIZE * 0.6 {
            return e;
        }
    }
    0.0
}

/// The strip lattice of one side of a curved segment (an existing one
/// when the segment lies along it, else a new one fitted to the segment)
/// and its blocks: `(lattice, [(min, max, front)])`.
fn place_strip(z: &mut Zoning, roads: &RoadNetwork, s: SegmentId, side: Side, depth: i32) -> (LatticeId, Vec<Rect>) {
    let seg = &roads.segments[s];
    let curve = &seg.curve;
    if let Some((id, c0, c1, out)) = find_strip(z, curve, side) {
        let st = z.lattice(id).and_then(|l| l.strip.as_ref()).expect("strip");
        return (id, strip_rects(st, c0, c1, out, depth));
    }
    let sgn = side_sign(side);
    let sd = curve.start_dir();
    let ed = curve.end_dir();
    let n0 = right_of(sd);
    let n1 = right_of(ed);
    let s0 = joint_shift(z, roads, s, seg.a, sd, [n0[0] * sgn, n0[1] * sgn]);
    let s1 = joint_shift(z, roads, s, seg.b, [-ed[0], -ed[1]], [n1[0] * sgn, n1[1] * sgn]);
    let st = Strip::new(0, strip_frames(curve, side, s0, s1));
    let rects = strip_rects(&st, 0, st.columns() - 1, sgn, depth);
    let l = Lattice::bent(st);
    let id = match z.lattices.iter().position(|x| x.is_none()) {
        Some(k) => {
            z.lattices[k] = Some(l);
            LatticeId(k as u16)
        }
        None => {
            z.lattices.push(Some(l));
            LatticeId((z.lattices.len() - 1) as u16)
        }
    };
    (id, rects)
}

/// Index of the lattice a draft lives on (an existing one it coincides
/// with, else a new one), and the draft's rectangle and front in it.
fn place(z: &mut Zoning, d: &Draft) -> (LatticeId, [i32; 2], [i32; 2], Facing) {
    let mut cand = Lattice::new(d.origin, d.u);
    let tol_a = balance::LATTICE_ANGLE_TOL_DEG.to_radians();
    let near = |x: f32| (x - libm::roundf(x)).abs() <= balance::LATTICE_OFFSET_TOL;
    let mut found: Option<LatticeId> = None;
    let mut slid = false;
    // The far corner of the draft's first row: the lattice must line up
    // there too (a slightly turned lattice drifts along a long road).
    let far = cand.world_at((d.max[0] + 1) as f32, d.min[1] as f32);
    for (id, l) in z.lattices() {
        if l.is_strip() {
            continue;
        }
        let a = curve::angle_between(l.u, d.u);
        let q = std::f32::consts::FRAC_PI_2;
        let r = a - libm::roundf(a / q) * q;
        if r.abs() > tol_a {
            continue;
        }
        let loc = l.local(d.origin);
        let loc_far = l.local(far);
        if near(loc[0]) && near(loc[1]) && near(loc_far[0]) && near(loc_far[1]) {
            found = Some(id);
            break;
        }
        // A straight road whose rows line up but whose cells are shifted
        // along it (split at a point between cell centres): slide it.
        if d.span.is_some() {
            let k = libm::roundf(a / q) as i32;
            let (perp, perp_far) = if k.rem_euclid(2) == 0 {
                (loc[1], loc_far[1])
            } else {
                (loc[0], loc_far[0])
            };
            if near(perp) && near(perp_far) {
                found = Some(id);
                cand.origin = l.world_at(libm::roundf(loc[0]), libm::roundf(loc[1]));
                slid = true;
                break;
            }
        }
    }
    let id = match found {
        Some(id) => id,
        None => {
            // A new lattice, its +i axis turned into [0°, 90°).
            let mut u = d.u;
            for _ in 0..4 {
                if u[0] > 0.0 && u[1] >= 0.0 || (u[1].abs() < 1e-7 && u[0] > 0.0) {
                    break;
                }
                u = right_of(u);
            }
            let l = Lattice::new(d.origin, u);
            match z.lattices.iter().position(|x| x.is_none()) {
                Some(k) => {
                    z.lattices[k] = Some(l);
                    LatticeId(k as u16)
                }
                None => {
                    z.lattices.push(Some(l));
                    LatticeId((z.lattices.len() - 1) as u16)
                }
            }
        }
    };
    let l = z.lattice(id).expect("placed");
    // The draft's rows (and, slid, the cells within half a cell beyond the
    // ends) mapped through world space.
    let (mut i0, mut i1) = (d.min[0], d.max[0]);
    if let (true, Some((a, len))) = (slid, d.span) {
        let along = |i: i32| geom::dot(geom::sub(cand.cell_center(i, d.min[1]), a), d.u);
        let lim = CELL_SIZE * 0.5 - 0.01;
        i0 = (d.min[0] - 1..=d.max[0] + 1)
            .find(|&i| along(i) >= -lim)
            .unwrap_or(d.min[0]);
        i1 = (d.min[0] - 1..=d.max[0] + 1)
            .rev()
            .find(|&i| along(i) <= len + lim)
            .unwrap_or(d.max[0]);
    }
    let a = l.cell_of(cand.cell_center(i0, d.min[1]));
    let b = l.cell_of(cand.cell_center(i1, d.max[1]));
    let min = [a.0.min(b.0), a.1.min(b.1)];
    let max = [a.0.max(b.0), a.1.max(b.1)];
    let fd = cand.dir(d.front);
    let front = Facing::ALL
        .into_iter()
        .max_by(|x, y| geom::dot(l.dir(*x), fd).total_cmp(&geom::dot(l.dir(*y), fd)))
        .expect("four facings");
    (id, min, max, front)
}

/// World bounds of a block.
fn block_bounds(z: &Zoning, b: &Block) -> ([f32; 2], [f32; 2]) {
    let c = z.block_outline(b).expect("block lattice exists");
    bounds_of(&c)
}

fn bounds_of(c: &[[f32; 2]]) -> ([f32; 2], [f32; 2]) {
    let lo = c
        .iter()
        .fold([f32::INFINITY; 2], |a, p| [a[0].min(p[0]), a[1].min(p[1])]);
    let hi = c
        .iter()
        .fold([f32::NEG_INFINITY; 2], |a, p| [a[0].max(p[0]), a[1].max(p[1])]);
    (lo, hi)
}

fn tile_range(lo: [f32; 2], hi: [f32; 2]) -> (usize, usize, usize, usize) {
    let t = |x: f32| (libm::floorf(x / TILE).max(-(BORDER as f32)) as i64 + BORDER).clamp(0, TILES as i64 - 1) as usize;
    (t(lo[0]), t(lo[1]), t(hi[0]), t(hi[1]))
}

/// Tile buckets of items by their bounds.
struct Tiles {
    items: Vec<Vec<u32>>,
}

impl Tiles {
    fn new() -> Tiles {
        Tiles {
            items: vec![Vec::new(); TILES * TILES],
        }
    }

    fn add(&mut self, k: u32, lo: [f32; 2], hi: [f32; 2]) {
        let (x0, y0, x1, y1) = tile_range(lo, hi);
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.items[y * TILES + x].push(k);
            }
        }
    }

    /// Items whose tiles touch the box (may repeat).
    fn query(&self, lo: [f32; 2], hi: [f32; 2], out: &mut Vec<u32>) {
        out.clear();
        let (x0, y0, x1, y1) = tile_range(lo, hi);
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.extend_from_slice(&self.items[y * TILES + x]);
            }
        }
        out.sort_unstable();
        out.dedup();
    }
}

/// Spatial context for resolving cells.
struct Ctx {
    blocks: Tiles,
    bbounds: Vec<([f32; 2], [f32; 2])>,
    roads: Tiles,
    /// Road polylines (centre line) and reserve, by index.
    polys: Vec<(Vec<[f32; 2]>, f32, [f32; 2], [f32; 2])>,
    buildings: Tiles,
    bpolys: Vec<(BuildingId, LatticeId, [[f32; 2]; 4])>,
}

/// The tiles some box touches.
struct Mask {
    on: Vec<bool>,
    count: usize,
}

impl Mask {
    fn new(boxes: &[([f32; 2], [f32; 2])]) -> Mask {
        let mut on = vec![false; TILES * TILES];
        let mut count = 0;
        for &(lo, hi) in boxes {
            let (x0, y0, x1, y1) = tile_range(lo, hi);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if !on[y * TILES + x] {
                        on[y * TILES + x] = true;
                        count += 1;
                    }
                }
            }
        }
        Mask { on, count }
    }

    /// Does the box `lo..hi`, grown by `m`, touch a marked tile?
    fn touches(&self, lo: [f32; 2], hi: [f32; 2], m: f32) -> bool {
        let (x0, y0, x1, y1) = tile_range([lo[0] - m, lo[1] - m], [hi[0] + m, hi[1] + m]);
        (y0..=y1).any(|y| (x0..=x1).any(|x| self.on[y * TILES + x]))
    }

    fn contains(&self, p: [f32; 2]) -> bool {
        let (x, y, _, _) = tile_range(p, p);
        self.on[y * TILES + x]
    }

    /// Marked tiles as boxes.
    fn tiles(&self) -> impl Iterator<Item = ([f32; 2], [f32; 2])> + '_ {
        (0..TILES * TILES).filter(|&k| self.on[k]).map(|k| {
            let (x, y) = ((k % TILES) as f32 - BORDER as f32, (k / TILES) as f32 - BORDER as f32);
            ([x * TILE, y * TILE], [(x + 1.0) * TILE, (y + 1.0) * TILE])
        })
    }
}

impl Ctx {
    /// Indices of what lies near the boxes.
    fn new(world: &World, mask: &Mask) -> Ctx {
        let z = &world.zoning;
        let m = 3.0 * CELL_SIZE;
        let mut blocks = Tiles::new();
        let mut bbounds = Vec::with_capacity(z.blocks.len());
        for (k, b) in z.blocks.iter().enumerate() {
            let (lo, hi) = block_bounds(z, b);
            if mask.touches(lo, hi, m) {
                blocks.add(k as u32, lo, hi);
            }
            bbounds.push((lo, hi));
        }
        let mut roads = Tiles::new();
        let mut polys = Vec::new();
        for seg in world.roads.segments.values() {
            let (lo, hi) = seg.curve.bounds();
            if !mask.touches(lo, hi, m) {
                continue;
            }
            let pts = seg.curve.flatten(0.05);
            let r = seg.kind.reserve_half_width_m();
            let (lo, hi) = bounds_of(&pts);
            roads.add(polys.len() as u32, [lo[0] - r, lo[1] - r], [hi[0] + r, hi[1] + r]);
            polys.push((pts, r, lo, hi));
        }
        let mut buildings = Tiles::new();
        let mut bpolys = Vec::new();
        for (id, b) in world.buildings.iter() {
            let c = z.lot_quad(&b.footprint).unwrap_or_else(|| b.corners());
            let (lo, hi) = bounds_of(&c);
            if !mask.touches(lo, hi, m) {
                continue;
            }
            buildings.add(bpolys.len() as u32, lo, hi);
            bpolys.push((id, b.footprint.lattice, c));
        }
        Ctx {
            blocks,
            bbounds,
            roads,
            polys,
            buildings,
            bpolys,
        }
    }
}

/// Cell square shrunk by `s` per side.
fn square(l: &Lattice, i: i32, j: i32, s: f32) -> [[f32; 2]; 4] {
    let k = s / CELL_SIZE;
    let (x, y) = (i as f32, j as f32);
    [
        l.world_at(x + k, y + k),
        l.world_at(x + 1.0 - k, y + k),
        l.world_at(x + 1.0 - k, y + 1.0 - k),
        l.world_at(x + k, y + 1.0 - k),
    ]
}

/// Separating-axis overlap of two convex quads.
pub(crate) fn quads_overlap(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> bool {
    for poly in [a, b] {
        for k in 0..4 {
            let e = geom::sub(poly[(k + 1) % 4], poly[k]);
            let n = [-e[1], e[0]];
            let proj = |q: &[[f32; 2]; 4]| {
                q.iter()
                    .map(|p| geom::dot(*p, n))
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)))
            };
            let (alo, ahi) = proj(a);
            let (blo, bhi) = proj(b);
            if ahi <= blo || bhi <= alo {
                return false;
            }
        }
    }
    true
}

/// Distance from a polyline to a convex quad (0 when they touch).
fn poly_quad_dist(pts: &[[f32; 2]], q: &[[f32; 2]; 4]) -> f32 {
    let mut best = f32::INFINITY;
    for w in pts.windows(2) {
        if in_quad_strict(q, w[0]) {
            return 0.0;
        }
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            if curve::segments_meet(w[0], w[1], a, b).is_some() {
                return 0.0;
            }
            best = best
                .min(crate::build::point_seg_dist(w[0], a, b))
                .min(crate::build::point_seg_dist(w[1], a, b))
                .min(crate::build::point_seg_dist(a, w[0], w[1]));
        }
    }
    if pts.len() == 1 {
        best = best.min(q.iter().map(|&c| geom::dist(c, pts[0])).fold(f32::INFINITY, f32::min));
    }
    best
}

fn in_quad_strict(q: &[[f32; 2]; 4], p: [f32; 2]) -> bool {
    crate::zoning::in_quad(q, p)
}

/// Is cell `(i, j)` of `l` under a road's reserve?
fn under_road(ctx: &Ctx, l: &Lattice, i: i32, j: i32, scratch: &mut Vec<u32>) -> bool {
    let sq = square(l, i, j, ROAD_SHRINK);
    let (lo, hi) = bounds_of(&sq);
    ctx.roads.query(lo, hi, scratch);
    scratch.iter().any(|&k| {
        let (pts, r, plo, phi) = &ctx.polys[k as usize];
        if plo[0] > hi[0] + r || phi[0] < lo[0] - r || plo[1] > hi[1] + r || phi[1] < lo[1] - r {
            return false;
        }
        poly_quad_dist(pts, &sq) < *r
    })
}

/// Regenerate blocks for changed segments and recompute the cells around
/// them. Returns buildings whose cells a road now covers (to demolish).
pub(crate) fn sync(world: &mut World) -> Vec<BuildingId> {
    if world.zoning.roads_version == world.versions.roads {
        return Vec::new();
    }
    world.zoning.roads_version = world.versions.roads;
    let roads: &RoadNetwork = &world.roads;
    // Segments whose blocks must change.
    let mut changed: BTreeSet<SegmentId> = BTreeSet::new();
    for (&s, sig) in &world.zoning.built_for {
        match roads.segments.get(s) {
            Some(seg) if seg.curve == sig.0 && seg.serial == sig.1 => {}
            _ => {
                changed.insert(s);
            }
        }
    }
    for (s, _) in roads.segments.iter() {
        if !world.zoning.built_for.contains_key(&s) {
            changed.insert(s);
        }
    }
    if changed.is_empty() {
        return Vec::new();
    }
    let started = std::time::Instant::now();
    let depth = balance::BLOCK_DEPTH.min(balance::MAX_BLOCK_DEPTH) as i32;
    let mut boxes: Vec<([f32; 2], [f32; 2])> = Vec::new();
    // Old blocks and old road reserves go.
    let z = &mut world.zoning;
    let mut keep = Vec::with_capacity(z.blocks.len());
    for b in std::mem::take(&mut z.blocks) {
        if changed.contains(&b.segment) {
            boxes.push(block_bounds(z, &b));
        } else {
            keep.push(b);
        }
    }
    z.blocks = keep;
    for s in &changed {
        if let Some((c, _)) = z.built_for.remove(s) {
            boxes.push(c.bounds());
        }
    }
    // New blocks: straight roads first (a curve continuing one lines its
    // first column up with the straight road's block).
    let mut order: Vec<SegmentId> = changed.iter().copied().collect();
    order.sort_by_key(|&s| (roads.segments.get(s).is_some_and(|x| !x.curve.is_straight()), s));
    for s in order {
        let Some(seg) = roads.segments.get(s) else { continue };
        boxes.push(seg.curve.bounds());
        if seg.curve.is_straight() {
            for d in drafts(&seg.curve, depth) {
                let (lattice, min, max, front) = place(z, &d);
                let b = Block {
                    segment: s,
                    side: d.side,
                    lattice,
                    min,
                    max,
                    front,
                    priority: (seg.serial, d.sub),
                };
                boxes.push(block_bounds(z, &b));
                z.blocks.push(b);
            }
        } else {
            for side in [Side::Right, Side::Left] {
                let (lattice, rects) = place_strip(z, roads, s, side, depth);
                for (k, (min, max, front)) in rects.into_iter().enumerate() {
                    let b = Block {
                        segment: s,
                        side,
                        lattice,
                        min,
                        max,
                        front,
                        priority: (seg.serial, k as u32),
                    };
                    boxes.push(block_bounds(z, &b));
                    z.blocks.push(b);
                }
            }
        }
        z.built_for.insert(s, (seg.curve, seg.serial));
    }
    z.blocks.sort_by(|a, b| {
        a.segment
            .cmp(&b.segment)
            .then(a.side.cmp(&b.side))
            .then(a.priority.cmp(&b.priority))
    });
    let m = 2.0 * CELL_SIZE;
    let boxes: Vec<([f32; 2], [f32; 2])> = boxes
        .into_iter()
        .map(|(lo, hi)| ([lo[0] - m, lo[1] - m], [hi[0] + m, hi[1] + m]))
        .collect();
    let doomed = resolve(world, &boxes);
    gc_lattices(&mut world.zoning);
    world.versions.grid += 1;
    world.cache.last_block_sync_ns = started.elapsed().as_nanos() as u64;
    doomed
}

/// Make every block again (an older save's zoning): zones move onto the
/// new cells, buildings left on cells no block covers any more move onto
/// the new lots under them where those are free (`growth::reseat`).
pub(crate) fn renew_all(world: &mut World) {
    world.zoning.built_for.clear();
    world.zoning.roads_version = world.versions.roads.wrapping_add(1);
    let doomed = sync(world);
    world.cache.forget_zoning();
    crate::growth::remove_buildings(world, &doomed);
    crate::growth::reseat(world);
    world.cache.forget_zoning();
}

/// Recompute every cell whose centre lies in one of the boxes (after
/// buildings changed there, for example).
pub(crate) fn refresh_boxes(world: &mut World, boxes: &[([f32; 2], [f32; 2])]) -> Vec<BuildingId> {
    let m = 2.0 * CELL_SIZE;
    let boxes: Vec<([f32; 2], [f32; 2])> = boxes
        .iter()
        .map(|&(lo, hi)| ([lo[0] - m, lo[1] - m], [hi[0] + m, hi[1] + m]))
        .collect();
    let d = resolve(world, &boxes);
    world.versions.grid += 1;
    d
}

/// Drop lattices nothing refers to (no blocks, no cells, no buildings).
fn gc_lattices(z: &mut Zoning) {
    let mut used = vec![false; z.lattices.len()];
    if let Some(u) = used.first_mut() {
        *u = true;
    }
    for b in &z.blocks {
        used[b.lattice.0 as usize] = true;
    }
    for (k, l) in z.lattices.iter_mut().enumerate() {
        if let Some(lat) = l
            && !used[k]
            && lat.cells.is_empty()
        {
            *l = None;
        }
    }
    while z.lattices.len() > 1 && z.lattices.last().is_some_and(|l| l.is_none()) {
        z.lattices.pop();
    }
}

/// Recompute the cells with centres in the boxes.
fn resolve(world: &mut World, boxes: &[([f32; 2], [f32; 2])]) -> Vec<BuildingId> {
    let mask = Mask::new(boxes);
    let ctx = Ctx::new(world, &mask);
    let z = &world.zoning;
    // Candidate cells: block cells and existing cells with their centre in
    // a marked tile.
    let mut cand: BTreeSet<CellRef> = BTreeSet::new();
    let mut scratch: Vec<u32> = Vec::new();
    for (k, b) in z.blocks.iter().enumerate() {
        let (lo, hi) = ctx.bbounds[k];
        if !mask.touches(lo, hi, 0.0) {
            continue;
        }
        let l = z.lattice(b.lattice).expect("block lattice");
        for c in b.cells() {
            if mask.contains(l.cell_center(c.i, c.j)) {
                cand.insert(c);
            }
        }
    }
    let lattices = z.lattices().count();
    if mask.count * lattices > 4 * z.lattices().map(|(_, l)| l.cells.len()).sum::<usize>().max(1) / 64 {
        for (c, _) in z.cells() {
            let l = z.lattice(c.lattice).expect("lattice");
            if mask.contains(l.cell_center(c.i, c.j)) {
                cand.insert(c);
            }
        }
    } else {
        // Strips: few cells each, and no straight bounds to search by.
        for (id, l) in z.lattices().filter(|(_, l)| l.is_strip()) {
            for &(i, j) in l.cells.keys() {
                if mask.contains(l.cell_center(i, j)) {
                    cand.insert(CellRef::new(id, i, j));
                }
            }
        }
        for (lo, hi) in mask.tiles() {
            for (id, l) in z.lattices().filter(|(_, l)| !l.is_strip()) {
                let corners = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]];
                let loc: Vec<[f32; 2]> = corners.iter().map(|&p| l.local(p)).collect();
                let (llo, lhi) = bounds_of(&loc);
                let (i0, i1) = (libm::floorf(llo[0]) as i32 - 1, libm::ceilf(lhi[0]) as i32 + 1);
                let (j0, j1) = (libm::floorf(llo[1]) as i32 - 1, libm::ceilf(lhi[1]) as i32 + 1);
                for (&(i, j), _) in l.cells.range((i0, i32::MIN)..=(i1, i32::MAX)) {
                    if j >= j0 && j <= j1 && mask.contains(l.cell_center(i, j)) {
                        cand.insert(CellRef::new(id, i, j));
                    }
                }
            }
        }
    }
    // Road cover of cells, memoised (also needed for contest partners).
    let mut road_memo: std::collections::HashMap<CellRef, bool> = std::collections::HashMap::new();
    let mut road_at = |c: CellRef, scratch: &mut Vec<u32>| -> bool {
        if let Some(&r) = road_memo.get(&c) {
            return r;
        }
        let l = z.lattice(c.lattice).expect("lattice");
        let r = under_road(&ctx, l, c.i, c.j, scratch);
        road_memo.insert(c, r);
        r
    };
    struct New {
        covered: bool,
        fronts: u8,
        blocked: Option<Blocked>,
    }
    let mut results: Vec<(CellRef, New)> = Vec::with_capacity(cand.len());
    let mut s2: Vec<u32> = Vec::new();
    for &c in &cand {
        let l = z.lattice(c.lattice).expect("lattice");
        let centre = l.cell_center(c.i, c.j);
        ctx.blocks.query(centre, centre, &mut scratch);
        let mut covered = false;
        let mut fronts = 0u8;
        // Who keeps a place where cells of two lattices overlap: the cell
        // nearer its own road (fewer rows from it), then the older road.
        let mut prio = (i32::MAX, u64::MAX, SegmentId::default(), u32::MAX);
        for &k in &scratch {
            let b = &z.blocks[k as usize];
            if b.contains(c) {
                covered = true;
                prio = prio.min((b.depth_of(c), b.priority.0, b.segment, b.priority.1));
                if b.in_front_row(c) {
                    fronts |= facing_bit(b.front);
                }
            }
        }
        let existing = z.cell(c);
        let has_building = existing.is_some_and(|x| x.building.is_some());
        let mut blocked = None;
        if covered || has_building {
            if road_at(c, &mut s2) {
                blocked = Some(Blocked::Road);
            } else if !has_building {
                let sq = square(l, c.i, c.j, SHRINK);
                let (lo, hi) = bounds_of(&sq);
                // Another lattice's building over it.
                ctx.buildings.query(lo, hi, &mut s2);
                if s2.iter().any(|&k| {
                    let (_, bl, poly) = &ctx.bpolys[k as usize];
                    *bl != c.lattice && quads_overlap(&sq, poly)
                }) {
                    blocked = Some(Blocked::Building);
                }
                // An older block's cell in another lattice.
                if blocked.is_none() {
                    let mut near: Vec<u32> = Vec::new();
                    ctx.blocks.query(lo, hi, &mut near);
                    'outer: for &k in &near {
                        let b = &z.blocks[k as usize];
                        if b.lattice == c.lattice || (0, b.priority.0, b.segment, b.priority.1) >= prio {
                            continue;
                        }
                        let (blo, bhi) = ctx.bbounds[k as usize];
                        if blo[0] > hi[0] || bhi[0] < lo[0] || blo[1] > hi[1] || bhi[1] < lo[1] {
                            continue;
                        }
                        let bl = z.lattice(b.lattice).expect("lattice");
                        let loc: Vec<[f32; 2]> = sq.iter().map(|&p| bl.local(p)).collect();
                        let (llo, lhi) = bounds_of(&loc);
                        let (i0, i1) = (
                            (libm::floorf(llo[0]) as i32).max(b.min[0]),
                            (libm::floorf(lhi[0]) as i32).min(b.max[0]),
                        );
                        let (j0, j1) = (
                            (libm::floorf(llo[1]) as i32).max(b.min[1]),
                            (libm::floorf(lhi[1]) as i32).min(b.max[1]),
                        );
                        for j in j0..=j1 {
                            for i in i0..=i1 {
                                let oc = CellRef::new(b.lattice, i, j);
                                if (b.depth_of(oc), b.priority.0, b.segment, b.priority.1) >= prio {
                                    continue;
                                }
                                let other = square(bl, i, j, SHRINK);
                                if quads_overlap(&sq, &other) && !road_at(oc, &mut s2) {
                                    blocked = Some(Blocked::Contested);
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }
        results.push((
            c,
            New {
                covered,
                fronts,
                blocked,
            },
        ));
    }
    // Apply.
    let mut doomed: Vec<BuildingId> = Vec::new();
    let mut created: Vec<CellRef> = Vec::new();
    let mut emptied: Vec<CellRef> = Vec::new();
    let zm = &mut world.zoning;
    for (c, n) in results {
        let l = zm.lattice_mut(c.lattice).expect("lattice");
        let entry = l.cells.get(&(c.i, c.j)).copied();
        let mut cell = entry.unwrap_or_default();
        cell.covered = n.covered;
        cell.fronts = if n.covered { n.fronts } else { 0 };
        cell.blocked = n.blocked;
        if n.blocked == Some(Blocked::Road) {
            cell.zone = None;
            if let Some(b) = cell.building {
                doomed.push(b);
            }
        }
        if cell.covered || cell.zone.is_some() || cell.building.is_some() {
            // Land that just became usable here (a new cell, or one that
            // took the place from another lattice's cell).
            if entry.is_none_or(|e| !e.usable()) && cell.usable() {
                created.push(c);
            }
            l.cells.insert((c.i, c.j), cell);
        } else if entry.is_some() {
            l.cells.remove(&(c.i, c.j));
            emptied.push(c);
        }
    }
    // Land that became usable takes the zone of a cell of another lattice
    // at the same place that is not usable any more: an uncovered one (its
    // road went) hands it over; a contested one (a nearer road's rows took
    // the place) keeps it too, for when it is usable again. Zoning
    // survives road edits.
    for c in created {
        let Some(p) = zm.cell_center(c) else { continue };
        if !zm.cell(c).is_some_and(|x| x.usable() && x.zone.is_none()) {
            continue;
        }
        let mut take: Option<(f32, CellRef, crate::Zone, bool)> = None;
        for (id, l) in zm.lattices() {
            if id == c.lattice {
                continue;
            }
            // The other lattice's cell under this one's middle.
            let (i, j) = l.cell_of(p);
            if let Some(x) = l.cells.get(&(i, j))
                && !x.usable()
                && x.building.is_none()
                && let Some(zone) = x.zone
            {
                let d = geom::dist(l.cell_center(i, j), p);
                if take.is_none_or(|t| d < t.0) {
                    take = Some((d, CellRef::new(id, i, j), zone, !x.covered));
                }
            }
        }
        if let Some((_, from, zone, hand_over)) = take {
            if let Some(x) = zm.cell_mut(c) {
                x.zone = Some(zone);
            }
            if hand_over && let Some(l) = zm.lattice_mut(from.lattice) {
                l.cells.remove(&(from.i, from.j));
            }
        }
    }
    let _ = emptied;
    // Zones or usability may have changed: growth's lists follow.
    zm.zones_version += 1;
    doomed.sort();
    doomed.dedup();
    doomed
}
