//! Road geometry from the road graph alone: every segment is a curve
//! (`RoadSegment::curve`), every junction the surface the sim publishes
//! (`junction::junction_shape`: the cut line of each arm and the kerbs
//! between them).
//!
//! * A segment is meshed between the cuts at its two ends along a
//!   [`Frame`]: centre-line samples from the curve's flattening (two for a
//!   straight road, more where it bends), so the carriageway, kerbs,
//!   sidewalks, lane lines, medians, arrows, wear and furniture all follow
//!   the curve. Everything that runs along the road is a ribbon over the
//!   frame at some offset to the right of travel (`a -> b`).
//! * A junction is the polygon of its arm cuts joined by rounded kerb
//!   corners (quadratic fillets towards where the two kerb lines meet,
//!   finely sampled), with the sidewalk following the same corners; each
//!   arm gets a zebra crossing square to it just inside its cut.
//! * Dead ends get a round cap; the highway entry continues off the map.
//!
//! Everything here is pure (no ECS) so it is unit-tested.

use std::f32::consts::{PI, TAU};
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use citysim_sim::junction::{ArmCut, junction_shape};
use citysim_sim::{Curve, NodeId, RoadKind, SegmentId, World};

use super::mesh_util::{Glow, MeshBuilder};
use super::palette::{self, lin, shade};

/// Top of the asphalt.
pub const ROAD_Y: f32 = 0.06;
/// Top of the sidewalk.
pub const WALK_Y: f32 = 0.19;
/// Painted markings, just above the asphalt.
pub const MARK_Y: f32 = ROAD_Y + 0.016;
/// Width of the darker kerb stone along the sidewalk edge.
const KERB_W: f32 = 0.22;
/// Flattening tolerance of road centre lines, metres.
const FLAT_TOL: f32 = 0.04;
/// Samples of a kerb corner.
const CORNER_STEPS: usize = 10;

/// Cross-section of a road kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Half-width of the carriageway (kerb to centre).
    pub h: f32,
    /// Sidewalk width on each side (0 = none).
    pub walk: f32,
    pub highway: bool,
    /// Half-width of the raised central median (0 = painted centre line).
    pub median: f32,
    /// Lanes per direction.
    pub lanes: u8,
}

impl Style {
    pub fn outer(&self) -> f32 {
        self.h + self.walk
    }
}

pub fn style(kind: RoadKind) -> Style {
    match kind {
        // 8 m in total: 5.5 m carriageway, 1.25 m sidewalks.
        RoadKind::Street => Style {
            h: RoadKind::Street.half_width_m(),
            walk: 1.25,
            highway: false,
            median: 0.0,
            lanes: 1,
        },
        // 13 m carriageway (two 2.75 m lanes each way either side of a 2 m
        // planted median, cut into turn pockets before junctions), wider
        // sidewalks: 16 m overall.
        RoadKind::Avenue => Style {
            h: RoadKind::Avenue.half_width_m(),
            walk: 1.5,
            highway: false,
            median: RoadKind::Avenue.median_half_width_m(),
            lanes: 2,
        },
        RoadKind::Highway => Style {
            h: RoadKind::Highway.half_width_m(),
            walk: 0.0,
            highway: true,
            median: 0.0,
            lanes: 2,
        },
    }
}

/// Lateral offset of lane `lane` (0 = kerb side) to the right of travel,
/// as the sim defines it.
pub fn lane_offset(kind: RoadKind, lane: u8) -> f32 {
    kind.lane_offset_m(lane)
}

/// Right of a direction (the sim's `right_of`: traffic drives on the right).
pub fn perp(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

pub fn lift(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

// ---------------------------------------------------------------- frames

/// Centre-line samples of a curve: `(distance along, point, unit
/// direction)`, dense where it bends, two points when it is straight.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub pts: Vec<(f32, Vec2, Vec2)>,
}

impl Frame {
    pub fn new(curve: &Curve) -> Frame {
        Self::with_tol(curve, FLAT_TOL)
    }

    pub fn with_tol(curve: &Curve, tol: f32) -> Frame {
        let pts = curve
            .flatten_offset(0.0, tol)
            .into_iter()
            .map(|(d, p)| (d, Vec2::from(p), Vec2::from(curve.pose_at(d, 0.0).dir)))
            .collect();
        Frame { pts }
    }

    /// Frame of a straight line.
    pub fn line(a: Vec2, b: Vec2) -> Frame {
        let d = (b - a).normalize_or(Vec2::X);
        Frame {
            pts: vec![(0.0, a, d), ((b - a).length(), b, d)],
        }
    }

    pub fn len(&self) -> f32 {
        self.pts.last().map_or(0.0, |p| p.0)
    }

    #[cfg(test)]
    pub fn is_straight(&self) -> bool {
        self.pts.len() <= 2
    }

    /// Point and unit direction `t` metres along (clamped to the ends,
    /// extrapolated straight beyond them).
    pub fn pose(&self, t: f32) -> (Vec2, Vec2) {
        let n = self.pts.len();
        if n == 0 {
            return (Vec2::ZERO, Vec2::X);
        }
        if t <= self.pts[0].0 {
            let (d, p, dir) = self.pts[0];
            return (p + dir * (t - d), dir);
        }
        if t >= self.pts[n - 1].0 {
            let (d, p, dir) = self.pts[n - 1];
            return (p + dir * (t - d), dir);
        }
        let k = self.pts.partition_point(|p| p.0 <= t).clamp(1, n - 1);
        let (d0, p0, r0) = self.pts[k - 1];
        let (d1, p1, r1) = self.pts[k];
        let u = ((t - d0) / (d1 - d0).max(1e-6)).clamp(0.0, 1.0);
        (p0.lerp(p1, u), r0.lerp(r1, u).normalize_or(r0))
    }

    /// The point `t` along and `o` to the right.
    pub fn at(&self, t: f32, o: f32) -> Vec2 {
        let (p, d) = self.pose(t);
        p + perp(d) * o
    }

    /// Distances of the samples strictly inside `t0..t1`, with both ends.
    pub fn steps(&self, t0: f32, t1: f32) -> Vec<f32> {
        let mut v = vec![t0];
        v.extend(self.pts.iter().map(|p| p.0).filter(|&d| d > t0 + 1e-3 && d < t1 - 1e-3));
        v.push(t1);
        v
    }

    /// The line `o` to the right between `t0` and `t1`.
    pub fn line_at(&self, t0: f32, t1: f32, o: f32) -> Vec<Vec2> {
        self.steps(t0, t1).into_iter().map(|t| self.at(t, o)).collect()
    }
}

/// Upward ribbon along `f` from `t0` to `t1`, between offsets `o0` and `o1`.
#[allow(clippy::too_many_arguments)]
pub fn strip(m: &mut MeshBuilder, f: &Frame, t0: f32, t1: f32, o0: f32, o1: f32, y: f32, c: [f32; 4]) {
    if t1 - t0 < 1e-4 {
        return;
    }
    let steps = f.steps(t0, t1);
    for w in steps.windows(2) {
        let q = [f.at(w[0], o0), f.at(w[1], o0), f.at(w[1], o1), f.at(w[0], o1)];
        m.quad_up(q.map(|p| lift(p, y)), c);
    }
}

/// Vertical face along `f` at offset `o` from `y0` to `y1`, facing towards
/// increasing offset when `out` is positive.
#[allow(clippy::too_many_arguments)]
fn side_face(m: &mut MeshBuilder, f: &Frame, t0: f32, t1: f32, o: f32, y0: f32, y1: f32, out: f32, color: Color) {
    let line = f.line_at(t0, t1, o);
    for (k, w) in line.windows(2).enumerate() {
        let (_, d) = f.pose(t0 + (t1 - t0) * (k as f32 + 0.5) / (line.len() - 1) as f32);
        vface(m, w[0], w[1], y0, y1, perp(d) * out, color);
    }
}

// ---------------------------------------------------------------- layout

/// One arm of a junction: where its road surface is cut.
#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub seg: SegmentId,
    /// Unit direction away from the node at the cut.
    pub dir: Vec2,
    /// Centre-line point of the cut.
    pub center: Vec2,
    pub style: Style,
    /// Metres from the node along the curve to the cut (the sim's stop
    /// line).
    pub setback: f32,
    /// Same as `setback` (kept for the furniture code: where the sim's
    /// stop line is).
    pub stop: f32,
}

