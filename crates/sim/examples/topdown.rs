//! Top-down debug renders of the simulation (no Bevy): roads with lanes,
//! stop lines coloured by the signal aspect, cars in their lanes and on
//! their junction curves, building lots with their front side.
//!
//! ```text
//! cargo run -p citysim_sim --release --example topdown -- signal out.png
//! cargo run -p citysim_sim --release --example topdown -- lots out.png
//! cargo run -p citysim_sim --release --example topdown -- video out.mp4 [seconds]
//! cargo run -p citysim_sim --release --example topdown -- day out.mp4 [seconds]
//! ```
//! `day`: a town through one whole day, cars coloured by trip purpose.
//! PNG and MP4 are written through `ffmpeg` (must be on the PATH).

use std::io::Write;
use std::process::{Command as Proc, Stdio};

use citysim_sim::junction::{self, Aspect, JunctionControl};
use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;

// ------------------------------------------------------------- canvas ----

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

type Rgb = [u8; 3];

impl Canvas {
    fn new(w: usize, h: usize, bg: Rgb) -> Canvas {
        let mut px = vec![0; w * h * 3];
        for p in px.chunks_mut(3) {
            p.copy_from_slice(&bg);
        }
        Canvas { w, h, px }
    }

    fn put(&mut self, x: i32, y: i32, c: Rgb) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            self.px[i..i + 3].copy_from_slice(&c);
        }
    }

    fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            for (k, &ck) in c.iter().enumerate() {
                self.px[i + k] = (self.px[i + k] as f32 * (1.0 - a) + ck as f32 * a) as u8;
            }
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb) {
        for yy in y.round() as i32..(y + h).round() as i32 {
            for xx in x.round() as i32..(x + w).round() as i32 {
                self.put(xx, yy, c);
            }
        }
    }

    /// Filled convex polygon (screen pixels).
    fn poly(&mut self, pts: &[[f32; 2]], c: Rgb, alpha: f32) {
        let ymin = pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as i32;
        let ymax = pts
            .iter()
            .map(|p| p[1])
            .fold(f32::MIN, f32::max)
            .ceil()
            .min(self.h as f32) as i32;
        for y in ymin..ymax {
            let yc = y as f32 + 0.5;
            let (mut x0, mut x1) = (f32::MAX, f32::MIN);
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                if (a[1] <= yc && b[1] > yc) || (b[1] <= yc && a[1] > yc) {
                    let t = (yc - a[1]) / (b[1] - a[1]);
                    let x = a[0] + (b[0] - a[0]) * t;
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                }
            }
            if x0 <= x1 {
                for x in x0.round() as i32..x1.round() as i32 {
                    if alpha >= 1.0 {
                        self.put(x, y, c);
                    } else {
                        self.blend(x, y, c, alpha);
                    }
                }
            }
        }
    }

    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: Rgb) {
        for y in (cy - r).floor() as i32..=(cy + r).ceil() as i32 {
            for x in (cx - r).floor() as i32..=(cx + r).ceil() as i32 {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                if dx * dx + dy * dy <= r * r {
                    self.put(x, y, c);
                }
            }
        }
    }

    fn text(&mut self, x: i32, y: i32, s: &str, scale: i32, c: Rgb) {
        let mut cx = x;
        for ch in s.chars() {
            let g = glyph(ch.to_ascii_uppercase());
            for (row, bits) in g.iter().enumerate() {
                for col in 0..5 {
                    if bits >> (4 - col) & 1 == 1 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                self.put(cx + col * scale + dx, y + row as i32 * scale + dy, c);
                            }
                        }
                    }
                }
            }
            cx += 6 * scale;
        }
    }

    fn blit(&mut self, other: &Canvas, ox: usize, oy: usize) {
        for y in 0..other.h {
            for x in 0..other.w {
                let i = (y * other.w + x) * 3;
                self.put(
                    (ox + x) as i32,
                    (oy + y) as i32,
                    [other.px[i], other.px[i + 1], other.px[i + 2]],
                );
            }
        }
    }
}

/// 5x7 bitmap font (rows top to bottom, 5 bits each).
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 2, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 17, 25, 21, 19, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [31, 2, 4, 2, 1, 17, 14],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [6, 8, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 2, 12],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ',' => [0, 0, 0, 0, 12, 4, 8],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        '%' => [24, 25, 2, 4, 8, 19, 3],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '=' => [0, 0, 31, 0, 31, 0, 0],
        '>' => [8, 4, 2, 1, 2, 4, 8],
        '<' => [2, 4, 8, 16, 8, 4, 2],
        _ => [0; 7],
    }
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}

// --------------------------------------------------------------- view ----

/// World window -> pixels.
#[derive(Clone, Copy)]
struct View {
    /// World metres at the canvas's top-left corner.
    x0: f32,
    y0: f32,
    /// Pixels per metre.
    k: f32,
}

impl View {
    fn px(&self, p: [f32; 2]) -> [f32; 2] {
        [(p[0] - self.x0) * self.k, (p[1] - self.y0) * self.k]
    }

    /// Centre `c` (metres) on a canvas of `w x h` pixels at `k` px/m.
    fn centred(c: [f32; 2], w: usize, h: usize, k: f32) -> View {
        View {
            x0: c[0] - w as f32 / (2.0 * k),
            y0: c[1] - h as f32 / (2.0 * k),
            k,
        }
    }
}

const GRASS: Rgb = [58, 74, 56];
const ASPHALT: Rgb = [62, 64, 70];
const KERB: Rgb = [140, 140, 134];
const MARK: Rgb = [212, 212, 204];
const YELLOW_MARK: Rgb = [214, 180, 60];
const MEDIAN: Rgb = [92, 128, 70];

fn aspect_rgb(a: Aspect) -> Rgb {
    match a {
        Aspect::Green => [60, 220, 90],
        Aspect::Yellow => [250, 200, 40],
        Aspect::Red => [235, 55, 50],
    }
}

fn zone_rgb(z: Zone) -> Rgb {
    match z {
        Zone::Residential => [96, 176, 104],
        Zone::Commercial => [92, 140, 220],
        Zone::Industrial => [214, 170, 70],
    }
}

/// Quad of a strip from `a` to `b` with half width `hw` (metres).
fn strip(v: &View, a: [f32; 2], b: [f32; 2], hw: f32, off: f32) -> [[f32; 2]; 4] {
    let d = geom::sub(b, a);
    let l = geom::dot(d, d).sqrt().max(1e-6);
    let n = [-d[1] / l, d[0] / l];
    let p = |q: [f32; 2], s: f32| v.px([q[0] + n[0] * (off + s), q[1] + n[1] * (off + s)]);
    [p(a, -hw), p(b, -hw), p(b, hw), p(a, hw)]
}

fn draw_world(c: &mut Canvas, w: &World, v: &View, show_zones: bool) {
    draw_world_opts(c, w, v, show_zones, true);
}

/// Quads of a polyline strip `off - hw .. off + hw` to the right of a
/// curve (pixels).
fn curve_strip(v: &View, cv: &Curve, d0: f32, d1: f32, hw: f32, off: f32) -> Vec<[[f32; 2]; 4]> {
    if d1 - d0 < 0.05 {
        return Vec::new();
    }
    let sub = cv.sub(d0, d1);
    let a = sub.flatten_offset(off - hw, 0.08);
    let b = sub.flatten_offset(off + hw, 0.08);
    let n = a.len().min(b.len());
    let mut out = Vec::new();
    if a.len() == b.len() {
        for k in 0..n.saturating_sub(1) {
            out.push([v.px(a[k].1), v.px(a[k + 1].1), v.px(b[k + 1].1), v.px(b[k].1)]);
        }
    } else {
        // Different sampling: resample both by distance.
        let m = ((d1 - d0) / 0.6).ceil().max(1.0) as usize;
        let at = |t: f32, o: f32| sub.pose_at(t, o).pos;
        let l = sub.length();
        for k in 0..m {
            let (t0, t1) = (l * k as f32 / m as f32, l * (k + 1) as f32 / m as f32);
            out.push([
                v.px(at(t0, off - hw)),
                v.px(at(t1, off - hw)),
                v.px(at(t1, off + hw)),
                v.px(at(t0, off + hw)),
            ]);
        }
    }
    out
}

fn fill_strip(c: &mut Canvas, quads: Vec<[[f32; 2]; 4]>, col: Rgb) {
    for q in quads {
        c.poly(&q, col, 1.0);
    }
}

/// Dashes along a curve, `dash` long every `every` metres.
#[allow(clippy::too_many_arguments)]
fn dashes(c: &mut Canvas, v: &View, cv: &Curve, lo: f32, hi: f32, off: f32, hw: f32, dash: f32, every: f32, col: Rgb) {
    let mut t = lo;
    while t < hi {
        let e = (t + dash).min(hi);
        fill_strip(c, curve_strip(v, cv, t, e, hw, off), col);
        t += every;
    }
}

fn in_view(v: &View, c: &Canvas, p: [f32; 2], margin: f32) -> bool {
    let q = v.px(p);
    q[0] > -margin && q[1] > -margin && q[0] < c.w as f32 + margin && q[1] < c.h as f32 + margin
}

