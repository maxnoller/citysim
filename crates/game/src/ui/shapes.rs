//! The road being drawn, as pure data: shapes (straight, curve, continuous,
//! grid), the chain of pieces drawn so far, and stretches of existing road
//! (the stretch between two junctions that the upgrade, bulldozer and
//! frontage tools work along). No Bevy state.

use citysim_sim::curve::{right_of, unit};
use citysim_sim::{Curve, SegmentId, World};

use super::fsnap::{Anchor, dist};

/// How the points of a piece make its curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Shape {
    /// Two points per piece.
    #[default]
    Straight,
    /// Start, the corner where the two directions meet, end.
    Curve,
    /// Each piece leaves the last one smoothly (two points: the previous
    /// end and the new one).
    Continuous,
    /// A corner, the width, the length: a street grid.
    Grid,
}

impl Shape {
    /// Points one piece takes, its start included.
    pub fn points(self) -> usize {
        match self {
            Shape::Straight | Shape::Continuous => 2,
            Shape::Curve | Shape::Grid => 3,
        }
    }
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}
fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

/// What placing a point did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    /// A point of the piece in progress.
    Point,
    /// It finished a piece (now part of the chain; the next piece starts
    /// at its end).
    Piece,
    /// It finished a grid (build it now).
    Grid,
    /// Ignored (too close to the previous point).
    Nothing,
}

/// Grid rectangle from its three points: corner `a`, `b` along the first
/// side, `c` across. Side lengths are whole blocks of `block_m` (at least
/// one). Returns the corner, the unit side directions and the block counts.
pub fn grid_frame(a: [f32; 2], b: [f32; 2], c: Option<[f32; 2]>, block_m: f32) -> Option<GridFrame> {
    let ab = sub(b, a);
    let len = (dot(ab, ab)).sqrt();
    if len < 1.0 || block_m < 1.0 {
        return None;
    }
    let u = unit(ab);
    let nu = ((len / block_m).round() as u32).max(1);
    let (v, nv) = match c {
        Some(c) => {
            let r = right_of(u);
            let ac = sub(c, a);
            let side = if dot(ac, r) >= 0.0 { 1.0 } else { -1.0 };
            let across = dot(ac, r).abs();
            (mul(r, side), (across / block_m).round() as u32)
        }
        None => (right_of(u), 0),
    };
    Some(GridFrame {
        a,
        u,
        v,
        nu,
        nv,
        block_m,
    })
}

/// A street grid: corner, unit side directions, blocks along each.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridFrame {
    pub a: [f32; 2],
    pub u: [f32; 2],
    pub v: [f32; 2],
    pub nu: u32,
    pub nv: u32,
    pub block_m: f32,
}

impl GridFrame {
    pub fn width_m(&self) -> f32 {
        self.nu as f32 * self.block_m
    }
    pub fn depth_m(&self) -> f32 {
        self.nv as f32 * self.block_m
    }
    /// Every street, the ones along `u` first (build order).
    pub fn streets(&self) -> Vec<Curve> {
        let mut out = Vec::new();
        let w = mul(self.u, self.width_m());
        let d = mul(self.v, self.depth_m());
        for j in 0..=self.nv {
            let s = add(self.a, mul(self.v, j as f32 * self.block_m));
            out.push(Curve::straight(s, add(s, w)));
        }
        if self.nv > 0 {
            for i in 0..=self.nu {
                let s = add(self.a, mul(self.u, i as f32 * self.block_m));
                out.push(Curve::straight(s, add(s, d)));
            }
        }
        out
    }
}

/// The road being drawn: finished pieces (built together as one action)
/// and the points of the piece in progress.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Draft {
    pub shape: Shape,
    /// Finished pieces, in build order.
    pub pieces: Vec<Curve>,
    /// Points of the piece in progress; the first is its start (the end of
    /// the last piece once there is one).
    pub points: Vec<[f32; 2]>,
    /// Direction the next piece leaves its start in: the end direction of
    /// the last piece, or of the road the chain started from.
    pub start_dir: Option<[f32; 2]>,
    /// Grid block size, metres (centre line to centre line).
    pub block_m: f32,
}

