//! People on foot: the walking network, walkers and the crossing rules.
//!
//! * **The network** ([`WalkNet`], derived from the roads, rebuilt when
//!   `versions.roads` changes, not saved): a pavement along both sides of
//!   every street and avenue (none along the highway), kerb corners round
//!   every junction, a zebra crossing across each arm of a junction where
//!   it fits (the zebras renderers draw, inside the junction margin), and
//!   mid-block crossings on long stretches ([`balance::MIDBLOCK_MIN_M`]).
//!   Doors join it where people step out ([`door`], the nearest pavement).
//! * **Walkers** ([`Walker`], `world.walking.walkers`, keyed by the citizen
//!   leading them; saved): a walked trip is a polyline found once when it
//!   starts (A* over the network) and walked at the party's pace. They
//!   wait at the kerb when they may not cross, and step onto the zebra
//!   when they may.
//! * **Crossing rules**: at signals people cross with the parallel green
//!   (their arm has no green movement in the phase, and it is not a
//!   protected-left phase) and call it when they wait; everywhere cars give
//!   way to anyone on the zebra or stepping onto it (cars about to enter a
//!   junction, turning in it, approaching a mid-block crossing), and nobody
//!   steps in front of a car that cannot stop (`PED_SAFETY_M`). Without
//!   signals people cross in groups and give the cars a turn once a car
//!   has waited (`CAR_PATIENCE_S`, `CAR_TURN_S`).
//!
//! Per tick: [`prepare`] (who holds which half of which zebra, from the
//! walkers' places) runs before the cars move; [`tick`] (which zebras a
//! car is over or cannot stop for, then the walkers move) after.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::citizens::{CitizenId, Location, TravelMode};
use crate::idmap::key_index;
use crate::junction::{JunctionControl, SignalStage};
use crate::roads::{NodeId, RoadKind, SegmentId};
use crate::{BuildingId, TICKS_PER_SECOND, World, balance, geom};

const DT: f32 = 1.0 / TICKS_PER_SECOND as f32;
/// Spatial buckets of pavement edges (door snapping), metres.
const BUCKET_M: f32 = 32.0;
const BUCKETS: i32 = (crate::MAP_SIZE_M / BUCKET_M) as i32;

/// A zebra crossing's stable name: the segment it crosses and where (0: at
/// the `a` end, 1: at the `b` end, `2 + k`: the `k`-th mid-block one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CrossingKey {
    pub segment: SegmentId,
    pub at: u8,
}

/// A zebra crossing.
#[derive(Debug, Clone, PartialEq)]
pub struct Crossing {
    pub key: CrossingKey,
    /// The junction it belongs to (`None` mid-block).
    pub node: Option<NodeId>,
    /// Its arm at that junction (index into the node's segments).
    pub arm: Option<u8>,
    /// Centre-line point of the crossing, metres from the segment's `a`.
    pub from_a: f32,
    pub center: [f32; 2],
    /// Road direction (`a -> b`) at the crossing.
    pub dir: [f32; 2],
    pub kind: RoadKind,
    /// Carriageway half-width.
    pub half_width: f32,
    /// Pavement points it joins: left of `a -> b` (side 0), right (side 1).
    pub ends: [[f32; 2]; 2],
    /// Network points of `ends`.
    pub points: [u32; 2],
}

impl Crossing {
    /// Lateral position across the road of point `p`, metres to the right of
    /// `a -> b` (side 1 positive).
    pub fn lateral(&self, p: [f32; 2]) -> f32 {
        geom::dot(geom::sub(p, self.center), crate::junction::right_of(self.dir))
    }

    /// The zebra's outline (four corners), for renderers.
    pub fn outline(&self) -> [[f32; 2]; 4] {
        let r = crate::junction::right_of(self.dir);
        let (h, w) = (self.half_width, balance::ZEBRA_WIDTH_M * 0.5);
        let c = |a: f32, b: f32| {
            [
                self.center[0] + self.dir[0] * a + r[0] * b,
                self.center[1] + self.dir[1] * a + r[1] * b,
            ]
        };
        [c(-w, -h), c(w, -h), c(w, h), c(-w, h)]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// Along a road side.
    Pavement,
    /// Round a kerb corner (or a dead end).
    Corner,
    /// Across the road (index into `crossings`).
    Crossing(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub len: f32,
    pub kind: EdgeKind,
    /// Polyline from point `a` to point `b`.
    pub pts: Vec<[f32; 2]>,
}

/// The walking network (see the module docs).
#[derive(Debug, Clone, Default)]
pub struct WalkNet {
    pub roads_version: u64,
    pub points: Vec<[f32; 2]>,
    pub edges: Vec<Edge>,
    pub crossings: Vec<Crossing>,
    adj_start: Vec<u32>,
    adj: Vec<u32>,
    by_key: BTreeMap<CrossingKey, u32>,
    /// Crossings per segment slot (ends and mid-block).
    seg_cross: Vec<Vec<u32>>,
    /// Mid-block crossings per segment slot: (metres from `a`, crossing).
    seg_mid: Vec<Vec<(f32, u32)>>,
    /// Junction crossings per node slot: (arm, crossing).
    node_cross: Vec<Vec<(u8, u32)>>,
    /// Pavement edges per bucket.
    grid: Vec<Vec<u32>>,
    // Search scratch.
    stamp: Vec<u32>,
    cur: u32,
    dist: Vec<f32>,
    via: Vec<u32>,
}

impl WalkNet {
    pub fn crossing(&self, k: CrossingKey) -> Option<(u32, &Crossing)> {
        let i = *self.by_key.get(&k)?;
        Some((i, &self.crossings[i as usize]))
    }

    /// Crossings on segment `s` (at its ends and along it).
    pub fn on_segment(&self, s: SegmentId) -> &[u32] {
        self.seg_cross.get(key_index(s)).map_or(&[], |v| v.as_slice())
    }

    /// The crossing across arm `arm` of junction `n`.
    pub fn at_arm(&self, n: NodeId, arm: u8) -> Option<u32> {
        self.node_cross
            .get(key_index(n))?
            .iter()
            .find(|(a, _)| *a == arm)
            .map(|x| x.1)
    }

    fn add_point(&mut self, p: [f32; 2]) -> u32 {
        self.points.push(p);
        (self.points.len() - 1) as u32
    }

    fn add_edge(&mut self, a: u32, b: u32, kind: EdgeKind, pts: Vec<[f32; 2]>) {
        let len = poly_len(&pts);
        self.edges.push(Edge { a, b, len, kind, pts });
    }

    /// Nearest point on a pavement within `max` metres of `p`: (edge,
    /// metres along it, point).
    pub fn snap(&self, p: [f32; 2], max: f32) -> Option<(u32, f32, [f32; 2])> {
        let r = (max / BUCKET_M).ceil() as i32;
        let (bx, by) = bucket(p);
        let mut best: Option<(f32, u32, f32, [f32; 2])> = None;
        for y in (by - r).max(0)..=(by + r).min(BUCKETS - 1) {
            for x in (bx - r).max(0)..=(bx + r).min(BUCKETS - 1) {
                for &e in &self.grid[(y * BUCKETS + x) as usize] {
                    let (d, along, q) = poly_nearest(&self.edges[e as usize].pts, p);
                    if d <= max && best.is_none_or(|b| (d, e) < (b.0, b.1)) {
                        best = Some((d, e, along, q));
                    }
                }
            }
        }
        best.map(|b| (b.1, b.2, b.3))
    }

    /// A walk from `from` to `to` (points near pavements), at most `limit`
    /// metres long.
    pub fn route(&mut self, from: [f32; 2], to: [f32; 2], limit: f32) -> Option<Route> {
        let snap_m = 45.0;
        let (se, sa, sp) = self.snap(from, snap_m)?;
        let (te, ta, tp) = self.snap(to, snap_m)?;
        let lead = geom::dist(from, sp) + geom::dist(to, tp);
        let mut out = Route::default();
        out.push(from, None);
        out.push(sp, None);
        if se == te {
            let e = &self.edges[se as usize];
            if (ta - sa).abs() + lead <= limit {
                for q in poly_slice(&e.pts, sa, ta) {
                    out.push(q, None);
                }
                out.push(to, None);
                return Some(out.finish());
            }
        }
        // A* from both ends of the start edge to either end of the goal edge.
        let n = self.points.len();
        if self.stamp.len() < n {
            self.stamp.resize(n, 0);
            self.dist.resize(n, f32::INFINITY);
            self.via.resize(n, u32::MAX);
        }
        self.cur = self.cur.wrapping_add(1);
        if self.cur == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.cur = 1;
        }
        let cur = self.cur;
        let (s_edge, t_edge) = (&self.edges[se as usize], &self.edges[te as usize]);
        let starts = [(s_edge.a, sa), (s_edge.b, s_edge.len - sa)];
        let goals = [(t_edge.a, ta), (t_edge.b, t_edge.len - ta)];
        let h = |p: [f32; 2]| geom::dist(p, tp);
        let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u32, u32)>> = Default::default();
        for (p, c) in starts {
            let i = p as usize;
            if self.stamp[i] != cur || c < self.dist[i] {
                self.stamp[i] = cur;
                self.dist[i] = c;
                self.via[i] = u32::MAX;
                heap.push(std::cmp::Reverse((ord(c + h(self.points[i])), p)));
            }
        }
        let mut best: Option<(f32, u32)> = None;
        let mut expanded = 0u32;
        while let Some(std::cmp::Reverse((f, u))) = heap.pop() {
            let ui = u as usize;
            let g = self.dist[ui];
            if from_ord(f) > g + h(self.points[ui]) + 1e-3 {
                continue;
            }
            if best.is_some_and(|b| from_ord(f) >= b.0) || from_ord(f) + lead > limit {
                break;
            }
            expanded += 1;
            if expanded > 20_000 {
                break;
            }
            for (p, rest) in goals {
                if p == u && best.is_none_or(|b| g + rest < b.0) {
                    best = Some((g + rest, u));
                }
            }
            for k in self.adj_start[ui]..self.adj_start[ui + 1] {
                let ei = self.adj[k as usize];
                let e = &self.edges[ei as usize];
                let v = if e.a == u { e.b } else { e.a };
                let cost = e.len
                    + match e.kind {
                        EdgeKind::Crossing(_) => balance::WALK_CROSSING_COST_M,
                        _ => 0.0,
                    };
                let ng = g + cost;
                let vi = v as usize;
                if self.stamp[vi] != cur || ng < self.dist[vi] {
                    self.stamp[vi] = cur;
                    self.dist[vi] = ng;
                    self.via[vi] = ei;
                    heap.push(std::cmp::Reverse((ord(ng + h(self.points[vi])), v)));
                }
            }
        }
        let (total, end) = best?;
        if total + lead > limit {
            return None;
        }
        // Edges from the start to `end`, then on along the goal edge.
        let mut chain = Vec::new();
        let mut at = end;
        while self.via[at as usize] != u32::MAX {
            let ei = self.via[at as usize];
            chain.push(ei);
            let e = &self.edges[ei as usize];
            at = if e.a == at { e.b } else { e.a };
        }
        chain.reverse();
        // Along the start edge to the point the chain leaves from.
        let s_edge = &self.edges[se as usize];
        let first = at;
        let to_d = if first == s_edge.a { 0.0 } else { s_edge.len };
        for q in poly_slice(&s_edge.pts, sa, to_d) {
            out.push(q, None);
        }
        let mut at = first;
        for ei in chain {
            let e = &self.edges[ei as usize];
            let fwd = e.a == at;
            let mark = match e.kind {
                EdgeKind::Crossing(c) => Some(self.crossings[c as usize].key),
                _ => None,
            };
            let pts: Vec<[f32; 2]> = if fwd {
                e.pts.clone()
            } else {
                e.pts.iter().rev().copied().collect()
            };
            // The crossing leg starts at the kerb point.
            out.push(pts[0], None);
            for (i, &q) in pts.iter().enumerate().skip(1) {
                out.push(q, if i == 1 { mark } else { None });
            }
            at = if fwd { e.b } else { e.a };
        }
        let t_edge = &self.edges[te as usize];
        let from_d = if at == t_edge.a { 0.0 } else { t_edge.len };
        for q in poly_slice(&t_edge.pts, from_d, ta) {
            out.push(q, None);
        }
        out.push(to, None);
        Some(out.finish())
    }
}

