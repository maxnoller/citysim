//! Generating zone blocks from the road network and resolving their cells
//! (see `zoning` for the model).
//!
//! [`sync`] runs after every road edit: blocks of new, changed or removed
//! segments are regenerated, then the cells in the boxes around them are
//! recomputed. A cell's state is a pure function of the blocks, roads and
//! buildings around it, so recomputing any region gives the same result as
//! recomputing the whole map.

use std::collections::{BTreeMap, BTreeSet};

use crate::curve::{self, Curve, right_of, unit};
use crate::roads::{RoadNetwork, SegmentId};
use crate::zoning::{Block, Blocked, CellRef, Lattice, LatticeId, Side, Zoning, facing_bit};
use crate::{BuildingId, CELL_SIZE, Facing, MAP_SIZE_M, World, balance, geom};

/// Side length of the tiles of the spatial indices, metres.
const TILE: f32 = 32.0;
const TILES: usize = (MAP_SIZE_M / TILE) as usize + 2;
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

/// Blocks of one segment, in lattice-free form.
fn drafts(curve: &Curve, depth: i32) -> Vec<Draft> {
    let len = curve.length();
    let mut out = Vec::new();
    if curve.is_straight() {
        let a = curve.start();
        let u = unit(geom::sub(curve.end(), a));
        let v = right_of(u);
        // Cell (0, 0) is centred on the `a` node; the road is row 0.
        let origin = [
            a[0] - (u[0] + v[0]) * CELL_SIZE * 0.5,
            a[1] - (u[1] + v[1]) * CELL_SIZE * 0.5,
        ];
        let n = libm::floorf(len / CELL_SIZE + balance::LATTICE_OFFSET_TOL) as i32;
        out.push(Draft {
            side: Side::Right,
            origin,
            u,
            min: [0, 1],
            max: [n, depth],
            front: Facing::MinusY,
            sub: 0,
            span: Some((a, len)),
        });
        out.push(Draft {
            side: Side::Left,
            origin,
            u,
            min: [0, -depth],
            max: [n, -1],
            front: Facing::PlusY,
            sub: 0,
            span: Some((a, len)),
        });
        return out;
    }
    // Curved: straight blocks along each kerb. Cells per block from the
    // curvature (at most ~12° of turn per block).
    let (rmin, _) = curve.min_radius();
    let per = (libm::floorf(rmin * 0.21 / CELL_SIZE) as i32).clamp(1, 8);
    for side in [Side::Right, Side::Left] {
        let off = if side == Side::Right {
            CELL_SIZE * 0.5
        } else {
            -CELL_SIZE * 0.5
        };
        // The kerb as a polyline with centre-line distances.
        let kerb = curve.flatten_offset(off, 0.05);
        let mut cum = vec![0.0f32];
        for w in kerb.windows(2) {
            let l = cum.last().copied().unwrap_or(0.0) + geom::dist(w[0].1, w[1].1);
            cum.push(l);
        }
        let klen = *cum.last().unwrap_or(&0.0);
        let at = |s: f32| -> (f32, [f32; 2]) {
            let k = cum.partition_point(|&c| c < s).clamp(1, cum.len() - 1);
            let (c0, c1) = (cum[k - 1], cum[k]);
            let f = if c1 > c0 {
                ((s - c0) / (c1 - c0)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (d0, p0) = kerb[k - 1];
            let (d1, p1) = kerb[k];
            (
                d0 + (d1 - d0) * f,
                [p0[0] + (p1[0] - p0[0]) * f, p0[1] + (p1[1] - p0[1]) * f],
            )
        };
        let total = libm::floorf(klen / CELL_SIZE + balance::LATTICE_OFFSET_TOL) as i32;
        let mut k = 0;
        let mut sub = 0;
        while k < total {
            let m = per.min(total - k);
            let (s0, s1) = (k as f32 * CELL_SIZE, (k + m) as f32 * CELL_SIZE);
            let (d0, k0) = at(s0);
            let (d1, k1) = at(s1.min(klen));
            let t0 = curve.pose_at(d0, 0.0).dir;
            let t1 = curve.pose_at(d1, 0.0).dir;
            let turn = curve::angle_between(t0, t1);
            // Inside of the bend: the chord of the kerb; outside: the
            // tangent at the middle of the kerb piece.
            let inside = (turn > 0.0) == (side == Side::Right);
            let (start, u) = if inside || turn.abs() < 1e-4 {
                (k0, unit(geom::sub(k1, k0)))
            } else {
                let (dm, km) = at((s0 + s1) * 0.5);
                let t = curve.pose_at(dm, 0.0).dir;
                (
                    [
                        km[0] - t[0] * m as f32 * CELL_SIZE * 0.5,
                        km[1] - t[1] * m as f32 * CELL_SIZE * 0.5,
                    ],
                    t,
                )
            };
            let (min, max, front) = match side {
                Side::Right => ([0, 0], [m - 1, depth - 1], Facing::MinusY),
                Side::Left => ([0, -depth], [m - 1, -1], Facing::PlusY),
            };
            out.push(Draft {
                side,
                origin: start,
                u,
                min,
                max,
                front,
                sub,
                span: None,
            });
            k += m;
            sub += 1;
        }
    }
    out
}

/// Index of the lattice a draft lives on (an existing one it coincides
/// with, else a new one), and the draft's rectangle and front in it.
fn place(z: &mut Zoning, d: &Draft) -> (LatticeId, [i32; 2], [i32; 2], Facing) {
    let mut cand = Lattice::new(d.origin, d.u);
    let tol_a = balance::LATTICE_ANGLE_TOL_DEG.to_radians();
    let near = |x: f32| (x - libm::roundf(x)).abs() <= balance::LATTICE_OFFSET_TOL;
    let mut found: Option<LatticeId> = None;
    let mut slid = false;
    for (id, l) in z.lattices() {
        let a = curve::angle_between(l.u, d.u);
        let q = std::f32::consts::FRAC_PI_2;
        let r = a - libm::roundf(a / q) * q;
        if r.abs() > tol_a {
            continue;
        }
        let loc = l.local(d.origin);
        if near(loc[0]) && near(loc[1]) {
            found = Some(id);
            break;
        }
        // A straight road whose rows line up but whose cells are shifted
        // along it (split at a point between cell centres): slide it.
        if d.span.is_some() {
            let k = libm::roundf(a / q) as i32;
            let perp = if k.rem_euclid(2) == 0 { loc[1] } else { loc[0] };
            if near(perp) {
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
    let c = z.block_corners(b).expect("block lattice exists");
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
    let t = |x: f32| (libm::floorf(x / TILE).max(-1.0) as i64 + 1).clamp(0, TILES as i64 - 1) as usize;
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
    polys: Vec<(Vec<[f32; 2]>, f32)>,
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
            let (x, y) = ((k % TILES) as f32 - 1.0, (k / TILES) as f32 - 1.0);
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
            polys.push((pts, r));
        }
        let mut buildings = Tiles::new();
        let mut bpolys = Vec::new();
        for (id, b) in world.buildings.iter() {
            let c = b.corners();
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
        let (pts, r) = &ctx.polys[k as usize];
        let (plo, phi) = bounds_of(pts);
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
    // New blocks.
    for &s in &changed {
        let Some(seg) = roads.segments.get(s) else { continue };
        boxes.push(seg.curve.bounds());
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
        for (lo, hi) in mask.tiles() {
            for (id, l) in z.lattices() {
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
    let mut road_memo: BTreeMap<CellRef, bool> = BTreeMap::new();
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
        let mut prio = (u64::MAX, SegmentId::default(), u32::MAX);
        for &k in &scratch {
            let b = &z.blocks[k as usize];
            if b.contains(c) {
                covered = true;
                prio = prio.min((b.priority.0, b.segment, b.priority.1));
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
                        if b.lattice == c.lattice || (b.priority.0, b.segment, b.priority.1) >= prio {
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
                                let other = square(bl, i, j, SHRINK);
                                if quads_overlap(&sq, &other) && !road_at(CellRef::new(b.lattice, i, j), &mut s2) {
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
            if entry.is_none() {
                created.push(c);
            }
            l.cells.insert((c.i, c.j), cell);
        } else if entry.is_some() {
            l.cells.remove(&(c.i, c.j));
            emptied.push(c);
        }
    }
    // New usable cells take the zone of an uncovered cell of another
    // lattice at the same place (zoning survives road edits).
    for c in created {
        let Some(p) = zm.cell_center(c) else { continue };
        if !zm.cell(c).is_some_and(|x| x.usable() && x.zone.is_none()) {
            continue;
        }
        let mut take: Option<(CellRef, crate::Zone)> = None;
        for (id, l) in zm.lattices() {
            if id == c.lattice {
                continue;
            }
            let (i, j) = l.cell_of(p);
            for dj in -1..=1 {
                for di in -1..=1 {
                    if let Some(x) = l.cells.get(&(i + di, j + dj))
                        && !x.covered
                        && x.building.is_none()
                        && let Some(zone) = x.zone
                        && geom::dist(l.cell_center(i + di, j + dj), p) < CELL_SIZE * 0.5
                    {
                        take = take.or(Some((CellRef::new(id, i + di, j + dj), zone)));
                    }
                }
            }
        }
        if let Some((from, zone)) = take {
            if let Some(x) = zm.cell_mut(c) {
                x.zone = Some(zone);
            }
            if let Some(l) = zm.lattice_mut(from.lattice) {
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