fn draw_world_opts(c: &mut Canvas, w: &World, v: &View, show_zones: bool, cars: bool) {
    let margin = 40.0 * v.k;
    // Zones (usable, free cells of every lattice).
    if show_zones {
        for (cr, cd) in w.zoning.cells() {
            let (Some(z), true) = (cd.zone, cd.free()) else {
                continue;
            };
            let Some(q) = w.zoning.cell_corners(cr) else { continue };
            if !in_view(v, c, q[0], margin) {
                continue;
            }
            let ctr = [(q[0][0] + q[2][0]) * 0.5, (q[0][1] + q[2][1]) * 0.5];
            let shrink = |p: [f32; 2]| v.px([p[0] + (ctr[0] - p[0]) * 0.12, p[1] + (ctr[1] - p[1]) * 0.12]);
            c.poly(&q.map(shrink), zone_rgb(z), 0.3);
        }
    }
    for b in w.buildings.values() {
        if !in_view(v, c, b.center(), margin) {
            let _ = 0;
            continue;
        }
        let q = b.corners();
        let ctr = b.center();
        let inset = |p: [f32; 2], k: f32| {
            let d = geom::sub(ctr, p);
            let l = geom::dot(d, d).sqrt().max(1e-6);
            [p[0] + d[0] / l * k, p[1] + d[1] / l * k]
        };
        let qi = q.map(|p| inset(p, 1.2));
        let base = zone_rgb(b.kind);
        let col = mix(base, [30, 30, 30], 0.35 - 0.12 * b.level as f32);
        c.poly(&qi.map(|p| v.px(p)), col, 1.0);
        // Front side: a bright band along the front edge.
        let f = b.front_dir();
        let back = |p: [f32; 2]| [p[0] - f[0] * 1.6, p[1] - f[1] * 1.6];
        c.poly(
            &[v.px(qi[0]), v.px(qi[1]), v.px(back(qi[1])), v.px(back(qi[0]))],
            [245, 240, 225],
            1.0,
        );
        for k in 0..4 {
            line(c, v.px(qi[k]), v.px(qi[(k + 1) % 4]), [24, 26, 28]);
        }
    }
    // Road bodies: kerbs, then asphalt, then junction surfaces.
    let segs: Vec<(SegmentId, &RoadSegment)> = w
        .roads
        .segments
        .iter()
        .filter(|(_, s)| {
            let (lo, hi) = s.curve.bounds();
            in_view(v, c, lo, margin + (hi[0] - lo[0] + hi[1] - lo[1]) * v.k)
        })
        .collect();
    let shapes: Vec<junction::JunctionShape> = w
        .roads
        .nodes
        .iter()
        .filter(|(_, n)| in_view(v, c, n.pos, margin))
        .filter_map(|(id, _)| junction::junction_shape(w, id, 0.6))
        .collect();
    let fan = |c: &mut Canvas, sh: &junction::JunctionShape, shrink: f32, col: Rgb| {
        let o = &sh.outline;
        for k in 0..o.len() {
            let (p, q) = (o[k], o[(k + 1) % o.len()]);
            let pull = |x: [f32; 2]| {
                let d = geom::sub(sh.pos, x);
                let l = geom::dot(d, d).sqrt().max(1e-6);
                [x[0] + d[0] / l * shrink.min(l), x[1] + d[1] / l * shrink.min(l)]
            };
            c.poly(&[v.px(sh.pos), v.px(pull(p)), v.px(pull(q))], col, 1.0);
        }
    };
    for (_, seg) in &segs {
        fill_strip(
            c,
            curve_strip(v, &seg.curve, 0.0, seg.length, seg.kind.half_width_m() + 0.6, 0.0),
            KERB,
        );
    }
    for sh in &shapes {
        fan(c, sh, 0.0, KERB);
    }
    for (_, seg) in &segs {
        fill_strip(
            c,
            curve_strip(v, &seg.curve, 0.0, seg.length, seg.kind.half_width_m(), 0.0),
            ASPHALT,
        );
    }
    for sh in &shapes {
        fan(c, sh, 0.6, ASPHALT);
    }
    // Nodes without a box (smooth continuations, dead ends): round them off.
    for (id, n) in w.roads.nodes.iter() {
        if !in_view(v, c, n.pos, margin) || shapes.iter().any(|s| s.node == id) {
            continue;
        }
        let r = n
            .segments
            .iter()
            .map(|&s| w.roads.segments[s].kind.half_width_m())
            .fold(0.0, f32::max);
        let p = v.px(n.pos);
        c.disc(p[0], p[1], r * v.k, ASPHALT);
    }
    // Lane markings on the drivable stretch.
    for &(s, seg) in &segs {
        let Some((lo, hi)) = drive::drivable_range(w, s, true) else {
            continue;
        };
        if hi - lo < 1.0 {
            continue;
        }
        let cv = &seg.curve;
        if seg.kind == RoadKind::Avenue {
            // The median, cut into turn pockets before junctions.
            let len = seg.length;
            let pf = junction::turn_pocket(w, s, true).map(|p| p.from_a(len));
            let pb = junction::turn_pocket(w, s, false).map(|p| p.from_a(len));
            let m0 = pb.map_or(lo, |p| p.1 + 0.3).max(lo);
            let m1 = pf.map_or(hi, |p| p.0 - 0.3).min(hi);
            let hw = seg.kind.median_half_width_m();
            if m1 > m0 {
                fill_strip(c, curve_strip(v, cv, m0, m1, hw, 0.0), MEDIAN);
            }
            for (p, side) in [(pf, 1.0f32), (pb, -1.0)] {
                if let Some((a, b)) = p {
                    fill_strip(c, curve_strip(v, cv, a, b, 0.08, side * hw), MARK);
                    fill_strip(c, curve_strip(v, cv, a, b, 0.08, -side * hw), YELLOW_MARK);
                    let stop = if side > 0.0 { b - 0.3 } else { a + 0.3 };
                    fill_strip(c, curve_strip(v, cv, stop - 0.2, stop + 0.2, hw, 0.0), MARK);
                }
            }
            let k = seg.kind;
            let off = (k.lane_offset_m(0) + k.lane_offset_m(1)) * 0.5;
            for side in [1.0, -1.0] {
                dashes(c, v, cv, lo, hi, side * off, 0.12, 3.0, 7.0, MARK);
            }
        } else if seg.kind == RoadKind::Highway {
            fill_strip(c, curve_strip(v, cv, lo, hi, 0.18, 0.35), YELLOW_MARK);
            fill_strip(c, curve_strip(v, cv, lo, hi, 0.18, -0.35), YELLOW_MARK);
            let k = seg.kind;
            let off = (k.lane_offset_m(0) + k.lane_offset_m(1)) * 0.5;
            for side in [1.0, -1.0] {
                dashes(c, v, cv, lo, hi, side * off, 0.12, 3.0, 7.0, MARK);
            }
        } else {
            dashes(c, v, cv, lo, hi, 0.0, 0.12, 2.5, 5.0, MARK);
        }
    }
    // Stop lines, coloured by signal aspect per lane.
    for (n, _) in w.roads.nodes.iter() {
        let Some(j) = junction::junction(w, n) else { continue };
        if j.control == JunctionControl::Roundabout {
            continue;
        }
        let view = junction::junction_view(w, n);
        let npos = w.roads.nodes[n].pos;
        for (i, arm) in j.arms.iter().enumerate() {
            let seg = &w.roads.segments[arm.segment];
            let pose = seg.pose_dir(seg.a == n, arm.setback.min(seg.length), 0.0);
            let _ = npos;
            let arm_dir = pose.dir;
            let din = [-arm_dir[0], -arm_dir[1]];
            let r = junction::right_of(din);
            let base = pose.pos;
            for lane in 0..arm.lanes_in {
                let off = arm.kind.lane_offset_m(lane);
                let lanew = if arm.kind == RoadKind::Street { 2.6 } else { 1.45 };
                let p0 = [base[0] + r[0] * (off - lanew), base[1] + r[1] * (off - lanew)];
                let p1 = [base[0] + r[0] * (off + lanew), base[1] + r[1] * (off + lanew)];
                let col = match (&view, j.control) {
                    (Some(vw), JunctionControl::Signal) => {
                        let ap = &vw.approaches[i];
                        let a = if lane + 1 == arm.lanes_in && arm.lanes_in > 1 {
                            ap.left_aspect
                        } else {
                            ap.aspect
                        };
                        aspect_rgb(a.unwrap_or(Aspect::Red))
                    }
                    _ => {
                        if arm.major {
                            MARK
                        } else {
                            [250, 250, 250]
                        }
                    }
                };
                let wide = if j.control == JunctionControl::Signal {
                    0.7
                } else {
                    0.35
                };
                c.poly(&strip(v, p0, p1, wide, 0.0), col, 1.0);
                if j.control == JunctionControl::Priority && !arm.major {
                    // Give-way triangles.
                    let q = [base[0] + arm_dir[0] * 2.2, base[1] + arm_dir[1] * 2.2];
                    let q = [q[0] + r[0] * off, q[1] + r[1] * off];
                    let tip = v.px([q[0] - arm_dir[0] * 1.2, q[1] - arm_dir[1] * 1.2]);
                    let l = v.px([q[0] + r[0] * 0.8, q[1] + r[1] * 0.8]);
                    let rr = v.px([q[0] - r[0] * 0.8, q[1] - r[1] * 0.8]);
                    c.poly(&[tip, l, rr], [250, 250, 250], 1.0);
                }
            }
            // Signal head beside the kerb.
            if let (Some(vw), JunctionControl::Signal) = (&view, j.control) {
                let ap = &vw.approaches[i];
                let hw = arm.kind.half_width_m() + 1.6;
                let p = v.px([base[0] + r[0] * hw, base[1] + r[1] * hw]);
                c.disc(p[0], p[1], 1.5 * v.k, [20, 20, 20]);
                c.disc(p[0], p[1], 1.0 * v.k, aspect_rgb(ap.aspect.unwrap_or(Aspect::Red)));
            }
        }
    }
    if cars {
        draw_cars(c, w, v);
    }
}

fn draw_cars(c: &mut Canvas, w: &World, v: &View) {
    // Vehicles.
    for (id, car) in w.vehicles.iter() {
        let (p, d) = drive::vehicle_pose(w, car);
        let truck = w.traffic.trips.get(id).is_some_and(|t| t.purpose == TripPurpose::Goods);
        let (len, wid) = if truck { (6.5, 2.3) } else { (4.4, 1.9) };
        let r = junction::right_of(d);
        let corner = |a: f32, b: f32| v.px([p[0] + d[0] * a + r[0] * b, p[1] + d[1] * a + r[1] * b]);
        let body = if car.junction.is_some() {
            [255, 255, 255]
        } else if car.stopped() {
            [250, 120, 110]
        } else if truck {
            [230, 200, 120]
        } else {
            [200, 220, 255]
        };
        c.poly(
            &[
                corner(len / 2.0, -wid / 2.0),
                corner(len / 2.0, wid / 2.0),
                corner(-len / 2.0, wid / 2.0),
                corner(-len / 2.0, -wid / 2.0),
            ],
            body,
            1.0,
        );
        // Windscreen.
        c.poly(
            &[
                corner(len / 2.0 - 0.6, -wid / 2.0 + 0.25),
                corner(len / 2.0 - 0.6, wid / 2.0 - 0.25),
                corner(len / 2.0 - 1.4, wid / 2.0 - 0.25),
                corner(len / 2.0 - 1.4, -wid / 2.0 + 0.25),
            ],
            [40, 50, 70],
            1.0,
        );
        if car.braking || car.stopped() {
            for b in [-wid / 2.0 + 0.3, wid / 2.0 - 0.3] {
                let q = corner(-len / 2.0 + 0.2, b);
                c.disc(q[0], q[1], 0.35 * v.k, [255, 40, 30]);
            }
        }
    }
}

fn line(c: &mut Canvas, a: [f32; 2], b: [f32; 2], col: Rgb) {
    let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) as usize).max(1);
    for k in 0..=n {
        let t = k as f32 / n as f32;
        c.put(
            (a[0] + (b[0] - a[0]) * t) as i32,
            (a[1] + (b[1] - a[1]) * t) as i32,
            col,
        );
    }
}

// ------------------------------------------------------------- output ----

fn ffmpeg_png(c: &Canvas, path: &str) {
    let mut child = Proc::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s"])
        .arg(format!("{}x{}", c.w, c.h))
        .args(["-i", "-", "-frames:v", "1", path])
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    child.stdin.take().unwrap().write_all(&c.px).unwrap();
    assert!(child.wait().unwrap().success(), "ffmpeg failed");
}

// ------------------------------------------------------------- scenes ----

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

