//! [`Curve`]: a cubic Bézier centre line in sim metres, the geometry of
//! every road segment and junction turn path.
//!
//! A straight road is the degenerate case with its handles on the line at a
//! third and two thirds ([`Curve::straight`]); it is parametrised exactly by
//! arc length. Curved ones use a table of [`ARC_SAMPLES`] chords: lengths,
//! distances and poses all come from that table, so `length()` and
//! `pose_at(length())` always agree.
//!
//! Distances (`d`) are metres along the curve from its start; offsets are
//! metres to the *right* of the direction of travel (sim space: the right of
//! a unit direction `(x, y)` is `(-y, x)`; traffic drives on the right).
//!
//! Determinism: only `+ - * / sqrt` and `libm` trigonometry, so the same
//! inputs give the same bits on every machine.

use serde::{Deserialize, Serialize};

use crate::geom;

/// Chords per curved Bézier in the arc-length table.
pub const ARC_SAMPLES: usize = 32;

/// A point and unit direction on a curve (`right` = `(-dir.y, dir.x)`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub pos: [f32; 2],
    pub dir: [f32; 2],
}

impl Pose {
    /// Unit vector to the right of travel.
    pub fn right(&self) -> [f32; 2] {
        [-self.dir[1], self.dir[0]]
    }

    /// Heading in radians, `atan2(dy, dx)`.
    pub fn heading(&self) -> f32 {
        libm::atan2f(self.dir[1], self.dir[0])
    }
}

/// Cubic Bézier `p[0]` (start), `p[1]`, `p[2]` (handles), `p[3]` (end), sim
/// metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub p: [[f32; 2]; 4],
}

/// Cumulative chord lengths of a curve at `t = k / ARC_SAMPLES` (empty for a
/// straight curve, which needs none; an empty table passed for a curved
/// curve is rebuilt on the fly).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArcTable {
    cum: Vec<f32>,
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}

fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn norm(a: [f32; 2]) -> f32 {
    geom::dot(a, a).sqrt()
}

/// `a` scaled to unit length (`[1, 0]` for a zero vector).
pub fn unit(a: [f32; 2]) -> [f32; 2] {
    let l = norm(a);
    if l > 1e-9 { [a[0] / l, a[1] / l] } else { [1.0, 0.0] }
}

/// Right of unit direction `d`.
pub fn right_of(d: [f32; 2]) -> [f32; 2] {
    [-d[1], d[0]]
}

/// Signed angle from `a` to `b` (radians, counter-clockwise in sim axes is
/// positive: from `+x` towards `+y`).
pub fn angle_between(a: [f32; 2], b: [f32; 2]) -> f32 {
    libm::atan2f(geom::cross(a, b), geom::dot(a, b))
}

/// Where two lines `p + t d` and `q + s e` meet: `(t, s)`, if not parallel.
pub fn line_meet(p: [f32; 2], d: [f32; 2], q: [f32; 2], e: [f32; 2]) -> Option<(f32, f32)> {
    let den = geom::cross(d, e);
    if den.abs() < 1e-6 {
        return None;
    }
    let qp = geom::sub(q, p);
    Some((geom::cross(qp, e) / den, geom::cross(qp, d) / den))
}