impl Arm {
    /// Point `along` metres further out from the cut (straight along the
    /// cut direction) and `o` to the right of `dir`.
    pub fn at(&self, along: f32, o: f32) -> Vec2 {
        self.center + self.dir * along + perp(self.dir) * o
    }
}

/// What a node needs drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// A junction surface (three or more arms, or a bend / merge box).
    Junction,
    /// A dead end: round cap beyond the node, towards `-dir` of its arm.
    End,
    /// A smooth continuation of two segments: nothing but a filler where
    /// the two ends meet at a slight angle.
    Through,
    /// The highway entry at the map edge: the road continues off the map.
    Entry,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeLayout {
    pub id: NodeId,
    pub pos: Vec2,
    pub kind: NodeKind,
    /// Arms in counter-clockwise order (sim axes).
    pub arms: Vec<Arm>,
    /// Number of segments meeting here.
    pub degree: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegLayout {
    /// `None` for the decorative highway beyond the map edge.
    pub id: Option<SegmentId>,
    pub kind: RoadKind,
    pub style: Style,
    /// Centre line `a -> b`.
    pub frame: Frame,
    /// Where the road surface starts / ends (metres from `a` / `b`).
    pub s_a: f32,
    pub s_b: f32,
    /// Number of segments at each end node.
    pub deg_a: usize,
    pub deg_b: usize,
    /// The end has a junction box (markings stop at the cut; arrows).
    pub box_a: bool,
    pub box_b: bool,
    /// Points on the sidewalks to keep clear (driveway mouths): no
    /// furniture there, lamps step aside.
    pub avoid: Vec<Vec2>,
    /// Turn pockets cut into the median (`junction::turn_pocket`), as
    /// `(entry or stop line, stop line or entry)` metres from `a`, sorted:
    /// `[0]` for travel `a -> b` (at `b`), `[1]` for `b -> a` (at `a`).
    pub pockets: [Option<(f32, f32)>; 2],
}

impl SegLayout {
    /// A segment alone (no junctions): tool ghosts.
    pub fn bare(curve: &Curve, kind: RoadKind) -> SegLayout {
        SegLayout {
            id: None,
            kind,
            style: style(kind),
            frame: Frame::new(curve),
            s_a: 0.0,
            s_b: 0.0,
            deg_a: 1,
            deg_b: 1,
            box_a: false,
            box_b: false,
            avoid: Vec::new(),
            pockets: [None; 2],
        }
    }

    /// Where the planted median runs: the drawn span, ended short of the
    /// turn pockets cut into it.
    pub fn median_span(&self) -> (f32, f32) {
        let (mut t0, mut t1) = self.span();
        if let Some((lo, _)) = self.pockets[0] {
            t1 = t1.min(lo - 0.3);
        }
        if let Some((_, hi)) = self.pockets[1] {
            t0 = t0.max(hi + 0.3);
        }
        (t0, t1)
    }

    pub fn len(&self) -> f32 {
        self.frame.len()
    }

    /// Start point of the centre line.
    pub fn a(&self) -> Vec2 {
        self.frame.pts.first().map_or(Vec2::ZERO, |p| p.1)
    }

    /// End point of the centre line.
    pub fn b(&self) -> Vec2 {
        self.frame.pts.last().map_or(Vec2::ZERO, |p| p.1)
    }

    pub fn mid(&self) -> Vec2 {
        self.frame.pose(self.len() * 0.5).0
    }

    /// Drawn part: from `t0` to `t1` along the centre line.
    pub fn span(&self) -> (f32, f32) {
        (self.s_a, self.len() - self.s_b)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RoadLayout {
    pub nodes: Vec<NodeLayout>,
    pub segs: Vec<SegLayout>,
}

/// Lay out junctions and segment pieces for the whole network.
pub fn layout(world: &World) -> RoadLayout {
    let roads = &world.roads;
    let mut out = RoadLayout::default();
    let mut cut: std::collections::HashMap<(SegmentId, NodeId), f32> = std::collections::HashMap::new();
    let mut boxed: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    for (nid, node) in roads.nodes.iter() {
        let pos = Vec2::from(node.pos);
        let degree = node.segments.len();
        let kind_of = |s: SegmentId| roads.segments.get(s).map_or(RoadKind::Street, |s| s.kind);
        if let Some(shape) = junction_shape(world, nid, 0.0) {
            let arms: Vec<Arm> = shape
                .arms
                .iter()
                .map(|a: &ArmCut| arm_of(a, kind_of(a.segment)))
                .collect();
            for a in &arms {
                cut.insert((a.seg, nid), a.setback);
            }
            boxed.insert(nid);
            out.nodes.push(NodeLayout {
                id: nid,
                pos,
                kind: NodeKind::Junction,
                arms,
                degree,
            });
            continue;
        }
        // No box: dead end, entry or smooth continuation. Arms at the node.
        let mut arms: Vec<Arm> = node
            .segments
            .iter()
            .filter_map(|&s| {
                let seg = roads.segments.get(s)?;
                Some(Arm {
                    seg: s,
                    dir: Vec2::from(seg.dir_from(nid)).normalize_or(Vec2::X),
                    center: pos,
                    style: style(seg.kind),
                    setback: 0.0,
                    stop: 0.0,
                })
            })
            .collect();
        arms.sort_by(|a, b| angle(a.dir).total_cmp(&angle(b.dir)));
        let kind = match degree {
            0 => continue,
            1 if roads.entry == Some(nid) => NodeKind::Entry,
            1 => NodeKind::End,
            _ => NodeKind::Through,
        };
        out.nodes.push(NodeLayout {
            id: nid,
            pos,
            kind,
            arms,
            degree,
        });
    }
    let degree = |n: NodeId| roads.nodes.get(n).map_or(0, |n| n.segments.len());
    for (sid, seg) in roads.segments.iter() {
        out.segs.push(SegLayout {
            id: Some(sid),
            kind: seg.kind,
            style: style(seg.kind),
            frame: Frame::new(&seg.curve),
            s_a: cut.get(&(sid, seg.a)).copied().unwrap_or(0.0),
            s_b: cut.get(&(sid, seg.b)).copied().unwrap_or(0.0),
            deg_a: degree(seg.a),
            deg_b: degree(seg.b),
            box_a: boxed.contains(&seg.a),
            box_b: boxed.contains(&seg.b),
            avoid: Vec::new(),
            pockets: [true, false].map(|fwd| {
                citysim_sim::junction::turn_pocket(world, sid, fwd).map(|p| p.from_a(seg.length))
            }),
        });
    }
    // The highway continues off the map into the fog.
    for node in &out.nodes {
        if node.kind == NodeKind::Entry {
            let arm = &node.arms[0];
            out.segs.push(SegLayout {
                id: None,
                kind: RoadKind::Highway,
                style: arm.style,
                frame: Frame::line(node.pos - arm.dir * super::scene::OUTSIDE, node.pos),
                s_a: 0.0,
                s_b: 0.0,
                deg_a: 2,
                deg_b: 2,
                box_a: false,
                box_b: false,
                avoid: Vec::new(),
                pockets: [None; 2],
            });
        }
    }
    out
}

fn arm_of(a: &ArmCut, kind: RoadKind) -> Arm {
    Arm {
        seg: a.segment,
        dir: Vec2::from(a.dir).normalize_or(Vec2::X),
        center: Vec2::from(a.center),
        style: style(kind),
        setback: a.setback,
        stop: a.setback,
    }
}

fn angle(v: Vec2) -> f32 {
    v.y.atan2(v.x)
}

// ---------------------------------------------------------------- hashing

/// Hasher input for floats.
fn hf<H: Hasher>(h: &mut H, v: f32) {
    v.to_bits().hash(h);
}

pub fn hash_node<H: Hasher>(n: &NodeLayout, h: &mut H) {
    hf(h, n.pos.x);
    hf(h, n.pos.y);
    (n.degree, n.kind == NodeKind::Junction, n.kind == NodeKind::Entry).hash(h);
    for a in &n.arms {
        for v in [
            a.dir.x,
            a.dir.y,
            a.center.x,
            a.center.y,
            a.setback,
            a.style.h,
            a.style.walk,
        ] {
            hf(h, v);
        }
    }
}

pub fn hash_seg<H: Hasher>(s: &SegLayout, h: &mut H) {
    for (d, p, _) in &s.frame.pts {
        for v in [*d, p.x, p.y] {
            hf(h, v);
        }
    }
    for v in [s.s_a, s.s_b] {
        hf(h, v);
    }
    (s.deg_a, s.deg_b, s.box_a, s.box_b, s.style.highway, s.style.lanes).hash(h);
    for p in &s.avoid {
        hf(h, p.x);
        hf(h, p.y);
    }
}

/// Is `p` clear of the points a segment must keep free?
fn clear_of(s: &SegLayout, p: Vec2, r: f32) -> bool {
    s.avoid.iter().all(|a| a.distance_squared(p) > r * r)
}

// ---------------------------------------------------------------- meshing

/// Vertical face along `a -> b` (2D), from `y0` to `y1`, facing `toward`.
pub fn vface(m: &mut MeshBuilder, a: Vec2, b: Vec2, y0: f32, y1: f32, toward: Vec2, color: Color) {
    if (b - a).length_squared() < 1e-8 {
        return;
    }
    let q = [lift(a, y0), lift(b, y0), lift(b, y1), lift(a, y1)];
    let n = (q[1] - q[0]).cross(q[2] - q[0]);
    if n.x * toward.x + n.z * toward.y >= 0.0 {
        m.quad(q[0], q[1], q[2], q[3], color);
    } else {
        m.quad(q[1], q[0], q[3], q[2], color);
    }
}

/// A sidewalk following paired kerb / outer points.
fn walk_strip(m: &mut MeshBuilder, kerb: &[Vec2], outer: &[Vec2]) {
    let walk = lin(palette::SIDEWALK);
    let kerb_col = lin(palette::KERB);
    for k in 0..kerb.len().min(outer.len()).saturating_sub(1) {
        let (k0, k1, o0, o1) = (kerb[k], kerb[k + 1], outer[k], outer[k + 1]);
        let w0 = (o0 - k0).length().max(1e-4);
        let w1 = (o1 - k1).length().max(1e-4);
        let m0 = k0 + (o0 - k0) * (KERB_W / w0).min(1.0);
        let m1 = k1 + (o1 - k1) * (KERB_W / w1).min(1.0);
        m.quad_up([k0, k1, m1, m0].map(|p| lift(p, WALK_Y)), kerb_col);
        m.quad_up([m0, m1, o1, o0].map(|p| lift(p, WALK_Y)), walk);
        let to_road = ((k0 + k1) - (o0 + o1)).normalize_or_zero();
        vface(m, k0, k1, ROAD_Y - 0.02, WALK_Y, to_road, palette::KERB);
        vface(m, o0, o1, 0.0, WALK_Y, -to_road, shade(palette::KERB, 0.92));
    }
}

/// Vertical end face closing a sidewalk that stops.
fn walk_end(m: &mut MeshBuilder, kerb: Vec2, outer: Vec2, toward: Vec2) {
    vface(m, kerb, outer, ROAD_Y - 0.02, WALK_Y, toward, palette::KERB);
}

fn arc(center: Vec2, r: f32, a0: f32, sweep: f32) -> Vec<Vec2> {
    let steps = ((sweep.abs() / (PI / 16.0)).ceil() as usize).max(1);
    (0..=steps)
        .map(|k| {
            let a = a0 + sweep * k as f32 / steps as f32;
            center + Vec2::from_angle(a) * r
        })
        .collect()
}

/// Where the lines `p + s d` and `q + u e` meet: `(s, u)`.
fn meet(p: Vec2, d: Vec2, q: Vec2, e: Vec2) -> Option<(f32, f32)> {
    let den = d.perp_dot(e);
    if den.abs() < 1e-5 {
        return None;
    }
    let w = q - p;
    Some((w.perp_dot(e) / den, w.perp_dot(d) / den))
}

/// The kerb corner from arm `a`'s counter-clockwise kerb to arm `b`'s
/// clockwise kerb, `extra` metres outside each carriageway (0 = the
/// asphalt edge): a quadratic towards where the two kerb lines meet (the
/// sim's outline uses the same construction), sampled finely; a straight
/// edge when they do not meet near the node.
fn corner(node: Vec2, a: &Arm, b: &Arm, extra_a: f32, extra_b: f32) -> Vec<Vec2> {
    let (ha, hb) = (a.style.h + extra_a, b.style.h + extra_b);
    let start = a.center + perp(a.dir) * ha;
    let end = b.center - perp(b.dir) * hb;
    let near = 3.0 * (a.style.h + b.style.h) + a.setback + b.setback + extra_a + extra_b;
    let ctrl = meet(start, a.dir, end, b.dir)
        .map(|(s, _)| start + a.dir * s)
        .filter(|c| c.distance(node) < near);
    match ctrl {
        Some(c) => (0..=CORNER_STEPS)
            .map(|k| {
                let t = k as f32 / CORNER_STEPS as f32;
                let u = 1.0 - t;
                start * (u * u) + c * (2.0 * u * t) + end * (t * t)
            })
            .collect(),
        None => vec![start, end],
    }
}

/// Junction surface, kerb corners, sidewalks and crossings of one node.
pub fn mesh_node(m: &mut MeshBuilder, node: &NodeLayout) {
    match node.kind {
        NodeKind::Entry => {}
        NodeKind::End => mesh_end(m, node),
        NodeKind::Through => mesh_through(m, node),
        NodeKind::Junction => mesh_junction(m, node),
    }
}

fn asphalt_of(arms: &[Arm]) -> [f32; 4] {
    lin(if arms.iter().any(|a| a.style.highway) {
        palette::ASPHALT_HIGHWAY
    } else {
        palette::ASPHALT
    })
}

fn mesh_junction(m: &mut MeshBuilder, node: &NodeLayout) {
    let arms = &node.arms;
    let k = arms.len();
    if k == 0 {
        return;
    }
    let mut ring: Vec<Vec2> = Vec::new();
    for i in 0..k {
        let (a, b) = (&arms[i], &arms[(i + 1) % k]);
        ring.push(a.center - perp(a.dir) * a.style.h);
        let kerb = corner(node.pos, a, b, 0.0, 0.0);
        ring.extend(&kerb);
        // The sidewalk round the corner.
        if a.style.walk > 0.0 && b.style.walk > 0.0 {
            let outer = corner(node.pos, a, b, a.style.walk, b.style.walk);
            if outer.len() == kerb.len() {
                walk_strip(m, &kerb, &outer);
            } else {
                // One side curved, the other straight: pair them by share.
                let n = kerb.len().max(outer.len());
                let pick = |v: &Vec<Vec2>, j: usize| {
                    let t = j as f32 / (n - 1) as f32 * (v.len() - 1) as f32;
                    let i0 = t.floor() as usize;
                    let i1 = (i0 + 1).min(v.len() - 1);
                    v[i0].lerp(v[i1], t - i0 as f32)
                };
                let k2: Vec<Vec2> = (0..n).map(|j| pick(&kerb, j)).collect();
                let o2: Vec<Vec2> = (0..n).map(|j| pick(&outer, j)).collect();
                walk_strip(m, &k2, &o2);
            }
        } else {
            if a.style.walk > 0.0 {
                let p = a.center + perp(a.dir) * a.style.h;
                walk_end(m, p, p + perp(a.dir) * a.style.walk, -a.dir);
            }
            if b.style.walk > 0.0 {
                let p = b.center - perp(b.dir) * b.style.h;
                walk_end(m, p, p - perp(b.dir) * b.style.walk, -b.dir);
            }
            if !arms.iter().any(|x| x.style.walk > 0.0) {
                // Highway pieces: exposed slab edge.
                for w in kerb.windows(2) {
                    vface(
                        m,
                        w[0],
                        w[1],
                        0.0,
                        ROAD_Y,
                        (w[0] + w[1]) * 0.5 - node.pos,
                        palette::KERB,
                    );
                }
            }
        }
    }
    let ring3: Vec<Vec3> = ring.iter().map(|&q| lift(q, ROAD_Y)).collect();
    m.fan_up(lift(node.pos, ROAD_Y), &ring3, asphalt_of(arms));
    // Zebra crossings just inside each arm's cut (the sim keeps its margin
    // between the stop line and the crossing carriageways free for them).
    // Arms meeting at a narrow angle can leave no room: a crossing that
    // would overlap another arm's is left out.
    if node.degree >= 3 {
        let white = lin(palette::MARKING);
        let width = (citysim_sim::balance::JUNCTION_MARGIN_M - 0.75).min(2.4);
        let zone = |a: &Arm| {
            [
                a.at(-0.4, -a.style.h),
                a.at(-0.4 - width, -a.style.h),
                a.at(-0.4 - width, a.style.h),
                a.at(-0.4, a.style.h),
            ]
        };
        let fits = |a: &Arm| !a.style.highway && width >= 1.2 && a.setback >= width + 1.0;
        for (i, a) in arms.iter().enumerate() {
            if !fits(a)
                || arms
                    .iter()
                    .enumerate()
                    .any(|(j, b)| j != i && fits(b) && quads_overlap(&zone(a), &zone(b)))
            {
                continue;
            }
            let zebra_w = 0.5;
            let mut o = -a.style.h + 0.45;
            while o + zebra_w <= a.style.h - 0.45 + 1e-3 {
                let q = [
                    a.at(-0.4, o),
                    a.at(-0.4 - width, o),
                    a.at(-0.4 - width, o + zebra_w),
                    a.at(-0.4, o + zebra_w),
                ];
                m.quad_up(q.map(|p| lift(p, MARK_Y + 0.002)), white);
                o += zebra_w * 2.0;
            }
        }
    }
}

/// Two convex quads overlap (separating axis test).
fn quads_overlap(a: &[Vec2; 4], b: &[Vec2; 4]) -> bool {
    for q in [a, b] {
        for k in 0..4 {
            let e = q[(k + 1) % 4] - q[k];
            let axis = Vec2::new(-e.y, e.x);
            let span = |p: &[Vec2; 4]| {
                p.iter()
                    .map(|v| v.dot(axis))
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

/// Round cap at a dead end, with the sidewalk around it.
fn mesh_end(m: &mut MeshBuilder, node: &NodeLayout) {
    let Some(a) = node.arms.first() else { return };
    let st = a.style;
    let n = perp(a.dir);
    // Half circle from the right kerb round the back to the left kerb.
    let a0 = angle(n);
    let pts = arc(node.pos, st.h, a0, PI);
    let ring3: Vec<Vec3> = pts.iter().map(|&q| lift(q, ROAD_Y)).collect();
    m.fan_up(lift(node.pos, ROAD_Y), &ring3, asphalt_of(&node.arms));
    if st.walk > 0.0 {
        let outer = arc(node.pos, st.outer(), a0, PI);
        walk_strip(m, &pts, &outer);
    } else {
        for w in pts.windows(2) {
            vface(
                m,
                w[0],
                w[1],
                0.0,
                ROAD_Y,
                (w[0] + w[1]) * 0.5 - node.pos,
                palette::KERB,
            );
        }
    }
}

/// Two segments meeting without a box: fill the wedge on the outside of
/// the slight angle between their ends (asphalt and sidewalk).
fn mesh_through(m: &mut MeshBuilder, node: &NodeLayout) {
    if node.arms.len() != 2 {
        return;
    }
    let (a, b) = (&node.arms[0], &node.arms[1]);
    // Travel through: in along -a.dir, out along b.dir.
    let (din, dout) = (-a.dir, b.dir);
    let turn = din.perp_dot(dout);
    if din.dot(dout) > 0.99995 {
        return;
    }
    let st = if a.style.h >= b.style.h { a.style } else { b.style };
    // The outside of the turn is to the left of travel for a right turn.
    let side = if turn > 0.0 { -1.0 } else { 1.0 };
    let (r_in, r_out) = (perp(din) * side, perp(dout) * side);
    let sweep_a = angle(r_in);
    let mut sweep = angle(r_out) - sweep_a;
    if sweep > PI {
        sweep -= TAU;
    }
    if sweep < -PI {
        sweep += TAU;
    }
    let road = arc(node.pos, st.h, sweep_a, sweep);
    let ring3: Vec<Vec3> = road.iter().map(|&q| lift(q, ROAD_Y)).collect();
    m.fan_up(lift(node.pos, ROAD_Y), &ring3, asphalt_of(&node.arms));
    if st.walk > 0.0 {
        let outer = arc(node.pos, st.outer(), sweep_a, sweep);
        walk_strip(m, &road, &outer);
    }
}

/// Dashed line along `f` at offset `off`, between `t0` and `t1`.
#[allow(clippy::too_many_arguments)]
fn dashes(m: &mut MeshBuilder, f: &Frame, off: f32, t0: f32, t1: f32, dash: f32, gap: f32, width: f32, c: [f32; 4]) {
    let span = t1 - t0;
    if span < dash * 0.5 {
        return;
    }
    // Centre the pattern within the span.
    let count = ((span + gap) / (dash + gap)).floor().max(1.0);
    let used = count * dash + (count - 1.0) * gap;
    let mut t = t0 + (span - used) * 0.5;
    for _ in 0..count as usize {
        let (s, e) = (t.max(t0), (t + dash).min(t1));
        strip(m, f, s, e, off - width * 0.5, off + width * 0.5, MARK_Y, c);
        t += dash + gap;
    }
}

/// The road surface of a segment between its two cuts.
pub fn mesh_segment(m: &mut MeshBuilder, s: &SegLayout) {
    mesh_segment_with(m, s, true);
}

/// `mesh_segment`, optionally without lamps and gantries (tool ghosts).
pub fn mesh_segment_with(m: &mut MeshBuilder, s: &SegLayout, furniture: bool) {
    let f = &s.frame;
    let (t0, t1) = s.span();
    if t1 - t0 < 1e-3 {
        return;
    }
    let st = s.style;
    let h = st.h;
    let asphalt = lin(if st.highway {
        palette::ASPHALT_HIGHWAY
    } else {
        palette::ASPHALT
    });
    strip(m, f, t0, t1, -h, h, ROAD_Y, asphalt);
    if st.walk > 0.0 {
        for side in [1.0f32, -1.0] {
            walk_strip(m, &f.line_at(t0, t1, side * h), &f.line_at(t0, t1, side * st.outer()));
        }
    } else {
        // Highway: gravel shoulders, exposed slab edges and guard rails.
        let gravel = lin(palette::GRAVEL);
        for side in [1.0f32, -1.0] {
            strip(m, f, t0, t1, side * h, side * (h + 1.4), 0.025, gravel);
            side_face(m, f, t0, t1, side * h, 0.0, ROAD_Y, side, palette::KERB);
            guardrail(m, f, side, h + 0.9, t0, t1);
        }
    }
    if furniture && st.walk > 0.0 {
        street_lamps(m, s);
    } else if furniture && s.id.is_none() && s.len() > 60.0 {
        // The road out of town: a sign gantry just past the map edge.
        let (p, d) = f.pose(s.len() - 35.0);
        sign_gantry(m, p, d, h);
    }
    // Markings stop short of a junction's cut and of a dead end.
    let white = lin(palette::MARKING);
    let margin = |deg: usize, boxed: bool| {
        if boxed {
            0.3
        } else if deg <= 1 {
            1.2
        } else {
            0.0
        }
    };
    let (m0, m1) = (t0 + margin(s.deg_a, s.box_a), t1 - margin(s.deg_b, s.box_b));
    if m1 <= m0 {
        return;
    }
    if st.highway {
        let yellow = lin(palette::MARKING_YELLOW);
        for side in [1.0f32, -1.0] {
            let (a, b) = (side * 0.1, side * 0.22);
            strip(m, f, m0, m1, a.min(b), a.max(b), MARK_Y, yellow);
            let (a, b) = (side * (h - 0.45), side * (h - 0.3));
            strip(m, f, m0, m1, a.min(b), a.max(b), MARK_Y, white);
            dashes(m, f, side * h * 0.5, m0, m1, 4.0, 6.0, 0.15, white);
        }
    } else if st.median > 0.0 {
        // Avenue: raised planted median (cut into turn pockets), lane
        // lines, turn arrows.
        let (md0, md1) = s.median_span();
        let (q0, q1) = ((m0 + 0.4).max(md0), (m1 - 0.4).min(md1));
        if q1 - q0 > 1.0 {
            median_strip(m, f, st.median, q0, q1);
        }
        let lane_line = (lane_offset(s.kind, 0) + lane_offset(s.kind, 1)) * 0.5;
        for side in [1.0f32, -1.0] {
            dashes(m, f, side * lane_line, m0, m1, 3.0, 5.0, 0.14, white);
        }
        for (k, pocket) in s.pockets.iter().enumerate() {
            if let Some(&(lo, hi)) = pocket.as_ref() {
                turn_pocket_marks(m, s, k == 0, lo.max(m0), hi.min(m1), white);
            }
        }
        for (at_a, deg) in [(true, s.deg_a), (false, s.deg_b)] {
            if deg < 3 || m1 - m0 < 16.0 {
                continue;
            }
            // Arriving at this end: travel direction and the right of it.
            let t = if at_a { m0 + 7.0 } else { m1 - 7.0 };
            let (p, d) = f.pose(t);
            let travel = if at_a { -d } else { d };
            let right = perp(travel);
            // With a turn pocket the inner lane goes straight on only.
            let pocket = s.pockets[usize::from(at_a)].is_some();
            for (lane, turn) in [(0u8, 1.0f32), (1, if pocket { 0.0 } else { -1.0 })] {
                turn_arrow(m, p + right * lane_offset(s.kind, lane), travel, turn, white);
            }
        }
    } else {
        dashes(m, f, 0.0, m0, m1, 2.6, 2.4, 0.16, white);
    }
}

/// Stable hash of a segment's geometry (for its detail placement).
fn seg_seed(s: &SegLayout) -> u32 {
    let (a, b) = (s.a(), s.b());
    super::noise::hash2((a.x * 3.0 + b.y) as i32, (a.y * 3.0 + b.x) as i32, 0x5EED)
}

/// Wear and street furniture along a segment: asphalt patches, manhole
/// covers and drains (always), and on the sidewalks benches with bins,
/// hydrants, bollards and, along avenues, street trees (`furniture` 0..1
/// thins them out: graphics quality). Kept clear of the junction cuts and
/// of the lamps.
pub fn road_detail(m: &mut MeshBuilder, s: &SegLayout, furniture: f32) {
    let f = &s.frame;
    let (t0, t1) = (s.s_a + 1.5, s.len() - s.s_b - 1.5);
    if t1 - t0 < 4.0 {
        return;
    }
    let st = s.style;
    let h = st.h;
    let mut rng = super::noise::Stream::new(seg_seed(s));
    let base = if st.highway {
        palette::ASPHALT_HIGHWAY
    } else {
        palette::ASPHALT
    };
    // Patches of newer / older asphalt.
    let mut t = t0;
    while t < t1 - 3.0 {
        if rng.chance(0.3) {
            let l = rng.range(2.0, 6.0).min(t1 - t);
            let w = rng.range(1.0, 2.6).min(h * 0.9);
            let o = rng.range(-h + 0.3, h - 0.3 - w);
            let c = lin(shade(base, if rng.chance(0.5) { 0.86 } else { 1.12 }));
            strip(m, f, t, t + l, o, o + w, ROAD_Y + 0.004, c);
        }
        t += rng.range(8.0, 16.0);
    }
    // Manhole covers in the lanes, drains at the kerb.
    let lid = lin(Color::srgb_u32(0x55585C));
    let rim = lin(Color::srgb_u32(0x2E3033));
    let mut t = t0 + rng.range(4.0, 14.0);
    while t < t1 - 1.0 {
        let o = if rng.chance(0.5) { 1.4 } else { -1.4 } * if st.lanes > 1 { 2.4 } else { 1.0 };
        let c = f.at(t, o);
        let ring = |r: f32, y: f32| -> Vec<Vec3> {
            (0..8)
                .map(|k| lift(c + Vec2::from_angle(k as f32 / 8.0 * TAU) * r, y))
                .collect()
        };
        m.fan_up(lift(c, ROAD_Y + 0.008), &ring(0.42, ROAD_Y + 0.008), rim);
        m.fan_up(lift(c, ROAD_Y + 0.009), &ring(0.34, ROAD_Y + 0.009), lid);
        t += rng.range(22.0, 38.0);
    }
    if st.walk > 0.0 {
        let grate = lin(Color::srgb_u32(0x232528));
        let mut t = t0 + 6.0;
        while t < t1 - 1.0 {
            strip(m, f, t, t + 0.7, h - 0.5, h - 0.08, ROAD_Y + 0.006, grate);
            strip(m, f, t, t + 0.7, -h + 0.08, -h + 0.5, ROAD_Y + 0.006, grate);
            t += 24.0;
        }
    }
    if furniture <= 0.0 || st.walk <= 0.0 {
        return;
    }
    let lamps: Vec<Vec2> = lamp_spots(s).into_iter().map(|(b, _)| b).collect();
    let clear = |p: Vec2| lamps.iter().all(|l| l.distance(p) > 2.5) && clear_of(s, p, 2.6);
    let off = h + st.walk * 0.55;
    // Street trees along avenue sidewalks.
    if st.median > 0.0 && furniture > 0.6 {
        let mut t = t0 + 6.0;
        while t < t1 - 5.0 {
            for side in [1.0f32, -1.0] {
                let p = f.at(t, side * (h + st.walk * 0.5));
                if clear(p) {
                    let canopy = rng.pick(&palette::CANOPIES);
                    m.cuboid(
                        lift(p - Vec2::splat(0.5), WALK_Y + 0.005),
                        lift(p + Vec2::splat(0.5), WALK_Y + 0.02),
                        shade(palette::EARTH, 0.6),
                    );
                    super::props::round_tree(m, lift(p, WALK_Y), rng.range(0.55, 0.7), canopy, rng.range(0.0, 6.0));
                }
            }
            t += 14.0;
        }
    }
    let mut t = t0 + rng.range(6.0, 16.0);
    while t < t1 - 2.0 {
        let side = if rng.chance(0.5) { 1.0f32 } else { -1.0 };
        let (_, d) = f.pose(t);
        let n = perp(d);
        let p = f.at(t, side * off);
        let roll = rng.f();
        if rng.f() < furniture && clear(p) {
            // Facing the road: towards -side * n.
            let face = -n * side;
            let yaw = -face.y.atan2(face.x) + std::f32::consts::FRAC_PI_2;
            let y = WALK_Y;
            if roll < 0.4 {
                // Bench and a bin beside it.
                let wood = Color::srgb_u32(0x9C7650);
                let iron = Color::srgb_u32(0x33373B);
                let back = p - face * 0.25;
                m.box_yaw(lift(p, y + 0.42), Vec3::new(1.6, 0.06, 0.45), yaw, wood, wood);
                m.box_yaw(lift(back, y + 0.48), Vec3::new(1.6, 0.4, 0.06), yaw, wood, wood);
                for k in [-0.7f32, 0.7] {
                    let q = p + d * k;
                    m.box_yaw(lift(q, y), Vec3::new(0.06, 0.44, 0.45), yaw, iron, iron);
                }
                let b = p + d * 1.3;
                m.cylinder(lift(b, y), 0.22, 0.85, 6, iron, shade(iron, 1.3));
            } else if roll < 0.65 {
                // Fire hydrant.
                let red = Color::srgb_u32(0xC8322B);
                m.cylinder(lift(p, y), 0.13, 0.6, 6, red, shade(red, 1.1));
                m.cylinder(lift(p, y + 0.6), 0.09, 0.12, 6, red, shade(red, 1.2));
                m.box_yaw(lift(p, y + 0.38), Vec3::new(0.42, 0.08, 0.08), yaw, red, red);
            } else if roll < 0.8 {
                // Litter bin.
                let green = Color::srgb_u32(0x2F5D49);
                m.cylinder(lift(p, y), 0.24, 0.9, 6, green, shade(green, 1.3));
            } else {
                // Bollards along the kerb.
                let post = Color::srgb_u32(0x4A4E52);
                for k in [-1.2f32, 0.0, 1.2] {
                    let q = f.at(t + k, side * (h + 0.35));
                    m.cylinder(lift(q, y), 0.08, 0.85, 5, post, palette::GUARDRAIL);
                }
            }
        }
        t += rng.range(18.0, 34.0);
    }
}

/// Raised median between `t0` and `t1`: kerbs and a grass top with pointed
/// ends, following the frame.
fn median_strip(m: &mut MeshBuilder, f: &Frame, hw: f32, t0: f32, t1: f32) {
    let y = ROAD_Y + 0.13;
    let nose = (hw * 2.5).min((t1 - t0) * 0.25);
    let grass = lin(palette::MEDIAN_GRASS);
    // Width along the strip: pointed at both ends.
    let width = |t: f32| hw * ((t - t0) / nose).min((t1 - t) / nose).clamp(0.0, 1.0);
    let mut steps = f.steps(t0, t1);
    for extra in [t0 + nose, t1 - nose] {
        if extra > t0 && extra < t1 {
            steps.push(extra);
        }
    }
    steps.sort_by(f32::total_cmp);
    steps.dedup_by(|a, b| (*a - *b).abs() < 1e-3);
    let left: Vec<Vec2> = steps.iter().map(|&t| f.at(t, -width(t))).collect();
    let right: Vec<Vec2> = steps.iter().map(|&t| f.at(t, width(t))).collect();
    for k in 0..steps.len() - 1 {
        let q = [left[k], left[k + 1], right[k + 1], right[k]];
        m.quad_up(q.map(|p| lift(p, y)), grass);
        let (_, d) = f.pose((steps[k] + steps[k + 1]) * 0.5);
        vface(m, left[k], left[k + 1], ROAD_Y - 0.02, y, -perp(d), palette::KERB);
        vface(m, right[k], right[k + 1], ROAD_Y - 0.02, y, perp(d), palette::KERB);
    }
}

/// Painted turn arrow at `p` pointing along `travel`; `turn` +1 adds a
/// right-turn head, -1 a left-turn head, 0 none (always with straight on).
fn turn_arrow(m: &mut MeshBuilder, p: Vec2, travel: Vec2, turn: f32, c: [f32; 4]) {
    let right = perp(travel);
    let y = MARK_Y;
    let q = |along: f32, side: f32| lift(p + travel * along + right * side, y);
    // Shaft.
    m.quad_up([q(-2.2, -0.09), q(1.0, -0.09), q(1.0, 0.09), q(-2.2, 0.09)], c);
    // Straight head.
    m.tri_up(q(1.0, -0.38), q(1.0, 0.38), q(2.0, 0.0), c);
    if turn != 0.0 {
        turn_branch(m, p, travel, turn, c);
    }
}

/// The bent branch and head of a turn arrow (from the shaft at `p`).
fn turn_branch(m: &mut MeshBuilder, p: Vec2, travel: Vec2, s: f32, c: [f32; 4]) {
    let right = perp(travel);
    let q = |along: f32, side: f32| lift(p + travel * along + right * side, MARK_Y);
    m.quad_up([q(-0.7, 0.0), q(-0.35, 0.0), q(0.1, s * 0.62), q(-0.25, s * 0.62)], c);
    m.tri_up(q(-0.45, s * 0.52), q(0.35, s * 0.52), q(0.05, s * 1.05), c);
}

/// A left-turn-only arrow (a pocket): the shaft bends left, no straight
/// head.
fn left_arrow(m: &mut MeshBuilder, p: Vec2, travel: Vec2, c: [f32; 4]) {
    let right = perp(travel);
    let q = |along: f32, side: f32| lift(p + travel * along + right * side, MARK_Y);
    m.quad_up([q(-2.2, -0.09), q(-0.5, -0.09), q(-0.5, 0.09), q(-2.2, 0.09)], c);
    turn_branch(m, p, travel, -1.0, c);
}

/// Markings of a turn pocket between `lo` and `hi` (metres from `a`;
/// `forward`: it serves travel `a -> b` and its stop line is at `hi`): its
/// edge to the through lane (dashed where cars move in, solid towards the
/// line), its edge to the oncoming lanes (solid), a left arrow and a stop
/// bar.
fn turn_pocket_marks(m: &mut MeshBuilder, s: &SegLayout, forward: bool, lo: f32, hi: f32, c: [f32; 4]) {
    if hi - lo < 4.0 {
        return;
    }
    let f = &s.frame;
    let hw = s.style.median;
    // Offsets are to the right of `a -> b`; the pocket's own right (the
    // through lanes beside it) is `side`.
    let side = if forward { 1.0f32 } else { -1.0 };
    let (inner, outer) = (side * hw, -side * hw);
    let solid = |m: &mut MeshBuilder, t0: f32, t1: f32, o: f32| {
        strip(m, f, t0, t1, o - 0.07, o + 0.07, MARK_Y, c);
    };
    let len = hi - lo;
    // Where cars come in (the entry end) the line to the through lane is
    // dashed.
    let dashed = (len * 0.4).min(12.0);
    if forward {
        dashes(m, f, inner, lo, lo + dashed, 1.5, 1.5, 0.14, c);
        solid(m, lo + dashed, hi, inner);
    } else {
        solid(m, lo, hi - dashed, inner);
        dashes(m, f, inner, hi - dashed, hi, 1.5, 1.5, 0.14, c);
    }
    solid(m, lo, hi, outer);
    // Stop bar across the pocket and its arrow.
    let (stop, toward) = if forward { (hi, -1.0f32) } else { (lo, 1.0) };
    let bar = [stop, stop + toward * 0.4];
    strip(m, f, bar[0].min(bar[1]), bar[0].max(bar[1]), -hw + 0.12, hw - 0.12, MARK_Y + 0.002, c);
    if len > 10.0 {
        let t = stop + toward * 6.0;
        let (p, d) = f.pose(t);
        let travel = if forward { d } else { -d };
        let o = s.kind.lane_offset_m(s.kind.pocket_lane().unwrap_or(0));
        left_arrow(m, p + perp(travel) * o, travel, c);
    }
}

/// Street lamp positions along a segment: (pole base, lamp head) on
/// alternating sides, clear of the junction cuts. Empty for roads without
/// sidewalks.
pub fn lamp_spots(s: &SegLayout) -> Vec<(Vec2, Vec2)> {
    if s.style.walk <= 0.0 {
        return Vec::new();
    }
    if s.style.median > 0.0 {
        // On the median, unless turn pockets leave too little of it: then
        // on the sidewalks like a street.
        let spots = median_lamp_spots(s);
        if !spots.is_empty() {
            return spots;
        }
    }
    let f = &s.frame;
    let off = s.style.h + s.style.walk * 0.62;
    let (t0, t1) = s.span();
    let (a, b) = (t0 + 5.0, t1 - 5.0);
    if b <= a {
        return Vec::new();
    }
    let count = ((b - a) / 26.0).floor() as usize + 1;
    (0..count)
        .map(|k| {
            let t = if count == 1 {
                (a + b) * 0.5
            } else {
                a + (b - a) * k as f32 / (count - 1) as f32
            };
            let (p, _) = f.pose(t);
            // Alternate sides, phase from the position so neighbours agree.
            let side = if (p.dot(Vec2::ONE) / 26.0).round() as i32 % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            // Step aside from a driveway.
            let t = [0.0f32, 3.0, -3.0, 5.5, -5.5]
                .into_iter()
                .map(|k| (t + k).clamp(a, b))
                .find(|&t| clear_of(s, f.at(t, side * off), 2.0))
                .unwrap_or(t);
            let (p, d) = f.pose(t);
            let n = perp(d);
            let base = p + n * side * off;
            (base, base - n * side * 1.1)
        })
        .collect()
}

/// Avenue lamps: double-armed poles on the median, one head over each
/// carriageway.
fn median_lamp_spots(s: &SegLayout) -> Vec<(Vec2, Vec2)> {
    let f = &s.frame;
    // On the planted median only (not in a turn pocket).
    let (m0, m1) = s.median_span();
    let (t0, t1) = (m0 + 6.0, m1 - 6.0);
    if t1 <= t0 {
        return Vec::new();
    }
    let count = ((t1 - t0) / 30.0).floor() as usize + 1;
    let mut out = Vec::new();
    for k in 0..count {
        let t = if count == 1 {
            (t0 + t1) * 0.5
        } else {
            t0 + (t1 - t0) * k as f32 / (count - 1) as f32
        };
        let (base, d) = f.pose(t);
        let n = perp(d);
        out.push((base, base + n * 2.6));
        out.push((base, base - n * 2.6));
    }
    out
}

/// Street lamps: pole, arm over the road and a glowing head.
fn street_lamps(m: &mut MeshBuilder, s: &SegLayout) {
    let pole = palette::LAMP;
    let on_median = s.style.median > 0.0 && !median_lamp_spots(s).is_empty();
    let ground = if on_median { ROAD_Y + 0.13 } else { WALK_Y };
    let mut last: Option<Vec2> = None;
    for (base, tip) in lamp_spots(s) {
        if last != Some(base) {
            m.box_yaw(
                lift(base, ground),
                Vec3::new(0.14, WALK_Y + 4.6 - ground, 0.14),
                0.0,
                pole,
                pole,
            );
        }
        last = Some(base);
        let mid = (base + tip) * 0.5;
        let d = tip - base;
        let yaw = -d.y.atan2(d.x);
        m.box_yaw(
            lift(mid, WALK_Y + 4.5),
            Vec3::new(d.length() + 0.1, 0.1, 0.1),
            yaw,
            pole,
            pole,
        );
        m.box_yaw(lift(tip, WALK_Y + 4.38), Vec3::new(0.62, 0.14, 0.32), yaw, pole, pole);
        m.glow(Glow::Lamp, 0.0, |m| {
            m.box_yaw(
                lift(tip, WALK_Y + 4.26),
                Vec3::new(0.5, 0.12, 0.24),
                yaw,
                palette::LAMP_GLASS,
                palette::LAMP_GLASS,
            )
        });
    }
}

/// Soft pool of lamp light on the ground around `p` (for an additive
/// material): bright in the middle, fading to nothing at `r`.
pub fn light_pool(m: &mut MeshBuilder, p: Vec2, r: f32, y: f32, color: Color) {
    let c = lin(color);
    let with_a = |a: f32| [c[0] * a, c[1] * a, c[2] * a, a];
    let rings = [
        (0.0f32, 1.0f32),
        (0.18, 0.9),
        (0.36, 0.68),
        (0.54, 0.42),
        (0.72, 0.2),
        (0.86, 0.07),
        (1.0, 0.0),
    ];
    let seg = 16;
    let at = |k: usize, i: usize| {
        let ang = i as f32 / seg as f32 * TAU;
        lift(p + Vec2::new(ang.cos(), ang.sin()) * r * rings[k].0, y)
    };
    for k in 0..rings.len() - 1 {
        for i in 0..seg {
            let q = [at(k, i), at(k, i + 1), at(k + 1, i + 1), at(k + 1, i)];
            let (ci, co) = (with_a(rings[k].1), with_a(rings[k + 1].1));
            let cols = [ci, ci, co, co];
            // Upward facing whatever the winding.
            if (q[1] - q[0]).cross(q[2] - q[0]).y >= 0.0 {
                m.quad_c(q, cols);
            } else {
                m.quad_c([q[3], q[2], q[1], q[0]], [co, co, ci, ci]);
            }
        }
    }
}

/// Overhead sign gantry across a highway at `p` (direction of travel `dir`).
fn sign_gantry(m: &mut MeshBuilder, p: Vec2, dir: Vec2, h: f32) {
    let n = perp(dir);
    let steel = palette::GUARDRAIL;
    let w = h + 1.6;
    let yaw = -n.y.atan2(n.x);
    for side in [1.0f32, -1.0] {
        let c = p + n * side * w;
        m.box_yaw(lift(c, 0.0), Vec3::new(0.35, 6.6, 0.35), 0.0, steel, steel);
    }
    m.box_yaw(lift(p, 6.0), Vec3::new(2.0 * w + 0.4, 0.45, 0.4), yaw, steel, steel);
    // A green board over each carriageway, facing the traffic.
    for side in [1.0f32, -1.0] {
        let c = p + n * side * h * 0.5 + dir * side * 0.3;
        m.box_yaw(
            lift(c, 5.0),
            Vec3::new(h * 0.8, 1.8, 0.12),
            yaw,
            palette::SIGN_GREEN,
            palette::SIGN_GREEN,
        );
        // White text bars.
        let face = c + dir * side * 0.07 * -1.0;
        for (k, len) in [(0, 0.55f32), (1, 0.4)] {
            m.box_yaw(
                lift(face, 6.15 - k as f32 * 0.55),
                Vec3::new(h * 0.8 * len, 0.16, 0.02),
                yaw,
                palette::MARKING,
                palette::MARKING,
            );
        }
    }
}

/// Low guard rail with posts along one side (`side` +1 right, -1 left) of
/// a frame, `off` metres from the centre line.
fn guardrail(m: &mut MeshBuilder, f: &Frame, side: f32, off: f32, t0: f32, t1: f32) {
    let rail = palette::GUARDRAIL;
    let post = shade(palette::GUARDRAIL, 0.7);
    for th in [0.06f32, -0.06] {
        let o = side * (off + th);
        side_face(
            m,
            f,
            t0,
            t1,
            o,
            0.45,
            0.78,
            side * th.signum(),
            if th > 0.0 { rail } else { shade(rail, 0.9) },
        );
    }
    let (a, b) = (side * (off - 0.06), side * (off + 0.06));
    strip(m, f, t0, t1, a.min(b), a.max(b), 0.78, lin(rail));
    // Posts only where they can be seen.
    let mut t = t0 + 1.0;
    while t < t1 {
        let c = f.at(t, side * (off + 0.12));
        if c.x > -1500.0 {
            m.box_yaw(lift(c, 0.0), Vec3::new(0.12, 0.7, 0.12), 0.0, post, post);
        }
        t += 4.0;
    }
}

/// Bucket key for spatial chunks of `size` metres.
pub fn chunk_of(p: Vec2, size: f32) -> IVec2 {
    IVec2::new((p.x / size).floor() as i32, (p.y / size).floor() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::{Cell, Command};

    fn world_with(cmds: &[Command]) -> World {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        for c in cmds {
            w.apply(c.clone()).expect("command");
        }
        w
    }

    fn road(a: (i32, i32), b: (i32, i32)) -> Command {
        Command::BuildRoad {
            from: Cell::new(a.0, a.1),
            to: Cell::new(b.0, b.1),
            kind: RoadKind::Street,
        }
    }

    fn curve(p: [[f32; 2]; 4], kind: RoadKind) -> Command {
        Command::BuildCurve {
            curve: Curve::new(p[0], p[1], p[2], p[3]),
            kind,
        }
    }

    fn mesh_all(l: &RoadLayout) -> MeshBuilder {
        let mut m = MeshBuilder::new();
        for n in &l.nodes {
            mesh_node(&mut m, n);
        }
        for s in &l.segs {
            mesh_segment(&mut m, s);
            road_detail(&mut m, s, 1.0);
        }
        m
    }

    #[test]
    fn frames_follow_the_curve_and_stay_cheap_when_straight() {
        let straight = Frame::new(&Curve::straight([0.0, 0.0], [100.0, 0.0]));
        assert_eq!(straight.pts.len(), 2);
        let c = Curve::new([0.0, 0.0], [60.0, 0.0], [100.0, 40.0], [100.0, 100.0]);
        let f = Frame::new(&c);
        assert!(f.pts.len() > 8, "{}", f.pts.len());
        for k in 0..=20 {
            let t = f.len() * k as f32 / 20.0;
            let want = Vec2::from(c.point_at(t, 0.0));
            assert!(f.at(t, 0.0).distance(want) < 0.1, "{t}");
            let want = Vec2::from(c.point_at(t, 3.0));
            assert!(f.at(t, 3.0).distance(want) < 0.15, "{t} offset");
        }
    }

    #[test]
    fn segments_end_at_the_sims_cuts() {
        let w = world_with(&[road((30, 120), (30, 140)), road((20, 130), (40, 130))]);
        let l = layout(&w);
        let node = l
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Junction)
            .expect("junction node");
        assert_eq!(node.arms.len(), 4);
        for a in &node.arms {
            let s = l.segs.iter().find(|s| s.id == Some(a.seg)).expect("arm segment");
            let cut_end = if s.a().distance(node.pos) < 0.01 { s.s_a } else { s.s_b };
            assert!((cut_end - a.setback).abs() < 1e-4);
            assert!(a.setback > 3.0, "{}", a.setback);
        }
    }

    #[test]
    fn curved_roads_and_odd_angle_junctions_mesh_without_nan() {
        let w = world_with(&[
            road((11, 128), (40, 128)),
            road((30, 120), (30, 140)),
            curve(
                [[324.0, 1028.0], [400.0, 1028.0], [430.0, 980.0], [470.0, 900.0]],
                RoadKind::Avenue,
            ),
            curve(
                [[330.0, 1000.0], [360.0, 1060.0], [380.0, 1080.0], [420.0, 1100.0]],
                RoadKind::Street,
            ),
        ]);
        let l = layout(&w);
        assert!(
            l.nodes.iter().any(|n| n.kind == NodeKind::Entry),
            "highway entry is marked"
        );
        assert!(l.segs.iter().any(|s| !s.frame.is_straight()));
        let m = mesh_all(&l);
        assert!(m.triangle_count() > 100);
        assert!(m.pos.iter().flatten().all(|v| v.is_finite()));
        // Every upward face of the road surface lies on a known level.
        for (p, nrm) in m.pos.iter().zip(&m.nrm) {
            if nrm[1] > 0.99 && p[1] < 0.5 {
                let y = p[1];
                assert!(
                    [
                        ROAD_Y,
                        WALK_Y,
                        MARK_Y,
                        MARK_Y + 0.002,
                        ROAD_Y + 0.13,
                        0.025,
                        ROAD_Y + 0.004,
                        ROAD_Y + 0.006,
                        ROAD_Y + 0.008,
                        ROAD_Y + 0.009,
                        WALK_Y + 0.005,
                        WALK_Y + 0.02,
                    ]
                    .iter()
                    .any(|l| (y - l).abs() < 1e-4),
                    "unexpected level {y}"
                );
            }
        }
    }
}