/// Always equal: derived data.
impl PartialEq for WalkNet {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// A walk: points from door to door and the crossings on it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Route {
    pub pts: Vec<[f32; 2]>,
    /// `(i, crossing)`: the leg from `pts[i]` to `pts[i + 1]` crosses it.
    pub marks: Vec<(u16, CrossingKey)>,
    /// Metres.
    pub len: f32,
}

impl Route {
    /// Add a point; `mark`: the leg ending at it is a crossing.
    fn push(&mut self, p: [f32; 2], mark: Option<CrossingKey>) {
        if let Some(&last) = self.pts.last()
            && geom::dist(last, p) < 0.02
        {
            if let Some(k) = mark {
                // A zero-length leg cannot be a crossing: drop the mark.
                let _ = k;
            }
            return;
        }
        if let Some(k) = mark {
            self.marks.push(((self.pts.len() - 1) as u16, k));
        }
        self.pts.push(p);
    }

    fn finish(mut self) -> Route {
        self.len = poly_len(&self.pts);
        self
    }
}

fn ord(x: f32) -> u32 {
    let b = x.to_bits();
    if b >> 31 == 1 { !b } else { b | 0x8000_0000 }
}

fn from_ord(o: u32) -> f32 {
    let b = if o >> 31 == 1 { o & 0x7FFF_FFFF } else { !o };
    f32::from_bits(b)
}

fn bucket(p: [f32; 2]) -> (i32, i32) {
    (
        ((p[0] / BUCKET_M) as i32).clamp(0, BUCKETS - 1),
        ((p[1] / BUCKET_M) as i32).clamp(0, BUCKETS - 1),
    )
}

pub fn poly_len(pts: &[[f32; 2]]) -> f32 {
    pts.windows(2).map(|w| geom::dist(w[0], w[1])).sum()
}