/// Two avenues crossing at (50, 128), streets around, with steady trips
/// from every arm through the junction.
fn signal_town() -> World {
    let mut w = World::new(11);
    scenarios::prebuilt(&mut w, |w| {
        scenarios::avenue(w, (11, 128), (95, 128));
        scenarios::avenue(w, (50, 100), (50, 156));
        scenarios::road(w, (30, 118), (70, 118));
        scenarios::road(w, (30, 138), (70, 138));
    });
    w
}

fn feed(w: &mut World, k: &mut usize) {
    let pts = [
        (c(14, 128), c(93, 128)),
        (c(93, 128), c(14, 128)),
        (c(50, 102), c(50, 154)),
        (c(50, 154), c(50, 102)),
        (c(14, 128), c(50, 154)),
        (c(93, 128), c(50, 102)),
        (c(50, 102), c(14, 128)),
        (c(50, 154), c(93, 128)),
        // Left turns.
        (c(14, 128), c(50, 102)),
        (c(93, 128), c(50, 154)),
        (c(14, 128), c(50, 102)),
        (c(93, 128), c(50, 154)),
    ];
    if w.time.tick.is_multiple_of(18) && w.traffic.pending.len() < 30 {
        let (a, b) = pts[*k % pts.len()];
        *k += 1;
        traffic::debug_request(w, a.center(), b.center());
    }
}

fn signal_sheet(out: &str) {
    let mut w = signal_town();
    let mut k = 0;
    // Warm up: queues and platoons form.
    for _ in 0..90 * TICKS_PER_SECOND {
        feed(&mut w, &mut k);
        w.step();
    }
    let node = junction::junction_at(&w, c(50, 128)).expect("junction");
    let (fw, fh, cols, rows) = (420usize, 420usize, 4usize, 2usize);
    let mut sheet = Canvas::new(
        cols * fw + (cols + 1) * 8,
        rows * fh + (rows + 1) * 8 + 70,
        [22, 24, 28],
    );
    sheet.text(
        12,
        14,
        "SIGNALLED AVENUE JUNCTION - ONE FULL CYCLE (TOP-DOWN, SIM ONLY)",
        3,
        [235, 235, 235],
    );
    sheet.text(
        12,
        44,
        "STOP LINES SHOW EACH LANE'S ASPECT. WHITE CARS ARE INSIDE THE BOX, RED CARS STAND.",
        2,
        [170, 175, 185],
    );
    // One frame at each stage change over a cycle, plus mid-green frames.
    let mut frames: Vec<(Canvas, String)> = Vec::new();
    let mut last = junction::junction_view(&w, node).unwrap().phase;
    let mut since = 0u32;
    let t0 = w.time.tick;
    while frames.len() < cols * rows && w.time.tick - t0 < 400 * TICKS_PER_SECOND as u64 {
        feed(&mut w, &mut k);
        w.step();
        since += 1;
        let vw = junction::junction_view(&w, node).unwrap();
        let mid = vw.phase.is_some_and(|p| p.1 == SignalStage::Green) && since == 4 * TICKS_PER_SECOND;
        if vw.phase != last || mid {
            if vw.phase != last {
                since = 0;
            }
            last = vw.phase;
            let mut f = Canvas::new(fw, fh, GRASS);
            let view = View::centred(w.roads.nodes[node].pos, fw, fh, 4.2);
            draw_world(&mut f, &w, &view, false);
            let (ph, st) = vw.phase.unwrap();
            let plan = &junction::junction(&w, node).unwrap().plan.as_ref().unwrap().phases[ph as usize];
            let arms: Vec<String> = plan
                .arms
                .iter()
                .map(|&a| {
                    let d = junction::junction(&w, node).unwrap().arms[a as usize].dir;
                    if d[0].abs() > d[1].abs() {
                        if d[0] > 0.0 { "E" } else { "W" }
                    } else if d[1] > 0.0 {
                        "S"
                    } else {
                        "N"
                    }
                    .to_string()
                })
                .collect();
            let label = format!(
                "T+{:>3}S PHASE {} {}{} {:?}",
                (w.time.tick - t0) / TICKS_PER_SECOND as u64,
                ph + 1,
                arms.join("+"),
                if plan.left_only { " LEFT" } else { "" },
                st
            );
            frames.push((f, label));
        }
    }
    for (i, (f, label)) in frames.iter().enumerate() {
        let (cx, cy) = (8 + (i % cols) * (fw + 8), 78 + (i / cols) * (fh + 8));
        sheet.blit(f, cx, cy);
        sheet.rect(cx as f32, (cy + fh - 26) as f32, fw as f32, 26.0, [16, 18, 20]);
        sheet.text(cx as i32 + 6, (cy + fh - 20) as i32, label, 2, [240, 240, 240]);
    }
    ffmpeg_png(&sheet, out);
    let j = junction::junction(&w, node).unwrap();
    eprintln!(
        "phases {} entered peak/min {} deadlocks {} stuck {}",
        j.plan.as_ref().unwrap().phases.len(),
        j.peak_minute_today,
        w.traffic.deadlocks_resolved,
        w.traffic.stuck_removed
    );
}

/// Blocks of the hierarchy player grown full, plus a diagonal street with
/// its own housing west of the first blocks.
fn lots(out: &str) {
    let mut w = World::new(5);
    let mut p = GridPlayer::hierarchy();
    while w.stats.population < 1_500 && w.time.day < 40 {
        p.act(&mut w);
        w.step();
    }
    scenarios::prebuilt(&mut w, |w| {
        scenarios::road(w, (15, 128), (4, 139));
        scenarios::road(w, (4, 139), (15, 150));
        scenarios::road(w, (15, 150), (20, 150));
        scenarios::zone(w, (2, 129), (18, 152), Zone::Residential);
    });
    while w.stats.population < 4_500 && w.time.day < 40 {
        p.act(&mut w);
        w.step();
    }
    let (wpx, hpx) = (1280usize, 900usize);
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    let view = View::centred([33.0 * CELL_SIZE, 131.0 * CELL_SIZE], wpx, hpx - 80, 2.9);
    let mut map = Canvas::new(wpx, hpx - 80, GRASS);
    draw_world_opts(&mut map, &w, &view, true, false);
    cv.blit(&map, 0, 80);
    let n = w.buildings.len();
    let deep = w.buildings.values().filter(|b| b.size().0 * b.size().1 == 2).count();
    let big = w.buildings.values().filter(|b| b.size().0 * b.size().1 == 4).count();
    let without = w
        .buildings
        .values()
        .filter(|b| !growth::has_frontage(&w, &b.footprint, b.facing))
        .count();
    let empty_zoned = w
        .zoning
        .cells()
        .filter(|(_, cd)| cd.zone.is_some() && cd.free())
        .count();
    cv.rect(0.0, 0.0, wpx as f32, 80.0, [22, 24, 28]);
    cv.text(
        12,
        12,
        "LOTS: EVERY BUILDING FRONTS A ROAD (BRIGHT EDGE = FRONT)",
        3,
        [235, 235, 235],
    );
    cv.text(
        12,
        46,
        &format!(
            "{n} BUILDINGS: {} 1X1, {deep} DEEP 1X2, {big} 2X2. WITHOUT FRONTAGE: {without}. ZONED CELLS LEFT EMPTY: {empty_zoned}"
            ,
            n - deep - big
        ),
        2,
        [170, 175, 185],
    );
    ffmpeg_png(&cv, out);
    if let Ok(dbg) = std::env::var("CITYSIM_TOPDOWN_DBG") {
        let mut d = Canvas::new(1280, 900, GRASS);
        let c: Vec<f32> = dbg.split(',').map(|x| x.parse().unwrap()).collect();
        let view = View::centred([c[0] * CELL_SIZE, c[1] * CELL_SIZE], 1280, 900, c[2]);
        draw_world(&mut d, &w, &view, false);
        ffmpeg_png(&d, "/tmp/dbg.png");
        if let Some(n) = junction::junction_at(&w, Cell::new(c[0] as i32, c[1] as i32)) {
            let j = junction::junction(&w, n).unwrap();
            eprintln!(
                "control {:?} signal {:?} plan {:?}",
                j.control,
                j.signal,
                j.plan.as_ref().map(|p| p
                    .phases
                    .iter()
                    .map(|ph| (ph.arms.clone(), ph.left_only))
                    .collect::<Vec<_>>())
            );
            for (i, a) in j.arms.iter().enumerate() {
                eprintln!(
                    "arm {i} dir {:?} kind {:?} sb {:.1} lanes {} q {}",
                    a.dir, a.kind, a.setback, a.lanes_in, j.queue[i]
                );
            }
            for (id, v) in w.vehicles.iter() {
                if let Some(jm) = v.junction.filter(|jm| jm.node == n) {
                    let m = &j.movements[jm.movement as usize];
                    eprintln!(
                        "box {id:?} {}->{} l{}->{} {:?} d {:.1}/{:.1} speed {:.2} wait {:?}",
                        m.from_arm,
                        m.to_arm,
                        m.from_lane,
                        m.to_lane,
                        m.turn,
                        jm.distance,
                        jm.length,
                        v.speed,
                        w.traffic.trips.get(id).map(|t| t.wait_ticks)
                    );
                }
            }
            eprintln!(
                "deadlocks {} stuck {} denials {:?}",
                w.traffic.deadlocks_resolved,
                w.traffic.stuck_removed,
                traffic::debug_denials(&w)
            );
        }
    }
    eprintln!("buildings {n} deep {deep} big {big} without frontage {without} empty zoned {empty_zoned}");
    eprintln!(
        "{:?} {:?} money {} {:?} commute {}",
        w.stats, w.demand, w.economy.money, w.economy.fiscal, w.traffic.avg_commute_s
    );
}