/// Intersection of the closed segments `a`-`b` and `c`-`d`: `(s, t)` with
/// the point at `a + s (b - a)` = `c + t (d - c)`.
pub fn segments_meet(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> Option<(f32, f32)> {
    let r = geom::sub(b, a);
    let s = geom::sub(d, c);
    let den = geom::cross(r, s);
    if den.abs() < 1e-9 {
        return None;
    }
    let ca = geom::sub(c, a);
    let t = geom::cross(ca, s) / den;
    let u = geom::cross(ca, r) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

impl Curve {
    pub fn new(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2]) -> Curve {
        Curve { p: [p0, p1, p2, p3] }
    }

    /// The straight road from `a` to `b` (handles at a third and two
    /// thirds: parametrised exactly by arc length).
    pub fn straight(a: [f32; 2], b: [f32; 2]) -> Curve {
        Curve::new(a, lerp(a, b, 1.0 / 3.0), lerp(a, b, 2.0 / 3.0), b)
    }

    /// A curve leaving `start` along unit `start_dir` and arriving at `end`
    /// travelling along unit `end_dir`, approximating a circular arc: the
    /// handle length is `(L1 + L2) / 6 * 4 tan(θ/4) / tan(θ/2)` (clamped to
    /// each tangent's length), where the tangent lines meet at the corner
    /// `L1` from the start and `L2` from the end and turn by `θ`. Parallel,
    /// collinear tangents give the straight road. `None` when the tangent
    /// lines do not meet ahead of the start and behind the end (an S-bend
    /// needs two curves, or [`Curve::new`]).
    pub fn from_tangents(start: [f32; 2], start_dir: [f32; 2], end: [f32; 2], end_dir: [f32; 2]) -> Option<Curve> {
        let (ds, de) = (unit(start_dir), unit(end_dir));
        let chord = geom::sub(end, start);
        let len = norm(chord);
        if len < 1e-4 {
            return None;
        }
        let Some((l1, l2)) = line_meet(start, ds, end, scale(de, -1.0)) else {
            // Parallel: straight if collinear and pointing at the end.
            let along = geom::dot(chord, ds);
            let off = geom::cross(ds, chord).abs();
            return (geom::dot(ds, de) > 0.0 && along > 0.0 && off < 1e-3 * len.max(1.0))
                .then(|| Curve::straight(start, end));
        };
        if l1 <= 1e-4 || l2 <= 1e-4 {
            return None;
        }
        Some(Curve::fit(start, ds, l1, end, de, l2))
    }

    /// Arc-like fit with tangent lengths `l1`, `l2` to the corner.
    fn fit(start: [f32; 2], ds: [f32; 2], l1: f32, end: [f32; 2], de: [f32; 2], l2: f32) -> Curve {
        let theta = angle_between(ds, de).abs();
        let k = if theta < 1e-4 {
            0.5
        } else {
            libm::tanf(theta / 4.0) / libm::tanf(theta / 2.0)
        };
        let h = (l1 + l2) / 6.0 * 4.0 * k;
        let (h1, h2) = (h.min(l1), h.min(l2));
        Curve::new(start, add(start, scale(ds, h1)), add(end, scale(de, -h2)), end)
    }

    /// The simple curve of a three-point tool: tangents run from `start`
    /// to `corner` and from `corner` to `end`. Degenerates to a straight
    /// road when the three points are collinear.
    pub fn through_corner(start: [f32; 2], corner: [f32; 2], end: [f32; 2]) -> Curve {
        let (a, b) = (geom::sub(corner, start), geom::sub(end, corner));
        let (l1, l2) = (norm(a), norm(b));
        if l1 < 1e-4 || l2 < 1e-4 {
            return Curve::straight(start, end);
        }
        Curve::fit(start, unit(a), l1, end, unit(b), l2)
    }

    /// A curve leaving `start` along unit `dir` (continuing the previous
    /// piece) and ending at `end`: the circular arc tangent to `dir` through
    /// `end` (straight when `end` lies ahead on the line). Arcs turning more
    /// than about 170° are clamped to the straight-ahead half plane.
    pub fn continuing(start: [f32; 2], dir: [f32; 2], end: [f32; 2]) -> Curve {
        let ds = unit(dir);
        let chord = geom::sub(end, start);
        let len = norm(chord);
        if len < 1e-4 {
            return Curve::straight(start, end);
        }
        let phi = angle_between(ds, chord).clamp(-1.48, 1.48);
        if phi.abs() < 1e-4 {
            return Curve::straight(start, end);
        }
        // An arc turning by 2 phi; the end direction is the start direction
        // rotated by 2 phi.
        let (s, c) = (libm::sinf(2.0 * phi), libm::cosf(2.0 * phi));
        let de = [ds[0] * c - ds[1] * s, ds[0] * s + ds[1] * c];
        // Tangent length of an arc of chord `len` turning by 2 phi.
        let l = len / (2.0 * libm::cosf(phi));
        Curve::fit(start, ds, l, end, de, l)
    }

    pub fn start(&self) -> [f32; 2] {
        self.p[0]
    }

    pub fn end(&self) -> [f32; 2] {
        self.p[3]
    }

    /// Unit direction of travel at the start.
    pub fn start_dir(&self) -> [f32; 2] {
        self.dir_at_t(0.0)
    }

    /// Unit direction of travel at the end.
    pub fn end_dir(&self) -> [f32; 2] {
        self.dir_at_t(1.0)
    }

    /// The same curve driven the other way.
    pub fn reversed(&self) -> Curve {
        Curve::new(self.p[3], self.p[2], self.p[1], self.p[0])
    }

    /// Are the handles on the straight line at a third and two thirds
    /// (within a millimetre)? Such curves are parametrised by arc length.
    pub fn is_straight(&self) -> bool {
        let s = Curve::straight(self.p[0], self.p[3]);
        geom::dist(s.p[1], self.p[1]) < 1e-3 && geom::dist(s.p[2], self.p[2]) < 1e-3
    }

    /// Point at parameter `t` in `0..=1`.
    pub fn point_t(&self, t: f32) -> [f32; 2] {
        let u = 1.0 - t;
        let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let p = &self.p;
        [
            b0 * p[0][0] + b1 * p[1][0] + b2 * p[2][0] + b3 * p[3][0],
            b0 * p[0][1] + b1 * p[1][1] + b2 * p[2][1] + b3 * p[3][1],
        ]
    }

    /// Derivative at parameter `t`.
    pub fn deriv_t(&self, t: f32) -> [f32; 2] {
        let u = 1.0 - t;
        let p = &self.p;
        let a = geom::sub(p[1], p[0]);
        let b = geom::sub(p[2], p[1]);
        let c = geom::sub(p[3], p[2]);
        let (k0, k1, k2) = (3.0 * u * u, 6.0 * u * t, 3.0 * t * t);
        [k0 * a[0] + k1 * b[0] + k2 * c[0], k0 * a[1] + k1 * b[1] + k2 * c[1]]
    }

    fn deriv2_t(&self, t: f32) -> [f32; 2] {
        let p = &self.p;
        let a = add(geom::sub(p[2], scale(p[1], 2.0)), p[0]);
        let b = add(geom::sub(p[3], scale(p[2], 2.0)), p[1]);
        [6.0 * ((1.0 - t) * a[0] + t * b[0]), 6.0 * ((1.0 - t) * a[1] + t * b[1])]
    }

    /// Unit tangent at parameter `t` (falls back to the chord where the
    /// derivative vanishes).
    pub fn dir_at_t(&self, t: f32) -> [f32; 2] {
        let d = self.deriv_t(t);
        if norm(d) > 1e-6 {
            return unit(d);
        }
        // A handle sits on its end point: look a little inside.
        let d = geom::sub(self.point_t((t + 1e-3).min(1.0)), self.point_t((t - 1e-3).max(0.0)));
        if norm(d) > 1e-9 {
            unit(d)
        } else {
            unit(geom::sub(self.p[3], self.p[0]))
        }
    }

    /// Arc-length table (empty for straight curves).
    pub fn arc_table(&self) -> ArcTable {
        if self.is_straight() {
            return ArcTable::default();
        }
        let mut cum = Vec::with_capacity(ARC_SAMPLES + 1);
        cum.push(0.0);
        let mut prev = self.p[0];
        let mut total = 0.0;
        for k in 1..=ARC_SAMPLES {
            let q = self.point_t(k as f32 / ARC_SAMPLES as f32);
            total += geom::dist(prev, q);
            cum.push(total);
            prev = q;
        }
        ArcTable { cum }
    }

    /// Length in metres.
    pub fn length(&self) -> f32 {
        let t = self.arc_table();
        match t.cum.last() {
            Some(&l) => l,
            None => geom::dist(self.p[0], self.p[3]),
        }
    }

    /// Length using a precomputed table of this curve.
    pub fn length_with(&self, table: &ArcTable) -> f32 {
        match table.cum.last() {
            Some(&l) => l,
            None if self.is_straight() => geom::dist(self.p[0], self.p[3]),
            None => self.length(),
        }
    }

    /// Parameter at distance `d` (clamped to the curve).
    pub fn t_at(&self, d: f32) -> f32 {
        self.t_at_with(&self.arc_table(), d)
    }

    pub fn t_at_with(&self, table: &ArcTable, d: f32) -> f32 {
        if table.cum.is_empty() && !self.is_straight() {
            return self.t_at_with(&self.arc_table(), d);
        }
        let cum = &table.cum;
        if cum.is_empty() {
            let l = geom::dist(self.p[0], self.p[3]);
            return if l > 0.0 { (d / l).clamp(0.0, 1.0) } else { 0.0 };
        }
        let total = cum[ARC_SAMPLES];
        if d <= 0.0 {
            return 0.0;
        }
        if d >= total {
            return 1.0;
        }
        // First sample at or beyond d.
        let k = cum.partition_point(|&c| c < d).clamp(1, ARC_SAMPLES);
        let (c0, c1) = (cum[k - 1], cum[k]);
        let f = if c1 > c0 { (d - c0) / (c1 - c0) } else { 0.0 };
        ((k - 1) as f32 + f) / ARC_SAMPLES as f32
    }

    /// Distance along the curve at parameter `t`.
    pub fn d_at_t(&self, t: f32) -> f32 {
        self.d_at_t_with(&self.arc_table(), t)
    }

    pub fn d_at_t_with(&self, table: &ArcTable, t: f32) -> f32 {
        if table.cum.is_empty() && !self.is_straight() {
            return self.d_at_t_with(&self.arc_table(), t);
        }
        let cum = &table.cum;
        let t = t.clamp(0.0, 1.0);
        if cum.is_empty() {
            return t * geom::dist(self.p[0], self.p[3]);
        }
        let x = t * ARC_SAMPLES as f32;
        let k = (x.floor() as usize).min(ARC_SAMPLES - 1);
        let f = x - k as f32;
        cum[k] + (cum[k + 1] - cum[k]) * f
    }

    /// Point and direction `d` metres along, `offset` metres to the right.
    pub fn pose_at(&self, d: f32, offset: f32) -> Pose {
        self.pose_at_with(&self.arc_table(), d, offset)
    }

    pub fn pose_at_with(&self, table: &ArcTable, d: f32, offset: f32) -> Pose {
        let t = self.t_at_with(table, d);
        let dir = self.dir_at_t(t);
        let p = self.point_t(t);
        let r = right_of(dir);
        Pose {
            pos: [p[0] + r[0] * offset, p[1] + r[1] * offset],
            dir,
        }
    }

    /// Point `d` metres along, `offset` metres to the right.
    pub fn point_at(&self, d: f32, offset: f32) -> [f32; 2] {
        self.pose_at(d, offset).pos
    }

    /// Nearest point of the curve to `p`: `(d along the curve, distance,
    /// point)`.
    pub fn nearest(&self, p: [f32; 2]) -> (f32, f32, [f32; 2]) {
        self.nearest_with(&self.arc_table(), p)
    }

    pub fn nearest_with(&self, table: &ArcTable, p: [f32; 2]) -> (f32, f32, [f32; 2]) {
        if table.cum.is_empty() && !self.is_straight() {
            return self.nearest_with(&self.arc_table(), p);
        }
        if table.cum.is_empty() {
            let (a, b) = (self.p[0], self.p[3]);
            let ab = geom::sub(b, a);
            let l2 = geom::dot(ab, ab);
            let t = if l2 > 0.0 {
                (geom::dot(geom::sub(p, a), ab) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = lerp(a, b, t);
            return (t * l2.sqrt(), geom::dist(p, q), q);
        }
        // Coarse: the nearest chord, then refine with a few bisections of
        // the parameter on each side.
        let n = ARC_SAMPLES;
        let mut best = (f32::INFINITY, 0.0f32);
        let mut prev = self.p[0];
        for k in 1..=n {
            let q = self.point_t(k as f32 / n as f32);
            let ab = geom::sub(q, prev);
            let l2 = geom::dot(ab, ab);
            let f = if l2 > 0.0 {
                (geom::dot(geom::sub(p, prev), ab) / l2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = geom::dist(p, lerp(prev, q, f));
            if d < best.0 {
                best = (d, ((k - 1) as f32 + f) / n as f32);
            }
            prev = q;
        }
        let mut t = best.1;
        let mut step = 0.5 / n as f32;
        let dist_t = |t: f32| geom::dist(p, self.point_t(t));
        let mut dt = dist_t(t);
        for _ in 0..12 {
            for cand in [(t - step).max(0.0), (t + step).min(1.0)] {
                let dc = dist_t(cand);
                if dc < dt {
                    dt = dc;
                    t = cand;
                }
            }
            step *= 0.5;
        }
        let q = self.point_t(t);
        (self.d_at_t_with(table, t), dt, q)
    }

    /// De Casteljau split at parameter `t`.
    pub fn split_t(&self, t: f32) -> (Curve, Curve) {
        let p = &self.p;
        let a = lerp(p[0], p[1], t);
        let b = lerp(p[1], p[2], t);
        let c = lerp(p[2], p[3], t);
        let d = lerp(a, b, t);
        let e = lerp(b, c, t);
        let f = lerp(d, e, t);
        (Curve::new(p[0], a, d, f), Curve::new(f, e, c, p[3]))
    }

    /// Split at distance `d`. Straight curves give straight halves.
    pub fn split_at(&self, d: f32) -> (Curve, Curve) {
        if self.is_straight() {
            let len = geom::dist(self.p[0], self.p[3]);
            let t = if len > 0.0 { (d / len).clamp(0.0, 1.0) } else { 0.0 };
            let m = lerp(self.p[0], self.p[3], t);
            return (Curve::straight(self.p[0], m), Curve::straight(m, self.p[3]));
        }
        self.split_t(self.t_at(d))
    }

    /// The part from `d0` to `d1` metres (`d0 < d1`).
    pub fn sub(&self, d0: f32, d1: f32) -> Curve {
        let len = self.length();
        let (d0, d1) = (d0.clamp(0.0, len), d1.clamp(0.0, len));
        if self.is_straight() {
            let t0 = if len > 0.0 { d0 / len } else { 0.0 };
            let t1 = if len > 0.0 { d1 / len } else { 0.0 };
            return Curve::straight(lerp(self.p[0], self.p[3], t0), lerp(self.p[0], self.p[3], t1));
        }
        let (t0, t1) = (self.t_at(d0), self.t_at(d1));
        let (_, rest) = self.split_t(t0);
        if t0 >= 1.0 {
            return Curve::straight(self.p[3], self.p[3]);
        }
        let (part, _) = rest.split_t(((t1 - t0) / (1.0 - t0)).clamp(0.0, 1.0));
        part
    }

    /// The same shape with its end points moved to `start` and `end`
    /// (handles move with their end point; straight curves stay straight).
    pub fn with_ends(&self, start: [f32; 2], end: [f32; 2]) -> Curve {
        if self.is_straight() {
            return Curve::straight(start, end);
        }
        let ds = geom::sub(start, self.p[0]);
        let de = geom::sub(end, self.p[3]);
        Curve::new(start, add(self.p[1], ds), add(self.p[2], de), end)
    }

    /// Axis-aligned bounds of the control polygon (contains the curve).
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let mut lo = self.p[0];
        let mut hi = self.p[0];
        for q in &self.p[1..] {
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
        (lo, hi)
    }

    /// Smallest radius of curvature and the distance where it occurs
    /// (`f32::INFINITY` for a straight curve).
    pub fn min_radius(&self) -> (f32, f32) {
        if self.is_straight() {
            return (f32::INFINITY, 0.0);
        }
        let table = self.arc_table();
        let n = 64;
        let mut best = (f32::INFINITY, 0.0f32);
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let d1 = self.deriv_t(t);
            let d2 = self.deriv2_t(t);
            let s = norm(d1);
            if s < 1e-4 {
                continue;
            }
            let k_abs = geom::cross(d1, d2).abs() / (s * s * s);
            let r = if k_abs > 1e-9 { 1.0 / k_abs } else { f32::INFINITY };
            if r < best.0 {
                best = (r, self.d_at_t_with(&table, t));
            }
        }
        best
    }

    /// Points along the curve no further than `tol` metres from it, start
    /// and end included (renderers mesh from these).
    pub fn flatten(&self, tol: f32) -> Vec<[f32; 2]> {
        self.flatten_t(tol).into_iter().map(|t| self.point_t(t)).collect()
    }

    /// Parameters of a flattening to tolerance `tol` (recursive
    /// subdivision until the handles lie within `tol` of the chord).
    fn flatten_t(&self, tol: f32) -> Vec<f32> {
        let mut out = vec![0.0];
        if self.is_straight() {
            out.push(1.0);
            return out;
        }
        fn rec(c: &Curve, t0: f32, t1: f32, tol: f32, depth: u32, out: &mut Vec<f32>) {
            let a = c.point_t(t0);
            let b = c.point_t(t1);
            let m = c.point_t((t0 + t1) * 0.5);
            let q1 = c.point_t(t0 + (t1 - t0) * 0.25);
            let q3 = c.point_t(t0 + (t1 - t0) * 0.75);
            let dev = |p: [f32; 2]| {
                let ab = geom::sub(b, a);
                let l = norm(ab);
                if l < 1e-6 {
                    geom::dist(p, a)
                } else {
                    geom::cross(ab, geom::sub(p, a)).abs() / l
                }
            };
            let flat = dev(m).max(dev(q1)).max(dev(q3)) <= tol;
            if flat || depth >= 10 {
                out.push(t1);
            } else {
                let tm = (t0 + t1) * 0.5;
                rec(c, t0, tm, tol, depth + 1, out);
                rec(c, tm, t1, tol, depth + 1, out);
            }
        }
        // At least four pieces so a loop-like curve is never read as flat.
        for k in 0..4 {
            rec(self, k as f32 / 4.0, (k + 1) as f32 / 4.0, tol.max(1e-3), 0, &mut out);
        }
        out
    }

    /// Flattening of the line `offset` metres to the right of the curve:
    /// `(d along the centre line, point)` pairs, start and end included.
    pub fn flatten_offset(&self, offset: f32, tol: f32) -> Vec<(f32, [f32; 2])> {
        let table = self.arc_table();
        let ts = if self.is_straight() {
            vec![0.0, 1.0]
        } else {
            // Offset lines bend more on the inside: tighten the tolerance.
            self.flatten_t(tol * 0.5)
        };
        ts.into_iter()
            .map(|t| {
                let p = self.point_t(t);
                let r = right_of(self.dir_at_t(t));
                (
                    self.d_at_t_with(&table, t),
                    [p[0] + r[0] * offset, p[1] + r[1] * offset],
                )
            })
            .collect()
    }

    /// Polyline with distances, for intersection tests.
    fn polyline(&self) -> Vec<(f32, [f32; 2])> {
        let table = self.arc_table();
        self.flatten_t(0.02)
            .into_iter()
            .map(|t| (self.d_at_t_with(&table, t), self.point_t(t)))
            .collect()
    }

    /// Points where this curve crosses `other`: `(d on self, d on other,
    /// point)`, sorted by `d` on self. Touching end points count.
    pub fn intersections(&self, other: &Curve) -> Vec<(f32, f32, [f32; 2])> {
        let (alo, ahi) = self.bounds();
        let (blo, bhi) = other.bounds();
        if alo[0] > bhi[0] + 1e-3 || blo[0] > ahi[0] + 1e-3 || alo[1] > bhi[1] + 1e-3 || blo[1] > ahi[1] + 1e-3 {
            return Vec::new();
        }
        let pa = self.polyline();
        let pb = other.polyline();
        let mut out: Vec<(f32, f32, [f32; 2])> = Vec::new();
        for wa in pa.windows(2) {
            let ((da0, a0), (da1, a1)) = (wa[0], wa[1]);
            for wb in pb.windows(2) {
                let ((db0, b0), (db1, b1)) = (wb[0], wb[1]);
                if let Some((s, t)) = segments_meet(a0, a1, b0, b1) {
                    let p = lerp(a0, a1, s);
                    let da = da0 + (da1 - da0) * s;
                    let db = db0 + (db1 - db0) * t;
                    if !out.iter().any(|o| geom::dist(o.2, p) < 0.05) {
                        out.push((da, db, p));
                    }
                }
            }
        }
        out.sort_by(|x, y| x.0.total_cmp(&y.0));
        out
    }

    /// A point where the curve crosses itself, if any.
    pub fn self_intersection(&self) -> Option<[f32; 2]> {
        if self.is_straight() {
            return None;
        }
        let pl = self.polyline();
        for i in 0..pl.len().saturating_sub(1) {
            for j in i + 2..pl.len().saturating_sub(1) {
                if let Some((s, _)) = segments_meet(pl[i].1, pl[i + 1].1, pl[j].1, pl[j + 1].1) {
                    return Some(lerp(pl[i].1, pl[i + 1].1, s));
                }
            }
        }
        None
    }

    /// Largest angle the direction turns through along the curve, radians
    /// (sum of the turning between samples; 0 for straight curves).
    pub fn total_turn(&self) -> f32 {
        if self.is_straight() {
            return 0.0;
        }
        let n = 32;
        let mut prev = self.dir_at_t(0.0);
        let mut sum = 0.0;
        for k in 1..=n {
            let d = self.dir_at_t(k as f32 / n as f32);
            sum += angle_between(prev, d).abs();
            prev = d;
        }
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn straight_is_exact() {
        let c = Curve::straight([0.0, 0.0], [30.0, 40.0]);
        assert!(c.is_straight());
        assert_eq!(c.length(), 50.0);
        let p = c.pose_at(25.0, 0.0);
        assert!(close(p.pos[0], 15.0, 1e-4) && close(p.pos[1], 20.0, 1e-4));
        let r = c.pose_at(0.0, 2.0);
        // Right of (0.6, 0.8) is (-0.8, 0.6).
        assert!(close(r.pos[0], -1.6, 1e-4) && close(r.pos[1], 1.2, 1e-4));
        assert!(c.min_radius().0.is_infinite());
    }

    #[test]
    fn quarter_circle_fit() {
        // Radius 50 quarter circle from (0,0) heading +x to (50,50) heading +y.
        let c = Curve::from_tangents([0.0, 0.0], [1.0, 0.0], [50.0, 50.0], [0.0, 1.0]).unwrap();
        let want = std::f32::consts::FRAC_PI_2 * 50.0;
        assert!(close(c.length(), want, 0.2), "{}", c.length());
        let (r, _) = c.min_radius();
        assert!(r > 45.0 && r < 55.0, "{r}");
        let mid = c.pose_at(c.length() * 0.5, 0.0).pos;
        let dc = geom::dist(mid, [0.0, 50.0]);
        assert!(close(dc, 50.0, 0.1), "{dc}");
        assert_eq!(
            Curve::through_corner([0.0, 0.0], [50.0, 0.0], [50.0, 50.0]),
            Curve::from_tangents([0.0, 0.0], [1.0, 0.0], [50.0, 50.0], [0.0, 1.0]).unwrap()
        );
    }

    #[test]
    fn continuing_reaches_its_end_tangentially() {
        let c = Curve::continuing([0.0, 0.0], [1.0, 0.0], [40.0, 20.0]);
        assert_eq!(c.end(), [40.0, 20.0]);
        let d = c.start_dir();
        assert!(close(d[0], 1.0, 1e-5) && close(d[1], 0.0, 1e-5));
        let s = Curve::continuing([0.0, 0.0], [1.0, 0.0], [40.0, 0.0]);
        assert!(s.is_straight());
    }

    #[test]
    fn split_nearest_and_intersections() {
        let c = Curve::from_tangents([0.0, 0.0], [1.0, 0.0], [50.0, 50.0], [0.0, 1.0]).unwrap();
        let l = c.length();
        let (a, b) = c.split_at(l * 0.3);
        assert!(close(a.length() + b.length(), l, 0.05));
        assert_eq!(a.end(), b.start());
        let p = c.pose_at(20.0, 3.0).pos;
        let (d, dist, _) = c.nearest(p);
        assert!(close(d, 20.0, 0.1) && close(dist, 3.0, 0.05), "{d} {dist}");
        let line = Curve::straight([25.0, -10.0], [25.0, 60.0]);
        let x = c.intersections(&line);
        assert_eq!(x.len(), 1);
        let (dc, dl, pt) = x[0];
        assert!(close(pt[0], 25.0, 0.01));
        assert!(close(geom::dist(c.pose_at(dc, 0.0).pos, pt), 0.0, 0.1));
        assert!(close(geom::dist(line.pose_at(dl, 0.0).pos, pt), 0.0, 0.01));
        assert!(c.self_intersection().is_none());
        let looped = Curve::new([0.0, 0.0], [100.0, 100.0], [100.0, -100.0], [0.0, 0.0]);
        assert!(looped.self_intersection().is_some() || looped.length() > 0.0);
        let sub = c.sub(10.0, 30.0);
        assert!(close(sub.length(), 20.0, 0.1));
    }

    #[test]
    fn flatten_stays_close() {
        let c = Curve::from_tangents([0.0, 0.0], [1.0, 0.0], [50.0, 50.0], [0.0, 1.0]).unwrap();
        let pts = c.flatten(0.05);
        assert!(pts.len() > 4);
        for w in pts.windows(2) {
            let m = lerp(w[0], w[1], 0.5);
            assert!(c.nearest(m).1 < 0.06);
        }
        assert_eq!(*pts.last().unwrap(), c.end());
    }
}