/// Nearest point of a polyline: (distance, metres along, point).
fn poly_nearest(pts: &[[f32; 2]], p: [f32; 2]) -> (f32, f32, [f32; 2]) {
    let mut best = (f32::INFINITY, 0.0, pts[0]);
    let mut along = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = geom::sub(b, a);
        let l2 = geom::dot(ab, ab);
        let t = if l2 > 1e-9 {
            (geom::dot(geom::sub(p, a), ab) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
        let d = geom::dist(p, q);
        let l = l2.sqrt();
        if d < best.0 {
            best = (d, along + l * t, q);
        }
        along += l;
    }
    best
}

/// Point `d` metres along a polyline.
fn poly_at(pts: &[[f32; 2]], d: f32) -> [f32; 2] {
    let mut left = d.max(0.0);
    for w in pts.windows(2) {
        let l = geom::dist(w[0], w[1]);
        if left <= l && l > 1e-9 {
            let t = left / l;
            return [w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t];
        }
        left -= l;
    }
    *pts.last().expect("polyline")
}

/// The points of a polyline from `d0` to `d1` metres along it (either
/// direction), both ends included.
fn poly_slice(pts: &[[f32; 2]], d0: f32, d1: f32) -> Vec<[f32; 2]> {
    let (lo, hi) = (d0.min(d1), d0.max(d1));
    let mut out = vec![poly_at(pts, lo)];
    let mut along = 0.0;
    for (i, w) in pts.windows(2).enumerate() {
        along += geom::dist(w[0], w[1]);
        if along > lo + 1e-3 && along < hi - 1e-3 && i + 1 < pts.len() {
            out.push(pts[i + 1]);
        }
    }
    out.push(poly_at(pts, hi));
    if d1 < d0 {
        out.reverse();
    }
    out
}

/// Pavement width beside a road of this kind (0: none).
pub fn pavement_m(kind: RoadKind) -> f32 {
    match kind {
        RoadKind::Street => balance::STREET_PAVEMENT_M,
        RoadKind::Avenue => balance::AVENUE_PAVEMENT_M,
        RoadKind::Highway => 0.0,
    }
}

/// Distance from the node of the line people cross an arm on (the middle
/// of the zebra), for an arm with stop line `setback`.
pub fn zebra_line(setback: f32) -> f32 {
    (setback - balance::ZEBRA_INSET_M - balance::ZEBRA_WIDTH_M * 0.5).max(0.0)
}

/// Where people step out of a building: the middle of the lot's front
/// edge, `DOOR_INSET_M` inside it.
pub fn door(b: &crate::Building) -> [f32; 2] {
    let f = b.front_dir();
    let depth = b.footprint.size[1] as f32 * crate::CELL_SIZE;
    let k = (depth * 0.5 - balance::DOOR_INSET_M).max(0.0);
    [b.footprint.center[0] + f[0] * k, b.footprint.center[1] + f[1] * k]
}

/// Build the network for the current roads.
pub fn build(world: &World) -> WalkNet {
    let roads = &world.roads;
    let mut net = WalkNet {
        roads_version: world.versions.roads,
        ..Default::default()
    };
    let nseg = roads.segments.slot_count();
    net.seg_cross = vec![Vec::new(); nseg];
    net.node_cross = vec![Vec::new(); roads.nodes.slot_count()];
    // Pavement end points per segment slot: [at a, at b][side].
    let mut ends: Vec<Option<[[u32; 2]; 2]>> = vec![None; nseg];
    let end_d = |n: NodeId, s: SegmentId| -> f32 {
        let deg = roads.nodes[n].segments.len();
        let sb = crate::junction::setback(world, n, s);
        if deg >= 3 { zebra_line(sb) } else { sb }
    };
    for (s, seg) in roads.segments.iter() {
        let wk = pavement_m(seg.kind);
        if wk <= 0.0 {
            continue;
        }
        let len = seg.length;
        let h = seg.kind.half_width_m();
        let off = h + wk * 0.5;
        let da = end_d(seg.a, s).min(len * 0.45);
        let db = len - end_d(seg.b, s).min(len * 0.45);
        // Mid-block crossings between the stop lines.
        let (sa, sb) = (
            crate::junction::setback(world, seg.a, s),
            crate::junction::setback(world, seg.b, s),
        );
        let stretch = len - sa - sb;
        let mut stations = vec![da];
        if stretch >= balance::MIDBLOCK_MIN_M {
            let n = ((stretch / balance::MIDBLOCK_SPACING_M) as usize).max(1);
            for k in 0..n {
                stations.push(sa + stretch * (k + 1) as f32 / (n + 1) as f32);
            }
        }
        stations.push(db.max(da));
        let flat: [Vec<(f32, [f32; 2])>; 2] = [
            seg.curve.flatten_offset(-off, 0.08),
            seg.curve.flatten_offset(off, 0.08),
        ];
        let mut ids = [Vec::new(), Vec::new()];
        for side in 0..2 {
            let o = if side == 1 { off } else { -off };
            for &d in &stations {
                let p = seg.pose(d, o).pos;
                let id = net.add_point(p);
                ids[side].push(id);
            }
            for k in 0..stations.len() - 1 {
                let (d0, d1) = (stations[k], stations[k + 1]);
                let mut pts = vec![net.points[ids[side][k] as usize]];
                pts.extend(
                    flat[side]
                        .iter()
                        .filter(|(d, _)| *d > d0 + 0.05 && *d < d1 - 0.05)
                        .map(|x| x.1),
                );
                pts.push(net.points[ids[side][k + 1] as usize]);
                net.add_edge(ids[side][k], ids[side][k + 1], EdgeKind::Pavement, pts);
            }
        }
        for k in 1..stations.len() - 1 {
            let d = stations[k];
            let pose = seg.pose(d, 0.0);
            let key = CrossingKey {
                segment: s,
                at: (k + 1) as u8,
            };
            let ci = net.crossings.len() as u32;
            let (p0, p1) = (ids[0][k], ids[1][k]);
            net.crossings.push(Crossing {
                key,
                node: None,
                arm: None,
                from_a: d,
                center: pose.pos,
                dir: pose.dir,
                kind: seg.kind,
                half_width: h,
                ends: [net.points[p0 as usize], net.points[p1 as usize]],
                points: [p0, p1],
            });
            net.add_edge(
                p0,
                p1,
                EdgeKind::Crossing(ci),
                vec![net.points[p0 as usize], net.points[p1 as usize]],
            );
            net.seg_cross[key_index(s)].push(ci);
        }
        let last = stations.len() - 1;
        ends[key_index(s)] = Some([[ids[0][0], ids[1][0]], [ids[0][last], ids[1][last]]]);
    }
    // Corners, dead ends and junction crossings.
    for (n, node) in roads.nodes.iter() {
        let deg = node.segments.len();
        // (angle, segment, dir away, [left point, right point] or none)
        let mut arms: Vec<(f32, SegmentId, [f32; 2], Option<[u32; 2]>)> = node
            .segments
            .iter()
            .filter_map(|&s| {
                let seg = roads.segments.get(s)?;
                let dir = seg.dir_from(n);
                let pts = ends[key_index(s)].map(|e| {
                    let at_a = seg.a == n;
                    let [l, r] = if at_a { e[0] } else { e[1] };
                    // Looking away from the node: at `a` that is `a -> b`
                    // (side 1 on the right), at `b` the other way.
                    if at_a { [l, r] } else { [r, l] }
                });
                Some((libm::atan2f(dir[1], dir[0]), s, dir, pts))
            })
            .collect();
        arms.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        if deg == 1 {
            if let Some((_, s, dir, Some([l, r]))) = arms.first().copied() {
                let seg = &roads.segments[s];
                let off = seg.kind.half_width_m() + pavement_m(seg.kind) * 0.5;
                let c = node.pos;
                let right = crate::junction::right_of(dir);
                let mut pts = vec![net.points[r as usize]];
                for k in 1..6 {
                    let a = k as f32 / 6.0 * std::f32::consts::PI;
                    let (cs, sn) = (libm::cosf(a), libm::sinf(a));
                    pts.push([
                        c[0] + right[0] * off * cs - dir[0] * off * sn,
                        c[1] + right[1] * off * cs - dir[1] * off * sn,
                    ]);
                }
                pts.push(net.points[l as usize]);
                net.add_edge(r, l, EdgeKind::Corner, pts);
            }
            continue;
        }
        let k = arms.len();
        for i in 0..k {
            let (a, b) = (arms[i], arms[(i + 1) % k]);
            if k == 2 && i == 1 && a.1 == b.1 {
                continue;
            }
            let (Some(pa), Some(pb)) = (a.3, b.3) else { continue };
            let (p, q) = (net.points[pa[1] as usize], net.points[pb[0] as usize]);
            let mut pts = vec![p];
            if let Some(c) = corner_ctrl(p, a.2, q, b.2, node.pos) {
                for t in [0.25f32, 0.5, 0.75] {
                    let u = 1.0 - t;
                    pts.push([
                        u * u * p[0] + 2.0 * u * t * c[0] + t * t * q[0],
                        u * u * p[1] + 2.0 * u * t * c[1] + t * t * q[1],
                    ]);
                }
            }
            pts.push(q);
            net.add_edge(pa[1], pb[0], EdgeKind::Corner, pts);
        }
        if deg < 3 {
            continue;
        }
        // Zebras where they fit (as renderers draw them): not across the
        // highway, a stop line far enough out, not overlapping another.
        let Some(j) = world.traffic.junction_states.get(n) else {
            continue;
        };
        let zone = |s: SegmentId| -> Option<[[f32; 2]; 4]> {
            let seg = roads.segments.get(s)?;
            let arm = j.arm_of(s)?;
            let sb = j.arms[arm].setback;
            if seg.kind == RoadKind::Highway || sb < balance::ZEBRA_WIDTH_M + 1.0 {
                return None;
            }
            let fwd = seg.a == n;
            let h = seg.kind.half_width_m();
            let c = |d: f32, o: f32| seg.pose_dir(fwd, d, o).pos;
            let (d0, d1) = (
                sb - balance::ZEBRA_INSET_M,
                sb - balance::ZEBRA_INSET_M - balance::ZEBRA_WIDTH_M,
            );
            Some([c(d0, -h), c(d1, -h), c(d1, h), c(d0, h)])
        };
        let zones: Vec<Option<[[f32; 2]; 4]>> = arms.iter().map(|a| zone(a.1)).collect();
        for (i, arm) in arms.iter().enumerate() {
            let Some(zi) = zones[i] else { continue };
            let Some(pts) = arm.3 else { continue };
            if zones
                .iter()
                .enumerate()
                .any(|(o, z)| o != i && z.is_some_and(|z| quads_overlap(&zi, &z)))
            {
                continue;
            }
            let s = arm.1;
            let seg = &roads.segments[s];
            let Some(arm_ix) = j.arm_of(s) else { continue };
            let sb = j.arms[arm_ix].setback;
            let at_a = seg.a == n;
            let d = zebra_line(sb);
            let from_a = if at_a { d } else { seg.length - d };
            let pose = seg.pose(from_a, 0.0);
            // `pts` are [left, right] looking away from the node; the
            // crossing's ends are by side of `a -> b`.
            let (p0, p1) = if at_a { (pts[0], pts[1]) } else { (pts[1], pts[0]) };
            let key = CrossingKey {
                segment: s,
                at: if at_a { 0 } else { 1 },
            };
            let ci = net.crossings.len() as u32;
            net.crossings.push(Crossing {
                key,
                node: Some(n),
                arm: Some(arm_ix as u8),
                from_a,
                center: pose.pos,
                dir: pose.dir,
                kind: seg.kind,
                half_width: seg.kind.half_width_m(),
                ends: [net.points[p0 as usize], net.points[p1 as usize]],
                points: [p0, p1],
            });
            net.add_edge(
                p0,
                p1,
                EdgeKind::Crossing(ci),
                vec![net.points[p0 as usize], net.points[p1 as usize]],
            );
            net.seg_cross[key_index(s)].push(ci);
            net.node_cross[key_index(n)].push((arm_ix as u8, ci));
        }
    }
    net.seg_mid = vec![Vec::new(); nseg];
    for (i, c) in net.crossings.iter().enumerate() {
        net.by_key.insert(c.key, i as u32);
        if c.node.is_none() {
            net.seg_mid[key_index(c.key.segment)].push((c.from_a, i as u32));
        }
    }
    // Adjacency (CSR) and the pavement buckets.
    let np = net.points.len();
    let mut count = vec![0u32; np + 1];
    for e in &net.edges {
        count[e.a as usize] += 1;
        count[e.b as usize] += 1;
    }
    let mut start = vec![0u32; np + 1];
    for i in 0..np {
        start[i + 1] = start[i] + count[i];
    }
    let mut fill = start.clone();
    let mut adj = vec![0u32; start[np] as usize];
    for (ei, e) in net.edges.iter().enumerate() {
        for p in [e.a, e.b] {
            adj[fill[p as usize] as usize] = ei as u32;
            fill[p as usize] += 1;
        }
    }
    net.adj_start = start;
    net.adj = adj;
    net.grid = vec![Vec::new(); (BUCKETS * BUCKETS) as usize];
    for (ei, e) in net.edges.iter().enumerate() {
        if e.kind != EdgeKind::Pavement {
            continue;
        }
        let (mut lo, mut hi) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
        for &p in &e.pts {
            let b = bucket(p);
            lo = (lo.0.min(b.0), lo.1.min(b.1));
            hi = (hi.0.max(b.0), hi.1.max(b.1));
        }
        for y in lo.1..=hi.1 {
            for x in lo.0..=hi.0 {
                net.grid[(y * BUCKETS + x) as usize].push(ei as u32);
            }
        }
    }
    net
}

/// Control point of a kerb corner from `p` (arm direction `da`, away from
/// the node) to `q` (`db`): where the two pavement lines meet, if that is
/// near the node.
fn corner_ctrl(p: [f32; 2], da: [f32; 2], q: [f32; 2], db: [f32; 2], node: [f32; 2]) -> Option<[f32; 2]> {
    let (t, _) = crate::curve::line_meet(p, da, q, db)?;
    let c = [p[0] + da[0] * t, p[1] + da[1] * t];
    let reach = geom::dist(p, node) + geom::dist(q, node);
    (geom::dist(c, node) < reach * 1.5 && geom::dist(c, p) > 0.05 && geom::dist(c, q) > 0.05).then_some(c)
}

/// Two convex quads overlap (separating axes).
fn quads_overlap(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> bool {
    for q in [a, b] {
        for k in 0..4 {
            let e = geom::sub(q[(k + 1) % 4], q[k]);
            let axis = [-e[1], e[0]];
            let span = |p: &[[f32; 2]; 4]| {
                p.iter()
                    .map(|v| geom::dot(*v, axis))
                    .fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(d), hi.max(d)))
            };
            let (a0, a1) = span(a);
            let (b0, b1) = span(b);
            if a1 <= b0 + 1e-4 || b1 <= a0 + 1e-4 {
                return false;
            }
        }
    }
    true
}