/// Points closer than this are the same point.
pub const SAME_POINT_M: f32 = 1.0;

impl Draft {
    pub fn new(shape: Shape, block_m: f32) -> Draft {
        Draft {
            shape,
            block_m,
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.pieces.is_empty()
    }

    /// The last placed point.
    pub fn last(&self) -> Option<[f32; 2]> {
        self.points.last().copied()
    }

    /// What snapping measures from for the next point.
    pub fn anchor(&self) -> Option<Anchor> {
        let last = self.last()?;
        let dir = match (self.shape, self.points.len()) {
            (Shape::Curve, 2) | (Shape::Grid, 2) => Some(unit(sub(self.points[1], self.points[0]))),
            _ => self.start_dir,
        };
        Some(Anchor { pos: last, dir })
    }

    /// Place a point (already snapped).
    pub fn place(&mut self, p: [f32; 2]) -> Placed {
        if self.last().is_some_and(|l| dist(l, p) < SAME_POINT_M) {
            return Placed::Nothing;
        }
        self.points.push(p);
        if self.points.len() < self.shape.points() {
            return Placed::Point;
        }
        if self.shape == Shape::Grid {
            return Placed::Grid;
        }
        let Some(curve) = self.piece(&self.points) else {
            self.points.pop();
            return Placed::Nothing;
        };
        self.pieces.push(curve);
        self.start_dir = Some(curve.end_dir());
        self.points = vec![curve.end()];
        Placed::Piece
    }

    /// Backspace: drop the last point (back into the last piece when the
    /// piece in progress has only its start). True when something went.
    pub fn pop(&mut self) -> bool {
        if self.points.len() > 1 || self.pieces.is_empty() {
            let had = self.points.pop().is_some();
            if self.points.is_empty() {
                self.start_dir = None;
            }
            return had;
        }
        // Reopen the last piece: its control points minus the end.
        let last = self.pieces.pop().expect("checked");
        self.start_dir = self
            .pieces
            .last()
            .map(|c| c.end_dir())
            .or(self.start_dir_of_first(&last));
        self.points = match self.shape {
            Shape::Curve => vec![last.start(), corner_of(&last)],
            _ => vec![last.start()],
        };
        if self.points.len() >= self.shape.points() {
            self.points.truncate(self.shape.points() - 1);
        }
        true
    }

    fn start_dir_of_first(&self, c: &Curve) -> Option<[f32; 2]> {
        (self.shape == Shape::Continuous).then(|| c.start_dir())
    }

    /// The curve of one piece from its points (all of them placed).
    fn piece(&self, pts: &[[f32; 2]]) -> Option<Curve> {
        match (self.shape, pts) {
            (Shape::Straight, [a, b]) => Some(Curve::straight(*a, *b)),
            (Shape::Continuous, [a, b]) => Some(match self.start_dir {
                Some(d) => Curve::continuing(*a, d, *b),
                None => Curve::straight(*a, *b),
            }),
            (Shape::Curve, [a, c, b]) => Some(Curve::through_corner(*a, *c, *b)),
            _ => None,
        }
    }

    /// The grid the points (and the live point) make.
    pub fn grid(&self, live: Option<[f32; 2]>) -> Option<GridFrame> {
        if self.shape != Shape::Grid {
            return None;
        }
        let mut pts = self.points.clone();
        pts.extend(live);
        match pts.as_slice() {
            [a, b] => grid_frame(*a, *b, None, self.block_m),
            [a, b, c, ..] => grid_frame(*a, *b, Some(*c), self.block_m),
            _ => None,
        }
    }

    /// Every curve the draft would build if `live` were placed next (the
    /// piece in progress drawn as far as its points allow).
    pub fn curves(&self, live: Option<[f32; 2]>) -> Vec<Curve> {
        let mut out = self.pieces.clone();
        let mut pts = self.points.clone();
        if let Some(l) = live
            && self.last().is_none_or(|p| dist(p, l) >= SAME_POINT_M)
        {
            pts.push(l);
        }
        match self.shape {
            Shape::Grid => {
                if let Some(g) = self.grid(live) {
                    out.extend(g.streets());
                }
            }
            Shape::Curve => match pts.as_slice() {
                // Corner chosen so far: the first tangent, straight.
                [a, c] => out.push(Curve::straight(*a, *c)),
                [_, _, _] => out.extend(self.piece(&pts)),
                _ => {}
            },
            _ => {
                if pts.len() == 2 {
                    out.extend(self.piece(&pts));
                }
            }
        }
        out
    }

    /// Control points to show: every piece's start, the corners, the
    /// points in progress and the live point.
    pub fn control_points(&self, live: Option<[f32; 2]>) -> Vec<[f32; 2]> {
        let mut out: Vec<[f32; 2]> = Vec::new();
        for c in &self.pieces {
            out.push(c.start());
            if self.shape == Shape::Curve {
                out.push(corner_of(c));
            }
        }
        out.extend(self.points.iter().copied());
        out.extend(live);
        out.dedup_by(|a, b| dist(*a, *b) < 0.01);
        out
    }

    /// Tangent handles: the curve tool's lines to its corner.
    pub fn handles(&self, live: Option<[f32; 2]>) -> Vec<([f32; 2], [f32; 2])> {
        if self.shape != Shape::Curve {
            return Vec::new();
        }
        let mut out = Vec::new();
        for c in &self.pieces {
            let k = corner_of(c);
            out.push((c.start(), k));
            out.push((c.end(), k));
        }
        match (self.points.as_slice(), live) {
            ([a, k], Some(e)) => {
                out.push((*a, *k));
                out.push((e, *k));
            }
            ([a], Some(k)) => out.push((*a, k)),
            _ => {}
        }
        out
    }
}

/// Where the tangents at the two ends of a curve meet (its "corner"); the
/// middle control point for a straight one.
pub fn corner_of(c: &Curve) -> [f32; 2] {
    let (a, b) = (c.start(), c.end());
    let (da, db) = (c.start_dir(), c.end_dir());
    let den = cross(da, db);
    if den.abs() < 1e-4 {
        return mul(add(a, b), 0.5);
    }
    let t = cross(sub(b, a), db) / den;
    add(a, mul(da, t))
}

/// The tightest bend of some curves: (point, centre of curvature, radius,
/// curve index).
pub fn tightest(curves: &[Curve]) -> Option<([f32; 2], [f32; 2], f32, usize)> {
    let mut best: Option<([f32; 2], [f32; 2], f32, usize)> = None;
    for (i, c) in curves.iter().enumerate() {
        if c.is_straight() {
            continue;
        }
        let (r, d) = c.min_radius();
        if !r.is_finite() || r > 2000.0 {
            continue;
        }
        if best.is_some_and(|b| b.2 <= r) {
            continue;
        }
        let pose = c.pose_at(d, 0.0);
        let ahead = c.pose_at((d + 1.0).min(c.length()), 0.0).dir;
        let behind = c.pose_at((d - 1.0).max(0.0), 0.0).dir;
        // Right of travel when turning right (cross > 0 in sim axes).
        let side = if cross(behind, ahead) >= 0.0 { 1.0 } else { -1.0 };
        let centre = add(pose.pos, mul(right_of(pose.dir), side * r));
        best = Some((pose.pos, centre, r, i));
    }
    best
}

// ---------------------------------------------------------------- stretches

/// One segment of a stretch, in stretch order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StretchPart {
    pub segment: SegmentId,
    /// Travelling along the stretch goes from the segment's `a` to `b`.
    pub forward: bool,
    pub length: f32,
}

