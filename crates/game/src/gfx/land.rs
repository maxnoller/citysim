//! What the land next to the roads looks like to the renderers: where the
//! drawn roads are (centre-line polylines with their drawn half width, and
//! junction pavement outlines), and how a rectangle (a building lot, a
//! zoning cell) is fitted clear of them. Rebuilt once per roads version
//! (`RoadEdges` resource); buildings, lots, the zone overlay and the trees
//! all read it.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use citysim_sim::zoning::{Lattice, ZoneCell};
use citysim_sim::{CellRef, Facing, World, Zone};

use super::road_geom;
use crate::shared::{GameSet, SimWorld};

/// Bucket size of the spatial index, metres.
const BUCKET: f32 = 64.0;
/// Clearance kept between a lot and a road's sidewalk edge, m.
pub const ROAD_CLEARANCE: f32 = 0.3;
/// A lot is never shrunk below this share of its depth / width.
pub const MIN_INSET_SCALE: f32 = 0.45;

/// Drawn road edges: polylines (sim metres) and the distance that must
/// stay clear of each (half the drawn road width incl. sidewalks, or 0 for
/// a junction's pavement outline).
#[derive(Resource, Default)]
pub struct RoadEdges {
    /// Roads key it was built for.
    pub key: Option<(u64, u64)>,
    lines: Vec<(Vec<Vec2>, f32)>,
    buckets: HashMap<IVec2, Vec<u32>>,
    /// Per bucket, a hash of the lines in it.
    hashes: HashMap<IVec2, u64>,
    /// Buckets whose lines differ from the index before (`None`: all of
    /// them, the first index or a new world), so what was fitted against the
    /// old index elsewhere can stay.
    changed: Option<std::collections::HashSet<IVec2>>,
}

fn bucket(p: Vec2) -> IVec2 {
    IVec2::new((p.x / BUCKET).floor() as i32, (p.y / BUCKET).floor() as i32)
}

