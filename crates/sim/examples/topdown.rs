//! Top-down debug renders of the simulation (no Bevy): roads with lanes,
//! stop lines coloured by the signal aspect, cars in their lanes and on
//! their junction curves, building lots with their front side.
//!
//! ```text
//! cargo run -p citysim_sim --release --example topdown -- signal out.png
//! cargo run -p citysim_sim --release --example topdown -- lots out.png
//! cargo run -p citysim_sim --release --example topdown -- video out.mp4 [seconds]
//! ```
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

fn draw_world_opts(c: &mut Canvas, w: &World, v: &View, show_zones: bool, cars: bool) {
    // Zones and lots.
    let cell = CELL_SIZE * v.k;
    let (cx0, cy0) = ((v.x0 / CELL_SIZE).floor() as i32, (v.y0 / CELL_SIZE).floor() as i32);
    let (cx1, cy1) = (
        cx0 + (c.w as f32 / cell).ceil() as i32 + 1,
        cy0 + (c.h as f32 / cell).ceil() as i32 + 1,
    );
    if show_zones {
        for y in cy0..cy1 {
            for x in cx0..cx1 {
                let Some(cd) = w.grid.get(Cell::new(x, y)) else {
                    continue;
                };
                if let (Some(z), Occupant::Empty) = (cd.zone, cd.occupant) {
                    let p = v.px([x as f32 * CELL_SIZE, y as f32 * CELL_SIZE]);
                    c.poly(
                        &[
                            [p[0] + 1.0, p[1] + 1.0],
                            [p[0] + cell - 1.0, p[1] + 1.0],
                            [p[0] + cell - 1.0, p[1] + cell - 1.0],
                            [p[0] + 1.0, p[1] + cell - 1.0],
                        ],
                        zone_rgb(z),
                        0.25,
                    );
                }
            }
        }
    }
    for b in w.buildings.values() {
        let r = b.footprint;
        if r.max.x < cx0 || r.min.x > cx1 || r.max.y < cy0 || r.min.y > cy1 {
            continue;
        }
        let inset = 0.9;
        let a = v.px([r.min.x as f32 * CELL_SIZE + inset, r.min.y as f32 * CELL_SIZE + inset]);
        let z = v.px([
            (r.max.x + 1) as f32 * CELL_SIZE - inset,
            (r.max.y + 1) as f32 * CELL_SIZE - inset,
        ]);
        let base = zone_rgb(b.kind);
        let col = mix(base, [30, 30, 30], 0.35 - 0.12 * b.level as f32);
        c.poly(&[[a[0], a[1]], [z[0], a[1]], [z[0], z[1]], [a[0], z[1]]], col, 1.0);
        // Front side: a bright band along the facing edge.
        let t = (1.6 * v.k).max(2.0);
        let (fa, fz) = match b.facing {
            Facing::PlusX => ([z[0] - t, a[1]], [z[0], z[1]]),
            Facing::MinusX => ([a[0], a[1]], [a[0] + t, z[1]]),
            Facing::PlusY => ([a[0], z[1] - t], [z[0], z[1]]),
            Facing::MinusY => ([a[0], a[1]], [z[0], a[1] + t]),
        };
        c.poly(
            &[[fa[0], fa[1]], [fz[0], fa[1]], [fz[0], fz[1]], [fa[0], fz[1]]],
            [245, 240, 225],
            1.0,
        );
        // Outline.
        for (p, q) in [
            ([a[0], a[1]], [z[0], a[1]]),
            ([z[0], a[1]], [z[0], z[1]]),
            ([z[0], z[1]], [a[0], z[1]]),
            ([a[0], z[1]], [a[0], a[1]]),
        ] {
            line(c, p, q, [24, 26, 28]);
        }
    }
    // Road bodies: kerbs, then asphalt, then junction boxes.
    for (s, seg) in w.roads.segments.iter() {
        let (a, b) = w.roads.segment_ends(s);
        let hw = seg.kind.half_width_m();
        c.poly(&strip(v, a, b, hw + 0.6, 0.0), KERB, 1.0);
    }
    for n in w.roads.nodes.values() {
        let r = n
            .segments
            .iter()
            .map(|&s| w.roads.segments[s].kind.half_width_m())
            .fold(0.0, f32::max);
        let p = v.px(n.pos);
        c.disc(p[0], p[1], (r + 0.6) * v.k, KERB);
    }
    for (s, seg) in w.roads.segments.iter() {
        let (a, b) = w.roads.segment_ends(s);
        c.poly(&strip(v, a, b, seg.kind.half_width_m(), 0.0), ASPHALT, 1.0);
    }
    for n in w.roads.nodes.values() {
        let r = n
            .segments
            .iter()
            .map(|&s| w.roads.segments[s].kind.half_width_m())
            .fold(0.0, f32::max);
        let p = v.px(n.pos);
        c.disc(p[0], p[1], r * v.k, ASPHALT);
    }
    // Lane markings on the drivable stretch.
    for (s, seg) in w.roads.segments.iter() {
        let (a, b) = w.roads.segment_ends(s);
        let Some((lo, hi)) = drive::drivable_range(w, s, true) else {
            continue;
        };
        let d = geom::sub(b, a);
        let l = seg.length.max(1e-3);
        let u = [d[0] / l, d[1] / l];
        let at = |t: f32| [a[0] + u[0] * t, a[1] + u[1] * t];
        let (pa, pb) = (at(lo), at(hi));
        if hi - lo < 1.0 {
            continue;
        }
        // Centre line.
        if seg.kind == RoadKind::Avenue || seg.kind == RoadKind::Highway {
            c.poly(&strip(v, pa, pb, 0.18, 0.35), YELLOW_MARK, 1.0);
            c.poly(&strip(v, pa, pb, 0.18, -0.35), YELLOW_MARK, 1.0);
            // Lane dividers, dashed.
            let k = seg.kind;
            let off = (k.lane_offset_m(0) + k.lane_offset_m(1)) * 0.5;
            let mut t = lo;
            while t < hi {
                let e = (t + 3.0).min(hi);
                for side in [1.0, -1.0] {
                    c.poly(&strip(v, at(t), at(e), 0.12, side * off), MARK, 1.0);
                }
                t += 7.0;
            }
        } else {
            let mut t = lo;
            while t < hi {
                let e = (t + 2.5).min(hi);
                c.poly(&strip(v, at(t), at(e), 0.12, 0.0), MARK, 1.0);
                t += 5.0;
            }
        }
    }
    // Stop lines, coloured by signal aspect per lane.
    for (n, _) in w.roads.nodes.iter() {
        let Some(j) = junction::junction(w, n) else { continue };
        let view = junction::junction_view(w, n);
        let npos = w.roads.nodes[n].pos;
        for (i, arm) in j.arms.iter().enumerate() {
            let din = [-arm.dir[0], -arm.dir[1]];
            let r = junction::right_of(din);
            let base = [npos[0] + arm.dir[0] * arm.setback, npos[1] + arm.dir[1] * arm.setback];
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
                    let q = [base[0] + arm.dir[0] * 2.2, base[1] + arm.dir[1] * 2.2];
                    let q = [q[0] + r[0] * off, q[1] + r[1] * off];
                    let tip = v.px([q[0] - arm.dir[0] * 1.2, q[1] - arm.dir[1] * 1.2]);
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
    // Vehicles.
    for (id, car) in w.vehicles.iter().filter(|_| cars) {
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
        .filter(|b| !growth::has_frontage(&w, b.footprint, b.facing))
        .count();
    let empty_zoned = w
        .grid
        .cells
        .iter()
        .filter(|cd| cd.zone.is_some() && cd.occupant == Occupant::Empty)
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
        "video" => video(&out, args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20)),
        "watch" => watch(&out, &args[2..]),
        _ => panic!("modes: signal, lots, video"),
    }
}