// ------------------------------------------------------------ walkers ----

/// What a walker is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WalkState {
    #[default]
    Walking,
    /// At the kerb, waiting to cross (the next leg is a crossing).
    Waiting,
    /// On a zebra.
    Crossing,
}

/// A person (or a parent with children) walking a route.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Walker {
    /// Children walking along (the leader is the key in `walkers`).
    pub party: Vec<CitizenId>,
    pub from: Option<BuildingId>,
    pub to: BuildingId,
    /// Boxed: the step reads only the fields below it.
    pub route: Box<Route>,
    /// Current leg (`route.pts[leg] -> route.pts[leg + 1]`) and metres along it.
    pub leg: u16,
    pub along: f32,
    /// Pace, m/s.
    pub speed: f32,
    /// Sideways from the route's line, metres to the right of travel (so
    /// people do not walk in single file on top of each other).
    pub offset: f32,
    pub seed: u32,
    /// Pose for renderers: position, heading (radians, `atan2` in sim
    /// axes), what they are doing.
    pub pos: [f32; 2],
    pub heading: f32,
    pub state: WalkState,
    /// Metres walked so far (the walk cycle's phase).
    pub walked_m: f32,
    /// Metres of the route before the current leg.
    pub leg_start_m: f32,
    pub started_tick: u64,
    /// Ticks spent waiting at kerbs on this walk.
    pub waited_ticks: u32,
    /// Ticks waiting at the current kerb.
    pub kerb_ticks: u32,
    /// The current leg: start, unit direction, length, and the index of
    /// the next leg that is a crossing (`u16::MAX`: none), kept here so a
    /// step reads nothing else.
    pub leg_a: [f32; 2],
    pub leg_d: [f32; 2],
    pub leg_len: f32,
    pub next_mark: u16,
}

impl Walker {
    /// The crossing on the current leg, if any.
    pub fn crossing(&self) -> Option<CrossingKey> {
        self.mark_at(self.leg)
    }

    fn mark_at(&self, leg: u16) -> Option<CrossingKey> {
        self.route.marks.iter().find(|m| m.0 == leg).map(|m| m.1)
    }

    /// Metres still to walk.
    pub fn left_m(&self) -> f32 {
        (self.route.len - self.done_m()).max(0.0)
    }

    /// Metres of the route behind them.
    pub fn done_m(&self) -> f32 {
        self.leg_start_m + self.along
    }

    /// The rest of the way: from where they are to the door.
    pub fn ahead(&self) -> Vec<[f32; 2]> {
        let mut out = vec![self.pos];
        out.extend(self.route.pts.iter().skip(self.leg as usize + 1).copied());
        out
    }

    /// Take up leg `self.leg` (its start, direction, length, the next
    /// crossing, the heading).
    fn enter_leg(&mut self) {
        let pts = &self.route.pts;
        let i = self.leg as usize;
        let a = pts.get(i).or(pts.last()).copied().unwrap_or([0.0, 0.0]);
        let b = pts.get(i + 1).copied().unwrap_or(a);
        let l = geom::dist(a, b);
        self.leg_a = a;
        self.leg_len = l;
        if l > 1e-6 {
            self.leg_d = [(b[0] - a[0]) / l, (b[1] - a[1]) / l];
            self.heading = libm::atan2f(self.leg_d[1], self.leg_d[0]);
        }
        self.next_mark = self
            .route
            .marks
            .iter()
            .map(|m| m.0)
            .filter(|&m| m >= self.leg)
            .min()
            .unwrap_or(u16::MAX);
    }

    /// Position and direction on the route's line.
    fn line_pose(&self) -> ([f32; 2], [f32; 2]) {
        let t = self.along.min(self.leg_len);
        (
            [self.leg_a[0] + self.leg_d[0] * t, self.leg_a[1] + self.leg_d[1] * t],
            self.leg_d,
        )
    }

    /// Update `pos` / `heading` from the route position.
    fn update_pose(&mut self) {
        let (p, d) = self.line_pose();
        // Sideways offset, eased out over the first and last metres (the
        // door).
        let k = (self.done_m() / 2.0).min(self.left_m() / 2.0).clamp(0.0, 1.0);
        let r = crate::junction::right_of(d);
        let o = self.offset * k;
        self.pos = [p[0] + r[0] * o, p[1] + r[1] * o];
    }
}

