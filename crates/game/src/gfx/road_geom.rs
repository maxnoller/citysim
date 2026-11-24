//! Road geometry from the road graph alone (never the grid).
//!
//! Every node gets a junction surface computed from the directions of the
//! segments that meet there: for each pair of neighbouring arms (sorted by
//! angle) the inner corner gets a rounded kerb (a fillet tangent to both
//! kerb lines), the outer side of a bend gets an arc around the node, and a
//! dead end gets a round cap. Each arm is pulled back from the node by a
//! "setback" so the straight segment pieces start exactly where the junction
//! surface ends: no gaps, no overlaps. Markings stop before junctions, which
//! get crosswalks and stop lines.
//!
//! Everything here is pure (no ECS) so it is unit-tested.

use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use citysim_sim::{NodeId, RoadKind, RoadNetwork, SegmentId};

use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, shade};

/// Top of the asphalt.
pub const ROAD_Y: f32 = 0.06;
/// Top of the sidewalk.
pub const WALK_Y: f32 = 0.19;
/// Painted markings, just above the asphalt.
pub const MARK_Y: f32 = ROAD_Y + 0.016;
/// Width of the darker kerb stone along the sidewalk edge.
const KERB_W: f32 = 0.22;
/// Preferred kerb corner radius at junctions.
const FILLET_R: f32 = 3.2;
/// Distance from the junction edge to the end of the centre line.
const JUNCTION_MARGIN: f32 = 3.6;

/// Cross-section of a road kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Half-width of the carriageway (kerb to centre).
    pub h: f32,
    /// Sidewalk width on each side (0 = none).
    pub walk: f32,
    pub highway: bool,
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
            h: 2.75,
            walk: 1.25,
            highway: false,
        },
        RoadKind::Highway => Style {
            h: 5.0,
            walk: 0.0,
            highway: true,
        },
    }
}

/// Lateral offset of the lane a vehicle drives in (to the right of travel).
pub fn lane_offset(kind: RoadKind, seed: u32) -> f32 {
    match kind {
        RoadKind::Street => 1.35,
        RoadKind::Highway => {
            if seed % 3 == 0 {
                3.6
            } else {
                1.3
            }
        }
    }
}

fn perp(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}

fn lift(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

/// One segment as seen from a node.
#[derive(Debug, Clone, PartialEq)]
pub struct Arm {
    pub seg: SegmentId,
    /// Unit direction from the node along the segment.
    pub dir: Vec2,
    pub style: Style,
    pub len: f32,
    /// Where the straight segment geometry starts, metres from the node.
    pub setback: f32,
}

/// Rounded inner corner between two arms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fillet {
    /// Centre relative to the node.
    pub center: Vec2,
    pub r: f32,
    /// Tangent point parameters along each arm.
    pub ti: f32,
    pub tj: f32,
}

