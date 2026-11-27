//! Snapping for the free-form road tools: pure maths on the world's roads
//! and zoning lattices (no Bevy state, no screen).
//!
//! Every rule produces candidates near the cursor; the strongest one within
//! reach wins (ties: the nearest). Rules, strongest first:
//! * ends and junctions of existing roads (`Node`);
//! * a guide line meeting a road, or two guide lines meeting (the point
//!   satisfies both: `matched` names both);
//! * onto a road to form a T (`Road`);
//! * parallel to a nearby road at the set distance (`Parallel`);
//! * the extension of a dead end (`Extension`);
//! * from the previous point: straight on and at right angles to the road
//!   it starts from (`Perpendicular`), along the axes of a nearby zoning
//!   lattice (`Grid`), and every 15° (`Angle`);
//! * a cell centre of a nearby zoning lattice, so lots line up across the
//!   street (`Grid`).
//!
//! Along a line from the previous point (and in open land) the length is
//! rounded to whole cells of 8 m (`Length`). Each rule can be switched off
//! in [`SnapOptions`]; holding Alt switches them all off.

use citysim_sim::curve::{angle_between, unit};
use citysim_sim::{CELL_SIZE, Curve, LatticeId, NodeId, SegmentId, World};

use crate::shared::{Guide, GuideKind, SnapTarget};

/// Which snapping rules are on (tool options).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapOptions {
    pub nodes: bool,
    pub roads: bool,
    pub angles: bool,
    pub lengths: bool,
    pub lattice: bool,
    pub parallel: bool,
    pub extensions: bool,
    /// Distance between parallel roads, centre to centre (metres).
    pub parallel_m: f32,
}

impl Default for SnapOptions {
    fn default() -> Self {
        SnapOptions {
            nodes: true,
            roads: true,
            angles: true,
            lengths: true,
            lattice: true,
            parallel: true,
            extensions: true,
            parallel_m: DEFAULT_PARALLEL_M,
        }
    }
}

impl SnapOptions {
    /// Everything off (Alt held).
    pub fn none() -> SnapOptions {
        SnapOptions {
            nodes: false,
            roads: false,
            angles: false,
            lengths: false,
            lattice: false,
            parallel: false,
            extensions: false,
            parallel_m: DEFAULT_PARALLEL_M,
        }
    }
}

/// Two rows of lots back to back between two streets: 2 x (4 m road
/// reserve + 2 cells) = 40 m between centre lines.
pub const DEFAULT_PARALLEL_M: f32 = 40.0;
/// Angle steps from the previous point.
pub const ANGLE_STEP_DEG: f32 = 15.0;

/// Where a snapped point sits on the existing network.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OnNet {
    Node(NodeId),
    /// `d` metres from the segment's `a` node.
    Road {
        segment: SegmentId,
        d: f32,
    },
}

/// The previous point of the road being drawn and the direction angles are
/// measured from (the road it starts from, or the previous piece).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub pos: [f32; 2],
    pub dir: Option<[f32; 2]>,
}

/// A snapped point and why it is there.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapped {
    pub point: [f32; 2],
    /// The strongest rule that placed it (`None`: free).
    pub target: SnapTarget,
    /// Every rule that agrees on it (includes `target`).
    pub matched: Vec<SnapTarget>,
    pub guides: Vec<Guide>,
    /// On an existing node or road.
    pub on: Option<OnNet>,
    /// Parallel snap: distance to the road it runs beside (metres).
    pub spacing_m: Option<f32>,
}

impl Snapped {
    pub fn free(p: [f32; 2]) -> Snapped {
        Snapped {
            point: p,
            target: SnapTarget::None,
            matched: Vec::new(),
            guides: Vec::new(),
            on: None,
            spacing_m: None,
        }
    }
}

// ---------------------------------------------------------------- vectors

pub fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
pub fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
pub fn mul(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}
pub fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}
pub fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}
/// `d` turned by `deg` degrees (positive: towards +y from +x).
pub fn rotate(d: [f32; 2], deg: f32) -> [f32; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [d[0] * c - d[1] * s, d[0] * s + d[1] * c]
}

// ---------------------------------------------------------------- network facts