/// Saved timers of a crossing (people taking turns with the cars).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CrossTimers {
    /// Tick the people now on the zebra started stepping onto it.
    pub group_start: u64,
    /// People let the cars go until this tick.
    pub turn_until: u64,
    /// Ticks a car has stood waiting for the people on it.
    pub car_wait: u32,
}

/// Per crossing, this tick (derived from walkers and cars).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CrossNow {
    /// People on it or stepping onto it hold each half of the road
    /// (`[side 0, side 1]`: cars driving `b -> a` use side 0).
    pub held: [bool; 2],
    pub people: u16,
    /// A car is over it or cannot stop short of it.
    pub hazard: bool,
    /// A moving car reaches it within `PED_GAP_S` (people without signals
    /// wait for a gap unless they have waited `PED_PATIENCE_S`).
    pub coming: bool,
    /// A car stands waiting for it.
    pub car_waiting: bool,
    /// Someone waits at its kerb.
    pub calling: bool,
    /// Someone is on it, waits at it or reaches its kerb now.
    pub active: bool,
    /// `hazard`, `coming`, `car_waiting` were worked out from the cars.
    pub evaluated: bool,
}

/// The walkers under way, keyed by the citizen leading each: a dense list
/// (in the order they set off, a removal moves the last one into the gap)
/// with an index by citizen.
#[derive(Debug, Clone, Default)]
pub struct Walkers {
    list: Vec<(CitizenId, Walker)>,
    index: crate::SideTable<CitizenId, u32>,
}

impl PartialEq for Walkers {
    fn eq(&self, o: &Self) -> bool {
        self.list == o.list
    }
}

impl Walkers {
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn get(&self, id: &CitizenId) -> Option<&Walker> {
        let i = *self.index.get(*id)? as usize;
        self.list.get(i).filter(|x| x.0 == *id).map(|x| &x.1)
    }

    pub fn get_mut(&mut self, id: &CitizenId) -> Option<&mut Walker> {
        let i = *self.index.get(*id)? as usize;
        self.list.get_mut(i).filter(|x| x.0 == *id).map(|x| &mut x.1)
    }

    pub fn contains_key(&self, id: &CitizenId) -> bool {
        self.get(id).is_some()
    }

    pub fn insert(&mut self, id: CitizenId, w: Walker) {
        if let Some(x) = self.get_mut(&id) {
            *x = w;
            return;
        }
        self.index.insert(id, self.list.len() as u32);
        self.list.push((id, w));
    }

    pub fn remove(&mut self, id: &CitizenId) -> Option<Walker> {
        let i = *self.index.get(*id)? as usize;
        if self.list.get(i).is_none_or(|x| x.0 != *id) {
            return None;
        }
        self.index.remove(*id);
        let (_, w) = self.list.swap_remove(i);
        if let Some(moved) = self.list.get(i) {
            self.index.insert(moved.0, i as u32);
        }
        Some(w)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&CitizenId, &Walker)> + '_ {
        self.list.iter().map(|(k, v)| (k, v))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&CitizenId, &mut Walker)> + '_ {
        self.list.iter_mut().map(|(k, v)| (&*k, v))
    }

    pub fn values(&self) -> impl Iterator<Item = &Walker> + '_ {
        self.list.iter().map(|x| &x.1)
    }

    pub fn keys(&self) -> impl Iterator<Item = &CitizenId> + '_ {
        self.list.iter().map(|x| &x.0)
    }
}

impl std::ops::Index<&CitizenId> for Walkers {
    type Output = Walker;
    fn index(&self, id: &CitizenId) -> &Walker {
        self.get(id).expect("no such walker")
    }
}

impl FromIterator<(CitizenId, Walker)> for Walkers {
    fn from_iter<I: IntoIterator<Item = (CitizenId, Walker)>>(it: I) -> Self {
        let mut w = Walkers::default();
        for (k, v) in it {
            w.insert(k, v);
        }
        w
    }
}

/// Everything about people on foot (`world.walking`): walkers and crossing
/// timers are saved (extension block 6); the network and the per-tick
/// crossing states are derived.
#[derive(Debug, Clone, Default)]
pub struct Walking {
    pub walkers: Walkers,
    pub net: WalkNet,
    /// By crossing index.
    pub timers: Vec<CrossTimers>,
    pub now: Vec<CrossNow>,
    /// Crossings whose `now` was set this tick (reset next tick).
    touched: Vec<u32>,
    /// Signal calls of people waiting: (node slot, arm), sorted.
    calls: Vec<(usize, u8)>,
    /// The tick `now` / `calls` were gathered for.
    prepared_for: u64,
    /// Walks made today / yesterday on the map, timed off screen, and car
    /// trips started (for the share of trips on foot), and by hour today.
    pub stats: WalkStats,
}