/// A run of road of one kind between two junctions or dead ends (through
/// nodes where only two segments meet).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stretch {
    pub parts: Vec<StretchPart>,
}

impl Stretch {
    /// The stretch containing `seg`.
    pub fn of(world: &World, seg: SegmentId) -> Stretch {
        let roads = &world.roads;
        let Some(s0) = roads.segments.get(seg) else {
            return Stretch::default();
        };
        let mut parts = vec![StretchPart {
            segment: seg,
            forward: true,
            length: s0.length,
        }];
        let mut seen = vec![seg];
        // Walk on from `b`, then back from `a`.
        for at_end in [true, false] {
            let mut node = if at_end { s0.b } else { s0.a };
            while let Some(n) = roads.nodes.get(node) {
                if n.segments.len() != 2 {
                    break;
                }
                let Some(&next) = n.segments.iter().find(|s| !seen.contains(s)) else {
                    break;
                };
                let Some(ns) = roads.segments.get(next) else { break };
                // A change of road kind ends the stretch too.
                if ns.kind != s0.kind {
                    break;
                }
                seen.push(next);
                // Leaving `node` along `next`: forward when `node` is its `a`.
                let leaves_a = ns.a == node;
                let part = StretchPart {
                    segment: next,
                    forward: if at_end { leaves_a } else { !leaves_a },
                    length: ns.length,
                };
                if at_end {
                    parts.push(part);
                } else {
                    parts.insert(0, part);
                }
                node = ns.other(node);
            }
        }
        Stretch { parts }
    }