/// Direction of the road at a point of the network, for measuring angles
/// from it: the tangent of a road, the outward direction of a dead end, or
/// for a junction the arm closest to `towards` (either way along it).
pub fn direction_at(world: &World, on: OnNet, towards: Option<[f32; 2]>) -> Option<[f32; 2]> {
    let roads = &world.roads;
    match on {
        OnNet::Road { segment, d } => roads.segments.get(segment).map(|s| s.pose(d, 0.0).dir),
        OnNet::Node(n) => {
            let node = roads.nodes.get(n)?;
            let arms: Vec<[f32; 2]> = node
                .segments
                .iter()
                .filter_map(|&s| roads.segments.get(s).map(|seg| seg.dir_from(n)))
                .collect();
            match arms.as_slice() {
                [] => None,
                // A dead end: carry on out of it.
                [one] => Some(mul(*one, -1.0)),
                _ => {
                    let t = towards.map(unit);
                    let best = arms.iter().copied().max_by(|a, b| {
                        let k = |d: [f32; 2]| t.map_or(0.0, |t| dot(d, t).abs());
                        k(*a).total_cmp(&k(*b))
                    })?;
                    Some(match t {
                        Some(t) if dot(best, t) < 0.0 => mul(best, -1.0),
                        _ => best,
                    })
                }
            }
        }
    }
}

/// The network point under `p` (a node within `node_r`, else a road
/// within `road_r`).
pub fn on_net(world: &World, p: [f32; 2], node_r: f32, road_r: f32) -> Option<OnNet> {
    let roads = &world.roads;
    let mut best: Option<(f32, NodeId)> = None;
    for (n, node) in roads.nodes.iter() {
        if node.segments.is_empty() {
            continue;
        }
        let d = dist(node.pos, p);
        if d <= node_r && best.is_none_or(|b| d < b.0) {
            best = Some((d, n));
        }
    }
    if let Some((_, n)) = best {
        return Some(OnNet::Node(n));
    }
    let (s, d, _) = roads.nearest_segment(p, road_r)?;
    Some(OnNet::Road { segment: s, d })
}

/// Lattices with cells within `reach` metres of `p` (the one under `p`
/// first), at most two.
pub fn lattices_near(world: &World, p: [f32; 2], reach: f32) -> Vec<LatticeId> {
    let mut out: Vec<LatticeId> = Vec::new();
    let mut probe = |q: [f32; 2]| {
        if let Some(c) = world.zoning.cell_at(q)
            && !out.contains(&c.lattice)
            && out.len() < 2
        {
            out.push(c.lattice);
        }
    };
    probe(p);
    for r in [reach * 0.5, reach] {
        for k in 0..8 {
            probe(add(p, rotate([r, 0.0], k as f32 * 45.0)));
        }
    }
    out
}

// ---------------------------------------------------------------- candidates

/// A guide line: from `origin` along unit `dir` (a ray), or a curve.
#[derive(Debug, Clone, Copy)]
struct Line {
    origin: [f32; 2],
    dir: [f32; 2],
    target: SnapTarget,
    guide: GuideKind,
    rank: u8,
    /// Starts at the anchor (length snapping applies along it).
    from_anchor: bool,
}

impl Line {
    /// Projection of `p` on the ray (`None` behind its origin).
    fn project(&self, p: [f32; 2]) -> Option<([f32; 2], f32)> {
        let t = dot(sub(p, self.origin), self.dir);
        if t <= 0.5 {
            return None;
        }
        let q = add(self.origin, mul(self.dir, t));
        Some((q, dist(q, p)))
    }

    fn guide_to(&self, p: [f32; 2]) -> Guide {
        let t = dot(sub(p, self.origin), self.dir);
        Guide {
            from: self.origin,
            to: add(self.origin, mul(self.dir, t + 24.0)),
            kind: self.guide,
        }
    }
}

#[derive(Debug, Clone)]
struct Cand {
    point: [f32; 2],
    target: SnapTarget,
    rank: u8,
    dist: f32,
    also: Vec<SnapTarget>,
    guides: Vec<Guide>,
    on: Option<OnNet>,
    spacing: Option<f32>,
    /// Index into the line list: length snapping slides along it.
    line: Option<usize>,
}

impl Cand {
    fn new(point: [f32; 2], target: SnapTarget, rank: u8, cursor: [f32; 2]) -> Cand {
        Cand {
            point,
            target,
            rank,
            dist: dist(point, cursor),
            also: Vec::new(),
            guides: Vec::new(),
            on: None,
            spacing: None,
            line: None,
        }
    }
}