/// Walkers and statistics compare; the network and per-tick states are
/// derived.
impl PartialEq for Walking {
    fn eq(&self, o: &Self) -> bool {
        self.walkers == o.walkers && self.stats == o.stats
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WalkStats {
    /// Trips started on foot today (drawn or timed) and by car / otherwise.
    pub walked_today: u32,
    pub other_today: u32,
    pub walked_yesterday: u32,
    pub other_yesterday: u32,
    pub walked_by_hour: [u32; 24],
    pub other_by_hour: [u32; 24],
    pub walked_by_hour_yesterday: [u32; 24],
    pub other_by_hour_yesterday: [u32; 24],
}

impl WalkStats {
    /// Share of today's trips (yesterday's while today has few) made on
    /// foot.
    pub fn share(&self) -> f32 {
        let (w, o) = if self.walked_today + self.other_today >= 50 {
            (self.walked_today, self.other_today)
        } else {
            (self.walked_yesterday, self.other_yesterday)
        };
        if w + o == 0 { 0.0 } else { w as f32 / (w + o) as f32 }
    }
}

/// The walking network, rebuilt if the roads changed.
pub(crate) fn ensure_net(world: &mut World) {
    if world.walking.net.roads_version == world.versions.roads && !world.walking.net.points.is_empty() {
        return;
    }
    crate::traffic::sync_junctions(world);
    let old_keys: Vec<CrossingKey> = world.walking.net.crossings.iter().map(|c| c.key).collect();
    let old_timers = std::mem::take(&mut world.walking.timers);
    let net = build(world);
    let mut timers = vec![CrossTimers::default(); net.crossings.len()];
    for (k, t) in old_keys.iter().zip(old_timers) {
        if let Some((i, _)) = net.crossing(*k) {
            timers[i as usize] = t;
        }
    }
    let changed = world.walking.net.roads_version != 0;
    world.walking.net = net;
    world.walking.timers = timers;
    world.walking.now = vec![CrossNow::default(); world.walking.net.crossings.len()];
    world.walking.touched.clear();
    if changed {
        reroute_all(world);
    }
}

/// Loading a save: walkers, crossing timers and counts back in place.
pub(crate) fn restore(
    world: &mut World,
    walkers: Vec<(CitizenId, Walker)>,
    timers: Vec<(CrossingKey, CrossTimers)>,
    stats: WalkStats,
) {
    ensure_net(world);
    world.walking.walkers = walkers.into_iter().collect();
    for (k, t) in timers {
        if let Some((i, _)) = world.walking.net.crossing(k) {
            world.walking.timers[i as usize] = t;
        }
    }
    world.walking.stats = stats;
}

/// The network for renderers and the interface (`None` until the
/// simulation has built it for the current roads).
pub fn network(world: &World) -> Option<&WalkNet> {
    let n = &world.walking.net;
    (n.roads_version == world.versions.roads && !n.points.is_empty()).then_some(n)
}

/// Pace of a citizen, m/s.
pub fn pace(world: &World, id: CitizenId) -> f32 {
    let Some(c) = world.citizens.get(id) else {
        return balance::WALK_SPEED;
    };
    let k = 1.0 + balance::WALK_SPEED_SPREAD * (crate::people::habit(c.seed, 0x3A1C) * 2.0 - 1.0);
    balance::WALK_SPEED * k * balance::WALK_STAGE_FACTOR[c.stage.index()]
}

/// Share of the people who walk a route of `len` metres: `WALK_SHARE_NEAR`
/// up to `WALK_NEAR_M`, then falling by `1/e` every `WALK_DECAY_M`, none
/// beyond `WALK_MAX_M` (older people: `WALK_SENIOR_RANGE` of the
/// distances).
pub fn walk_share(len: f32, senior: bool) -> f32 {
    let r = if senior { balance::WALK_SENIOR_RANGE } else { 1.0 };
    if len > balance::WALK_MAX_M * r {
        return 0.0;
    }
    let over = (len - balance::WALK_NEAR_M * r).max(0.0);
    balance::WALK_SHARE_NEAR * libm::expf(-over / (balance::WALK_DECAY_M * r))
}

/// How many people may walk on the map at once.
pub fn walker_budget(world: &World) -> usize {
    (world.stats.population as usize * balance::WALKERS_PER_1000 / 1000)
        .clamp(balance::MIN_WALKERS, balance::MAX_WALKERS)
}

/// A walking route between two buildings (door to door), if one exists
/// within `limit` metres.
pub(crate) fn route_between(world: &mut World, from: BuildingId, to: BuildingId, limit: f32) -> Option<Route> {
    let (a, b) = (world.buildings.get(from)?, world.buildings.get(to)?);
    let (pa, pb) = (door(a), door(b));
    if geom::dist(pa, pb) > limit {
        return None;
    }
    ensure_net(world);
    world.walking.net.route(pa, pb, limit)
}

/// Start a walker for `leader` (and `party`) along `route`.
pub(crate) fn start(
    world: &mut World,
    leader: CitizenId,
    party: &[CitizenId],
    from: Option<BuildingId>,
    to: BuildingId,
    route: Route,
) {
    let mut speed = pace(world, leader);
    for &p in party {
        speed = speed.min(pace(world, p));
    }
    let seed = world.citizens.get(leader).map_or(0, |c| c.seed);
    let h = crate::people::hash(seed as u64 ^ 0x0FF5E7) as u32;
    let offset = -0.25 + 0.6 * (h % 1000) as f32 / 1000.0;
    let mut w = Walker {
        party: party.to_vec(),
        from,
        to,
        route: Box::new(route),
        leg: 0,
        along: 0.0,
        speed,
        offset,
        seed,
        pos: [0.0, 0.0],
        heading: 0.0,
        state: WalkState::Walking,
        walked_m: 0.0,
        leg_start_m: 0.0,
        started_tick: world.time.tick,
        waited_ticks: 0,
        kerb_ticks: 0,
        leg_a: [0.0, 0.0],
        leg_d: [1.0, 0.0],
        leg_len: 0.0,
        next_mark: u16::MAX,
    };
    w.enter_leg();
    w.update_pose();
    world.walking.walkers.insert(leader, w);
}

/// After a road edit: everyone on foot finds their way again from where
/// they are (or finishes off screen if no way is left).
fn reroute_all(world: &mut World) {
    let ids: Vec<CitizenId> = world.walking.walkers.keys().copied().collect();
    for id in ids {
        let w = &world.walking.walkers[&id];
        let (pos, to) = (w.pos, w.to);
        let Some(target) = world.buildings.get(to).map(door) else {
            continue;
        };
        let left = w.left_m();
        let route = world
            .walking
            .net
            .route(pos, target, (left * 2.0 + 200.0).max(balance::WALK_MAX_M));
        match route {
            Some(r) => {
                let w = world.walking.walkers.get_mut(&id).expect("walker");
                let walked = w.walked_m;
                w.route = Box::new(r);
                w.leg = 0;
                w.along = 0.0;
                w.leg_start_m = 0.0;
                w.state = WalkState::Walking;
                w.walked_m = walked;
                w.enter_leg();
                w.update_pose();
            }
            None => {
                world.walking.walkers.remove(&id);
                crate::day::continue_off_screen(world, id);
            }
        }
    }
}

/// Before the cars move: which zebras people hold, who calls a signal.
pub(crate) fn prepare(world: &mut World) {
    let built = world.walking.net.roads_version;
    ensure_net(world);
    // Worked out at the end of the last walkers' tick (while they were
    // still warm in the cache), unless the network changed or the world was
    // just loaded.
    if world.walking.prepared_for == world.time.tick && built == world.walking.net.roads_version {
        return;
    }
    gather(world);
}

/// Who holds which half of which zebra, who calls a signal, from the
/// walkers' places.
fn gather(world: &mut World) {
    let Walking {
        walkers,
        net,
        now,
        touched,
        calls,
        ..
    } = &mut world.walking;
    for &ci in touched.iter() {
        if let Some(c) = now.get_mut(ci as usize) {
            *c = CrossNow::default();
        }
    }
    touched.clear();
    calls.clear();
    for w in walkers.values() {
        if w.state == WalkState::Walking {
            // About to reach a kerb: that crossing looks at the cars.
            if w.next_mark == w.leg + 1
                && w.leg_len - w.along < 1.0
                && let Some(k) = w.mark_at(w.leg + 1)
                && let Some((ci, _)) = net.crossing(k)
            {
                now[ci as usize].active = true;
                touched.push(ci);
            }
            continue;
        }
        let Some(k) = w.mark_at(w.leg) else { continue };
        let Some((ci, c)) = net.crossing(k) else { continue };
        let st = &mut now[ci as usize];
        st.active = true;
        touched.push(ci);
        match w.state {
            WalkState::Waiting => {
                st.calling = true;
                if let (Some(n), Some(a)) = (c.node, c.arm) {
                    calls.push((key_index(n), a));
                }
            }
            WalkState::Crossing => {
                st.people += 1 + w.party.len() as u16;
                let x = c.lateral(w.pos);
                let pts = &w.route.pts;
                let i = w.leg as usize;
                let plus = c.lateral(pts[(i + 1).min(pts.len() - 1)]) >= c.lateral(pts[i]);
                let slack = if w.party.is_empty() { 0.8 } else { 1.6 };
                let h = c.half_width;
                let (s1, s0) = if plus {
                    (x < h + slack, x < slack)
                } else {
                    (x > -slack, x > -h - slack)
                };
                st.held[1] |= s1;
                st.held[0] |= s0;
            }
            WalkState::Walking => {}
        }
    }
}

/// Signal calls from people waiting: (node slot, arm), sorted.
pub(crate) fn calls(world: &World) -> Vec<(usize, u8)> {
    let mut out = world.walking.calls.clone();
    out.sort_unstable();
    out.dedup();
    out
}

/// Does someone on the zebra across arm `arm` of junction `n` hold the half
/// a car uses that drives on `segment` towards the node (`toward`) or
/// away from it?
pub(crate) fn held_at(walking: &Walking, roads: &crate::RoadNetwork, n: NodeId, arm: u8, toward: bool) -> bool {
    let Some(ci) = walking.net.at_arm(n, arm) else {
        return false;
    };
    let c = &walking.net.crossings[ci as usize];
    let Some(seg) = roads.segments.get(c.key.segment) else {
        return false;
    };
    // Travel `a -> b` uses side 1.
    let forward = if toward { seg.b == n } else { seg.a == n };
    walking
        .now
        .get(ci as usize)
        .is_some_and(|s| s.held[usize::from(forward)])
}

/// Must a car about to enter junction `n` from arm `from` towards arm `to`
/// wait for people on the zebras it would cross (its own arm's, just past
/// its stop line, and the one it turns onto)? The crossing it waits for.
pub(crate) fn blocks_entry(
    walking: &Walking,
    roads: &crate::RoadNetwork,
    n: NodeId,
    from: u8,
    to: Option<u8>,
) -> Option<u32> {
    if held_at(walking, roads, n, from, true) {
        return walking.net.at_arm(n, from);
    }
    to.filter(|&b| held_at(walking, roads, n, b, false))
        .and_then(|b| walking.net.at_arm(n, b))
}

/// The nearest mid-block zebra ahead of `progress` on segment `s` driven
/// `forward`: (near edge in travel metres, far edge, held for this
/// direction).
pub(crate) fn zebra_ahead(
    walking: &Walking,
    s: SegmentId,
    forward: bool,
    len: f32,
    progress: f32,
) -> Option<(f32, f32, bool)> {
    let mids = walking.net.seg_mid.get(key_index(s))?;
    if mids.is_empty() {
        return None;
    }
    let mut best: Option<(f32, f32, bool)> = None;
    for &(from_a, ci) in mids {
        let at = if forward { from_a } else { len - from_a };
        let (near, far) = (at - balance::ZEBRA_WIDTH_M * 0.5, at + balance::ZEBRA_WIDTH_M * 0.5);
        if far < progress - 6.0 || best.is_some_and(|b| near >= b.0) {
            continue;
        }
        let held = walking
            .now
            .get(ci as usize)
            .is_some_and(|st| st.held[usize::from(forward)]);
        if far >= progress - 6.0 {
            best = Some((near, far, held));
        }
    }
    best
}

/// May someone step onto crossing `ci` now? (No car that cannot stop, the
/// signal shows the parallel green, the cars have had their turn.)
/// Neither a red signal for people nor the cars' turn at crossing `ci`.
fn could_cross(world: &World, ci: u32) -> bool {
    let c = &world.walking.net.crossings[ci as usize];
    if world.time.tick < world.walking.timers[ci as usize].turn_until {
        return false;
    }
    if let (Some(n), Some(arm)) = (c.node, c.arm)
        && let Some(j) = world.traffic.junction_states.get(n)
        && j.control == JunctionControl::Signal
    {
        return signal_walk(j, arm, 0.0);
    }
    true
}

fn may_cross(world: &World, ci: u32, cross_s: f32, waited: u32) -> bool {
    let tick = world.time.tick;
    let st = world.walking.now[ci as usize];
    if st.hazard {
        return false;
    }
    let c = &world.walking.net.crossings[ci as usize];
    let t = world.walking.timers[ci as usize];
    if tick < t.turn_until {
        return false;
    }
    if let (Some(n), Some(arm)) = (c.node, c.arm)
        && let Some(j) = world.traffic.junction_states.get(n)
        && j.control == JunctionControl::Signal
    {
        return signal_walk(j, arm, cross_s);
    }
    // Without signals: wait for a gap in the traffic (for a while), join a
    // group still stepping on, else start a new one.
    if st.coming && waited < secs(balance::PED_PATIENCE_S) as u32 {
        return false;
    }
    st.people == 0 || tick < t.group_start + secs(balance::PED_GROUP_S) || !st.car_waiting
}

/// At a signal: does the current phase let people cross arm `arm` (it
/// sends no traffic from that arm, is not a protected-left phase, and has
/// green left for the crossing)?
pub fn signal_walk(j: &crate::junction::JunctionState, arm: u8, cross_s: f32) -> bool {
    let Some(plan) = j.plan.as_ref() else {
        return true;
    };
    let Some(phase) = plan.phases.get(j.signal.phase as usize) else {
        return false;
    };
    let el = j.signal.ticks as f32 / TICKS_PER_SECOND as f32;
    j.signal.stage == SignalStage::Green
        && !phase.left_only
        && !phase.arms.contains(&arm)
        && el + cross_s <= phase.max_green_s.max(phase.min_green_s + cross_s)
}

/// Does junction `n`'s signal show people the green across arm `arm` now
/// (renderers: the walk light)? `None` without signals there.
pub fn walk_signal(world: &World, n: NodeId, arm: u8) -> Option<bool> {
    let j = world.traffic.junction_states.get(n)?;
    if j.control != JunctionControl::Signal {
        return None;
    }
    Some(signal_walk(j, arm, 0.0))
}

/// People waiting at a kerb look at the traffic every this many ticks.
const WAIT_CHECK: u64 = 4;

fn secs(s: f32) -> u64 {
    (s * TICKS_PER_SECOND as f32).round() as u64
}

/// What the cars are doing at crossing `ci` (after they moved this tick):
/// (a car is over it or cannot stop short of it, a moving car reaches it
/// within `PED_GAP_S`, a car stands waiting for the people on it). Looks
/// only at the cars on its road and, at a junction, on the junction's
/// arms and in its box (`Derived::walk_lanes` / `walk_boxed`, filled by
/// the driving step).
fn eval_crossing(world: &World, ci: u32) -> (bool, bool, bool) {
    let net = &world.walking.net;
    let roads = &world.roads;
    let states = &world.traffic.junction_states;
    let c = &net.crossings[ci as usize];
    let st = world.walking.now[ci as usize];
    let zw = balance::ZEBRA_WIDTH_M;
    let (mut hazard, mut coming, mut waiting) = (false, false, false);
    let lanes = &world.cache.walk_lanes;
    let mut check = |v: &crate::Vehicle, l: f32, near: f32, front: f32, rear: f32, half: usize| {
        // A standing car can stop (it sees the people before it moves).
        let sd = if v.speed > balance::WAIT_SPEED {
            v.speed * v.speed / (2.0 * balance::BRAKING) + 2.0 * v.speed * DT + balance::PED_SAFETY_M
        } else {
            0.0
        };
        let over = rear < near + zw && front > near;
        let ahead = near - front;
        let _ = l;
        hazard |= over || (0.0..sd).contains(&ahead);
        waiting |= v.speed < balance::WAIT_SPEED && (-0.5..12.0).contains(&ahead) && st.held[half];
        coming |= v.speed > 2.0 && ahead >= 0.0 && ahead < v.speed * balance::PED_GAP_S;
    };
    // Cars on lanes of segment slot `slot`.
    let on = |slot: usize| {
        let lo = lanes.partition_point(|x| (x.0 as usize) < slot);
        lanes[lo..]
            .iter()
            .take_while(move |x| x.0 as usize == slot)
            .map(|x| x.1)
    };
    let empty: &[SegmentId] = &[];
    let arms = c
        .node
        .and_then(|n| roads.nodes.get(n))
        .map_or(empty, |node| node.segments.as_slice());
    let boxed = c.node.map_or(&[][..], |n| {
        let bx = &world.cache.walk_boxed;
        let slot = key_index(n) as u32;
        let lo = bx.partition_point(|x| x.0 < slot);
        let hi = lo + bx[lo..].iter().take_while(|x| x.0 == slot).count();
        &bx[lo..hi]
    });
    // Every car once: those on its road, on the junction's other arms, in
    // the box.
    let cars = on(key_index(c.key.segment))
        .chain(
            arms.iter()
                .filter(|&&x| x != c.key.segment)
                .flat_map(|&x| on(key_index(x))),
        )
        .chain(boxed.iter().map(|x| x.1));
    for id in cars {
        let Some(v) = world.vehicles.get(id) else { continue };
        let Some(s) = v.segment else { continue };
        let Some(seg) = roads.segments.get(s) else { continue };
        // The longest body (a truck): no need to look the trip up.
        let l = balance::TRUCK_BODY_M[0];
        match &v.junction {
            Some(jm) => {
                if c.node != Some(jm.node) {
                    continue;
                }
                let (front, rear) = (jm.distance + l * 0.5, jm.distance - l);
                if jm.from == c.key.segment {
                    let toward = roads.segments.get(jm.from).is_some_and(|x| x.b == jm.node);
                    check(v, l, balance::ZEBRA_INSET_M, front, rear, usize::from(toward));
                }
                if jm.to == c.key.segment {
                    let away = roads.segments.get(jm.to).is_some_and(|x| x.a == jm.node);
                    check(
                        v,
                        l,
                        jm.length - balance::ZEBRA_INSET_M - zw,
                        front,
                        rear,
                        usize::from(away),
                    );
                }
            }
            None => {
                let len = seg.length;
                let (front, rear) = (v.progress + l * 0.5, v.progress - l);
                if s == c.key.segment {
                    let at = if v.forward { c.from_a } else { len - c.from_a };
                    check(v, l, at - zw * 0.5, front, rear, usize::from(v.forward));
                    continue;
                }
                // Approaching the junction: the zebra it turns onto.
                let Some(n) = c.node else { continue };
                let node = if v.forward { seg.b } else { seg.a };
                if node != n || v.route.first() != Some(&c.key.segment) {
                    continue;
                }
                let Some(j) = states.get(node) else { continue };
                let (Some(a), Some(b)) = (j.arm_of(s), j.arm_of(c.key.segment)) else {
                    continue;
                };
                let stop = len - j.arms[a].setback;
                let Some(m) = j.movement(a, v.lane, b) else { continue };
                let lm = j.movements[m].length;
                let away = roads.segments.get(c.key.segment).is_some_and(|x| x.a == node);
                check(
                    v,
                    l,
                    stop + lm - balance::ZEBRA_INSET_M - zw,
                    front,
                    rear,
                    usize::from(away),
                );
            }
        }
    }
    (hazard, coming, waiting)
}

/// [`eval_crossing`] once per crossing and tick.
fn evaluate(world: &mut World, ci: u32) {
    if world.walking.now[ci as usize].evaluated {
        return;
    }
    world.walking.touched.push(ci);
    let (h, c, w) = eval_crossing(world, ci);
    let st = &mut world.walking.now[ci as usize];
    st.evaluated = true;
    st.hazard |= h;
    st.coming |= c;
    st.car_waiting |= w;
}

/// After the cars moved: walkers move (see the module docs).
pub(crate) fn tick(world: &mut World) {
    let tick = world.time.tick;
    if world.time.tick_of_day == 0 {
        let s = &mut world.walking.stats;
        s.walked_yesterday = std::mem::take(&mut s.walked_today);
        s.other_yesterday = std::mem::take(&mut s.other_today);
        s.walked_by_hour_yesterday = std::mem::take(&mut s.walked_by_hour);
        s.other_by_hour_yesterday = std::mem::take(&mut s.other_by_hour);
    }
    if world.walking.walkers.is_empty() {
        return;
    }
    // Crossings people wait at while others are on them (and may cross:
    // not red, not the cars' turn): do cars stand waiting for them? Looked
    // at every `WAIT_CHECK` ticks.
    if tick.is_multiple_of(WAIT_CHECK) {
        for k in 0..world.walking.touched.len() {
            let ci = world.walking.touched[k];
            let st = world.walking.now[ci as usize];
            if st.calling && st.people > 0 && could_cross(world, ci) {
                evaluate(world, ci);
            }
        }
    }
    for ci in std::mem::take(&mut world.cache.walk_waits) {
        if let Some(st) = world.walking.now.get_mut(ci as usize) {
            st.car_waiting = true;
            world.walking.touched.push(ci);
        }
    }
    // People give the cars a turn once one has waited long enough.
    let patience = secs(balance::CAR_PATIENCE_S) as u32;
    if tick.is_multiple_of(WAIT_CHECK) {
        let Walking {
            timers, now, touched, ..
        } = &mut world.walking;
        for &ci in touched.iter() {
            let (t, st) = (&mut timers[ci as usize], &now[ci as usize]);
            if st.car_waiting && st.people > 0 {
                t.car_wait += WAIT_CHECK as u32;
                if t.car_wait >= patience {
                    t.turn_until = tick + secs(balance::CAR_TURN_S);
                    t.car_wait = 0;
                }
            } else if st.evaluated && !st.car_waiting {
                t.car_wait = 0;
            }
        }
    }
    let mut walkers = std::mem::take(&mut world.walking.walkers);
    let mut arrived: Vec<(CitizenId, Vec<CitizenId>)> = Vec::new();
    let mut lost: Vec<CitizenId> = Vec::new();
    for (&id, w) in walkers.iter_mut() {
        // Still on this walk? (Looked at now and then, and on arrival: the
        // citizens are big and far apart in memory.)
        let to = w.to;
        let still = |world: &World| {
            world.citizens.get(id).is_some_and(|c| {
                matches!(c.location, Location::Travelling(j) if j.mode == TravelMode::Walking && j.to == to && j.arrive_tick.is_none())
            })
        };
        if (tick + key_index(id) as u64).is_multiple_of(16) && !still(world) {
            lost.push(id);
            continue;
        }
        let mut step = w.speed * DT;
        // Plain walking along a leg: nothing else to read.
        if w.state == WalkState::Walking && w.along + step < w.leg_len {
            w.along += step;
            w.walked_m += step;
            w.update_pose();
            continue;
        }
        loop {
            let leg = w.leg as usize;
            if leg + 1 >= w.route.pts.len() {
                if still(world) {
                    arrived.push((id, w.party.clone()));
                } else {
                    lost.push(id);
                }
                break;
            }
            if w.state == WalkState::Waiting && !(tick + w.seed as u64).is_multiple_of(WAIT_CHECK) && w.kerb_ticks > 0 {
                w.waited_ticks += 1;
                w.kerb_ticks += 1;
                break;
            }
            if w.state == WalkState::Waiting {
                let k = w.mark_at(w.leg);
                let go = match k.and_then(|k| world.walking.net.crossing(k)) {
                    Some((ci, c)) => {
                        let cross_s = (2.0 * (c.half_width + pavement_m(c.kind))) / w.speed.max(0.3);
                        let go = could_cross(world, ci) && {
                            evaluate(world, ci);
                            may_cross(world, ci, cross_s, w.kerb_ticks)
                        };
                        world.walking.touched.push(ci);
                        let st = &mut world.walking.now[ci as usize];
                        if go {
                            if st.people == 0 {
                                world.walking.timers[ci as usize].group_start = tick;
                            }
                            st.people += 1 + w.party.len() as u16;
                            st.held = [true, true];
                        } else {
                            st.calling = true;
                        }
                        go
                    }
                    // The crossing is gone (a road edit): walk on.
                    None => true,
                };
                if !go {
                    w.waited_ticks += 1;
                    w.kerb_ticks += 1;
                    break;
                }
                w.kerb_ticks = 0;
                w.state = WalkState::Crossing;
            }
            let rem = w.leg_len - w.along;
            if step < rem {
                w.along += step;
                w.walked_m += step;
                break;
            }
            step -= rem.max(0.0);
            w.walked_m += rem.max(0.0);
            w.leg_start_m += w.leg_len;
            w.leg += 1;
            w.along = 0.0;
            w.enter_leg();
            w.state = if w.next_mark == w.leg && (w.leg as usize) + 1 < w.route.pts.len() {
                WalkState::Waiting
            } else {
                WalkState::Walking
            };
        }
        w.update_pose();
    }
    for id in &lost {
        walkers.remove(id);
    }
    for (id, _) in &arrived {
        walkers.remove(id);
    }
    world.walking.walkers = walkers;
    for (id, party) in arrived {
        crate::day::walk_arrived(world, id, &party);
    }
    // The next tick's zebra states, while the walkers are warm in memory.
    gather(world);
    world.walking.prepared_for = tick + 1;
}

/// Count a trip started (for the share on foot).
pub(crate) fn count_trip(world: &mut World, on_foot: bool) {
    let h = (world.time.hour() as usize).min(23);
    let s = &mut world.walking.stats;
    if on_foot {
        s.walked_today += 1;
        s.walked_by_hour[h] += 1;
    } else {
        s.other_today += 1;
        s.other_by_hour[h] += 1;
    }
}

/// Where a walking citizen is, if walked on the map (leader or party).
pub fn walker_of(world: &World, id: CitizenId) -> Option<(CitizenId, &Walker)> {
    if let Some(w) = world.walking.walkers.get(&id) {
        return Some((id, w));
    }
    let c = world.citizens.get(id)?;
    let Location::Travelling(j) = c.location else {
        return None;
    };
    if j.mode != TravelMode::Walking {
        return None;
    }
    let lead = j.driver?;
    world.walking.walkers.get(&lead).map(|w| (lead, w))
}

/// Every walker's body as a circle (centre, radius) on the crossings, for
/// tests: party members stand beside the leader.
pub fn people_on_crossings(world: &World) -> Vec<([f32; 2], f32)> {
    let mut out = Vec::new();
    for w in world.walking.walkers.values() {
        if w.state != WalkState::Crossing {
            continue;
        }
        out.push((w.pos, 0.3 + 0.7 * (w.party.len().min(1) as f32)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_floats_round_trip() {
        for x in [0.0f32, 1.5, 1e6, 3.25] {
            assert_eq!(from_ord(ord(x)), x);
        }
        assert!(ord(1.0) < ord(2.0));
    }

    #[test]
    fn polyline_slices_go_both_ways() {
        let p = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]];
        assert_eq!(poly_slice(&p, 5.0, 15.0), vec![[5.0, 0.0], [10.0, 0.0], [10.0, 5.0]]);
        assert_eq!(poly_slice(&p, 15.0, 5.0), vec![[10.0, 5.0], [10.0, 0.0], [5.0, 0.0]]);
        assert!((poly_len(&p) - 20.0).abs() < 1e-5);
    }
}

/// Car ticks held by people on zebras so far (see `Derived::walk_holds`):
/// entering at signals, entering elsewhere, in a box, mid-block (people),
/// mid-block (no room beyond).
pub fn debug_holds(world: &World) -> [u64; 5] {
    world.cache.walk_holds
}