/// The gap between arm `i` and the next arm counter-clockwise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wedge {
    pub i: usize,
    pub j: usize,
    /// Angle from arm `i` to arm `j`, in `(0, 2 pi]`.
    pub alpha: f32,
    pub fillet: Option<Fillet>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeLayout {
    pub id: NodeId,
    pub pos: Vec2,
    pub arms: Vec<Arm>,
    pub wedges: Vec<Wedge>,
    /// The highway entry: drawn as a road running off the map instead.
    pub entry: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegLayout {
    /// `None` for the decorative highway beyond the map edge.
    pub id: Option<SegmentId>,
    pub kind: RoadKind,
    pub a: Vec2,
    pub b: Vec2,
    pub style: Style,
    pub s_a: f32,
    pub s_b: f32,
    /// Number of segments at each end node.
    pub deg_a: usize,
    pub deg_b: usize,
    /// Straight continuation at a degree-2 node (markings run through).
    pub straight_a: bool,
    pub straight_b: bool,
}

impl SegLayout {
    pub fn len(&self) -> f32 {
        (self.b - self.a).length()
    }

    pub fn dir(&self) -> Vec2 {
        (self.b - self.a).normalize_or(Vec2::X)
    }

    pub fn mid(&self) -> Vec2 {
        (self.a + self.b) * 0.5
    }
}

#[derive(Debug, Clone, Default)]
pub struct RoadLayout {
    pub nodes: Vec<NodeLayout>,
    pub segs: Vec<SegLayout>,
}

/// Fillet between arm `i`'s left kerb and arm `j`'s right kerb, with the
/// radius reduced so the tangent points stay within `lim_i` / `lim_j`.
fn fillet(ai: &Arm, aj: &Arm, alpha: f32, lim_i: f32, lim_j: f32) -> Fillet {
    let (s, c) = alpha.sin_cos();
    let (hi, hj) = (ai.style.h, aj.style.h);
    // Tangent parameters are linear in R with slope cot(alpha / 2).
    let k = (1.0 + c) / s;
    let x0 = (hi * c + hj) / s;
    let tj0 = x0 * c + hi * s;
    let r_max = ((lim_i - x0) / k).min((lim_j - tj0) / k);
    let r = FILLET_R.min(r_max).max(0.0);
    let y = hi + r;
    let x = (y * c + hj + r) / s;
    let center = ai.dir * x + perp(ai.dir) * y;
    Fillet {
        center,
        r,
        ti: x,
        tj: x * c + y * s,
    }
}

/// Lay out junctions and segment pieces for the whole network.
pub fn layout(roads: &RoadNetwork) -> RoadLayout {
    let mut out = RoadLayout::default();
    let mut setbacks: HashMap<(SegmentId, NodeId), f32> = HashMap::new();
    let mut straight: HashMap<(SegmentId, NodeId), bool> = HashMap::new();
    for (nid, node) in roads.nodes.iter() {
        let pos = Vec2::from(node.pos);
        let mut arms: Vec<Arm> = node
            .segments
            .iter()
            .filter_map(|&sid| {
                let seg = roads.segments.get(sid)?;
                let other = roads.nodes.get(seg.other(nid))?;
                let d = Vec2::from(other.pos) - pos;
                let len = d.length();
                (len > 1e-3).then(|| Arm {
                    seg: sid,
                    dir: d / len,
                    style: style(seg.kind),
                    len,
                    setback: 0.0,
                })
            })
            .collect();
        if arms.is_empty() {
            continue;
        }
        arms.sort_by(|a, b| a.dir.y.atan2(a.dir.x).total_cmp(&b.dir.y.atan2(b.dir.x)));
        let n = arms.len();
        let mut wedges = Vec::with_capacity(n);
        for i in 0..n {
            let j = (i + 1) % n;
            let (ai, aj) = (&arms[i], &arms[j]);
            let mut alpha = aj.dir.y.atan2(aj.dir.x) - ai.dir.y.atan2(ai.dir.x);
            if alpha <= 1e-4 {
                alpha += TAU;
            }
            let fillet = (alpha < PI - 0.02).then(|| fillet(ai, aj, alpha, ai.len * 0.48, aj.len * 0.48));
            wedges.push(Wedge { i, j, alpha, fillet });
        }
        for w in &wedges {
            if let Some(f) = w.fillet {
                arms[w.i].setback = arms[w.i].setback.max(f.ti);
                arms[w.j].setback = arms[w.j].setback.max(f.tj);
            }
        }
        for a in &arms {
            setbacks.insert((a.seg, nid), a.setback);
            let is_straight = n == 2 && wedges.iter().all(|w| (w.alpha - PI).abs() < 0.02);
            straight.insert((a.seg, nid), is_straight);
        }
        out.nodes.push(NodeLayout {
            id: nid,
            pos,
            entry: roads.entry == Some(nid) && n == 1,
            arms,
            wedges,
        });
    }
    let degree = |n: NodeId| roads.nodes.get(n).map_or(0, |n| n.segments.len());
    for (sid, seg) in roads.segments.iter() {
        let (Some(na), Some(nb)) = (roads.nodes.get(seg.a), roads.nodes.get(seg.b)) else {
            continue;
        };
        out.segs.push(SegLayout {
            id: Some(sid),
            kind: seg.kind,
            a: Vec2::from(na.pos),
            b: Vec2::from(nb.pos),
            style: style(seg.kind),
            s_a: setbacks.get(&(sid, seg.a)).copied().unwrap_or(0.0),
            s_b: setbacks.get(&(sid, seg.b)).copied().unwrap_or(0.0),
            deg_a: degree(seg.a),
            deg_b: degree(seg.b),
            straight_a: straight.get(&(sid, seg.a)).copied().unwrap_or(false),
            straight_b: straight.get(&(sid, seg.b)).copied().unwrap_or(false),
        });
    }
    // The highway continues off the map into the fog.
    for node in &out.nodes {
        if node.entry {
            let arm = &node.arms[0];
            out.segs.push(SegLayout {
                id: None,
                kind: RoadKind::Highway,
                a: node.pos - arm.dir * super::scene::OUTSIDE,
                b: node.pos,
                style: arm.style,
                s_a: 0.0,
                s_b: 0.0,
                deg_a: 2,
                deg_b: 2,
                straight_a: true,
                straight_b: true,
            });
        }
    }
    out
}

// ---------------------------------------------------------------- hashing

/// Hasher input for floats.
fn hf<H: Hasher>(h: &mut H, v: f32) {
    v.to_bits().hash(h);
}

pub fn hash_node<H: Hasher>(n: &NodeLayout, h: &mut H) {
    hf(h, n.pos.x);
    hf(h, n.pos.y);
    n.entry.hash(h);
    for a in &n.arms {
        hf(h, a.dir.x);
        hf(h, a.dir.y);
        hf(h, a.setback);
        a.style.highway.hash(h);
    }
}

pub fn hash_seg<H: Hasher>(s: &SegLayout, h: &mut H) {
    for v in [s.a.x, s.a.y, s.b.x, s.b.y, s.s_a, s.s_b] {
        hf(h, v);
    }
    (s.deg_a, s.deg_b, s.straight_a, s.straight_b, s.style.highway).hash(h);
}

// ---------------------------------------------------------------- meshing

/// Vertical face along `a -> b` (2D), from `y0` to `y1`, facing `toward`.
fn vface(m: &mut MeshBuilder, a: Vec2, b: Vec2, y0: f32, y1: f32, toward: Vec2, color: Color) {
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

/// A sidewalk following paired kerb / outer points. `road_side` points from
/// the walk towards the road at each pair.
fn walk_strip(m: &mut MeshBuilder, kerb: &[Vec2], outer: &[Vec2]) {
    let walk = lin(palette::SIDEWALK);
    let kerb_col = lin(palette::KERB);
    for k in 0..kerb.len().saturating_sub(1) {
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
    let steps = ((sweep.abs() / (PI / 12.0)).ceil() as usize).max(1);
    (0..=steps)
        .map(|k| {
            let a = a0 + sweep * k as f32 / steps as f32;
            center + Vec2::from_angle(a) * r
        })
        .collect()
}

fn angle(v: Vec2) -> f32 {
    v.y.atan2(v.x)
}

/// Junction surface, kerb corners and end caps of one node.
pub fn mesh_node(m: &mut MeshBuilder, node: &NodeLayout) {
    if node.entry {
        return;
    }
    let p = node.pos;
    let asphalt = lin(if node.arms.iter().any(|a| a.style.highway) {
        palette::ASPHALT_HIGHWAY
    } else {
        palette::ASPHALT
    });
    let mut ring: Vec<Vec2> = Vec::new();
    for w in &node.wedges {
        let (ai, aj) = (&node.arms[w.i], &node.arms[w.j]);
        let (ni, nj) = (perp(ai.dir), perp(aj.dir));
        let (hi, hj) = (ai.style.h, aj.style.h);
        ring.push(p + ai.dir * ai.setback - ni * hi);
        ring.push(p + ai.dir * ai.setback + ni * hi);
        let both_walk = ai.style.walk > 0.0 && aj.style.walk > 0.0;
        match w.fillet {
            Some(f) => {
                let o = p + f.center;
                let pts = arc(o, f.r, angle(-ni), -(PI - w.alpha));
                ring.extend(&pts);
                // Sidewalks: bands from the tangent points out to the
                // setbacks, and the rounded corner between them.
                if ai.style.walk > 0.0 {
                    let (k0, k1) = (p + ai.dir * f.ti + ni * hi, p + ai.dir * ai.setback + ni * hi);
                    let off = ni * ai.style.walk;
                    walk_strip(m, &[k0, k1], &[k0 + off, k1 + off]);
                    if !both_walk {
                        walk_end(m, k0, k0 + off, -ai.dir);
                    }
                }
                if aj.style.walk > 0.0 {
                    let (k0, k1) = (p + aj.dir * aj.setback - nj * hj, p + aj.dir * f.tj - nj * hj);
                    let off = -nj * aj.style.walk;
                    walk_strip(m, &[k0, k1], &[k0 + off, k1 + off]);
                    if !both_walk {
                        walk_end(m, k1, k1 + off, -aj.dir);
                    }
                }
                if both_walk {
                    let width = ai.style.walk.min(aj.style.walk);
                    let outer: Vec<Vec2> = pts
                        .iter()
                        .map(|&q| o + (q - o).normalize_or_zero() * (f.r - width).max(0.0))
                        .collect();
                    walk_strip(m, &pts, &outer);
                }
            }
            None => {
                let sweep = w.alpha - PI;
                let pts: Vec<Vec2> = arc(Vec2::ZERO, 1.0, angle(ni), sweep)
                    .into_iter()
                    .enumerate()
                    .map(|(k, u)| {
                        let t = k as f32 / (((sweep / (PI / 12.0)).ceil() as usize).max(1)) as f32;
                        p + u * (hi + (hj - hi) * t)
                    })
                    .collect();
                ring.extend(&pts);
                if ai.style.walk > 0.0 {
                    let (k0, k1) = (p + ni * hi, p + ai.dir * ai.setback + ni * hi);
                    let off = ni * ai.style.walk;
                    walk_strip(m, &[k0, k1], &[k0 + off, k1 + off]);
                    if !both_walk {
                        walk_end(m, k0, k0 + off, -ai.dir);
                    }
                }
                if aj.style.walk > 0.0 {
                    let (k0, k1) = (p + aj.dir * aj.setback - nj * hj, p - nj * hj);
                    let off = -nj * aj.style.walk;
                    walk_strip(m, &[k0, k1], &[k0 + off, k1 + off]);
                    if !both_walk {
                        walk_end(m, k1, k1 + off, -aj.dir);
                    }
                }
                if both_walk && sweep > 1e-3 {
                    let width = ai.style.walk.min(aj.style.walk);
                    let outer: Vec<Vec2> = pts.iter().map(|&q| q + (q - p).normalize_or_zero() * width).collect();
                    walk_strip(m, &pts, &outer);
                }
                if !node.arms.iter().any(|a| a.style.walk > 0.0) && sweep > 1e-3 {
                    // Highway end: exposed slab edge.
                    for k in 0..pts.len() - 1 {
                        vface(m, pts[k], pts[k + 1], 0.0, ROAD_Y, pts[k] - p, palette::KERB);
                    }
                }
            }
        }
    }
    let center = lift(p, ROAD_Y);
    let ring3: Vec<Vec3> = ring.iter().map(|&q| lift(q, ROAD_Y)).collect();
    m.fan_up(center, &ring3, asphalt);
}

/// Dashed line along `dir` at lateral offset `off`, between params.
#[allow(clippy::too_many_arguments)]
fn dashes(
    m: &mut MeshBuilder,
    a: Vec2,
    dir: Vec2,
    off: f32,
    t0: f32,
    t1: f32,
    dash: f32,
    gap: f32,
    width: f32,
    color: [f32; 4],
) {
    let span = t1 - t0;
    if span < dash * 0.5 {
        return;
    }
    let n = perp(dir);
    // Centre the pattern within the span.
    let count = ((span + gap) / (dash + gap)).floor().max(1.0);
    let used = count * dash + (count - 1.0) * gap;
    let mut t = t0 + (span - used) * 0.5;
    for _ in 0..count as usize {
        let (s, e) = (t.max(t0), (t + dash).min(t1));
        rect(m, a, dir, n, s, e, off - width * 0.5, off + width * 0.5, MARK_Y, color);
        t += dash + gap;
    }
}

/// Upward rectangle in a segment's frame: along `t0..t1`, across `o0..o1`.
#[allow(clippy::too_many_arguments)]
fn rect(m: &mut MeshBuilder, a: Vec2, dir: Vec2, n: Vec2, t0: f32, t1: f32, o0: f32, o1: f32, y: f32, c: [f32; 4]) {
    let q = [
        a + dir * t0 + n * o0,
        a + dir * t1 + n * o0,
        a + dir * t1 + n * o1,
        a + dir * t0 + n * o1,
    ];
    m.quad_up(q.map(|p| lift(p, y)), c);
}

/// Straight part of a segment between its two junction setbacks.
pub fn mesh_segment(m: &mut MeshBuilder, s: &SegLayout) {
    let len = s.len();
    let dir = s.dir();
    let n = perp(dir);
    let (t0, t1) = (s.s_a, len - s.s_b);
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
    rect(m, s.a, dir, n, t0, t1, -h, h, ROAD_Y, asphalt);
    let at = |t: f32, o: f32| s.a + dir * t + n * o;
    if st.walk > 0.0 {
        for side in [1.0f32, -1.0] {
            let k = [at(t0, side * h), at(t1, side * h)];
            let o = [at(t0, side * st.outer()), at(t1, side * st.outer())];
            walk_strip(m, &k, &o);
        }
    } else {
        // Highway: gravel shoulders, exposed slab edges and guard rails.
        let gravel = lin(palette::GRAVEL);
        for side in [1.0f32, -1.0] {
            rect(m, s.a, dir, n, t0, t1, side * h, side * (h + 1.4), 0.025, gravel);
            vface(
                m,
                at(t0, side * h),
                at(t1, side * h),
                0.0,
                ROAD_Y,
                n * side,
                palette::KERB,
            );
            guardrail(m, s.a, dir, n * side, h + 0.9, t0, t1);
        }
    }
    // Markings.
    let white = lin(palette::MARKING);
    let margin = |deg: usize, straight: bool| match deg {
        0 | 1 => 1.2,
        2 if straight => 0.0,
        2 => 1.0,
        _ => JUNCTION_MARGIN,
    };
    let (m0, m1) = (t0 + margin(s.deg_a, s.straight_a), t1 - margin(s.deg_b, s.straight_b));
    if st.highway {
        let yellow = lin(palette::MARKING_YELLOW);
        for side in [1.0f32, -1.0] {
            rect(m, s.a, dir, n, m0, m1, side * 0.1, side * 0.22, MARK_Y, yellow);
            rect(
                m,
                s.a,
                dir,
                n,
                m0,
                m1,
                side * (h - 0.45),
                side * (h - 0.3),
                MARK_Y,
                white,
            );
            dashes(m, s.a, dir, side * h * 0.5, m0, m1, 4.0, 6.0, 0.15, white);
        }
    } else {
        dashes(m, s.a, dir, 0.0, m0, m1, 2.6, 2.4, 0.16, white);
    }
    // Crosswalks and stop lines where a junction (3+ arms) ends the segment.
    if !st.highway {
        for (at_a, deg) in [(true, s.deg_a), (false, s.deg_b)] {
            if deg < 3 || t1 - t0 < 2.0 * JUNCTION_MARGIN + 1.0 {
                continue;
            }
            // Work in the frame of the end: `e` is the junction edge param and
            // `inward` points away from the junction.
            let (e, inward) = if at_a { (t0, 1.0) } else { (t1, -1.0) };
            let along = |d: f32| e + inward * d;
            let zebra_w = 0.5;
            let mut o = -h + 0.45;
            while o + zebra_w <= h - 0.45 + 1e-3 {
                let (ta, tb) = (along(0.5), along(2.6));
                rect(m, s.a, dir, n, ta.min(tb), ta.max(tb), o, o + zebra_w, MARK_Y, white);
                o += zebra_w * 2.0;
            }
            // Stop line across the lane arriving at this junction (right of
            // travel, i.e. -n when arriving at b, +n at a).
            let (ta, tb) = (along(3.0), along(3.35));
            let side = if at_a { 1.0 } else { -1.0 };
            let (oa, ob) = (side * 0.15, side * (h - 0.25));
            rect(
                m,
                s.a,
                dir,
                n,
                ta.min(tb),
                ta.max(tb),
                oa.min(ob),
                oa.max(ob),
                MARK_Y,
                white,
            );
        }
    }
}

/// Low guard rail with posts along a segment side.
fn guardrail(m: &mut MeshBuilder, a: Vec2, dir: Vec2, n: Vec2, off: f32, t0: f32, t1: f32) {
    let rail = palette::GUARDRAIL;
    let post = shade(palette::GUARDRAIL, 0.7);
    let p0 = a + dir * t0 + n * off;
    let p1 = a + dir * t1 + n * off;
    let th = n * 0.06;
    // Rail band (both faces) and top.
    vface(m, p0 + th, p1 + th, 0.45, 0.78, n, rail);
    vface(m, p0 - th, p1 - th, 0.45, 0.78, -n, shade(rail, 0.9));
    m.quad_up([p0 + th, p1 + th, p1 - th, p0 - th].map(|p| lift(p, 0.78)), lin(rail));
    // Posts only where they can be seen.
    let mut t = t0 + 1.0;
    let near_end = |t: f32| (a + dir * t).x > -1500.0;
    while t < t1 {
        if near_end(t) {
            let c = a + dir * t + n * (off + 0.12);
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
    use citysim_sim::{Cell, Command, World};

    fn world_with(cmds: &[Command]) -> World {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        for c in cmds {
            w.apply(*c).expect("command");
        }
        w
    }

    fn road(a: (i32, i32), b: (i32, i32)) -> Command {
        Command::BuildRoad {
            from: Cell::new(a.0, a.1),
            to: Cell::new(b.0, b.1),
        }
    }

    #[test]
    fn crossing_has_symmetric_setbacks_and_fillets() {
        let w = world_with(&[road((30, 120), (30, 140)), road((20, 130), (40, 130))]);
        let l = layout(&w.roads);
        let centre = Vec2::from(Cell::new(30, 130).center());
        let node = l
            .nodes
            .iter()
            .find(|n| n.pos.distance(centre) < 0.01)
            .expect("junction node");
        assert_eq!(node.arms.len(), 4);
        for a in &node.arms {
            assert!((a.setback - (2.75 + FILLET_R)).abs() < 1e-3, "setback {}", a.setback);
        }
        for w in &node.wedges {
            let f = w.fillet.expect("90 degree corners get fillets");
            assert!((f.r - FILLET_R).abs() < 1e-4);
        }
    }

    #[test]
    fn setbacks_never_exceed_half_a_short_segment() {
        // A one-cell segment between two junctions, plus 45 degree joins.
        let w = world_with(&[
            road((20, 130), (40, 130)),
            road((20, 131), (40, 131)),
            road((30, 120), (30, 140)),
            road((25, 125), (35, 135)),
        ]);
        let l = layout(&w.roads);
        for s in &l.segs {
            if s.id.is_none() {
                continue;
            }
            assert!(s.s_a + s.s_b <= s.len() * 0.96 + 1e-3, "{s:?}");
        }
    }

    #[test]
    fn every_node_and_segment_meshes_without_nan() {
        let w = world_with(&[
            road((11, 128), (40, 128)),
            road((30, 120), (30, 140)),
            road((20, 110), (38, 128)),
            road((40, 128), (40, 100)),
        ]);
        let l = layout(&w.roads);
        assert!(l.nodes.iter().any(|n| n.entry), "highway entry is marked");
        let mut m = MeshBuilder::new();
        for n in &l.nodes {
            mesh_node(&mut m, n);
        }
        for s in &l.segs {
            mesh_segment(&mut m, s);
        }
        assert!(m.triangle_count() > 100);
        assert!(m.pos.iter().flatten().all(|v| v.is_finite()));
        // Every upward face of the road surface lies on a known level.
        for (p, nrm) in m.pos.iter().zip(&m.nrm) {
            if nrm[1] > 0.99 {
                let y = p[1];
                assert!(
                    [ROAD_Y, WALK_Y, MARK_Y, 0.025, 0.78, 0.7]
                        .iter()
                        .any(|l| (y - l).abs() < 1e-4),
                    "unexpected level {y}"
                );
            }
        }
    }
}