/// Ranks (lower wins).
const R_NODE: u8 = 0;
const R_LINE_ROAD: u8 = 1;
const R_LINES: u8 = 2;
const R_ROAD: u8 = 3;
const R_PARALLEL: u8 = 4;
const R_EXTENSION: u8 = 5;
const R_AXIS: u8 = 6;
const R_ANGLE: u8 = 7;
const R_GRID: u8 = 8;

/// How far each rule reaches, as a share of the snap radius.
fn reach(rank: u8) -> f32 {
    match rank {
        R_NODE => 1.3,
        R_LINE_ROAD | R_LINES => 1.1,
        R_ROAD => 1.0,
        R_GRID => 0.6,
        _ => 0.8,
    }
}

/// Snap `cursor` (sim metres). `radius` is the reach in metres (about 18
/// screen pixels); `anchor` the previous point of the road being drawn.
pub fn snap(world: &World, anchor: Option<Anchor>, cursor: [f32; 2], radius: f32, opts: &SnapOptions) -> Snapped {
    let roads = &world.roads;
    let mut cands: Vec<Cand> = Vec::new();
    // Ends and junctions.
    if opts.nodes {
        for (n, node) in roads.nodes.iter() {
            if node.segments.is_empty() {
                continue;
            }
            let d = dist(node.pos, cursor);
            if d <= radius * reach(R_NODE) {
                let mut c = Cand::new(node.pos, SnapTarget::Node, R_NODE, cursor);
                c.on = Some(OnNet::Node(n));
                cands.push(c);
            }
        }
    }
    // Onto a road (a T).
    let near_road = roads.nearest_segment(cursor, radius * reach(R_ROAD));
    if opts.roads
        && let Some((s, d, _)) = near_road
    {
        let p = roads.segments[s].pose(d, 0.0).pos;
        let mut c = Cand::new(p, SnapTarget::Road, R_ROAD, cursor);
        c.on = Some(OnNet::Road { segment: s, d });
        cands.push(c);
    }
    // Guide lines.
    let mut lines: Vec<Line> = Vec::new();
    let lats = if opts.lattice {
        lattices_near(world, anchor.map_or(cursor, |a| a.pos), 24.0)
    } else {
        Vec::new()
    };
    if let Some(a) = anchor {
        if opts.angles {
            let base = a.dir.map(unit).unwrap_or([1.0, 0.0]);
            let steps = (360.0 / ANGLE_STEP_DEG) as i32;
            for k in 0..steps {
                let deg = k as f32 * ANGLE_STEP_DEG;
                let quarter = k % (steps / 4) == 0;
                // Straight back along the road it starts from is never a road.
                if a.dir.is_some() && k == steps / 2 {
                    continue;
                }
                let (target, rank) = if quarter && a.dir.is_some() {
                    (SnapTarget::Perpendicular, R_AXIS)
                } else {
                    (SnapTarget::Angle, R_ANGLE)
                };
                lines.push(Line {
                    origin: a.pos,
                    dir: rotate(base, deg),
                    target,
                    guide: GuideKind::Angle,
                    rank,
                    from_anchor: true,
                });
            }
        }
        for &l in &lats {
            if let Some(lat) = world.zoning.lattice(l) {
                let (u, v) = (unit(lat.u), unit(lat.v()));
                for d in [u, v, mul(u, -1.0), mul(v, -1.0)] {
                    lines.push(Line {
                        origin: a.pos,
                        dir: d,
                        target: SnapTarget::Grid,
                        guide: GuideKind::Perpendicular,
                        rank: R_AXIS,
                        from_anchor: true,
                    });
                }
            }
        }
    }
    if opts.extensions {
        let far = 600.0;
        for (n, node) in roads.nodes.iter() {
            if node.segments.len() != 1 || dist(node.pos, cursor) > far {
                continue;
            }
            let Some(seg) = roads.segments.get(node.segments[0]) else {
                continue;
            };
            let out = mul(seg.dir_from(n), -1.0);
            // Not the anchor's own dead end (that is "straight on").
            if anchor.is_some_and(|a| dist(a.pos, node.pos) < 0.5) {
                continue;
            }
            lines.push(Line {
                origin: node.pos,
                dir: unit(out),
                target: SnapTarget::Extension,
                guide: GuideKind::Extension,
                rank: R_EXTENSION,
                from_anchor: false,
            });
        }
    }
    // Candidates on single lines.
    let mut on_lines: Vec<(usize, [f32; 2], f32)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some((q, d)) = l.project(cursor) {
            if d <= radius * reach(l.rank) {
                let mut c = Cand::new(q, l.target, l.rank, cursor);
                c.guides.push(l.guide_to(q));
                c.line = Some(i);
                cands.push(c);
            }
            if d <= radius * 2.5 {
                on_lines.push((i, q, d));
            }
        }
    }
    // Two lines meeting near the cursor.
    for x in 0..on_lines.len() {
        for y in x + 1..on_lines.len() {
            let (a, b) = (&lines[on_lines[x].0], &lines[on_lines[y].0]);
            if a.from_anchor && b.from_anchor {
                continue;
            }
            let den = cross(a.dir, b.dir);
            if den.abs() < 0.05 {
                continue;
            }
            let w = sub(b.origin, a.origin);
            let (t, u) = (cross(w, b.dir) / den, cross(w, a.dir) / den);
            if t <= 0.5 || u <= 0.5 {
                continue;
            }
            let p = add(a.origin, mul(a.dir, t));
            if dist(p, cursor) <= radius * reach(R_LINES) {
                let (strong, weak) = if a.rank <= b.rank { (a, b) } else { (b, a) };
                let mut c = Cand::new(p, strong.target, R_LINES, cursor);
                c.also.push(weak.target);
                c.guides.push(strong.guide_to(p));
                c.guides.push(weak.guide_to(p));
                cands.push(c);
            }
        }
    }
    // A line meeting a road near the cursor.
    if opts.roads && !on_lines.is_empty() {
        for (s, seg) in roads.segments.iter() {
            let (lo, hi) = seg.curve.bounds();
            let r = radius * 2.0;
            if cursor[0] < lo[0] - r || cursor[0] > hi[0] + r || cursor[1] < lo[1] - r || cursor[1] > hi[1] + r {
                continue;
            }
            for &(i, q, _) in &on_lines {
                let l = &lines[i];
                let a = sub(q, mul(l.dir, radius * 3.0));
                let b = add(q, mul(l.dir, radius * 3.0));
                for (d_seg, _, p) in seg.curve.intersections(&Curve::straight(a, b)) {
                    if dist(p, cursor) <= radius * reach(R_LINE_ROAD) && dot(sub(p, l.origin), l.dir) > 0.5 {
                        let mut c = Cand::new(p, l.target, R_LINE_ROAD, cursor);
                        c.also.push(SnapTarget::Road);
                        c.guides.push(l.guide_to(p));
                        c.on = Some(OnNet::Road { segment: s, d: d_seg });
                        cands.push(c);
                    }
                }
            }
        }
    }
    // Parallel to a nearby road at the set distance.
    if opts.parallel && opts.parallel_m > 0.0 {
        let spacing = opts.parallel_m;
        if let Some((s, d, dd)) = roads.nearest_segment(cursor, spacing + radius)
            && (dd - spacing).abs() <= radius * reach(R_PARALLEL)
        {
            let seg = &roads.segments[s];
            let pose = seg.pose(d, 0.0);
            let side = if cross(pose.dir, sub(cursor, pose.pos)) > 0.0 {
                1.0
            } else {
                -1.0
            };
            // `pose_at` offsets to the right of travel: +y-from-x is right.
            let p = seg.curve.point_at(d, spacing * side);
            let mut c = Cand::new(p, SnapTarget::Parallel, R_PARALLEL, cursor);
            c.spacing = Some(spacing);
            c.guides.push(Guide {
                from: pose.pos,
                to: p,
                kind: GuideKind::Spacing,
            });
            let len = seg.curve.length();
            let (d0, d1) = ((d - 40.0).max(0.0), (d + 40.0).min(len));
            let pts: Vec<[f32; 2]> = (0..=8)
                .map(|k| seg.curve.point_at(d0 + (d1 - d0) * k as f32 / 8.0, spacing * side))
                .collect();
            for w in pts.windows(2) {
                c.guides.push(Guide {
                    from: w[0],
                    to: w[1],
                    kind: GuideKind::Parallel,
                });
            }
            cands.push(c);
        }
    }
    // Cell centres of a nearby lattice.
    for &l in &lats {
        if let Some(lat) = world.zoning.lattice(l) {
            let (i, j) = lat.cell_of(cursor);
            let p = lat.cell_center(i, j);
            if dist(p, cursor) <= radius * reach(R_GRID) {
                cands.push(Cand::new(p, SnapTarget::Grid, R_GRID, cursor));
            }
        }
    }
    let best = cands
        .iter()
        .filter(|c| c.dist <= radius * reach(c.rank))
        .min_by(|a, b| a.rank.cmp(&b.rank).then(a.dist.total_cmp(&b.dist)))
        .cloned();
    let mut out = match best {
        Some(c) => {
            let mut matched = vec![c.target];
            matched.extend(c.also.iter().copied());
            for o in &cands {
                if dist(o.point, c.point) < 0.75 && !matched.contains(&o.target) && o.target != SnapTarget::None {
                    matched.push(o.target);
                }
            }
            let mut s = Snapped {
                point: c.point,
                target: c.target,
                matched,
                guides: c.guides.clone(),
                on: c.on,
                spacing_m: c.spacing,
            };
            // Whole cells along a line from the previous point.
            if opts.lengths
                && let (Some(i), Some(a)) = (c.line, anchor)
                && lines[i].from_anchor
            {
                let l = &lines[i];
                let len = dot(sub(c.point, a.pos), l.dir);
                // Under half a cell from the previous point is that point
                // (never a cell behind it: the cursor resting where the
                // last piece ended must not start a stub).
                let cells = (len / CELL_SIZE).round().max(0.0);
                s.point = add(a.pos, mul(l.dir, cells * CELL_SIZE));
                s.guides = vec![l.guide_to(s.point)];
                if !s.matched.contains(&SnapTarget::Length) {
                    s.matched.push(SnapTarget::Length);
                }
            }
            if s.on.is_none() {
                s.on = on_net(world, s.point, 0.5, 0.3);
            }
            s
        }
        None => {
            let mut s = Snapped::free(cursor);
            if opts.lengths
                && let Some(a) = anchor
            {
                let len = dist(cursor, a.pos);
                if len > 1.0 {
                    let cells = (len / CELL_SIZE).round();
                    s.point = add(a.pos, mul(unit(sub(cursor, a.pos)), cells * CELL_SIZE));
                    s.target = SnapTarget::Length;
                    s.matched = vec![SnapTarget::Length];
                }
            }
            s
        }
    };
    out.matched.dedup();
    out
}