/// A city built as a hierarchy (avenues + streets) by the scripted player,
/// at the morning rush: the camera rides along the main avenue.
fn video(out: &str, seconds: u32) {
    let mut w = World::new(7);
    let mut p = GridPlayer::hierarchy();
    let pop = std::env::var("CITYSIM_TOPDOWN_POP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6000u32);
    while w.stats.population < pop && w.time.day < 60 {
        p.act(&mut w);
        w.step();
    }
    // Next morning, 07:30.
    let target = w.time.day as u64 * balance::TICKS_PER_DAY as u64 + (7.5 * balance::TICKS_PER_HOUR as f32) as u64
        - balance::START_TICK_OF_DAY as u64;
    while w.time.tick < target {
        p.act(&mut w);
        w.step();
    }
    let (wpx, hpx) = (1280usize, 720usize);
    let fps = 30;
    let mut child = Proc::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s"])
        .arg(format!("{wpx}x{hpx}"))
        .args([
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "20",
        ])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut pipe = child.stdin.take().unwrap();
    let frames = seconds * fps;
    // First half: hover over the busiest signalled avenue junction; second
    // half: glide along the avenue.
    let busiest = junction::junction_nodes(&w)
        .filter(|&n| junction::junction_control(&w, n) == Some(JunctionControl::Signal))
        .max_by_key(|&n| junction::junction(&w, n).map_or(0, |j| j.peak_minute_today))
        .map(|n| w.roads.nodes[n].pos)
        .unwrap_or([50.0 * CELL_SIZE, 128.0 * CELL_SIZE]);
    for f in 0..frames {
        // Real time: 20 ticks per second of video over 30 frames.
        // 30 frames per second at 2x: 40 ticks per second of video.
        let ticks = if f % 3 == 2 { 2 } else { 1 };
        for _ in 0..ticks {
            p.act(&mut w);
            w.step();
        }
        let half = frames / 2;
        let (centre, k) = if f < half {
            (busiest, 4.0)
        } else {
            let t = (f - half) as f32 / half as f32;
            let x = 22.0 * CELL_SIZE + t * 70.0 * CELL_SIZE;
            ([x, 128.5 * CELL_SIZE], 3.0)
        };
        let mut cv = Canvas::new(wpx, hpx, GRASS);
        let view = View::centred(centre, wpx, hpx, k);
        draw_world(&mut cv, &w, &view, false);
        cv.rect(0.0, 0.0, wpx as f32, 34.0, [22, 24, 28]);
        cv.text(
            10,
            10,
            &format!(
                "2X  MORNING RUSH {:02}:{:02}  POP {}  CARS {}  AVG {:.0} KM/H  JAMMED {:.0}%  ({})",
                w.time.hour() as u32,
                (w.time.hour().fract() * 60.0) as u32,
                w.stats.population,
                w.stats.vehicles_on_road,
                w.stats.avg_speed * 3.6,
                w.stats.congestion_share * 100.0,
                if f < half {
                    "SIGNALLED AVENUE JUNCTION"
                } else {
                    "ALONG THE AVENUE, COORDINATED SIGNALS"
                }
            ),
            2,
            [235, 235, 235],
        );
        pipe.write_all(&cv.px).unwrap();
    }
    drop(pipe);
    assert!(child.wait().unwrap().success());
}

/// Trip purpose colours (the charts' categorical order).
const PURPOSE_RGB: [Rgb; 7] = [
    [0x2a, 0x78, 0xd6],
    [0xeb, 0x68, 0x34],
    [0x1b, 0xaf, 0x7a],
    [0xed, 0xa1, 0x00],
    [0xe8, 0x7b, 0xa4],
    [0x00, 0xa3, 0x00],
    [0x8a, 0x78, 0xf0],
];
const PURPOSE_LABEL: [&str; 7] = [
    "TO WORK",
    "HOME FROM WORK",
    "MOVING IN",
    "MOVING OUT",
    "SHOPPING",
    "GOODS",
    "LEISURE",
];

/// Cars as small boxes coloured by trip purpose (trucks longer).
fn draw_cars_by_purpose(c: &mut Canvas, w: &World, v: &View) {
    for (id, car) in w.vehicles.iter() {
        let Some(t) = w.traffic.trips.get(id) else { continue };
        let (p, d) = drive::vehicle_pose(w, car);
        let truck = t.purpose == TripPurpose::Goods;
        let (len, wid) = if truck { (7.5, 2.8) } else { (5.0, 2.6) };
        let r = junction::right_of(d);
        let corner = |a: f32, b: f32| v.px([p[0] + d[0] * a + r[0] * b, p[1] + d[1] * a + r[1] * b]);
        c.poly(
            &[
                corner(len / 2.0, -wid / 2.0),
                corner(len / 2.0, wid / 2.0),
                corner(-len / 2.0, wid / 2.0),
                corner(-len / 2.0, -wid / 2.0),
            ],
            PURPOSE_RGB[t.purpose.index()],
            1.0,
        );
    }
}

/// A town (the grid player, ~5,000 residents) through one whole day in
/// `seconds` of video: cars coloured by trip purpose, the clock, and what
/// the citizens are doing.
fn day(out: &str, seconds: u32) {
    let mut w = World::new(7);
    // `CITYSIM_TOPDOWN_LAYOUT=hierarchy`: the avenue city.
    let mut p = if std::env::var("CITYSIM_TOPDOWN_LAYOUT").as_deref() == Ok("hierarchy") {
        GridPlayer::hierarchy()
    } else {
        GridPlayer::new(&Zone::ALL)
    };
    let pop = std::env::var("CITYSIM_TOPDOWN_POP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000u32);
    while w.stats.population < pop && w.time.day < 60 {
        p.act(&mut w);
        w.step();
    }
    while w.time.tick_of_day != 0 {
        p.act(&mut w);
        w.step();
    }
    // Frame the built-up area.
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for b in w.buildings.values() {
        let q = derived::rect_center(b.footprint);
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let (mut wpx, hpx) = (1280usize, 720usize);
    let mut centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0 + 10.0];
    let mut k = ((wpx as f32 - 40.0) / (hi[0] - lo[0] + 60.0)).min((hpx as f32 - 140.0) / (hi[1] - lo[1] + 60.0));
    // `CITYSIM_TOPDOWN_VIEW="width,x,y,scale"`: a fixed frame (the same for
    // two cities side by side).
    if let Ok(v) = std::env::var("CITYSIM_TOPDOWN_VIEW") {
        let n: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
        if n.len() == 4 {
            wpx = n[0] as usize;
            centre = [n[1], n[2]];
            k = n[3];
        }
    }
    let fps = 30;
    let mut child = Proc::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s"])
        .arg(format!("{wpx}x{hpx}"))
        .args([
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "20",
        ])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut pipe = child.stdin.take().unwrap();
    let frames = seconds * fps;
    let per_frame = balance::TICKS_PER_DAY as f32 / frames as f32;
    let mut done = 0f32;
    let view = View::centred(centre, wpx, hpx, k);
    for f in 0..frames {
        done += per_frame;
        while (w.time.tick_of_day as f32) < done.min(balance::TICKS_PER_DAY as f32 - 1.0) && f + 1 < frames {
            p.act(&mut w);
            w.step();
        }
        let mut cv = Canvas::new(wpx, hpx, GRASS);
        draw_world_opts(&mut cv, &w, &view, false, false);
        draw_cars_by_purpose(&mut cv, &w, &view);
        cv.rect(0.0, 0.0, wpx as f32, 34.0, [22, 24, 28]);
        let a = inspect::activity_shares(&w);
        cv.text(
            10,
            10,
            &format!(
                "DAY {}  {:02}:{:02}   POP {}   CARS {}   ASLEEP {:.0}%  HOME {:.0}%  WORK {:.0}%  SHOPS {:.0}%  OUT {:.0}%  TRAVEL {:.0}%",
                w.time.day,
                w.time.hour() as u32,
                (w.time.hour().fract() * 60.0) as u32,
                w.stats.population,
                w.stats.vehicles_on_road,
                a[0] * 100.0,
                a[1] * 100.0,
                a[2] * 100.0,
                a[3] * 100.0,
                a[4] * 100.0,
                a[5] * 100.0,
            ),
            2,
            [235, 235, 235],
        );
        // Legend.
        cv.rect(0.0, hpx as f32 - 30.0, wpx as f32, 30.0, [22, 24, 28]);
        let mut x = 12.0;
        for (i, label) in PURPOSE_LABEL.iter().enumerate() {
            if i == 3 {
                continue;
            }
            cv.rect(x, hpx as f32 - 21.0, 14.0, 12.0, PURPOSE_RGB[i]);
            cv.text(x as i32 + 20, hpx as i32 - 22, label, 2, [235, 235, 235]);
            x += 30.0 + label.len() as f32 * 12.0;
        }
        pipe.write_all(&cv.px).unwrap();
    }
    drop(pipe);
    assert!(child.wait().unwrap().success());
}

/// Frames of one spot of a scripted city, `every` ticks apart (debugging).
fn watch(out: &str, args: &[String]) {
    let which = args.first().map(String::as_str).unwrap_or("hierarchy");
    let pop: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let x: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(40.0);
    let y: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(123.0);
    let every: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut w = World::new(7);
    let mut p = match which {
        "streets" => GridPlayer::new(&Zone::ALL),
        "single_artery" => GridPlayer::single_artery(),
        _ => GridPlayer::hierarchy(),
    };
    while w.stats.population < pop && w.time.day < 60 {
        p.act(&mut w);
        w.step();
    }
    let target = w.time.day as u64 * balance::TICKS_PER_DAY as u64 + 2 * balance::TICKS_PER_HOUR as u64;
    while w.time.tick < target {
        p.act(&mut w);
        w.step();
    }
    let (fw, fh, cols, rows) = (420usize, 420usize, 4usize, 3usize);
    let mut sheet = Canvas::new(cols * fw + (cols + 1) * 6, rows * fh + (rows + 1) * 6, [22, 24, 28]);
    for i in 0..cols * rows {
        let mut f = Canvas::new(fw, fh, GRASS);
        let view = View::centred([x * CELL_SIZE + 4.0, y * CELL_SIZE + 4.0], fw, fh, 4.0);
        draw_world(&mut f, &w, &view, false);
        f.text(
            6,
            6,
            &format!("T+{}", i as u32 * every / TICKS_PER_SECOND),
            2,
            [255, 255, 255],
        );
        sheet.blit(&f, 6 + (i % cols) * (fw + 6), 6 + (i / cols) * (fh + 6));
        for _ in 0..every {
            p.act(&mut w);
            w.step();
        }
    }
    ffmpeg_png(&sheet, out);
}

/// A polyline of `width` metres (pixels from the view).
fn polyline(c: &mut Canvas, v: &View, pts: &[[f32; 2]], width: f32, col: Rgb) {
    for w in pts.windows(2) {
        c.poly(&strip(v, w[0], w[1], width * 0.5, 0.0), col, 1.0);
    }
}

fn outline(c: &mut Canvas, v: &View, pts: &[[f32; 2]], width: f32, col: Rgb) {
    let mut p: Vec<[f32; 2]> = pts.to_vec();
    if let Some(&f) = pts.first() {
        p.push(f);
    }
    polyline(c, v, &p, width, col);
}

fn turn_rgb(t: junction::Turn) -> Rgb {
    match t {
        junction::Turn::Straight => [120, 200, 255],
        junction::Turn::Right => [120, 230, 120],
        junction::Turn::Left => [250, 170, 60],
        junction::Turn::UTurn => [200, 120, 220],
    }
}

/// Lattice colours for cell plots.
fn lattice_rgb(l: LatticeId) -> Rgb {
    const P: [Rgb; 8] = [
        [96, 176, 104],
        [92, 140, 220],
        [214, 170, 70],
        [200, 110, 160],
        [80, 190, 190],
        [230, 120, 90],
        [160, 160, 230],
        [190, 200, 90],
    ];
    P[l.0 as usize % P.len()]
}

/// Cells of every lattice: usable ones in their lattice colour (bright
/// edge on the road side), blocked ones by why.
fn draw_cells(c: &mut Canvas, w: &World, v: &View, by_lattice: bool) {
    for (cr, cd) in w.zoning.cells() {
        let Some(q) = w.zoning.cell_corners(cr) else { continue };
        if !in_view(v, c, q[0], 60.0) {
            continue;
        }
        let ctr = [(q[0][0] + q[2][0]) * 0.5, (q[0][1] + q[2][1]) * 0.5];
        let shrink = |p: [f32; 2], k: f32| v.px([p[0] + (ctr[0] - p[0]) * k, p[1] + (ctr[1] - p[1]) * k]);
        let col = match cd.blocked {
            Some(Blocked::Road) => [90, 90, 96],
            Some(Blocked::Contested) => [235, 80, 60],
            Some(Blocked::Building) => [140, 60, 140],
            None if !cd.covered => [70, 70, 60],
            None => {
                if by_lattice {
                    lattice_rgb(cr.lattice)
                } else {
                    cd.zone.map_or([120, 150, 110], zone_rgb)
                }
            }
        };
        let alpha = if cd.blocked.is_some() { 0.55 } else { 0.8 };
        c.poly(&q.map(|p| shrink(p, 0.08)), col, alpha);
        // Two fronts (a shared corner cell): a white dot.
        if cd.fronts.count_ones() >= 2 && cd.usable() {
            let p = v.px(ctr);
            c.disc(p[0], p[1], 0.9 * v.k, [250, 250, 250]);
        }
    }
}

fn draw_blocks(c: &mut Canvas, w: &World, v: &View) {
    for b in &w.zoning.blocks {
        let Some(q) = w.zoning.block_corners(b) else { continue };
        if !in_view(v, c, q[0], 200.0) {
            continue;
        }
        outline(c, v, &q, 0.35, [250, 250, 250]);
    }
}

// ------------------------------------------------------------ zone bed ----

/// Cell coordinates (integer = centre) to metres.
fn zm(x: f32, y: f32) -> [f32; 2] {
    [(x + 0.5) * CELL_SIZE, (y + 0.5) * CELL_SIZE]
}

/// The zoning test bed of awkward roads (the same layout `scripts/zplay.py`
/// draws in the game): name, centre (cells) and view size (metres).
fn zone_bed(w: &mut World) -> Vec<(&'static str, [f32; 2], f32)> {
    let st = RoadKind::Street;
    let b = |w: &mut World, c: Curve, k: RoadKind| {
        if let Err(e) = w.apply(Command::BuildCurve { curve: c, kind: k }) {
            eprintln!("refused {:?}: {e}", c.start());
        }
    };
    let s = |a: (f32, f32), z: (f32, f32)| Curve::straight(zm(a.0, a.1), zm(z.0, z.1));
    let cv =
        |a: (f32, f32), c: (f32, f32), z: (f32, f32)| Curve::through_corner(zm(a.0, a.1), zm(c.0, c.1), zm(z.0, z.1));
    b(w, s((11.0, 128.0), (185.0, 128.0)), st);
    b(w, cv((16.0, 128.0), (16.0, 102.0), (42.0, 102.0)), st);
    b(w, cv((24.0, 128.0), (24.0, 131.0), (27.0, 131.0)), st);
    b(w, s((27.0, 131.0), (38.0, 131.0)), st);
    b(w, cv((56.0, 128.0), (56.0, 120.0), (62.0, 118.0)), st);
    b(w, cv((62.0, 118.0), (68.0, 116.0), (68.0, 106.0)), st);
    b(w, s((66.0, 128.0), (66.0, 134.0)), st);
    b(w, cv((66.0, 134.0), (75.0, 134.0), (75.0, 143.0)), st);
    b(w, cv((75.0, 143.0), (75.0, 152.0), (66.0, 152.0)), st);
    b(w, cv((66.0, 152.0), (57.0, 152.0), (57.0, 143.0)), st);
    b(w, cv((57.0, 143.0), (57.0, 134.0), (66.0, 134.0)), st);
    b(w, cv((86.0, 128.0), (86.0, 118.0), (95.0, 114.0)), st);
    b(w, s((90.0, 128.0), (90.0, 146.0)), st);
    b(w, s((100.0, 128.0), (100.0, 146.0)), st);
    b(w, s((90.0, 137.0), (100.0, 137.0)), st);
    b(w, s((90.0, 146.0), (100.0, 146.0)), st);
    b(w, cv((100.0, 137.0), (110.0, 137.0), (114.0, 148.0)), st);
    b(w, cv((108.0, 128.0), (108.0, 104.0), (134.0, 100.0)), RoadKind::Avenue);
    // A rotated grid of whole-cell blocks (as the grid tool draws it).
    let th = (13.0f32).to_radians();
    let (u, v) = ([libm::cosf(th), libm::sinf(th)], [-libm::sinf(th), libm::cosf(th)]);
    let o = zm(138.0, 132.0);
    let g = |i: f32, j: f32| [o[0] + (u[0] * i + v[0] * j) * 8.0, o[1] + (u[1] * i + v[1] * j) * 8.0];
    for j in [0.0, 8.0, 16.0, 24.0] {
        b(w, Curve::straight(g(0.0, j), g(24.0, j)), st);
    }
    for i in [0.0, 8.0, 16.0, 24.0] {
        b(w, Curve::straight(g(i, 0.0), g(i, 24.0)), st);
    }
    b(w, Curve::straight(zm(140.0, 128.0), g(0.0, 0.0)), st);
    // Rotated streets one by one, blocks not whole cells.
    let th = (-30.0f32).to_radians();
    let (u, v) = ([libm::cosf(th), libm::sinf(th)], [libm::sinf(th), -libm::cosf(th)]);
    let o = zm(146.0, 128.0);
    let g = |i: f32, j: f32| [o[0] + (u[0] * i + v[0] * j) * 8.0, o[1] + (u[1] * i + v[1] * j) * 8.0];
    for k in 0..3 {
        b(w, Curve::straight(g(0.0, 6.6 * k as f32), g(29.2, 6.6 * k as f32)), st);
    }
    for k in 0..5 {
        b(w, Curve::straight(g(7.3 * k as f32, 0.0), g(7.3 * k as f32, 13.2)), st);
    }
    vec![
        ("GENTLE CURVE", zm(28.0, 112.0), 300.0),
        ("TIGHT CURVE", zm(29.0, 133.0), 140.0),
        ("S-BEND", zm(62.0, 118.0), 220.0),
        ("RING", zm(66.0, 143.0), 220.0),
        ("CUL-DE-SAC", zm(89.0, 120.0), 160.0),
        ("GRID + CURVE", zm(101.0, 139.0), 240.0),
        ("CURVED AVENUE", zm(120.0, 113.0), 300.0),
        ("ROTATED GRID", zm(151.0, 146.0), 300.0),
        ("ROTATED STREETS", zm(160.0, 113.0), 300.0),
    ]
}

/// `zonebed out.png [days]`: the test bed zoned everywhere (homes; shops
/// and industry on some sites), grown for `days`, one panel per site:
/// cells by lattice above, zones and buildings below.
fn zone_bed_sheet(out: &str, days: u32) {
    let mut w = World::new(7);
    w.economy.money = i64::MAX / 4;
    let sites = zone_bed(&mut w);
    for (k, (_, c, size)) in sites.iter().enumerate() {
        let h = size * 0.5;
        let z = [
            Zone::Residential,
            Zone::Residential,
            Zone::Commercial,
            Zone::Residential,
            Zone::Residential,
            Zone::Commercial,
            Zone::Industrial,
            Zone::Residential,
            Zone::Residential,
        ][k];
        let _ = w.apply(Command::ZoneArea {
            quad: [
                [c[0] - h, c[1] - h],
                [c[0] + h, c[1] - h],
                [c[0] + h, c[1] + h],
                [c[0] - h, c[1] + h],
            ],
            zone: Some(z),
        });
    }
    for _ in 0..days * balance::TICKS_PER_DAY {
        w.step();
    }
    w.economy.money = 0;
    let (pw, ph) = (520usize, 520usize);
    let cols = 3;
    let rows = sites.len().div_ceil(cols);
    let mut cv = Canvas::new(pw * cols * 2, ph * rows + 64, GRASS);
    for (k, (name, c, size)) in sites.iter().enumerate() {
        let kpx = pw as f32 / size;
        let view = View::centred(*c, pw - 6, ph - 6, kpx);
        let mut a = Canvas::new(pw - 6, ph - 6, GRASS);
        draw_cells(&mut a, &w, &view, true);
        draw_world_opts(&mut a, &w, &view, false, false);
        a.text(6, 6, name, 2, [250, 250, 250]);
        let mut bm = Canvas::new(pw - 6, ph - 6, GRASS);
        draw_world_opts(&mut bm, &w, &view, true, false);
        let (x, y) = ((k % cols) * 2 * pw, (k / cols) * ph + 64);
        cv.blit(&a, x + 3, y + 3);
        cv.blit(&bm, x + pw + 3, y + 3);
    }
    let n = |f: &dyn Fn(&ZoneCell) -> bool| w.zoning.cells().filter(|(_, c)| f(c)).count();
    header(
        &mut cv,
        "ZONING TEST BED: CELLS BY LATTICE (RED CONTESTED, GREY UNDER ROAD) | ZONES AND BUILDINGS",
        &format!(
            "{} LATTICES, {} BLOCKS. CELLS {} USABLE, {} CONTESTED, {} UNDER ROADS. {} BUILDINGS, {} RESIDENTS AFTER {} DAYS",
            w.zoning.lattices().count(),
            w.zoning.blocks.len(),
            n(&|c| c.usable()),
            n(&|c| c.blocked == Some(Blocked::Contested)),
            n(&|c| c.blocked == Some(Blocked::Road)),
            w.buildings.len(),
            w.stats.population,
            days
        ),
    );
    ffmpeg_png(&cv, out);
}

/// A small world without the highway (for close-up sheets).
fn sheet_world() -> World {
    let mut w = World::new(3);
    w.economy.money = i64::MAX / 4;
    w
}

fn build(w: &mut World, c: Curve, k: RoadKind) {
    if let Err(e) = w.apply(Command::BuildCurve { curve: c, kind: k }) {
        panic!("refused {c:?}: {e} at {:?}", e.at());
    }
}

/// A curved avenue meeting curved and straight streets, lanes drawn, cars
/// driving.
fn curve_sheet(out: &str) {
    let mut w = World::new(11);
    scenarios::prebuilt(&mut w, |w| {
        scenarios::organic(w);
    });
    for _ in 0..(5 * balance::TICKS_PER_DAY + 8 * balance::TICKS_PER_HOUR - balance::START_TICK_OF_DAY) {
        w.step();
    }
    let (wpx, hpx) = (1600usize, 1000usize);
    let view = View::centred([560.0, 948.0], wpx, hpx - 64, 5.0);
    let mut map = Canvas::new(wpx, hpx - 64, GRASS);
    draw_world_opts(&mut map, &w, &view, true, true);
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    cv.blit(&map, 0, 64);
    let curved = w.roads.segments.values().filter(|s| !s.curve.is_straight()).count();
    let tightest = w
        .roads
        .segments
        .values()
        .map(|s| s.curve.min_radius().0)
        .fold(f32::INFINITY, f32::min);
    header(
        &mut cv,
        "CURVED AVENUE: LANES, KERBS AND CARS FOLLOW THE CURVE",
        &format!(
            "{} OF {} SEGMENTS CURVED, TIGHTEST RADIUS {:.0} M, {} CARS, 08:00",
            curved,
            w.roads.segments.len(),
            tightest,
            w.vehicles.len()
        ),
    );
    ffmpeg_png(&cv, out);
}

/// Junctions at odd angles: outlines, arm cut lines and every turn path.
fn junction_sheet(out: &str) {
    let mut w = sheet_world();
    let o = [600.0, 600.0];
    let at = |dx: f32, dy: f32| [o[0] + dx, o[1] + dy];
    // An avenue crossed by a street at 35 degrees, a curved street joining
    // it at 60, and a five-arm junction of streets.
    build(
        &mut w,
        Curve::straight(at(-160.0, 0.0), at(160.0, 0.0)),
        RoadKind::Avenue,
    );
    let d35 = [libm::cosf(35f32.to_radians()), libm::sinf(35f32.to_radians())];
    build(
        &mut w,
        Curve::straight(
            at(-90.0 * d35[0] - 40.0, -90.0 * d35[1]),
            at(90.0 * d35[0] - 40.0, 90.0 * d35[1]),
        ),
        RoadKind::Street,
    );
    build(
        &mut w,
        Curve::from_tangents(at(120.0, -110.0), [0.0, 1.0], at(80.0, 0.0), [-0.5, 0.866])
            .unwrap_or_else(|| Curve::through_corner(at(120.0, -110.0), at(120.0, -50.0), at(80.0, 0.0))),
        RoadKind::Street,
    );
    let hub = at(60.0, 120.0);
    for deg in [0.0f32, 80.0, 150.0, 215.0, 290.0] {
        let d = [libm::cosf(deg.to_radians()), libm::sinf(deg.to_radians())];
        build(
            &mut w,
            Curve::straight(hub, [hub[0] + d[0] * 70.0, hub[1] + d[1] * 70.0]),
            RoadKind::Street,
        );
    }
    traffic_sync(&mut w);
    let (wpx, hpx) = (1620usize, 760usize);
    let mut shapes = 0;
    let mut paths = 0;
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    let panels = [at(-40.0, 0.0), at(80.0, -6.0), hub];
    for (k, &centre) in panels.iter().enumerate() {
        let pw = wpx / 3;
        let view = View::centred(centre, pw - 6, hpx - 64, 6.5);
        let mut map = Canvas::new(pw - 6, hpx - 64, GRASS);
        draw_world_opts(&mut map, &w, &view, false, false);
        for (n, node) in w.roads.nodes.iter() {
            if geom::dist(node.pos, centre) > 30.0 {
                continue;
            }
            let Some(sh) = junction::junction_shape(&w, n, 0.0) else {
                continue;
            };
            shapes += 1;
            outline(&mut map, &view, &sh.outline, 0.2, [255, 255, 255]);
            for a in &sh.arms {
                polyline(&mut map, &view, &[a.left, a.right], 0.3, [255, 140, 40]);
            }
            if let Some(j) = w.traffic.junction_states.get(n).filter(|j| j.arms.len() >= 3) {
                for m in &j.movements {
                    if m.turn == junction::Turn::UTurn {
                        continue;
                    }
                    paths += 1;
                    polyline(&mut map, &view, &m.curve.flatten(0.03), 0.16, turn_rgb(m.turn));
                }
            }
        }
        cv.blit(&map, k * pw + 3, 64);
    }
    header(
        &mut cv,
        "JUNCTIONS AT ANY ANGLE: OUTLINE, ARM CUT LINES, TURN PATHS",
        &format!(
            "AVENUE X STREET AT 35 DEG, CURVED STREET T, 5 ARMS: {shapes} SURFACES, {paths} PATHS (BLUE STRAIGHT, GREEN RIGHT, ORANGE LEFT)"
        ),
    );
    ffmpeg_png(&cv, out);
}

fn traffic_sync(w: &mut World) {
    // One step builds junction states for the new roads.
    w.step();
}

/// Blocks along an S-curve: straight blocks along the kerbs, wedge gaps
/// outside each bend, overlaps trimmed inside, cells under the road
/// blocked.
fn blocks_sheet(out: &str) {
    let mut w = sheet_world();
    let a = [300.0, 700.0];
    let b = [560.0, 600.0];
    let c2 = [820.0, 560.0];
    build(
        &mut w,
        Curve::from_tangents(a, [1.0, 0.0], b, [0.894, -0.447]).expect("bend 1"),
        RoadKind::Street,
    );
    build(
        &mut w,
        Curve::from_tangents(b, [0.894, -0.447], c2, [1.0, 0.0]).expect("bend 2"),
        RoadKind::Street,
    );
    // A straight street crossing it (its blocks lose where they overlap).
    build(
        &mut w,
        Curve::straight([600.0, 500.0], [560.0, 760.0]),
        RoadKind::Street,
    );
    let (wpx, hpx) = (1600usize, 1000usize);
    let view = View::centred([560.0, 630.0], wpx, hpx - 64, 2.6);
    let mut map = Canvas::new(wpx, hpx - 64, GRASS);
    draw_cells(&mut map, &w, &view, true);
    draw_world_opts(&mut map, &w, &view, false, false);
    draw_blocks(&mut map, &w, &view);
    let n = |f: &dyn Fn(&ZoneCell) -> bool| w.zoning.cells().filter(|(_, c)| f(c)).count();
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    cv.blit(&map, 0, 64);
    header(
        &mut cv,
        "ZONE BLOCKS ALONG AN S-CURVE (WHITE: BLOCK OUTLINES, COLOUR: LATTICE)",
        &format!(
            "{} BLOCKS ON {} LATTICES. CELLS: {} USABLE, {} CONTESTED (RED), {} UNDER ROADS (GREY)",
            w.zoning.blocks.len(),
            w.zoning.lattices().count() - 1,
            n(&|c| c.usable()),
            n(&|c| c.blocked == Some(Blocked::Contested)),
            n(&|c| c.blocked == Some(Blocked::Road)),
        ),
    );
    ffmpeg_png(&cv, out);
}

/// A right-angle grid on the world lattice (corner cells shared by two
/// roads) next to a grid turned by 22 degrees that keeps its own lattice;
/// where they meet the older grid keeps the cells.
fn lattice_sheet(out: &str) {
    let mut w = sheet_world();
    let c = |x: i32, y: i32| Cell::new(x, y).center();
    for y in [60, 65, 70] {
        build(&mut w, Curve::straight(c(40, y), c(60, y)), RoadKind::Street);
    }
    for x in [40, 50, 60] {
        build(&mut w, Curve::straight(c(x, 60), c(x, 70)), RoadKind::Street);
    }
    let th = 22f32.to_radians();
    let (u, v2) = ([libm::cosf(th), libm::sinf(th)], [-libm::sinf(th), libm::cosf(th)]);
    let o = c(67, 57);
    let p = |i: f32, j: f32| [o[0] + u[0] * i + v2[0] * j, o[1] + u[1] * i + v2[1] * j];
    for j in [0.0, 40.0, 80.0] {
        build(&mut w, Curve::straight(p(0.0, j), p(160.0, j)), RoadKind::Street);
    }
    for i in [0.0, 80.0, 160.0] {
        build(&mut w, Curve::straight(p(i, 0.0), p(i, 80.0)), RoadKind::Street);
    }
    // A link between them.
    build(&mut w, Curve::straight(c(60, 65), p(0.0, 40.0)), RoadKind::Street);
    let zone = |w: &mut World, q: [[f32; 2]; 4]| {
        let _ = w.apply(Command::ZoneArea {
            quad: q,
            zone: Some(Zone::Residential),
        });
    };
    zone(&mut w, [c(38, 58), c(62, 58), c(62, 72), c(38, 72)]);
    zone(
        &mut w,
        [p(-10.0, -10.0), p(170.0, -10.0), p(170.0, 90.0), p(-10.0, 90.0)],
    );
    let (wpx, hpx) = (1600usize, 1000usize);
    let view = View::centred([540.0, 540.0], wpx, hpx - 64, 3.4);
    let mut map = Canvas::new(wpx, hpx - 64, GRASS);
    draw_cells(&mut map, &w, &view, true);
    draw_world_opts(&mut map, &w, &view, false, false);
    let shared = w
        .zoning
        .cells()
        .filter(|(c, x)| c.lattice == LatticeId::WORLD && x.usable() && x.fronts.count_ones() >= 2)
        .count();
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    cv.blit(&map, 0, 64);
    header(
        &mut cv,
        "A RIGHT-ANGLE GRID SHARES ONE LATTICE; A GRID TURNED 22 DEG KEEPS ITS OWN",
        &format!(
            "LATTICES {}, SHARED CORNER CELLS (WHITE DOTS) {shared}, CONTESTED CELLS (RED) {}",
            w.zoning.lattices().count(),
            w.zoning
                .cells()
                .filter(|(_, x)| x.blocked == Some(Blocked::Contested))
                .count()
        ),
    );
    ffmpeg_png(&cv, out);
}

/// Cars queueing on a curved approach to a signalised junction.
fn queue_sheet(out: &str) {
    let mut w = sheet_world();
    // A crossing of avenues whose south arm is a quarter circle.
    build(
        &mut w,
        Curve::straight([440.0, 640.0], [840.0, 640.0]),
        RoadKind::Avenue,
    );
    build(
        &mut w,
        Curve::straight([640.0, 440.0], [640.0, 640.0]),
        RoadKind::Avenue,
    );
    build(
        &mut w,
        Curve::from_tangents([470.0, 800.0], [1.0, 0.0], [640.0, 640.0], [0.0, -1.0]).expect("curved arm"),
        RoadKind::Avenue,
    );
    build(
        &mut w,
        Curve::straight([300.0, 800.0], [470.0, 800.0]),
        RoadKind::Avenue,
    );
    let mut k = 0usize;
    let ends = [[450.0, 640.0], [830.0, 640.0], [640.0, 450.0]];
    for t in 0..(3 * 60 * TICKS_PER_SECOND) {
        if t % 7 == 0 {
            // Most traffic comes up the curve; some crosses it.
            if k % 4 == 3 {
                traffic::debug_request(&mut w, ends[k % 2], ends[(k + 1) % 2]);
            } else {
                traffic::debug_request(&mut w, [310.0, 800.0], ends[k % 3]);
            }
            k += 1;
        }
        w.step();
    }
    let (wpx, hpx) = (1600usize, 1000usize);
    let view = View::centred([590.0, 690.0], wpx, hpx - 64, 4.6);
    let mut map = Canvas::new(wpx, hpx - 64, GRASS);
    draw_world_opts(&mut map, &w, &view, false, true);
    let on_curve = w
        .vehicles
        .values()
        .filter(|v| v.segment.is_some_and(|s| !w.roads.segments[s].curve.is_straight()) && v.stopped())
        .count();
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    cv.blit(&map, 0, 64);
    header(
        &mut cv,
        "A QUEUE ON A CURVED APPROACH TO A SIGNAL (RED CARS STAND)",
        &format!(
            "{} CARS ON THE ROADS, {on_curve} STANDING ON THE CURVE, IN BOTH LANES, BY ARC LENGTH",
            w.vehicles.len()
        ),
    );
    ffmpeg_png(&cv, out);
}

/// The organic town, zoomed in on the turned grid and the avenue.
fn organic_zoom(out: &str, days: u32) {
    let mut w = World::new(11);
    scenarios::prebuilt(&mut w, |w| {
        scenarios::organic(w);
    });
    for _ in 0..(days * balance::TICKS_PER_DAY + 8 * balance::TICKS_PER_HOUR - balance::START_TICK_OF_DAY) {
        w.step();
    }
    let (wpx, hpx) = (1600usize, 1000usize);
    let view = View::centred([520.0, 800.0], wpx, hpx - 64, 2.4);
    let mut map = Canvas::new(wpx, hpx - 64, GRASS);
    draw_world_opts(&mut map, &w, &view, true, true);
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    cv.blit(&map, 0, 64);
    let turned = w
        .buildings
        .values()
        .filter(|b| {
            let r = b.rotation().to_degrees().rem_euclid(90.0);
            r > 1.0 && r < 89.0
        })
        .count();
    header(
        &mut cv,
        "ORGANIC TOWN: LOTS AND BUILDINGS AT THEIR ROAD'S ANGLE (BRIGHT EDGE = FRONT)",
        &format!(
            "DAY {} 08:00  POP {}  BUILDINGS {} ({} TURNED OFF THE AXES)  CARS {}",
            w.time.day,
            w.stats.population,
            w.buildings.len(),
            turned,
            w.vehicles.len()
        ),
    );
    ffmpeg_png(&cv, out);
}

/// The organic town through one day, top-down (20-30 s).
fn organic_video(out: &str, seconds: u32) {
    let mut w = World::new(11);
    scenarios::prebuilt(&mut w, |w| {
        scenarios::organic(w);
    });
    // Grow it first, to midnight of day 12.
    while w.time.day < 12 || w.time.tick_of_day != 0 {
        w.step();
    }
    let (wpx, hpx) = (1280usize, 800usize);
    let fps = 25u32;
    let frames = seconds * fps;
    let per_frame = balance::TICKS_PER_DAY / frames;
    let mut child = Proc::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s"])
        .arg(format!("{wpx}x{hpx}"))
        .args([
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "20",
            out,
        ])
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let stdin = child.stdin.as_mut().expect("stdin");
    for f in 0..frames {
        for _ in 0..per_frame {
            w.step();
        }
        // Slow pan from the turned grid over the avenue to the ring.
        let t = f as f32 / frames as f32;
        let c = [430.0 + 260.0 * t, 820.0 + 220.0 * t];
        let view = View::centred(c, wpx, hpx - 48, 1.45);
        let mut map = Canvas::new(wpx, hpx - 48, GRASS);
        draw_world_opts(&mut map, &w, &view, true, true);
        let mut cv = Canvas::new(wpx, hpx, GRASS);
        cv.blit(&map, 0, 48);
        cv.rect(0.0, 0.0, wpx as f32, 48.0, [22, 24, 28]);
        let h = w.time.hour();
        cv.text(
            12,
            14,
            &format!(
                "ORGANIC TOWN  {:02}:{:02}  POP {}  CARS {}",
                h as u32,
                ((h - h.floor()) * 60.0) as u32,
                w.stats.population,
                w.vehicles.len()
            ),
            2,
            [235, 235, 235],
        );
        stdin.write_all(&cv.px).expect("write frame");
    }
    drop(child.stdin.take());
    child.wait().expect("ffmpeg");
}

/// Header bar with a title and a line of numbers.
/// A roundabout's island, apron, lane line and give-way marks (over the
/// junction surface drawn by `draw_world_opts`).
fn draw_ring(c: &mut Canvas, w: &World, v: &View, n: NodeId) {
    let Some(ring) = roundabout::ring(w, n) else { return };
    let d = ring.dims;
    let p = v.px(ring.center);
    if d.lanes == 2 {
        // Lane line: dashes round the ring.
        let r = d.outer_r - d.lane_w;
        let steps = (r * std::f32::consts::TAU / 1.0) as usize;
        for k in 0..steps {
            if k % 4 >= 2 {
                continue;
            }
            let a0 = k as f32 / steps as f32 * std::f32::consts::TAU;
            let a1 = (k + 1) as f32 / steps as f32 * std::f32::consts::TAU;
            let q0 = [ring.center[0] + r * libm::cosf(a0), ring.center[1] + r * libm::sinf(a0)];
            let q1 = [ring.center[0] + r * libm::cosf(a1), ring.center[1] + r * libm::sinf(a1)];
            c.poly(&strip(v, q0, q1, 0.1, 0.0), MARK, 1.0);
        }
    }
    // Apron (trucks may run over it) and the island.
    c.disc(p[0], p[1], d.inner_r * v.k, [150, 120, 100]);
    if d.mountable {
        c.disc(p[0], p[1], d.island_r * v.k, [205, 205, 198]);
        c.disc(p[0], p[1], (d.island_r - 0.4) * v.k, [180, 180, 174]);
    } else {
        c.disc(p[0], p[1], d.island_r * v.k, KERB);
        c.disc(p[0], p[1], (d.island_r - 0.35) * v.k, MEDIAN);
        c.disc(p[0], p[1], (d.island_r * 0.25).max(1.0) * v.k, [60, 100, 50]);
    }
    // Give-way lines: dashes across the lanes in, and a splitter.
    let j = w.traffic.junction_states.get(n).expect("ring junction");
    for (arm, ra) in j.arms.iter().zip(&ring.arms) {
        let seg = &w.roads.segments[arm.segment];
        let pose = seg.pose_dir(seg.a == n, ra.setback, 0.0);
        let din = [-pose.dir[0], -pose.dir[1]];
        let r = junction::right_of(din);
        let hw = arm.kind.half_width_m();
        let steps = 6;
        for k in 0..steps {
            if k % 2 == 1 {
                continue;
            }
            let o0 = hw * k as f32 / steps as f32;
            let o1 = hw * (k + 1) as f32 / steps as f32;
            let q0 = [pose.pos[0] + r[0] * o0, pose.pos[1] + r[1] * o0];
            let q1 = [pose.pos[0] + r[0] * o1, pose.pos[1] + r[1] * o1];
            c.poly(&strip(v, q0, q1, 0.25, 0.0), [250, 250, 250], 1.0);
        }
        if ra.splitter {
            let len = 7.0;
            let w0 = if arm.kind == RoadKind::Avenue { 1.0 } else { 0.35 };
            let tip = [pose.pos[0] + pose.dir[0] * len, pose.pos[1] + pose.dir[1] * len];
            let a = [pose.pos[0] + r[0] * w0, pose.pos[1] + r[1] * w0];
            let b = [pose.pos[0] - r[0] * w0, pose.pos[1] - r[1] * w0];
            let tip_a = [tip[0] + r[0] * w0 * 0.5, tip[1] + r[1] * w0 * 0.5];
            let tip_b = [tip[0] - r[0] * w0 * 0.5, tip[1] - r[1] * w0 * 0.5];
            c.poly(&[v.px(a), v.px(tip_a), v.px(tip_b), v.px(b)], KERB, 1.0);
        }
    }
}

/// Every roundabout size at a street crossing and an avenue crossing, with
/// the bench's traffic after `secs` seconds.
fn rings_sheet(out: &str, secs: u32) {
    use citysim_sim::capacity::{Bench, Load};
    let (pw, ph) = (560usize, 560usize);
    let mut cv = Canvas::new(pw * 4 + 10, ph * 2 + 64 + 40, GRASS);
    let rows = [
        (RoadKind::Street, RoadKind::Street),
        (RoadKind::Avenue, RoadKind::Avenue),
    ];
    for (row, &(main, cross)) in rows.iter().enumerate() {
        for (col, &size) in RoundaboutSize::ALL.iter().enumerate() {
            let rate = if main == RoadKind::Avenue { 22.0 } else { 9.0 };
            let b = Bench::crossing(main, cross, JunctionSetting::Priority)
                .turning(0.2, 0.2)
                .loads(Load::Rate(rate), Load::Rate(rate));
            let fits = {
                let (w, n) = citysim_sim::capacity::build(&Bench::crossing(main, cross, JunctionSetting::Priority));
                n.map(|n| roundabout::sizes(&w, n)[size as usize].1).unwrap_or(Ok(()))
            };
            let mut map = Canvas::new(pw - 6, ph - 6, GRASS);
            let mut label = format!("{} {:?}S", size.name(), main).to_uppercase();
            match fits {
                Ok(()) => {
                    let b = b.roundabout(size);
                    let mut done = false;
                    citysim_sim::capacity::run_traced(&b, 3, 0, secs, &mut |w, n, t| {
                        if done || t + 1 < secs * TICKS_PER_SECOND {
                            return;
                        }
                        done = true;
                        let Some(n) = n else { return };
                        let centre = w.roads.nodes[n].pos;
                        let view = View::centred(centre, pw - 6, ph - 6, 7.0);
                        draw_world_opts(&mut map, w, &view, false, false);
                        draw_ring(&mut map, w, &view, n);
                        draw_world_cars(&mut map, w, &view);
                    });
                    let d = size.dims();
                    label = format!("{label} R{:.1}M {}L ${}", d.outer_r, d.lanes, size.cost());
                }
                Err(p) => {
                    let (w, n) = citysim_sim::capacity::build(&Bench::crossing(main, cross, JunctionSetting::Priority));
                    if let Some(n) = n {
                        let centre = w.roads.nodes[n].pos;
                        let view = View::centred(centre, pw - 6, ph - 6, 7.0);
                        draw_world_opts(&mut map, &w, &view, false, false);
                    }
                    label = format!("{label}: {}", p.to_string().to_uppercase());
                    if label.len() > 44 {
                        label.truncate(44);
                    }
                }
            }
            map.text(10, 10, &label, 2, [250, 250, 250]);
            cv.blit(&map, col * pw + 3, 64 + row * (ph + 20) + 3);
        }
    }
    header(
        &mut cv,
        "ROUNDABOUTS: MINI, SMALL, MEDIUM, LARGE AT A STREET CROSSING AND AN AVENUE CROSSING",
        "THE ROADS STAY, THE ARMS END AT THE RING; WHITE CARS ARE ON THE RING, RED ONES STAND",
    );
    ffmpeg_png(&cv, out);
}

/// Cars only (over a world drawn without them).
fn draw_world_cars(c: &mut Canvas, w: &World, v: &View) {
    let mut empty = Canvas::new(1, 1, GRASS);
    let _ = &mut empty;
    draw_cars(c, w, v);
}

fn header(cv: &mut Canvas, title: &str, info: &str) {
    cv.rect(0.0, 0.0, cv.w as f32, 64.0, [22, 24, 28]);
    cv.text(12, 10, title, 3, [235, 235, 235]);
    cv.text(12, 40, info, 2, [190, 190, 190]);
}

/// The organic town, grown for `days` days.
fn organic(out: &str, days: u32) {
    let mut w = World::new(11);
    let mut refused = Vec::new();
    scenarios::prebuilt(&mut w, |w| refused = scenarios::organic(w));
    for (c, e) in &refused {
        eprintln!("refused {:?} -> {:?}: {e} ({:?})", c.start(), c.end(), e.at());
    }
    for _ in 0..days * balance::TICKS_PER_DAY {
        w.step();
    }
    let (wpx, hpx) = (1600usize, 1200usize);
    let view = View::centred([700.0, 1000.0], wpx, hpx - 64, 0.95);
    let mut map = Canvas::new(wpx, hpx - 64, GRASS);
    draw_world_opts(&mut map, &w, &view, true, true);
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    cv.blit(&map, 0, 64);
    let lattices = w.zoning.lattices().count();
    header(
        &mut cv,
        "ORGANIC TOWN: CURVED AVENUE, 17 DEG GRID, RING, CUL-DE-SACS",
        &format!(
            "DAY {}  POP {}  BUILDINGS {}  ROADS {}  LATTICES {}  CARS {}  REFUSED {}",
            w.time.day,
            w.stats.population,
            w.buildings.len(),
            w.roads.segments.len(),
            lattices,
            w.vehicles.len(),
            refused.len()
        ),
    );
    ffmpeg_png(&cv, out);
    eprintln!(
        "pop {} buildings {} refused {}",
        w.stats.population,
        w.buildings.len(),
        refused.len()
    );
}

/// A saved world run on to an hour of its next day, top-down:
/// `save out.png <save> <hour> <x m> <y m> <px per m> [title]`.
fn save_view(out: &str, args: &[String]) {
    let bytes = std::fs::read(&args[0]).expect("save file");
    let hour: f32 = args[1].parse().unwrap();
    let cx: f32 = args[2].parse().unwrap();
    let cy: f32 = args[3].parse().unwrap();
    let k: f32 = args[4].parse().unwrap();
    let title = args.get(5).cloned().unwrap_or_default();
    let mut w = World::load(&bytes).expect("loads");
    let target = (hour * balance::TICKS_PER_HOUR as f32) as u32;
    let day = w.time.day;
    while w.time.day == day && w.time.tick_of_day < target {
        w.step();
    }
    let (wpx, hpx) = (1600usize, 1000usize);
    let view = View::centred([cx, cy], wpx, hpx - 64, k);
    let mut map = Canvas::new(wpx, hpx - 64, GRASS);
    draw_world_opts(&mut map, &w, &view, false, true);
    let mut cv = Canvas::new(wpx, hpx, GRASS);
    cv.blit(&map, 0, 64);
    let stopped = w.vehicles.values().filter(|v| v.stopped()).count();
    header(
        &mut cv,
        &title,
        &format!(
            "DAY {} {:05.2}H  POP {}  CARS {} ({} STANDING)  GIVEN UP {}",
            w.time.day,
            w.time.hour(),
            w.stats.population,
            w.vehicles.len(),
            stopped,
            w.traffic.stuck_removed
        ),
    );
    ffmpeg_png(&cv, out);
}

/// Twelve frames of a saved world from an hour of its next day:
/// `savewatch out.png <save> <hour> <x m> <y m> <ticks between frames>`.
fn save_watch(out: &str, args: &[String]) {
    let bytes = std::fs::read(&args[0]).expect("save file");
    let hour: f32 = args[1].parse().unwrap();
    let cx: f32 = args[2].parse().unwrap();
    let cy: f32 = args[3].parse().unwrap();
    let every: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(40);
    let mut w = World::load(&bytes).expect("loads");
    w.traffic.junctions_version = u64::MAX;
    let target = (hour * balance::TICKS_PER_HOUR as f32) as u32;
    let day = w.time.day;
    while w.time.day == day && w.time.tick_of_day < target {
        w.step();
    }
    let (fw, fh, cols, rows) = (420usize, 420usize, 4usize, 3usize);
    let mut sheet = Canvas::new(cols * fw + (cols + 1) * 6, rows * fh + (rows + 1) * 6, [22, 24, 28]);
    for i in 0..cols * rows {
        let mut f = Canvas::new(fw, fh, GRASS);
        let view = View::centred(
            [cx, cy],
            fw,
            fh,
            std::env::var("TD_ZOOM")
                .ok()
                .and_then(|z| z.parse().ok())
                .unwrap_or(5.0),
        );
        draw_world(&mut f, &w, &view, false);
        f.text(
            6,
            6,
            &format!(
                "{:05.2}H T+{:.0}S",
                w.time.hour(),
                (i as u32 * every) as f32 / TICKS_PER_SECOND as f32
            ),
            2,
            [255, 255, 255],
        );
        sheet.blit(&f, 6 + (i % cols) * (fw + 6), 6 + (i / cols) * (fh + 6));
        for _ in 0..every {
            w.step();
        }
    }
    ffmpeg_png(&sheet, out);
}

/// The avenue city (`GridPlayer::hierarchy`, seed 7) at
/// `CITYSIM_TOPDOWN_POP` residents (default 10,000) through its evening rush,
/// 16:00 to 19:30, cars coloured by trip purpose.
fn rush(out: &str, seconds: u32) {
    let mut w = World::new(7);
    // `CITYSIM_TOPDOWN_LAYOUT=streets`: the same blocks on streets only,
    // standing cars red (for side-by-side comparisons).
    let streets = std::env::var("CITYSIM_TOPDOWN_LAYOUT").is_ok_and(|v| v == "streets");
    let red = std::env::var("CITYSIM_TOPDOWN_RED").is_ok();
    let mut p = if streets {
        GridPlayer::new(&Zone::ALL)
    } else {
        GridPlayer::hierarchy()
    };
    let pop = std::env::var("CITYSIM_TOPDOWN_POP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000u32);
    while w.stats.population < pop && w.time.day < 60 {
        p.act(&mut w);
        w.step();
    }
    let (start, end) = (
        16 * balance::TICKS_PER_HOUR,
        19 * balance::TICKS_PER_HOUR + balance::TICKS_PER_HOUR / 2,
    );
    while w.time.tick_of_day != start {
        p.act(&mut w);
        w.step();
    }
    // Frame the built-up area.
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for b in w.buildings.values() {
        let q = derived::rect_center(b.footprint);
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let (mut wpx, hpx) = (1280usize, 720usize);
    let mut centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0 + 10.0];
    let mut k = ((wpx as f32 - 40.0) / (hi[0] - lo[0] + 60.0)).min((hpx as f32 - 140.0) / (hi[1] - lo[1] + 60.0));
    // `CITYSIM_TOPDOWN_VIEW="width,x,y,scale"`: a fixed frame (the same for
    // two cities side by side).
    if let Ok(v) = std::env::var("CITYSIM_TOPDOWN_VIEW") {
        let n: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
        if n.len() == 4 {
            wpx = n[0] as usize;
            centre = [n[1], n[2]];
            k = n[3];
        }
    }
    let fps = 30;
    let mut child = Proc::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s"])
        .arg(format!("{wpx}x{hpx}"))
        .args([
            "-r",
            &fps.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "20",
        ])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut pipe = child.stdin.take().unwrap();
    let frames = seconds * fps;
    let per_frame = (end - start) as f32 / frames as f32;
    let mut done = start as f32;
    let view = View::centred(centre, wpx, hpx, k);
    for f in 0..frames {
        done += per_frame;
        while (w.time.tick_of_day as f32) < done && f + 1 < frames {
            p.act(&mut w);
            w.step();
        }
        let mut cv = Canvas::new(wpx, hpx, GRASS);
        draw_world_opts(&mut cv, &w, &view, false, red);
        if !red {
            draw_cars_by_purpose(&mut cv, &w, &view);
        }
        cv.rect(0.0, 0.0, wpx as f32, 34.0, [22, 24, 28]);
        let a = inspect::activity_shares(&w);
        let title = if streets { "STREETS ONLY" } else { "AVENUE HIERARCHY" };
        cv.text(
            10,
            10,
            &format!(
                "{title} {:02}:{:02} POP {} CARS {} ({:.0}% STAND)",
                w.time.hour() as u32,
                (w.time.hour().fract() * 60.0) as u32,
                w.stats.population,
                w.stats.vehicles_on_road,
                w.traffic.stopped_share * 100.0,
            ),
            2,
            [235, 235, 235],
        );
        let _ = a;
        // Legend.
        cv.rect(0.0, hpx as f32 - 30.0, wpx as f32, 30.0, [22, 24, 28]);
        let mut x = 12.0;
        if red {
            cv.text(
                12,
                hpx as i32 - 22,
                &format!(
                    "RED: STANDING  COMMUTE X{:.1} FREE FLOW  GIVEN UP {}",
                    traffic::delay_factor(&w),
                    w.traffic.stuck_removed
                ),
                2,
                [235, 235, 235],
            );
            pipe.write_all(&cv.px).unwrap();
            continue;
        }
        if red {
            cv.text(
                12,
                hpx as i32 - 22,
                &format!(
                    "RED: STANDING  COMMUTE X{:.1} FREE FLOW  GIVEN UP {}",
                    traffic::delay_factor(&w),
                    w.traffic.stuck_removed
                ),
                2,
                [235, 235, 235],
            );
            pipe.write_all(&cv.px).unwrap();
            continue;
        }
        for (i, label) in PURPOSE_LABEL.iter().enumerate() {
            if i == 3 {
                continue;
            }
            cv.rect(x, hpx as f32 - 21.0, 14.0, 12.0, PURPOSE_RGB[i]);
            cv.text(x as i32 + 20, hpx as i32 - 22, label, 2, [235, 235, 235]);
            x += 30.0 + label.len() as f32 * 12.0;
        }
        pipe.write_all(&cv.px).unwrap();
    }
    drop(pipe);
    assert!(child.wait().unwrap().success());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("signal");
    let out = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| format!("/tmp/topdown-{mode}.png"));
    match mode {
        "signal" => signal_sheet(&out),
        "lots" => lots(&out),
        "zonebed" => zone_bed_sheet(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(6)),
        "video" => video(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20)),
        "day" => day(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20)),
        "watch" => watch(&out, &args[2..]),
        "organic" => organic(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0)),
        "curve" => curve_sheet(&out),
        "junctions" => junction_sheet(&out),
        "blocks" => blocks_sheet(&out),
        "lattices" => lattice_sheet(&out),
        "queue" => queue_sheet(&out),
        "organic-zoom" => organic_zoom(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(12)),
        "organic-video" => organic_video(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(25)),
        "save" => save_view(&out, &args[2..]),
        "savewatch" => save_watch(&out, &args[2..]),
        "rush" => rush(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20)),
        "rings" => rings_sheet(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(90)),
        _ => panic!("modes: signal, lots, video, day, watch, organic, curve, junctions, blocks, lattices, queue"),
    }
}