impl RoadEdges {
    pub fn new(world: &World) -> RoadEdges {
        let mut lines = Vec::new();
        for (_, seg) in world.roads.segments.iter() {
            let pts: Vec<Vec2> = seg.curve.flatten(0.25).into_iter().map(Vec2::from).collect();
            lines.push((pts, road_geom::style(seg.kind).outer()));
        }
        for (n, node) in world.roads.nodes.iter() {
            let walk = node
                .segments
                .iter()
                .filter_map(|&s| world.roads.segments.get(s))
                .map(|s| road_geom::style(s.kind).walk)
                .fold(0.0, f32::max);
            if let Some(shape) = citysim_sim::junction::junction_shape(world, n, walk) {
                let mut pts: Vec<Vec2> = shape.outline.iter().map(|p| Vec2::from(*p)).collect();
                if let Some(&first) = pts.first() {
                    pts.push(first);
                }
                lines.push((pts, 0.0));
            } else if node.segments.len() == 1 {
                // Dead-end cap: a point with the road's half width.
                if let Some(s) = node.segments.first().and_then(|&s| world.roads.segments.get(s)) {
                    lines.push((vec![Vec2::from(node.pos)], road_geom::style(s.kind).outer()));
                }
            }
        }
        let mut buckets: HashMap<IVec2, Vec<u32>> = HashMap::new();
        for (i, (pts, keep)) in lines.iter().enumerate() {
            let pad = Vec2::splat(keep + ROAD_CLEARANCE);
            let lo = pts.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p)) - pad;
            let hi = pts.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p)) + pad;
            let (b0, b1) = (bucket(lo), bucket(hi));
            for y in b0.y..=b1.y {
                for x in b0.x..=b1.x {
                    buckets.entry(IVec2::new(x, y)).or_default().push(i as u32);
                }
            }
        }
        let line_hash: Vec<u64> = lines
            .iter()
            .map(|(pts, keep)| {
                let mut h = DefaultHasher::new();
                keep.to_bits().hash(&mut h);
                for p in pts {
                    p.x.to_bits().hash(&mut h);
                    p.y.to_bits().hash(&mut h);
                }
                h.finish()
            })
            .collect();
        let hashes = buckets
            .iter()
            .map(|(b, ids)| {
                let mut v: Vec<u64> = ids.iter().map(|&i| line_hash[i as usize]).collect();
                v.sort_unstable();
                let mut h = DefaultHasher::new();
                v.hash(&mut h);
                (*b, h.finish())
            })
            .collect();
        RoadEdges {
            key: None,
            lines,
            buckets,
            hashes,
            changed: None,
        }
    }

    /// Mark the buckets that differ from `old` (same world).
    fn diff(&mut self, old: &RoadEdges) {
        let mut changed = std::collections::HashSet::new();
        for (b, h) in &self.hashes {
            if old.hashes.get(b) != Some(h) {
                changed.insert(*b);
            }
        }
        for b in old.hashes.keys() {
            if !self.hashes.contains_key(b) {
                changed.insert(*b);
            }
        }
        self.changed = Some(changed);
    }

    /// Whether every line that could touch `lo..hi` is as in the index
    /// before (false after a new world or the first index).
    pub fn unchanged_in(&self, lo: Vec2, hi: Vec2) -> bool {
        let Some(changed) = &self.changed else { return false };
        let (b0, b1) = (bucket(lo), bucket(hi));
        (b0.y..=b1.y).all(|y| (b0.x..=b1.x).all(|x| !changed.contains(&IVec2::new(x, y))))
    }

    /// Indices of the lines whose padded bounds touch `lo..hi`.
    fn near(&self, lo: Vec2, hi: Vec2) -> Vec<u32> {
        let (b0, b1) = (bucket(lo), bucket(hi));
        let mut out: Vec<u32> = Vec::new();
        for y in b0.y..=b1.y {
            for x in b0.x..=b1.x {
                if let Some(v) = self.buckets.get(&IVec2::new(x, y)) {
                    out.extend(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// How far `p` is outside every drawn road (negative inside one).
    pub fn clearance(&self, p: Vec2) -> f32 {
        let mut best = f32::MAX;
        for i in self.near(p, p) {
            let (pts, keep) = &self.lines[i as usize];
            let d = if pts.len() == 1 {
                p.distance(pts[0])
            } else {
                pts.windows(2)
                    .map(|w| point_seg(p, w[0], w[1]))
                    .fold(f32::MAX, f32::min)
            };
            best = best.min(d - keep);
        }
        best
    }

    /// Every line near the lot, in the lot's frame.
    fn local_lines(&self, lot: &Lot) -> Vec<(Vec<Vec2>, f32)> {
        let r = Vec2::splat(lot.half.length() + 0.5);
        self.near(lot.c - r, lot.c + r)
            .into_iter()
            .map(|i| {
                let (pts, keep) = &self.lines[i as usize];
                (pts.iter().map(|p| lot.local(*p)).collect(), *keep)
            })
            .collect()
    }
}

/// Keep `RoadEdges` in step with the roads.
fn update_edges(sim: Res<SimWorld>, mut edges: ResMut<RoadEdges>) {
    let key = sim.key(sim.world.versions.roads);
    if edges.key != Some(key) {
        let mut new = RoadEdges::new(&sim.world);
        // Same world (not a load or a new game): note what changed.
        if edges.key.is_some_and(|(g, _)| g == key.0) {
            new.diff(&edges);
        }
        new.key = Some(key);
        *edges = new;
    }
}

/// Runs before everything else that draws.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LandUpdate;

pub struct LandPlugin;

impl Plugin for LandPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RoadEdges>()
            .configure_sets(Update, LandUpdate.in_set(GameSet::Render))
            .add_systems(Update, update_edges.in_set(LandUpdate));
    }
}

/// Distance from point `p` to segment `a..b`.
pub fn point_seg(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Distance between segment `a..b` and the rectangle `min..max` (0 when
/// they intersect).
pub fn seg_rect(a: Vec2, b: Vec2, min: Vec2, max: Vec2) -> f32 {
    // Liang-Barsky clip: does the segment enter the rectangle?
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let mut inside = true;
    for (p, q) in [
        (-d.x, a.x - min.x),
        (d.x, max.x - a.x),
        (-d.y, a.y - min.y),
        (d.y, max.y - a.y),
    ] {
        if p.abs() < 1e-9 {
            if q < 0.0 {
                inside = false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if inside && t0 <= t1 {
        return 0.0;
    }
    let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
    let to_rect = |p: Vec2| p.clamp(min, max).distance(p);
    corners
        .iter()
        .map(|&c| point_seg(c, a, b))
        .chain([to_rect(a), to_rect(b)])
        .fold(f32::MAX, f32::min)
}

/// A rectangle in its own frame: centre `c`, front `f` (unit, sim axes),
/// `half` = (half width, half depth). Its local x runs along the model's
/// x (to the left of the front direction seen from outside), local y
/// towards the front (the model's +z).
#[derive(Debug, Clone, Copy)]
pub struct Lot {
    pub c: Vec2,
    pub f: Vec2,
    pub half: Vec2,
}

impl Lot {
    /// Sim point -> lot frame.
    pub fn local(&self, p: Vec2) -> Vec2 {
        let d = p - self.c;
        Vec2::new(d.x * self.f.y - d.y * self.f.x, d.dot(self.f))
    }

    /// Lot frame -> sim point.
    pub fn world(&self, l: Vec2) -> Vec2 {
        self.c + Vec2::new(self.f.y, -self.f.x) * l.x + self.f * l.y
    }
}

/// How a rectangle is fitted clear of the roads, in its own frame: squeezed
/// (`scale.x` width, `scale.z` depth) and its centre moved by `shift`
/// (local x, y = model x, z).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inset {
    pub scale: Vec3,
    pub shift: Vec2,
}

impl Inset {
    pub const NONE: Inset = Inset {
        scale: Vec3::ONE,
        shift: Vec2::ZERO,
    };

    /// The fitted rectangle's corners in the lot frame `(min, max)`.
    pub fn rect(&self, lot: &Lot) -> (Vec2, Vec2) {
        let half = lot.half * Vec2::new(self.scale.x, self.scale.z);
        (self.shift - half, self.shift + half)
    }
}

/// Fit a rectangle clear of the roads: pull in its front, then its sides,
/// then its back, each only as far as needed (never below
/// `MIN_INSET_SCALE` of the size).
pub fn fit(lot: Lot, edges: &RoadEdges) -> Inset {
    fit_checked(lot, edges).0
}

/// `fit`, and whether the fitted rectangle really is clear (it may not be
/// when a road runs through the middle of it).
pub fn fit_checked(lot: Lot, edges: &RoadEdges) -> (Inset, bool) {
    let near = edges.local_lines(&lot);
    if near.is_empty() {
        return (Inset::NONE, true);
    }
    let clear = |min: Vec2, max: Vec2| {
        near.iter().all(|(pts, keep)| {
            if pts.len() == 1 {
                return pts[0].clamp(min, max).distance(pts[0]) >= keep + ROAD_CLEARANCE;
            }
            pts.windows(2)
                .all(|w| seg_rect(w[0], w[1], min, max) >= keep + ROAD_CLEARANCE)
        })
    };
    let (lo, hi) = (-lot.half, lot.half);
    if clear(lo, hi) {
        return (Inset::NONE, true);
    }
    // Largest share `s` (down to MIN_INSET_SCALE) for which `ok(s)` holds,
    // or None when not even the smallest is clear.
    let largest = |ok: &dyn Fn(f32) -> bool| {
        if !ok(MIN_INSET_SCALE) {
            return None;
        }
        let (mut a, mut b) = (MIN_INSET_SCALE, 1.0f32);
        for _ in 0..12 {
            let mid = (a + b) * 0.5;
            if ok(mid) {
                a = mid;
            } else {
                b = mid;
            }
        }
        Some(a)
    };
    let size = hi - lo;
    // Squeezing one side in: front (+y, the usual case: a wide road in
    // front), either side, the back. Each keeps the opposite side.
    let squeezes: [&dyn Fn(f32) -> (Vec2, Vec2); 4] = [
        &|s| (lo, Vec2::new(hi.x, lo.y + size.y * s)),
        &|s| (lo, Vec2::new(lo.x + size.x * s, hi.y)),
        &|s| (Vec2::new(hi.x - size.x * s, lo.y), hi),
        &|s| (Vec2::new(lo.x, hi.y - size.y * s), hi),
    ];
    let mut best: Option<(f32, Vec2, Vec2)> = None;
    for f in squeezes {
        if let Some(s) = largest(&|s| {
            let (a, b) = f(s);
            clear(a, b)
        }) && best.is_none_or(|b| s > b.0 + 1e-3)
        {
            let (a, b) = f(s);
            best = Some((s, a, b));
        }
    }
    let (rlo, rhi, ok) = match best {
        Some((_, a, b)) => (a, b, true),
        None => {
            // Roads on more than one side: shrink evenly towards the point
            // of the lot farthest from them.
            let pts = [
                lo,
                hi,
                Vec2::new(lo.x, hi.y),
                Vec2::new(hi.x, lo.y),
                Vec2::new(0.0, lo.y),
                Vec2::new(0.0, hi.y),
                Vec2::new(lo.x, 0.0),
                Vec2::new(hi.x, 0.0),
                Vec2::ZERO,
            ];
            let room = |p: Vec2| {
                near.iter()
                    .map(|(pts, keep)| {
                        let d = if pts.len() == 1 {
                            p.distance(pts[0])
                        } else {
                            pts.windows(2)
                                .map(|w| point_seg(p, w[0], w[1]))
                                .fold(f32::MAX, f32::min)
                        };
                        d - keep
                    })
                    .fold(f32::MAX, f32::min)
            };
            let anchor = pts
                .into_iter()
                .max_by(|a, b| room(*a).total_cmp(&room(*b)))
                .unwrap_or(Vec2::ZERO);
            let at = |s: f32| (anchor + (lo - anchor) * s, anchor + (hi - anchor) * s);
            match largest(&|s| {
                let (a, b) = at(s);
                clear(a, b)
            }) {
                Some(s) => {
                    let (a, b) = at(s);
                    (a, b, true)
                }
                None => {
                    let (a, b) = at(MIN_INSET_SCALE);
                    (a, b, false)
                }
            }
        }
    };
    (
        Inset {
            scale: Vec3::new((rhi.x - rlo.x) / size.x, 1.0, (rhi.y - rlo.y) / size.y),
            shift: (rlo + rhi) * 0.5,
        },
        ok,
    )
}

/// The lot frame of zoning cell `(i, j)` of `l`: local x along the
/// lattice's `+i`, local y along `+j` (a strip cell: the rectangle of its
/// mean width, turned to its middle).
pub fn cell_lot(l: &Lattice, i: i32, j: i32) -> Lot {
    let (_, v) = l.axes_at(i as f32 + 0.5);
    let half = if l.is_strip() {
        let q = l.cell_corners(i, j).map(Vec2::from);
        Vec2::new(
            (q[0].distance(q[1]) + q[3].distance(q[2])) * 0.25,
            citysim_sim::CELL_SIZE * 0.5,
        )
    } else {
        Vec2::splat(citysim_sim::CELL_SIZE * 0.5)
    };
    Lot {
        c: Vec2::from(l.cell_center(i, j)),
        f: Vec2::from(v),
        half,
    }
}

/// A fitted rectangle (`inset` of `lot`) laid onto the quadrilateral `q`
/// the lot stands for (corners in lattice order, as
/// `Lattice::cell_corners`): the same shares of its width and depth.
pub fn quad_corners(lot: &Lot, inset: &Inset, q: [Vec2; 4]) -> [Vec2; 4] {
    let (lo, hi) = inset.rect(lot);
    let at = |p: Vec2| {
        let s = (p.x / lot.half.x + 1.0) * 0.5;
        let t = (p.y / lot.half.y + 1.0) * 0.5;
        q[0] * (1.0 - s) * (1.0 - t) + q[1] * s * (1.0 - t) + q[2] * s * t + q[3] * (1.0 - s) * t
    };
    [at(lo), at(Vec2::new(hi.x, lo.y)), at(hi), at(Vec2::new(lo.x, hi.y))]
}

/// Fit a building's lot clear of the roads and inside the quadrilateral
/// `quad` of its cells (front-left, front-right, back-right, back-left;
/// on a curve the lot narrows or widens towards the back), keeping
/// `gap` metres from the quad's sides so neighbours never touch.
pub fn fit_in_quad(lot: Lot, quad: [Vec2; 4], gap: f32, edges: &RoadEdges) -> Inset {
    // Lot frame: x to the left of the front seen from outside, y to the
    // front. Allowed x range at the lot's front and back edges.
    let l = quad.map(|p| lot.local(p));
    // Left side: back-left (3) -> front-left (0); right: 2 -> 1.
    let x_at = |a: Vec2, b: Vec2, y: f32| {
        if (b.y - a.y).abs() < 1e-4 {
            a.x
        } else {
            a.x + (b.x - a.x) * ((y - a.y) / (b.y - a.y))
        }
    };
    let (yf, yb) = (lot.half.y, -lot.half.y);
    let (l0, l1) = (x_at(l[3], l[0], yf), x_at(l[3], l[0], yb));
    let (r0, r1) = (x_at(l[2], l[1], yf), x_at(l[2], l[1], yb));
    // The quad's two sides bound the narrower of the front and back.
    let (left, right) = if l0 < r0 {
        (l0.max(l1), r0.min(r1))
    } else {
        (r0.max(r1), l0.min(l1))
    };
    let room = ((right - left) * 0.5 - gap).max(lot.half.x * MIN_INSET_SCALE);
    let half_x = lot.half.x.min(room);
    let mid = if half_x < lot.half.x { (left + right) * 0.5 } else { 0.0 };
    let narrowed = Lot {
        c: lot.world(Vec2::new(mid, 0.0)),
        f: lot.f,
        half: Vec2::new(half_x, lot.half.y),
    };
    let i = fit(narrowed, edges);
    let k = half_x / lot.half.x;
    Inset {
        scale: Vec3::new(i.scale.x * k, i.scale.y, i.scale.z),
        shift: Vec2::new(mid + i.shift.x, i.shift.y),
    }
}

/// Corners of a fitted rectangle in sim metres, counter-clockwise in the
/// lot frame: (lo, lo), (hi, lo), (hi, hi), (lo, hi) — for a zoning cell
/// that is `(i, j)`, `(i+1, j)`, `(i+1, j+1)`, `(i, j+1)` like
/// `Lattice::cell_corners`.
pub fn corners(lot: &Lot, inset: &Inset) -> [Vec2; 4] {
    let (lo, hi) = inset.rect(lot);
    [
        lot.world(lo),
        lot.world(Vec2::new(hi.x, lo.y)),
        lot.world(hi),
        lot.world(Vec2::new(lo.x, hi.y)),
    ]
}

/// One zoning cell as the renderers draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct CellView {
    pub cell: CellRef,
    pub zone: Option<Zone>,
    /// Usable and without a building.
    pub free: bool,
    /// The cell fitted clear of the roads (see [`corners`]).
    pub quad: [Vec2; 4],
    /// Lattice directions of `+i` and `+j`.
    pub u: Vec2,
    pub v: Vec2,
    /// Sides with a road in front (`ZoneCell::fronts`).
    pub fronts: u8,
    /// The neighbouring cells in the order of the quad's edges: `-j`
    /// (corners 0-1), `+i` (1-2), `+j` (2-3), `-i` (3-0); across the band
    /// boundary of a block's grid the next band's cell.
    pub nbrs: [CellRef; 4],
    /// The land drawn for it: `quad`, or where the rows of two roads meet
    /// at an angle the cell's land up to the line between them
    /// (`zoning::cell_shape`, fitted clear of the roads). Convex.
    pub poly: Vec<Vec2>,
    /// The cell across each edge of `poly` (edge `k` runs from `poly[k]`
    /// to `poly[k + 1]`), if any is drawn there.
    pub edge_nbrs: Vec<Option<CellRef>>,
}

impl CellView {
    /// Centre of the drawn land.
    pub fn poly_center(&self) -> Vec2 {
        self.poly.iter().copied().sum::<Vec2>() / self.poly.len().max(1) as f32
    }
}

/// Fitted cell quads and cell land shapes, valid for one roads version.
#[derive(Default)]
pub struct FitCache {
    key: Option<(u64, u64)>,
    quads: HashMap<CellRef, Option<[Vec2; 4]>>,
    shapes: Option<citysim_sim::zoning::LandShapes>,
    shape_of: HashMap<CellRef, Option<Vec<Vec2>>>,
}

/// Clip a convex polygon to the side of the line `a -> b` that holds `inner`.
fn clip_half(poly: &[Vec2], a: Vec2, b: Vec2, inner: Vec2) -> Vec<Vec2> {
    let d = b - a;
    let side = |p: Vec2| d.perp_dot(p - a);
    let s0 = side(inner).signum();
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 2);
    for k in 0..n {
        let (p, q) = (poly[k], poly[(k + 1) % n]);
        let (sp, sq) = (side(p) * s0, side(q) * s0);
        if sp >= 0.0 {
            out.push(p);
        }
        if (sp >= 0.0) != (sq >= 0.0) {
            out.push(p + (q - p) * (sp / (sp - sq)));
        }
    }
    out
}

/// Is `p` on the line through `a`, `b` (within 5 cm)?
fn on_line(p: Vec2, a: Vec2, b: Vec2) -> bool {
    let d = b - a;
    let l = d.length();
    l > 1e-4 && (d.perp_dot(p - a) / l).abs() < 0.05
}

/// Even-odd point in polygon.
fn in_poly(poly: &[Vec2], p: Vec2) -> bool {
    let mut c = false;
    let n = poly.len();
    for k in 0..n {
        let (a, b) = (poly[k], poly[(k + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            c = !c;
        }
    }
    c
}

/// Usable zoning cells (`ZoneCell::usable`) that `keep` accepts, fitted
/// clear of the roads, bucketed into square chunks of `chunk` metres by
/// their centre.
pub fn cells_by_chunk(
    world: &World,
    edges: &RoadEdges,
    cache: &mut FitCache,
    chunk: f32,
    keep: impl Fn(&ZoneCell) -> bool,
) -> HashMap<IVec2, Vec<CellView>> {
    if cache.key != edges.key {
        cache.key = edges.key;
        cache.quads.clear();
        cache.shapes = None;
        cache.shape_of.clear();
    }
    let multi = world.zoning.lattices().nth(1).is_some();
    if multi && cache.shapes.is_none() {
        cache.shapes = Some(citysim_sim::zoning::LandShapes::new(world));
    }
    let mut out: HashMap<IVec2, Vec<CellView>> = HashMap::new();
    let mut shaped: Vec<(IVec2, usize)> = Vec::new();
    for (id, l) in world.zoning.lattices() {
        for (&(i, j), zc) in &l.cells {
            if !zc.usable() || !keep(zc) {
                continue;
            }
            let lot = cell_lot(l, i, j);
            let cell = CellRef::new(id, i, j);
            // Cells a road runs through, or that only a sliver of is left
            // of, are not drawn as land.
            let Some(quad) = *cache.quads.entry(cell).or_insert_with(|| {
                let (inset, ok) = fit_checked(lot, edges);
                (ok && inset.scale.x * inset.scale.z > 0.3).then(|| {
                    if l.is_strip() {
                        quad_corners(&lot, &inset, l.cell_corners(i, j).map(Vec2::from))
                    } else {
                        corners(&lot, &inset)
                    }
                })
            }) else {
                continue;
            };
            let (u, v) = l.axes_at(i as f32 + 0.5);
            let nbrs = [Facing::MinusY, Facing::PlusX, Facing::PlusY, Facing::MinusX]
                .map(|f| world.zoning.cell_across(cell, f));
            // The cell's land where it differs from its quad, fitted clear
            // of the roads like the quad (on the edges the fit moved).
            let shape = match &cache.shapes {
                Some(ls) => cache
                    .shape_of
                    .entry(cell)
                    .or_insert_with(|| ls.shape(world, cell).map(|p| p.into_iter().map(Vec2::from).collect()))
                    .clone(),
                None => None,
            };
            let raw = l.cell_corners(i, j).map(Vec2::from);
            let (poly, edge_nbrs) = match shape {
                None => (quad.to_vec(), nbrs.map(Some).to_vec()),
                Some(mut p) => {
                    let mid = (quad[0] + quad[2]) * 0.5;
                    for k in 0..4 {
                        let (a, b) = (quad[k], quad[(k + 1) % 4]);
                        if !(on_line(a, raw[k], raw[(k + 1) % 4]) && on_line(b, raw[k], raw[(k + 1) % 4])) {
                            p = clip_half(&p, a, b, mid);
                        }
                    }
                    if p.len() < 3 {
                        continue;
                    }
                    // Edges along the quad's own sides have its neighbours;
                    // the others are found below.
                    let e = (0..p.len())
                        .map(|k| {
                            let (a, b) = (p[k], p[(k + 1) % p.len()]);
                            (0..4)
                                .find(|&s| {
                                    let (c0, c1) = (quad[s], quad[(s + 1) % 4]);
                                    on_line(a, c0, c1) && on_line(b, c0, c1)
                                })
                                .map(|s| nbrs[s])
                        })
                        .collect();
                    (p, e)
                }
            };
            let key = road_geom::chunk_of(lot.c, chunk);
            let list = out.entry(key).or_default();
            if poly.len() != 4 || edge_nbrs.iter().any(Option::is_none) {
                shaped.push((key, list.len()));
            }
            list.push(CellView {
                cell,
                zone: zc.zone,
                free: zc.free(),
                quad,
                u: Vec2::from(u),
                v: Vec2::from(v),
                fronts: zc.fronts,
                nbrs,
                poly,
                edge_nbrs,
            });
        }
    }
    // Edges with no drawn cell of the same lattice beyond (cut land, or a
    // neighbour another road's rows took): the cell drawn just beyond.
    let drawn: std::collections::HashSet<CellRef> = out.values().flatten().map(|v| v.cell).collect();
    for (key, list) in &out {
        for (n, v) in list.iter().enumerate() {
            if v.edge_nbrs.iter().any(|e| e.is_none_or(|c| !drawn.contains(&c))) {
                shaped.push((*key, n));
            }
        }
    }
    shaped.sort_by_key(|(k, n)| (k.x, k.y, *n));
    shaped.dedup();
    if !shaped.is_empty() {
        let bucket = |p: Vec2| IVec2::new((p.x / 16.0).floor() as i32, (p.y / 16.0).floor() as i32);
        let mut index: HashMap<IVec2, Vec<(IVec2, usize)>> = HashMap::new();
        for (key, list) in &out {
            for (n, v) in list.iter().enumerate() {
                let lo = v.poly.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p));
                let hi = v.poly.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p));
                let (b0, b1) = (bucket(lo), bucket(hi));
                for y in b0.y..=b1.y {
                    for x in b0.x..=b1.x {
                        index.entry(IVec2::new(x, y)).or_default().push((*key, n));
                    }
                }
            }
        }
        for (key, n) in shaped {
            let Some(v) = out.get(&key).and_then(|l| l.get(n)) else {
                continue;
            };
            let (poly, cell) = (v.poly.clone(), v.cell);
            let ctr = poly.iter().copied().sum::<Vec2>() / poly.len() as f32;
            let mut found = Vec::new();
            for k in 0..poly.len() {
                if v.edge_nbrs[k].is_some_and(|c| drawn.contains(&c)) {
                    found.push(None);
                    continue;
                }
                let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                let mid = (a + b) * 0.5;
                let beyond = mid + (mid - ctr).normalize_or_zero() * 0.6;
                let hit = index.get(&bucket(beyond)).and_then(|c| {
                    c.iter()
                        .map(|&(k2, n2)| &out[&k2][n2])
                        .find(|o| o.cell != cell && in_poly(&o.poly, beyond))
                        .map(|o| o.cell)
                });
                found.push(Some(hit));
            }
            let v = out.get_mut(&key).expect("chunk").get_mut(n).expect("cell");
            for (k, f) in found.into_iter().enumerate() {
                if let Some(f) = f {
                    v.edge_nbrs[k] = f;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(lines: Vec<(Vec<Vec2>, f32)>) -> RoadEdges {
        let mut buckets: HashMap<IVec2, Vec<u32>> = HashMap::new();
        for i in 0..lines.len() {
            for y in -5..5 {
                for x in -5..5 {
                    buckets.entry(IVec2::new(x, y)).or_default().push(i as u32);
                }
            }
        }
        RoadEdges {
            key: None,
            lines,
            buckets,
            hashes: HashMap::new(),
            changed: None,
        }
    }

    #[test]
    fn only_buckets_whose_roads_changed_are_marked() {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        let road = |w: &mut World, a: (i32, i32), b: (i32, i32)| {
            w.apply(citysim_sim::Command::BuildRoad {
                from: citysim_sim::Cell::new(a.0, a.1),
                to: citysim_sim::Cell::new(b.0, b.1),
                kind: citysim_sim::RoadKind::Street,
            })
            .expect("road");
        };
        road(&mut w, (20, 20), (40, 20));
        road(&mut w, (200, 200), (220, 200));
        let old = RoadEdges::new(&w);
        assert!(!old.unchanged_in(Vec2::ZERO, Vec2::ONE), "a first index knows nothing");
        road(&mut w, (200, 210), (220, 210));
        let mut new = RoadEdges::new(&w);
        new.diff(&old);
        // Far from the new road: unchanged; at it: changed.
        assert!(new.unchanged_in(Vec2::new(160.0, 160.0), Vec2::new(330.0, 165.0)));
        assert!(!new.unchanged_in(Vec2::new(1700.0, 1680.0), Vec2::new(1710.0, 1690.0)));
    }

    #[test]
    fn a_lot_clear_of_roads_is_left_alone() {
        let e = edges(vec![(vec![Vec2::new(-50.0, 30.0), Vec2::new(50.0, 30.0)], 4.0)]);
        let lot = Lot {
            c: Vec2::ZERO,
            f: Vec2::Y,
            half: Vec2::new(8.0, 8.0),
        };
        assert_eq!(fit(lot, &e), Inset::NONE);
    }

    #[test]
    fn a_wide_road_in_front_pulls_the_front_back_only() {
        // Front faces +y; the road's drawn edge reaches 3 m into the lot.
        let e = edges(vec![(vec![Vec2::new(-50.0, 13.0), Vec2::new(50.0, 13.0)], 8.0)]);
        let lot = Lot {
            c: Vec2::ZERO,
            f: Vec2::Y,
            half: Vec2::new(8.0, 8.0),
        };
        let i = fit(lot, &e);
        let (lo, hi) = i.rect(&lot);
        assert!((lo.y + 8.0).abs() < 1e-3, "back stays: {lo}");
        assert!(
            hi.y <= 13.0 - 8.0 - ROAD_CLEARANCE + 1e-2 && hi.y > 4.0,
            "front pulled in: {hi}"
        );
        assert!((hi.x - 8.0).abs() < 1e-3 && (lo.x + 8.0).abs() < 1e-3, "width kept");
    }

    #[test]
    fn a_road_beside_the_lot_squeezes_that_side() {
        // A road along the lot's left side (local -x is sim +x for front +y).
        let lot = Lot {
            c: Vec2::ZERO,
            f: Vec2::Y,
            half: Vec2::new(8.0, 8.0),
        };
        let side = lot.world(Vec2::new(-12.0, 0.0));
        let along = lot.world(Vec2::new(-12.0, 1.0)) - side;
        let e = edges(vec![(vec![side - along * 50.0, side + along * 50.0], 6.0)]);
        let i = fit(lot, &e);
        let (lo, hi) = i.rect(&lot);
        assert!(lo.x >= -12.0 + 6.0 + ROAD_CLEARANCE - 1e-2, "{lo}");
        assert!((hi.x - 8.0).abs() < 1e-3 && (hi.y - 8.0).abs() < 1e-3, "{hi}");
    }
}