/// Signed angle in degrees from `a` to `b` (positive: turning right in sim
/// axes, where +y is south).
pub fn turn_deg(a: [f32; 2], b: [f32; 2]) -> f32 {
    angle_between(a, b).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::{Command, RoadKind};

    fn town() -> World {
        let mut w = World::new(1);
        w.economy.money = 10_000_000;
        // An east-west street from the highway stub and a north-south one.
        let road = |w: &mut World, a: [f32; 2], b: [f32; 2]| {
            w.apply(Command::BuildCurve {
                curve: Curve::straight(a, b),
                kind: RoadKind::Street,
            })
            .unwrap();
        };
        road(&mut w, [92.0, 1028.0], [300.0, 1028.0]);
        road(&mut w, [204.0, 1028.0], [204.0, 900.0]);
        w
    }

    #[test]
    fn snaps_to_a_dead_end_before_anything_else() {
        let w = town();
        let s = snap(&w, None, [302.0, 1026.0], 6.0, &SnapOptions::default());
        assert_eq!(s.target, SnapTarget::Node);
        assert!(dist(s.point, [300.0, 1028.0]) < 0.01);
        assert!(matches!(s.on, Some(OnNet::Node(_))));
    }

    #[test]
    fn snaps_onto_a_road_for_a_t() {
        let w = town();
        let s = snap(&w, None, [250.0, 1030.5], 6.0, &SnapOptions::default());
        assert_eq!(s.target, SnapTarget::Road);
        assert!((s.point[1] - 1028.0).abs() < 0.01, "{:?}", s.point);
        assert!(matches!(s.on, Some(OnNet::Road { .. })));
    }

    #[test]
    fn alt_snaps_nothing() {
        let w = town();
        let s = snap(&w, None, [302.0, 1026.0], 6.0, &SnapOptions::none());
        assert_eq!(s.target, SnapTarget::None);
        assert_eq!(s.point, [302.0, 1026.0]);
    }

    #[test]
    fn right_angle_from_the_road_it_starts_on_with_whole_cells() {
        let w = town();
        let a = Anchor {
            pos: [250.0, 1028.0],
            dir: Some([1.0, 0.0]),
        };
        // Nearly straight south, 61 m.
        let s = snap(&w, Some(a), [251.5, 1089.0], 6.0, &SnapOptions::default());
        assert_eq!(s.target, SnapTarget::Perpendicular, "{s:?}");
        assert!(s.matched.contains(&SnapTarget::Length));
        assert!((s.point[0] - 250.0).abs() < 0.01);
        // 61 m rounds to 8 cells.
        assert!((s.point[1] - (1028.0 + 64.0)).abs() < 0.01, "{:?}", s.point);
        assert!(!s.guides.is_empty());
    }

    #[test]
    fn a_cursor_resting_on_the_last_point_stays_there() {
        // The cursor a few metres from where the last piece ended (the
        // player clicked the end and pressed Enter): never a cell away,
        // which would start a stub (often backwards, and refused).
        let w = town();
        let a = Anchor {
            pos: [600.0, 700.0],
            dir: Some([0.0, -1.0]),
        };
        for off in [[0.5, 0.3], [-2.0, 1.5], [1.0, 3.0], [0.0, -3.0]] {
            let s = snap(&w, Some(a), add(a.pos, off), 6.0, &SnapOptions::default());
            assert!(dist(s.point, a.pos) < 0.01, "{off:?}: {s:?}");
        }
        // Past half a cell it does snap a whole cell on.
        let s = snap(&w, Some(a), [600.5, 694.0], 6.0, &SnapOptions::default());
        assert!((dist(s.point, a.pos) - 8.0).abs() < 0.01, "{s:?}");
    }

    #[test]
    fn fifteen_degree_steps() {
        let w = town();
        let a = Anchor {
            pos: [250.0, 1028.0],
            dir: Some([1.0, 0.0]),
        };
        // About 30° to the right of east (south-east in sim axes).
        let p = add(a.pos, mul(rotate([1.0, 0.0], 31.5), 80.0));
        let s = snap(&w, Some(a), p, 6.0, &SnapOptions::default());
        assert_eq!(s.target, SnapTarget::Angle);
        let d = unit(sub(s.point, a.pos));
        assert!((turn_deg([1.0, 0.0], d) - 30.0).abs() < 0.1, "{d:?}");
    }

    #[test]
    fn extension_of_a_dead_end_and_its_meeting_with_an_angle_line() {
        let w = town();
        // The north-south street ends at (204, 900): its extension runs
        // north. A cursor on it, far from the end.
        let s = snap(&w, None, [205.0, 820.0], 6.0, &SnapOptions::default());
        assert_eq!(s.target, SnapTarget::Extension, "{s:?}");
        assert!((s.point[0] - 204.0).abs() < 0.01);
        // From an anchor to the west, heading east: the straight-on line
        // meets the extension at (204, 820): both rules agree there.
        let a = Anchor {
            pos: [120.0, 820.0],
            dir: Some([1.0, 0.0]),
        };
        let s = snap(&w, Some(a), [206.0, 822.0], 6.0, &SnapOptions::default());
        assert!(dist(s.point, [204.0, 820.0]) < 0.05, "{s:?}");
        assert!(s.matched.contains(&SnapTarget::Extension), "{:?}", s.matched);
        assert!(s.matched.contains(&SnapTarget::Perpendicular), "{:?}", s.matched);
    }

    #[test]
    fn parallel_at_the_set_distance() {
        let w = town();
        let opts = SnapOptions {
            angles: false,
            lattice: false,
            ..SnapOptions::default()
        };
        // 38 m south of the east-west street.
        let s = snap(&w, None, [260.0, 1066.0], 6.0, &opts);
        assert_eq!(s.target, SnapTarget::Parallel, "{s:?}");
        assert!((s.point[1] - 1068.0).abs() < 0.05, "{:?}", s.point);
        assert_eq!(s.spacing_m, Some(40.0));
    }

    #[test]
    fn lattice_cells_line_up_across_the_street() {
        let w = town();
        let opts = SnapOptions {
            parallel: false,
            ..SnapOptions::default()
        };
        // Beside the street, near lot cells of lattice 0: a cell centre.
        let s = snap(&w, None, [241.0, 1043.0], 6.0, &opts);
        assert_eq!(s.target, SnapTarget::Grid, "{s:?}");
        assert!((s.point[0] / 8.0 - 0.5).fract().abs() < 1e-3, "{:?}", s.point);
    }

    #[test]
    fn direction_at_a_dead_end_points_out_of_it() {
        let w = town();
        let Some(OnNet::Node(n)) = on_net(&w, [300.0, 1028.0], 1.0, 1.0) else {
            panic!()
        };
        let d = direction_at(&w, OnNet::Node(n), None).unwrap();
        assert!((d[0] - 1.0).abs() < 1e-3 && d[1].abs() < 1e-3, "{d:?}");
    }
}