    pub fn length(&self) -> f32 {
        self.parts.iter().map(|p| p.length).sum()
    }

    pub fn segments(&self) -> Vec<SegmentId> {
        self.parts.iter().map(|p| p.segment).collect()
    }

    /// Position along the stretch (metres from its start) nearest to `p`.
    pub fn locate(&self, world: &World, p: [f32; 2]) -> Option<f32> {
        let mut best: Option<(f32, f32)> = None;
        let mut at = 0.0;
        for part in &self.parts {
            let seg = world.roads.segments.get(part.segment)?;
            let (d, dd, _) = seg.nearest(p);
            let along = if part.forward { d } else { part.length - d };
            if best.is_none_or(|b| dd < b.0) {
                best = Some((dd, at + along));
            }
            at += part.length;
        }
        best.map(|b| b.1)
    }

    /// The part of each segment between positions `s0` and `s1` along the
    /// stretch: `(segment, from_m, to_m)` in the segment's own metres from
    /// its `a` node (`from_m < to_m`). Empty pieces are left out.
    pub fn ranges(&self, s0: f32, s1: f32) -> Vec<(SegmentId, f32, f32)> {
        let (lo, hi) = (s0.min(s1), s0.max(s1));
        let mut out = Vec::new();
        let mut at = 0.0;
        for part in &self.parts {
            let (p0, p1) = (at, at + part.length);
            at = p1;
            let (a, b) = (lo.max(p0), hi.min(p1));
            if b - a < 0.05 {
                continue;
            }
            let (x, y) = if part.forward {
                (a - p0, b - p0)
            } else {
                (p1 - b, p1 - a)
            };
            let snap = |v: f32| {
                if v < 0.05 {
                    0.0
                } else if v > part.length - 0.05 {
                    part.length
                } else {
                    v
                }
            };
            out.push((part.segment, snap(x), snap(y)));
        }
        out
    }

