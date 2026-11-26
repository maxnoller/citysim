//! What the land next to the roads looks like to the renderers: where the
//! drawn roads are (centre-line polylines with their drawn half width, and
//! junction pavement outlines), and how a rectangle (a building lot, a
//! zoning cell) is fitted clear of them. Rebuilt once per roads version
//! (`RoadEdges` resource); buildings, lots, the zone overlay and the trees
//! all read it.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::zoning::{Lattice, ZoneCell};
use citysim_sim::{CellRef, World, Zone};

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
        RoadEdges {
            key: None,
            lines,
            buckets,
        }
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
        *edges = RoadEdges::new(&sim.world);
        edges.key = Some(key);
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
/// lattice's `+i`, local y along `+j`.
pub fn cell_lot(l: &Lattice, i: i32, j: i32) -> Lot {
    Lot {
        c: Vec2::from(l.cell_center(i, j)),
        f: Vec2::from(l.v()),
        half: Vec2::splat(citysim_sim::CELL_SIZE * 0.5),
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
}

impl CellView {
    pub fn center(&self) -> Vec2 {
        (self.quad[0] + self.quad[2]) * 0.5
    }
}

/// Fitted cell quads, valid for one roads version.
#[derive(Default)]
pub struct FitCache {
    key: Option<(u64, u64)>,
    quads: HashMap<CellRef, Option<[Vec2; 4]>>,
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
    }
    let mut out: HashMap<IVec2, Vec<CellView>> = HashMap::new();
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
                (ok && inset.scale.x * inset.scale.z > 0.3).then(|| corners(&lot, &inset))
            }) else {
                continue;
            };
            out.entry(road_geom::chunk_of(lot.c, chunk))
                .or_default()
                .push(CellView {
                    cell,
                    zone: zc.zone,
                    free: zc.free(),
                    quad,
                    u: Vec2::from(l.u),
                    v: Vec2::from(l.v()),
                    fronts: zc.fronts,
                });
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
        }
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
