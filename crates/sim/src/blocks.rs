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
use crate::faces::{self, Half, Plan};
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
    let id = alloc(z, Lattice::bent(st));
    (id, rects)
}

/// A new lattice in the lowest free slot.
fn alloc(z: &mut Zoning, l: Lattice) -> LatticeId {
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

/// The lattice of one band of a block's grid: the existing one with
/// exactly this strip, else a new one.
fn face_lattice(z: &mut Zoning, st: &Strip) -> LatticeId {
    if let Some(&id) = z
        .face_lattices
        .iter()
        .find(|&&id| z.lattice(id).and_then(|l| l.strip.as_ref()) == Some(st))
    {
        return id;
    }
    let id = alloc(z, Lattice::bent(st.clone()));
    z.face_lattices.insert(id);
    id
}

/// Link the bands of a planned grid that exist (see `BandLink`).
fn link_bands(z: &mut Zoning, plan: &Plan) {
    let ids: Option<Vec<LatticeId>> = plan
        .bands
        .iter()
        .map(|st| {
            z.face_lattices
                .iter()
                .copied()
                .find(|&id| z.lattice(id).and_then(|l| l.strip.as_ref()) == Some(st))
        })
        .collect();
    let Some(ids) = ids else { return };
    for (k, &id) in ids.iter().enumerate() {
        z.band_links.insert(
            id,
            crate::zoning::BandLink {
                prev: k.checked_sub(1).map(|p| ids[p]),
                next: ids.get(k + 1).copied(),
                rows: plan.rows[k],
            },
        );
    }
}

/// The blocks of one side of a segment as they are: `(lattice, min, max,
/// front)`, sorted.
fn side_blocks(z: &Zoning, s: SegmentId, side: Side) -> Vec<(LatticeId, [i32; 2], [i32; 2], u8)> {
    let mut v: Vec<_> = z
        .blocks_of(s)
        .filter(|b| b.side == side)
        .map(|b| (b.lattice, b.min, b.max, b.front as u8))
        .collect();
    v.sort();
    v
}

/// Does one side of a segment lie on its face's grid as planned (or, with
/// no plan, on no block grid at all)?
fn side_as_planned(z: &Zoning, s: SegmentId, side: Side, plan: Option<&Plan>) -> bool {
    let have = side_blocks(z, s, side);
    let Some(plan) = plan else {
        return have.iter().all(|b| !z.face_lattices.contains(&b.0));
    };
    let mut want = Vec::new();
    for &(t, sd, band, (min, max, front)) in &plan.blocks {
        if t != s || sd != side {
            continue;
        }
        let Some(&id) = z
            .face_lattices
            .iter()
            .find(|&&id| z.lattice(id).and_then(|l| l.strip.as_ref()) == Some(&plan.bands[band]))
        else {
            return false;
        };
        want.push((id, min, max, front as u8));
    }
    want.sort();
    want == have
}

/// Faces of the road graph met while regenerating blocks: each half-edge's
/// face and each face's plan.
#[derive(Default)]
struct FacePlans {
    face_of: BTreeMap<Half, Half>,
    plans: BTreeMap<Half, (Vec<Half>, Option<Plan>)>,
}

impl FacePlans {
    /// Walk the face of `h` (once) and plan it; its key.
    fn visit(&mut self, roads: &RoadNetwork, h: Half, depth: i32) -> Option<Half> {
        if let Some(&k) = self.face_of.get(&h) {
            return Some(k);
        }
        let face = faces::walk(roads, h)?;
        let k = faces::key(&face);
        for &x in &face {
            self.face_of.insert(x, k);
        }
        let plan = faces::plan(roads, &face, depth);
        self.plans.insert(k, (face, plan));
        Some(k)
    }

    fn plan_of(&self, h: Half) -> Option<&Plan> {
        self.plans.get(self.face_of.get(&h)?)?.1.as_ref()
    }
}

/// Grow the set of segments whose blocks are made again until every face
/// around them is consistent: a segment whose face got a new grid (or lost
/// its grid) is redone, and so is every segment on a grid that one of them
/// leaves.
fn close_over_faces(z: &Zoning, roads: &RoadNetwork, changed: &mut BTreeSet<SegmentId>, depth: i32) -> FacePlans {
    let mut fp = FacePlans::default();
    let first: BTreeSet<SegmentId> = changed.clone();
    let mut todo: Vec<SegmentId> = changed.iter().copied().collect();
    // Nodes at the ends of the edited roads (before and after).
    let mut ends: Vec<[f32; 2]> = Vec::new();
    for s in &first {
        if let Some((c, _)) = z.built_for.get(s) {
            ends.extend([c.start(), c.end()]);
        }
        if let Some(seg) = roads.segments.get(*s) {
            ends.extend([seg.curve.start(), seg.curve.end()]);
        }
    }
    let mut around: Vec<SegmentId> = Vec::new();
    if !ends.is_empty() {
        for (_, n) in roads.nodes.iter() {
            if ends.iter().any(|&p| geom::same_pos(p, n.pos)) {
                around.extend(n.segments.iter().copied());
            }
        }
    }
    around.sort();
    around.dedup();
    let mut starts: Vec<Half> = around.iter().flat_map(|&s| [(s, true), (s, false)]).collect();
    while let Some(s) = todo.pop() {
        // Grids this segment's blocks are on now: all their roads redo.
        let lats: BTreeSet<LatticeId> = z
            .blocks_of(s)
            .filter(|b| z.face_lattices.contains(&b.lattice))
            .map(|b| b.lattice)
            .collect();
        if !lats.is_empty() {
            for b in &z.blocks {
                if lats.contains(&b.lattice) && changed.insert(b.segment) {
                    todo.push(b.segment);
                }
            }
        }
        if roads.segments.contains_key(s) {
            starts.extend([(s, true), (s, false)]);
        }
        for h in std::mem::take(&mut starts) {
            if fp.face_of.contains_key(&h) {
                continue;
            }
            let Some(k) = fp.visit(roads, h, depth) else { continue };
            let (face, plan) = &fp.plans[&k];
            for &x in face {
                if !changed.contains(&x.0) && !side_as_planned(z, x.0, faces::side_of(x), plan.as_ref()) {
                    changed.insert(x.0);
                    todo.push(x.0);
                }
            }
        }
    }
    fp
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
            alloc(z, Lattice::new(d.origin, u))
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
    let l = z.lattice(b.lattice).expect("block lattice exists");
    let (y0, y1) = (b.min[1] as f32, (b.max[1] + 1) as f32);
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    let mut add = |p: [f32; 2]| {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    };
    let (x0, x1) = (b.min[0], b.max[0] + 1);
    let step = if l.is_strip() { 1 } else { (x1 - x0).max(1) };
    let mut x = x0;
    loop {
        add(l.world_at(x as f32, y0));
        add(l.world_at(x as f32, y1));
        if x >= x1 {
            break;
        }
        x = (x + step).min(x1);
    }
    (lo, hi)
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

/// A road's centre line, reserve half width and bounds.
type RoadPoly = (Vec<[f32; 2]>, f32, [f32; 2], [f32; 2]);

/// Spatial context for resolving cells.
struct Ctx {
    blocks: Tiles,
    bbounds: Vec<([f32; 2], [f32; 2])>,
    roads: Tiles,
    /// Road polylines (centre line) and reserve, by index.
    polys: Vec<RoadPoly>,
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
            // Cheap test first: a circle round the lot.
            let r = (b.footprint.size[0].max(b.footprint.size[1]) as f32) * CELL_SIZE * 0.75;
            let p = b.footprint.center;
            if !mask.touches([p[0] - r, p[1] - r], [p[0] + r, p[1] + r], m) {
                continue;
            }
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
    // Blocks of land enclosed by straight roads share one grid: their
    // roads are redone together.
    let fp = close_over_faces(&world.zoning, roads, &mut changed, depth);
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
        // Sides on a block's grid.
        let mut gridded = [false; 2];
        for (k, side) in [Side::Right, Side::Left].into_iter().enumerate() {
            let Some(plan) = fp.plan_of(faces::half(s, side)) else {
                continue;
            };
            gridded[k] = true;
            for (n, &(t, sd, band, (min, max, front))) in plan.blocks.iter().enumerate() {
                if t != s || sd != side {
                    continue;
                }
                let lattice = face_lattice(z, &plan.bands[band]);
                let b = Block {
                    segment: s,
                    side,
                    lattice,
                    min,
                    max,
                    front,
                    priority: (seg.serial, n as u32),
                };
                boxes.push(block_bounds(z, &b));
                z.blocks.push(b);
            }
        }
        if seg.curve.is_straight() {
            for d in drafts(&seg.curve, depth) {
                if gridded[usize::from(d.side == Side::Left)] {
                    continue;
                }
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
            for (k, side) in [Side::Right, Side::Left].into_iter().enumerate() {
                if gridded[k] {
                    continue;
                }
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
    for (_, plan) in fp.plans.values() {
        if let Some(plan) = plan {
            link_bands(z, plan);
        }
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

/// After loading a save: find the block grids again (strips under the
/// blocks of straight roads; not stored), then make the blocks of every
/// face whose grid is not as planned again (a save from before block
/// grids): zones move onto the new cells, buildings onto the new lots
/// under them where those are free. Returns how many segments were redone.
pub(crate) fn derive_faces(world: &mut World) -> usize {
    let z = &mut world.zoning;
    let marks: BTreeSet<LatticeId> = z
        .blocks
        .iter()
        .filter(|b| {
            z.lattice(b.lattice).is_some_and(|l| l.is_strip())
                && world
                    .roads
                    .segments
                    .get(b.segment)
                    .is_some_and(|s| s.curve.is_straight())
        })
        .map(|b| b.lattice)
        .collect();
    z.face_lattices = marks;
    let depth = balance::BLOCK_DEPTH.min(balance::MAX_BLOCK_DEPTH) as i32;
    let mut fp = FacePlans::default();
    let mut redo: BTreeSet<SegmentId> = BTreeSet::new();
    for (s, _) in world.roads.segments.iter() {
        for side in [Side::Right, Side::Left] {
            let h = faces::half(s, side);
            fp.visit(&world.roads, h, depth);
            if !side_as_planned(&world.zoning, s, side, fp.plan_of(h)) {
                redo.insert(s);
            }
        }
    }
    world.zoning.band_links.clear();
    for (_, plan) in fp.plans.values() {
        if let Some(plan) = plan {
            link_bands(&mut world.zoning, plan);
        }
    }
    if redo.is_empty() {
        return 0;
    }
    for s in &redo {
        world.zoning.built_for.remove(s);
    }
    world.zoning.roads_version = world.versions.roads.wrapping_add(1);
    let doomed = sync(world);
    world.cache.forget_zoning();
    crate::growth::remove_buildings(world, &doomed);
    crate::growth::reseat(world);
    world.cache.forget_zoning();
    redo.len()
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
            z.face_lattices.remove(&LatticeId(k as u16));
            z.band_links.remove(&LatticeId(k as u16));
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
    let mut cand: Vec<CellRef> = Vec::new();
    let mut scratch: Vec<u32> = Vec::new();
    for (k, b) in z.blocks.iter().enumerate() {
        let (lo, hi) = ctx.bbounds[k];
        if !mask.touches(lo, hi, 0.0) {
            continue;
        }
        let l = z.lattice(b.lattice).expect("block lattice");
        for c in b.cells() {
            if mask.contains(l.cell_center(c.i, c.j)) {
                cand.push(c);
            }
        }
    }
    let lattices = z.lattices().count();
    if mask.count * lattices > 4 * z.lattices().map(|(_, l)| l.cells.len()).sum::<usize>().max(1) / 64 {
        for (c, _) in z.cells() {
            let l = z.lattice(c.lattice).expect("lattice");
            if mask.contains(l.cell_center(c.i, c.j)) {
                cand.push(c);
            }
        }
    } else {
        // Strips: few cells each, and no straight bounds to search by.
        for (id, l) in z.lattices().filter(|(_, l)| l.is_strip()) {
            for &(i, j) in l.cells.keys() {
                if mask.contains(l.cell_center(i, j)) {
                    cand.push(CellRef::new(id, i, j));
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
                        cand.push(CellRef::new(id, i, j));
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
    cand.sort_unstable();
    cand.dedup();
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

// ------------------------------------------------------------ land shapes ----

/// Rows from a block's road to the point at lattice coordinates `q`
/// (continuous; 0 on the kerb line), if `q` lies in the block's rectangle.
fn depth_in(b: &Block, q: [f32; 2], tol: f32) -> Option<f32> {
    let (x0, y0, x1, y1) = (
        b.min[0] as f32,
        b.min[1] as f32,
        (b.max[0] + 1) as f32,
        (b.max[1] + 1) as f32,
    );
    if q[0] < x0 - tol || q[0] > x1 + tol || q[1] < y0 - tol || q[1] > y1 + tol {
        return None;
    }
    Some(match b.front {
        Facing::MinusY => q[1] - y0,
        Facing::PlusY => y1 - q[1],
        Facing::MinusX => q[0] - x0,
        Facing::PlusX => x1 - q[0],
    })
}

/// Rows from a block's road to `q` whether or not `q` is in it.
fn depth_of_point(b: &Block, q: [f32; 2]) -> f32 {
    depth_in(b, q, f32::INFINITY).unwrap_or(f32::INFINITY)
}

/// Clip a convex polygon to where `g <= 0` (`g` linear along each edge).
fn clip_poly(poly: &[[f32; 2]], g: &dyn Fn([f32; 2]) -> f32) -> Vec<[f32; 2]> {
    let n = poly.len();
    let vals: Vec<f32> = poly.iter().map(|&p| g(p)).collect();
    let mut out = Vec::with_capacity(n + 2);
    for k in 0..n {
        let (a, b) = (poly[k], poly[(k + 1) % n]);
        let (ga, gb) = (vals[k], vals[(k + 1) % n]);
        if ga <= 0.0 {
            out.push(a);
        }
        if (ga <= 0.0) != (gb <= 0.0) {
            // Where g changes sign along the edge (refined: on a strip g is
            // not quite linear).
            let at = |t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            let mut t = (ga / (ga - gb)).clamp(0.0, 1.0);
            for _ in 0..8 {
                let v = g(at(t));
                if (v <= 0.0) == (ga <= 0.0) {
                    lo = t;
                } else {
                    hi = t;
                }
                t = (lo + hi) * 0.5;
            }
            out.push(at(t));
        }
    }
    out
}

/// Blocks by 32 m tiles, for the land-shape queries.
pub(crate) struct BlockIndex {
    tiles: Tiles,
    bounds: Vec<([f32; 2], [f32; 2])>,
}

impl BlockIndex {
    pub(crate) fn new(z: &Zoning) -> BlockIndex {
        let mut tiles = Tiles::new();
        let mut bounds = Vec::with_capacity(z.blocks.len());
        for (k, b) in z.blocks.iter().enumerate() {
            let (lo, hi) = block_bounds(z, b);
            tiles.add(k as u32, lo, hi);
            bounds.push((lo, hi));
        }
        BlockIndex { tiles, bounds }
    }

    fn near(&self, lo: [f32; 2], hi: [f32; 2], out: &mut Vec<u32>) {
        self.tiles.query(lo, hi, out);
        out.retain(|&k| {
            let (blo, bhi) = self.bounds[k as usize];
            blo[0] <= hi[0] && bhi[0] >= lo[0] && blo[1] <= hi[1] && bhi[1] >= lo[1]
        });
    }
}

/// The land a usable cell holds (see `zoning::cell_shape`), or `None` when
/// it is exactly its own quadrilateral.
pub(crate) fn cell_shape(
    z: &Zoning,
    idx: &BlockIndex,
    c: crate::zoning::CellRef,
    scratch: &mut Vec<u32>,
) -> Option<Vec<[f32; 2]>> {
    use crate::zoning::CellRef;
    let cell = z.cell(c)?;
    if !cell.usable() {
        return None;
    }
    let l = z.lattice(c.lattice)?;
    let quad = l.cell_corners(c.i, c.j);
    let (lo, hi) = bounds_of(&quad);
    let m = CELL_SIZE * 1.5;
    idx.near([lo[0] - m, lo[1] - m], [hi[0] + m, hi[1] + m], scratch);
    // Own blocks holding the cell; the nearest road's decides "behind".
    let own: Vec<&Block> = scratch
        .iter()
        .map(|&k| &z.blocks[k as usize])
        .filter(|b| b.contains(c))
        .collect();
    let front_block = own.iter().min_by_key(|b| (b.depth_of(c), b.priority))?;
    // The cells behind it that another road's rows took: their land on
    // this side of the line where the two meet joins this cell.
    let mut back = c;
    loop {
        let n = back.step(opposite(front_block.front));
        let lost = z
            .cell(n)
            .is_some_and(|x| x.covered && x.blocked == Some(crate::zoning::Blocked::Contested) && x.building.is_none())
            && front_block.contains(n);
        if !lost {
            break;
        }
        back = n;
    }
    let merge = back != c;
    let (a, b) = ([c.i.min(back.i), c.j.min(back.j)], [c.i.max(back.i), c.j.max(back.j)]);
    let (x0, y0, x1, y1) = (a[0] as f32, a[1] as f32, (b[0] + 1) as f32, (b[1] + 1) as f32);
    let mut poly = vec![
        l.world_at(x0, y0),
        l.world_at(x1, y0),
        l.world_at(x1, y1),
        l.world_at(x0, y1),
    ];
    // Other lattices' blocks around all of it.
    let (plo, phi) = bounds_of(&[poly[0], poly[1], poly[2], poly[3]]);
    idx.near([plo[0] - m, plo[1] - m], [phi[0] + m, phi[1] + m], scratch);
    let rivals: Vec<&Block> = scratch
        .iter()
        .map(|&k| &z.blocks[k as usize])
        .filter(|b| b.lattice != c.lattice)
        .collect();
    if rivals.is_empty() && !merge {
        return None;
    }
    let d_own = |p: [f32; 2]| {
        let q = l.local(p);
        own.iter().map(|b| depth_of_point(b, q)).fold(f32::INFINITY, f32::min)
    };
    let ctr = |poly: &[[f32; 2]]| {
        let n = poly.len().max(1) as f32;
        let s = poly.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
        [s[0] / n, s[1] / n]
    };
    let mut clipped = false;
    for r in rivals {
        let Some(rl) = z.lattice(r.lattice) else { continue };
        // Only where the rival's rows reach this land.
        let p0 = ctr(&poly);
        let loc: Vec<[f32; 2]> = poly.iter().map(|&p| rl.local(p)).collect();
        let rect = [
            r.min[0] as f32,
            r.min[1] as f32,
            (r.max[0] + 1) as f32,
            (r.max[1] + 1) as f32,
        ];
        if !poly_meets_rect(&loc, rect) {
            continue;
        }
        // Not where the rival's own cell there is under a road (its rows
        // running on across a road).
        let (ri, rj) = rl.cell_of(p0);
        if z.cell(CellRef::new(r.lattice, ri, rj))
            .is_some_and(|x| x.blocked == Some(crate::zoning::Blocked::Road))
        {
            continue;
        }
        let g = |p: [f32; 2]| d_own(p) - depth_of_point(r, rl.local(p));
        let before = poly.len();
        let vals: Vec<f32> = poly.iter().map(|&p| g(p)).collect();
        if vals.iter().all(|&v| v <= 0.0) {
            continue;
        }
        poly = clip_poly(&poly, &g);
        clipped |= poly.len() != before || vals.iter().any(|&v| v > 0.0);
        if poly.len() < 3 || poly_area_abs(&poly) < 0.5 {
            return Some(Vec::new());
        }
    }
    (merge || clipped).then_some(poly)
}

fn poly_area_abs(p: &[[f32; 2]]) -> f32 {
    let n = p.len();
    ((0..n).map(|k| geom::cross(p[k], p[(k + 1) % n])).sum::<f32>() * 0.5).abs()
}

/// Do a convex polygon (lattice coordinates) and the rectangle `[x0, y0,
/// x1, y1]` overlap (separating axes)?
fn poly_meets_rect(p: &[[f32; 2]], r: [f32; 4]) -> bool {
    if p.len() < 3 {
        return false;
    }
    let (lo, hi) = bounds_of(p);
    if hi[0] <= r[0] || lo[0] >= r[2] || hi[1] <= r[1] || lo[1] >= r[3] {
        return false;
    }
    let rect = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
    let n = p.len();
    for k in 0..n {
        let e = geom::sub(p[(k + 1) % n], p[k]);
        let ax = [-e[1], e[0]];
        let proj = |q: &[[f32; 2]]| {
            q.iter()
                .map(|v| geom::dot(*v, ax))
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), x| (a.min(x), b.max(x)))
        };
        let (a0, a1) = proj(p);
        let (b0, b1) = proj(&rect);
        if a1 <= b0 + 1e-4 || b1 <= a0 + 1e-4 {
            return false;
        }
    }
    true
}