    /// The whole stretch as a polyline (for highlights).
    pub fn polyline(&self, world: &World) -> Vec<[f32; 2]> {
        let mut out: Vec<[f32; 2]> = Vec::new();
        for part in &self.parts {
            let Some(seg) = world.roads.segments.get(part.segment) else {
                continue;
            };
            let mut pts = seg.curve.flatten(0.3);
            if !part.forward {
                pts.reverse();
            }
            for p in pts {
                if out.last().is_none_or(|q| dist(*q, p) > 0.01) {
                    out.push(p);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::{Command, RoadKind};

    #[test]
    fn straight_chain_builds_piece_after_piece() {
        let mut d = Draft::new(Shape::Straight, 40.0);
        assert_eq!(d.place([0.0, 0.0]), Placed::Point);
        assert_eq!(d.place([0.4, 0.0]), Placed::Nothing);
        assert_eq!(d.place([50.0, 0.0]), Placed::Piece);
        assert_eq!(d.place([50.0, 60.0]), Placed::Piece);
        assert_eq!(d.pieces.len(), 2);
        assert_eq!(d.points, vec![[50.0, 60.0]]);
        // The live point adds a third piece to the preview.
        assert_eq!(d.curves(Some([90.0, 60.0])).len(), 3);
        // Backspace reopens the last piece.
        assert!(d.pop());
        assert_eq!(d.pieces.len(), 1);
        assert_eq!(d.points, vec![[50.0, 0.0]]);
    }

    #[test]
    fn curve_through_its_corner() {
        let mut d = Draft::new(Shape::Curve, 40.0);
        d.place([0.0, 0.0]);
        d.place([100.0, 0.0]);
        // Two points so far: the first tangent, and a handle to the cursor.
        assert_eq!(d.curves(Some([100.0, 100.0])).len(), 1);
        assert_eq!(d.place([100.0, 100.0]), Placed::Piece);
        let c = d.pieces[0];
        assert!(!c.is_straight());
        let k = corner_of(&c);
        assert!(dist(k, [100.0, 0.0]) < 0.5, "{k:?}");
        // Leaves east, arrives going south.
        assert!(c.start_dir()[0] > 0.99 && c.end_dir()[1] > 0.99);
        let (p, centre, r, _) = tightest(&d.pieces).unwrap();
        assert!(r > 50.0 && r < 120.0, "{r}");
        assert!((dist(p, centre) - r).abs() < 0.5);
        // The centre of a right turn (east to south) is south-west of the bend.
        assert!(centre[1] > p[1] - 1.0);
    }

    #[test]
    fn continuous_pieces_leave_smoothly() {
        let mut d = Draft::new(Shape::Continuous, 40.0);
        d.start_dir = Some([1.0, 0.0]);
        d.place([0.0, 0.0]);
        d.place([80.0, 30.0]);
        d.place([140.0, 100.0]);
        let (a, b) = (d.pieces[0], d.pieces[1]);
        let (e, s) = (a.end_dir(), b.start_dir());
        assert!((e[0] * s[0] + e[1] * s[1]) > 0.999, "{e:?} {s:?}");
        assert!(a.start_dir()[0] > 0.999);
    }

    #[test]
    fn grid_rounds_to_whole_blocks_at_any_angle() {
        let a = [100.0, 100.0];
        let u = unit([3.0, 1.0]);
        let b = add(a, mul(u, 125.0));
        let c = add(b, mul(right_of(u), 78.0));
        let g = grid_frame(a, b, Some(c), 40.0).unwrap();
        assert_eq!((g.nu, g.nv), (3, 2));
        let streets = g.streets();
        // 3 along, 4 across.
        assert_eq!(streets.len(), 3 + 4);
        assert!((streets[0].length() - 120.0).abs() < 0.01);
        assert!((streets[3].length() - 80.0).abs() < 0.01);
        let mut d = Draft::new(Shape::Grid, 40.0);
        d.place(a);
        d.place(b);
        assert_eq!(d.place(c), Placed::Grid);
        assert_eq!(d.curves(None).len(), 7);
    }

    #[test]
    fn stretch_runs_between_junctions() {
        let mut w = World::new(1);
        w.economy.money = 10_000_000;
        let road = |w: &mut World, a: [f32; 2], b: [f32; 2]| {
            w.apply(Command::BuildCurve {
                curve: Curve::straight(a, b),
                kind: RoadKind::Street,
            })
            .unwrap();
        };
        // A bent street: two segments through a degree-2 node, then a T.
        road(&mut w, [92.0, 1028.0], [200.0, 1028.0]);
        road(&mut w, [200.0, 1028.0], [260.0, 980.0]);
        road(&mut w, [150.0, 1028.0], [150.0, 1100.0]);
        let (s, _, _) = w.roads.nearest_segment([230.0, 1004.0], 2.0).unwrap();
        let st = Stretch::of(&w, s);
        // From the T at x = 150 to the dead end.
        assert_eq!(st.parts.len(), 2, "{st:?}");
        assert!((st.length() - (50.0 + 76.84)).abs() < 0.1, "{}", st.length());
        let at = st.locate(&w, [175.0, 1030.0]).unwrap();
        assert!((at - 25.0).abs() < 0.2, "{at}");
        let r = st.ranges(10.0, 60.0);
        assert_eq!(r.len(), 2);
        let total: f32 = r.iter().map(|(_, a, b)| b - a).sum();
        assert!((total - 50.0).abs() < 0.1, "{total}");
    }
}
