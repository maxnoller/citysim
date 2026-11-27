//! Low-poly vehicles: cars of six body types (saloon, hatchback, van,
//! pickup, estate, SUV) and delivery trucks of three (box, tipper,
//! flatbed). Local frame: the vehicle points along +x, centred on the
//! origin, wheels on `y = 0`.
//!
//! A car is lofted from a side profile: the lower body between the sill
//! (cut by round wheel arches with dark liners, the wheels set inside) and
//! the belt line (bonnet, waist, boot, with chamfered top edges), and a
//! glasshouse whose sides lean in (tumblehome) with glass between the
//! pillars. Bumpers, grille, head and tail lights (night glow tags; the
//! tail lights brighten with the `MeshTag` brake bit), and near the
//! camera mirrors, door seams, handles and number plates.
//!
//! Three levels of detail ([`Lod`]): near (everything), mid (no small
//! parts, simpler wheels) and far (a body and lights).

use bevy::prelude::*;

use super::building_gen::Lod;
use super::mesh_util::{Glow, MeshBuilder};
use super::palette::{self, lin, mix, shade};

/// Number of car body types.
pub const SHAPES: u32 = 6;
/// Number of truck body types.
pub const TRUCK_KINDS: u32 = 3;

/// Length and width of a car shape, metres. Lengths fit the sim's 5.5 m
/// queue spacing (with lane compression) and `balance::CAR_BODY_M`.
pub fn car_size(shape: u32) -> (f32, f32) {
    match shape {
        1 => (3.75, 1.74),
        2 => (4.35, 1.86),
        3 => (4.4, 1.86),
        4 => (4.35, 1.8),
        5 => (4.3, 1.88),
        _ => (4.2, 1.78),
    }
}

/// Car paints with believable frequencies (whites, blacks, greys and
/// silvers make two thirds of a real car park): index into
/// `palette::CAR_BODIES` for a hash.
pub fn paint(h: u32) -> usize {
    // Weights in per mille, in `CAR_BODIES` order.
    const W: [u32; 12] = [200, 140, 150, 170, 80, 85, 25, 30, 15, 35, 45, 25];
    let total: u32 = W.iter().sum();
    let mut r = h % total;
    for (i, w) in W.iter().enumerate() {
        if r < *w {
            return i;
        }
        r -= w;
    }
    0
}

/// Width of the delivery truck, metres.
pub const TRUCK_WIDTH: f32 = 1.94;

/// Length of the delivery truck, metres. The sim queues vehicles 5.5 m
/// centre to centre; `vehicles` keeps lanes from compressing that below
/// what a truck behind a truck needs.
pub const TRUCK_LEN: f32 = 4.8;

/// Side profile of a body type: (x, y) points of the belt line from the
/// front bumper to the back, the glasshouse (windscreen base, roof front,
/// roof back, rear window base), heights.
struct Profile {
    /// Ground clearance (sill), wheel radius, wheelbase half.
    sill: f32,
    wheel: f32,
    base: f32,
    /// Belt line from the front: (x, y) points, front first.
    belt: Vec<Vec2>,
    /// Glasshouse outline (convex), counter-clockwise seen from +z:
    /// base front, roof front, roof back, base back (more allowed).
    cabin: Vec<Vec2>,
    /// Glasshouse sides lean in by this much at the roof.
    lean: f32,
    /// Front and rear faces: the nose and tail lean back by this at the top.
    nose: f32,
    tail: f32,
}

fn profile(shape: u32, hl: f32) -> Profile {
    match shape {
        // Hatchback: short bonnet, roof to the back, steep hatch.
        1 => Profile {
            sill: 0.27,
            wheel: 0.31,
            base: hl - 0.62,
            belt: vec![
                Vec2::new(hl, 0.7),
                Vec2::new(hl - 0.75, 0.88),
                Vec2::new(-hl + 0.2, 0.92),
                Vec2::new(-hl, 0.9),
            ],
            cabin: vec![
                Vec2::new(hl - 0.8, 0.88),
                Vec2::new(hl - 1.55, 1.42),
                Vec2::new(-hl + 0.38, 1.44),
                Vec2::new(-hl + 0.06, 0.92),
            ],
            lean: 0.13,
            nose: 0.12,
            tail: 0.08,
        },
        // Van / people carrier: short bonnet, tall long cabin.
        2 => Profile {
            sill: 0.3,
            wheel: 0.34,
            base: hl - 0.72,
            belt: vec![
                Vec2::new(hl, 0.78),
                Vec2::new(hl - 0.6, 1.0),
                Vec2::new(-hl + 0.1, 1.02),
                Vec2::new(-hl, 1.0),
            ],
            cabin: vec![
                Vec2::new(hl - 0.62, 1.0),
                Vec2::new(hl - 1.35, 1.92),
                Vec2::new(-hl + 0.06, 1.95),
                Vec2::new(-hl + 0.02, 1.02),
            ],
            lean: 0.08,
            nose: 0.14,
            tail: 0.02,
        },
        // Pickup: crew cab, the bed behind it.
        3 => Profile {
            sill: 0.36,
            wheel: 0.37,
            base: hl - 0.78,
            belt: vec![Vec2::new(hl, 0.88), Vec2::new(hl - 1.0, 1.04), Vec2::new(-hl, 1.04)],
            cabin: vec![
                Vec2::new(hl - 1.05, 1.04),
                Vec2::new(hl - 1.65, 1.68),
                Vec2::new(-0.35, 1.7),
                Vec2::new(-0.3, 1.04),
            ],
            lean: 0.1,
            nose: 0.08,
            tail: 0.0,
        },
        // Estate: long roof, near-vertical tailgate.
        4 => Profile {
            sill: 0.27,
            wheel: 0.32,
            base: hl - 0.78,
            belt: vec![
                Vec2::new(hl, 0.68),
                Vec2::new(hl - 1.05, 0.88),
                Vec2::new(-hl + 0.15, 0.94),
                Vec2::new(-hl, 0.9),
            ],
            cabin: vec![
                Vec2::new(hl - 1.1, 0.88),
                Vec2::new(hl - 1.85, 1.4),
                Vec2::new(-hl + 0.22, 1.42),
                Vec2::new(-hl + 0.05, 0.94),
            ],
            lean: 0.13,
            nose: 0.12,
            tail: 0.05,
        },
        // SUV: tall, upright, big wheels.
        5 => Profile {
            sill: 0.38,
            wheel: 0.37,
            base: hl - 0.75,
            belt: vec![
                Vec2::new(hl, 0.9),
                Vec2::new(hl - 0.95, 1.06),
                Vec2::new(-hl + 0.12, 1.1),
                Vec2::new(-hl, 1.06),
            ],
            cabin: vec![
                Vec2::new(hl - 1.0, 1.06),
                Vec2::new(hl - 1.7, 1.66),
                Vec2::new(-hl + 0.22, 1.68),
                Vec2::new(-hl + 0.08, 1.1),
            ],
            lean: 0.1,
            nose: 0.08,
            tail: 0.04,
        },
        // Saloon: bonnet, cabin, boot.
        _ => Profile {
            sill: 0.27,
            wheel: 0.32,
            base: hl - 0.75,
            belt: vec![
                Vec2::new(hl, 0.66),
                Vec2::new(hl - 1.05, 0.86),
                Vec2::new(-hl + 0.95, 0.92),
                Vec2::new(-hl + 0.1, 0.92),
                Vec2::new(-hl, 0.86),
            ],
            cabin: vec![
                Vec2::new(hl - 1.1, 0.86),
                Vec2::new(hl - 1.85, 1.38),
                Vec2::new(-hl + 1.4, 1.4),
                Vec2::new(-hl + 0.85, 0.92),
            ],
            lean: 0.14,
            nose: 0.14,
            tail: 0.1,
        },
    }
}

/// Height of a polyline (x falling along it) at `x`.
fn along(line: &[Vec2], x: f32) -> f32 {
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        if x <= a.x && x >= b.x {
            let t = (a.x - x) / (a.x - b.x).max(1e-5);
            return a.y + (b.y - a.y) * t;
        }
    }
    if x > line[0].x {
        line[0].y
    } else {
        line[line.len() - 1].y
    }
}

/// Bottom edge of the body side at `x`: the sill, round over the arches.
fn sill_at(p: &Profile, x: f32) -> f32 {
    let ra = p.wheel + 0.07;
    let mut y = p.sill;
    for xw in [p.base, -p.base] {
        let d = (x - xw).abs();
        if d < ra {
            y = y.max(p.wheel + (ra * ra - d * d).sqrt());
        }
    }
    y
}

/// Wheel: tyre with a tread and sidewall, a rim and hub on the outer side.
fn wheel(m: &mut MeshBuilder, c: Vec3, r: f32, w: f32, outer: f32, lod: Lod) {
    let n = if lod == Lod::Near { 10 } else { 7 };
    let p = |i: usize, rad: f32, z: f32| {
        let a = (i as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
        Vec3::new(c.x + rad * a.cos(), c.y + rad * a.sin(), z)
    };
    let s = outer;
    let zo = c.z + s * w * 0.5;
    let rim = shade(palette::GUARDRAIL, 0.78);
    let hub = shade(palette::GUARDRAIL, 0.5);
    let wall = shade(palette::TYRE, 1.3);
    let rr = r * 0.62;
    for i in 0..n {
        let j = (i + 1) % n;
        // Tread, facing away from the axle.
        m.quad(
            p(i, r, c.z - w * 0.5),
            p(j, r, c.z - w * 0.5),
            p(j, r, c.z + w * 0.5),
            p(i, r, c.z + w * 0.5),
            palette::TYRE,
        );
        let (a, b) = if s > 0.0 { (i, j) } else { (j, i) };
        // Sidewall round the rim, the rim (dished in near), the hub.
        m.quad(p(a, r, zo), p(b, r, zo), p(b, rr, zo), p(a, rr, zo), wall);
        if lod == Lod::Near {
            let zr = zo - s * 0.03;
            m.quad(
                p(a, rr, zo),
                p(b, rr, zo),
                p(b, rr * 0.92, zr),
                p(a, rr * 0.92, zr),
                rim,
            );
            m.quad(
                p(a, rr * 0.92, zr),
                p(b, rr * 0.92, zr),
                p(b, rr * 0.3, zr),
                p(a, rr * 0.3, zr),
                if i % 2 == 0 { rim } else { shade(rim, 0.7) },
            );
            m.tri(
                Vec3::new(c.x, c.y, zr + s * 0.01),
                p(a, rr * 0.3, zr),
                p(b, rr * 0.3, zr),
                hub,
            );
        } else {
            m.tri(
                Vec3::new(c.x, c.y, zo + s * 0.004),
                p(a, rr, zo + s * 0.004),
                p(b, rr, zo + s * 0.004),
                rim,
            );
        }
    }
}

/// A quad between the two sides of the body at heights / x given by `a`
/// and `b` (profile points), at half widths `wa` and `wb`, facing `out`.
fn across(m: &mut MeshBuilder, a: Vec2, b: Vec2, wa: f32, wb: f32, out: Vec3, color: Color) {
    m.quad_facing(
        [
            Vec3::new(a.x, a.y, -wa),
            Vec3::new(b.x, b.y, -wb),
            Vec3::new(b.x, b.y, wb),
            Vec3::new(a.x, a.y, wa),
        ],
        out,
        color,
    );
}

/// A car centred at `c`, rotated `yaw` about +y, into `out`.
pub fn car(out: &mut MeshBuilder, c: Vec3, yaw: f32, body: Color, shape: u32) {
    let m = car_model(body, shape, Lod::Mid);
    out.append(&m, c, yaw, Vec3::ONE, [1.0; 4]);
}

/// Car model of body type `shape` in its local frame.
pub fn car_model(body: Color, shape: u32, lod: Lod) -> MeshBuilder {
    let mut m = lod.builder(0.0);
    let (len, wid) = car_size(shape);
    let (hl, hw) = (len * 0.5, wid * 0.5);
    let p = profile(shape, hl);
    let dark = shade(body, 0.7);
    let trim = Color::srgb_u32(0x2B2E33);
    let glass = palette::CAR_GLASS;
    let near = lod == Lod::Near;
    let far = lod == Lod::Far;
    let chamfer = if far { 0.0 } else { 0.09 };
    // Sample points along the body: ends, the belt's corners, the arches.
    let mut xs: Vec<f32> = vec![hl, -hl];
    xs.extend(p.belt.iter().map(|v| v.x));
    let ra = p.wheel + 0.07;
    let arch_n = if far {
        2
    } else if near {
        7
    } else {
        5
    };
    for xw in [p.base, -p.base] {
        for k in 0..=arch_n {
            xs.push(xw - ra + 2.0 * ra * k as f32 / arch_n as f32);
        }
    }
    xs.retain(|x| x.abs() <= hl + 1e-4);
    xs.sort_by(|a, b| b.total_cmp(a));
    xs.dedup_by(|a, b| (*a - *b).abs() < 0.02);
    // Nose and tail lean: the side outline's x at a height.
    let bottom = |x: f32| sill_at(&p, x);
    let top = |x: f32| along(&p.belt, x);
    let lean_x = |x: f32, y: f32| {
        if x >= hl - 1e-3 {
            x - p.nose * ((y - p.sill) / (top(hl) - p.sill)).clamp(0.0, 1.0)
        } else if x <= -hl + 1e-3 {
            x + p.tail * ((y - p.sill) / (top(-hl) - p.sill)).clamp(0.0, 1.0)
        } else {
            x
        }
    };
    // Lower body sides: a strip from the sill (round over the arches) up
    // to the belt line, on both sides.
    for s in [1.0f32, -1.0] {
        let z = s * hw;
        for w in xs.windows(2) {
            let (x0, x1) = (w[0], w[1]);
            let q = [
                Vec3::new(lean_x(x0, bottom(x0)), bottom(x0), z),
                Vec3::new(lean_x(x1, bottom(x1)), bottom(x1), z),
                Vec3::new(lean_x(x1, top(x1)), top(x1), z),
                Vec3::new(lean_x(x0, top(x0)), top(x0), z),
            ];
            m.quad_facing(q, Vec3::Z * s, body);
        }
    }
    // Top of the lower body (bonnet, waist under the glass, boot) with
    // chamfered edges; the nose and tail faces.
    for w in p.belt.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = Vec3::new(b.y - a.y, a.x - b.x, 0.0).normalize_or(Vec3::Y);
        let (a2, b2) = (
            Vec2::new(lean_x(a.x, a.y), a.y + chamfer * 0.3),
            Vec2::new(lean_x(b.x, b.y), b.y + chamfer * 0.3),
        );
        across(&mut m, a2, b2, hw - chamfer, hw - chamfer, n, shade(body, 1.04));
        if chamfer > 0.0 {
            for s in [1.0f32, -1.0] {
                let (a1, b1) = (Vec2::new(lean_x(a.x, a.y), a.y), Vec2::new(lean_x(b.x, b.y), b.y));
                m.quad_facing(
                    [
                        Vec3::new(a1.x, a1.y, s * hw),
                        Vec3::new(b1.x, b1.y, s * hw),
                        Vec3::new(b2.x, b2.y, s * (hw - chamfer)),
                        Vec3::new(a2.x, a2.y, s * (hw - chamfer)),
                    ],
                    (n + Vec3::Z * s).normalize(),
                    body,
                );
            }
        }
    }
    for (x, sgn, lean) in [(hl, 1.0f32, p.nose), (-hl, -1.0, p.tail)] {
        let yt = top(x);
        let xt = x - sgn * lean;
        let nrm = Vec3::new(sgn, lean, 0.0).normalize();
        across(&mut m, Vec2::new(x, p.sill), Vec2::new(xt, yt), hw, hw, nrm, body);
        if chamfer > 0.0 {
            // Close the chamfer corners at the top of the nose / tail.
            across(
                &mut m,
                Vec2::new(xt, yt),
                Vec2::new(xt, yt + chamfer * 0.3),
                hw - chamfer,
                hw - chamfer,
                nrm,
                body,
            );
        }
    }
    // Underside, dark (seen past the arches and from low angles).
    m.quad_facing(
        [
            Vec3::new(-hl, p.sill, -hw),
            Vec3::new(hl, p.sill, -hw),
            Vec3::new(hl, p.sill, hw),
            Vec3::new(-hl, p.sill, hw),
        ],
        -Vec3::Y,
        shade(palette::TYRE, 0.8),
    );
    // Glasshouse: its sides lean in; glass between the pillars.
    let belt_y = p.cabin.iter().map(|v| v.y).fold(f32::MAX, f32::min);
    let roof_y = p.cabin.iter().map(|v| v.y).fold(f32::MIN, f32::max);
    let cw = hw - chamfer - 0.01;
    let half = |y: f32| cw - p.lean * ((y - belt_y) / (roof_y - belt_y)).clamp(0.0, 1.0);
    let n = p.cabin.len();
    for i in 0..n {
        let (a, b) = (p.cabin[i], p.cabin[(i + 1) % n]);
        if (a - b).length() < 1e-3 {
            continue;
        }
        let nrm = Vec3::new(b.y - a.y, a.x - b.x, 0.0).normalize_or(Vec3::Y);
        // Front (windscreen) and back edges are glass, framed; the roof is
        // body colour; the bottom edge lies on the lower body.
        let is_roof = nrm.y > 0.9;
        let is_bottom = nrm.y < -0.5;
        if is_bottom {
            continue;
        }
        let glass_edge = !is_roof && (a.y - b.y).abs() > 0.2;
        if glass_edge && !far {
            across(&mut m, a, b, half(a.y), half(b.y), nrm, body);
            // Glass inset from the frame.
            let (ia, ib) = (a + (b - a) * 0.06, b + (a - b) * 0.06);
            let off = nrm * 0.008;
            let (ha, hb) = (half(ia.y) - 0.07, half(ib.y) - 0.07);
            m.quad_facing(
                [
                    Vec3::new(ia.x, ia.y, -ha) + off,
                    Vec3::new(ib.x, ib.y, -hb) + off,
                    Vec3::new(ib.x, ib.y, hb) + off,
                    Vec3::new(ia.x, ia.y, ha) + off,
                ],
                nrm,
                glass,
            );
        } else {
            across(
                &mut m,
                a,
                b,
                half(a.y),
                half(b.y),
                nrm,
                if glass_edge { glass } else { body },
            );
        }
    }
    // Cabin sides: the pillars in body colour, glass panes over them.
    let vans = shape == 2;
    for s in [1.0f32, -1.0] {
        let ring: Vec<Vec3> = p.cabin.iter().map(|v| Vec3::new(v.x, v.y, s * half(v.y))).collect();
        let nrm = (Vec3::Z * s + Vec3::Y * (p.lean / (roof_y - belt_y))).normalize();
        for i in 1..ring.len() - 1 {
            m.tri_facing(ring[0], ring[i], ring[i + 1], nrm, if far { glass } else { body });
        }
        if far {
            continue;
        }
        // Glass: the outline shrunk towards its centre, lifted off the side;
        // split by a B pillar (and a C pillar on long cabins).
        let c = ring.iter().copied().sum::<Vec3>() / ring.len() as f32;
        let shrink = |v: Vec3, k: f32| c + (v - c) * k + nrm * 0.008;
        let inner: Vec<Vec3> = ring
            .iter()
            .map(|&v| {
                let d = v - c;
                let k = 1.0 - 0.09 / (d.length().max(0.1));
                let mut q = shrink(v, k);
                // Keep the glass above the belt line.
                q.y = q.y.max(belt_y + 0.07);
                q
            })
            .collect();
        let xb = if vans {
            p.cabin[1].x - 0.55
        } else {
            (p.cabin[1].x + p.cabin[2].x) * 0.5 + 0.1
        };
        let pillar = if near { 0.09 } else { 0.07 };
        // Clip the glass polygon to x ranges: front pane and back pane.
        let panes: Vec<(f32, f32)> = if vans {
            vec![(xb + pillar, f32::MAX), (-hl + 0.6, xb - pillar)]
        } else {
            vec![(xb + pillar, f32::MAX), (f32::MIN, xb - pillar)]
        };
        for (lo, hi) in panes {
            let poly = clip_x(&inner, lo, hi);
            if poly.len() >= 3 {
                for i in 1..poly.len() - 1 {
                    m.tri_facing(poly[0], poly[i], poly[i + 1], nrm, glass);
                }
            }
        }
    }
    // Pickup bed: walls and floor behind the cab.
    if shape == 3 {
        let (x0, x1) = (-hl + 0.05, -0.35);
        let yb = top(x0);
        let bed = shade(body, 0.55);
        m.quad_up(
            [
                Vec3::new(x0, yb - 0.35, -hw + 0.1),
                Vec3::new(x1, yb - 0.35, -hw + 0.1),
                Vec3::new(x1, yb - 0.35, hw - 0.1),
                Vec3::new(x0, yb - 0.35, hw - 0.1),
            ],
            lin(bed),
        );
        // The bed's inner walls (the outer ones are the body sides).
        for s in [1.0f32, -1.0] {
            m.quad_facing(
                [
                    Vec3::new(x0, yb - 0.35, s * (hw - 0.1)),
                    Vec3::new(x1, yb - 0.35, s * (hw - 0.1)),
                    Vec3::new(x1, yb + 0.03, s * (hw - 0.1)),
                    Vec3::new(x0, yb + 0.03, s * (hw - 0.1)),
                ],
                -Vec3::Z * s,
                dark,
            );
        }
        m.quad_facing(
            [
                Vec3::new(x0 + 0.08, yb - 0.35, -hw),
                Vec3::new(x0 + 0.08, yb - 0.35, hw),
                Vec3::new(x0 + 0.08, yb + 0.03, hw),
                Vec3::new(x0 + 0.08, yb + 0.03, -hw),
            ],
            Vec3::X,
            dark,
        );
    }
    // Lights: headlights at the front corners, tail lights at the back.
    let head = Color::srgb(0.98, 0.96, 0.85);
    let tail = Color::srgb(0.75, 0.12, 0.1);
    let yh = top(hl) - 0.16;
    let yt = top(-hl) - 0.14;
    let (xh, xt) = (lean_x(hl, yh) + 0.006, lean_x(-hl, yt) - 0.006);
    for s in [-1.0f32, 1.0] {
        let z0 = s * (hw - 0.08);
        let z1 = s * (hw - 0.42);
        m.glow(Glow::Headlight, 0.0, |m| {
            m.quad_facing(
                [
                    Vec3::new(xh, yh - 0.08, z0),
                    Vec3::new(xh, yh - 0.08, z1),
                    Vec3::new(xh - 0.02, yh + 0.06, z1),
                    Vec3::new(xh - 0.02, yh + 0.06, z0),
                ],
                Vec3::X,
                head,
            )
        });
        m.glow(Glow::TailLight, 0.0, |m| {
            m.quad_facing(
                [
                    Vec3::new(xt, yt - 0.1, z0),
                    Vec3::new(xt, yt - 0.1, z1),
                    Vec3::new(xt + 0.01, yt + 0.06, z1),
                    Vec3::new(xt + 0.01, yt + 0.06, z0),
                ],
                -Vec3::X,
                tail,
            )
        });
        // Wrap-round of the lamps onto the sides (mid).
        if !far {
            let zs = s * (hw + 0.004);
            m.glow(Glow::TailLight, 0.0, |m| {
                m.quad_facing(
                    [
                        Vec3::new(-hl + 0.02, yt - 0.1, zs),
                        Vec3::new(-hl + 0.2, yt - 0.1, zs),
                        Vec3::new(-hl + 0.2, yt + 0.06, zs),
                        Vec3::new(-hl + 0.02, yt + 0.06, zs),
                    ],
                    Vec3::Z * s,
                    tail,
                )
            });
        }
    }
    if far {
        return m;
    }
    // Bumpers (dark plastic or body colour), the grille between the lamps.
    let bumper = if shape == 5 || shape == 3 {
        trim
    } else {
        shade(body, 0.85)
    };
    for (x, sgn, lean) in [(hl, 1.0f32, p.nose), (-hl, -1.0, p.tail)] {
        let _ = lean;
        let y0 = p.sill - 0.02;
        let y1 = p.sill + 0.2;
        m.cuboid2(
            Vec3::new(if sgn > 0.0 { x - 0.12 } else { x - 0.05 }, y0, -hw - 0.02),
            Vec3::new(if sgn > 0.0 { x + 0.05 } else { x + 0.12 }, y1, hw + 0.02),
            bumper,
            shade(bumper, 1.1),
        );
    }
    let gy = top(hl) - 0.2;
    let gx = lean_x(hl, gy) + 0.008;
    m.quad_facing(
        [
            Vec3::new(gx, p.sill + 0.22, -hw + 0.45),
            Vec3::new(gx, p.sill + 0.22, hw - 0.45),
            Vec3::new(gx - 0.02, gy, hw - 0.45),
            Vec3::new(gx - 0.02, gy, -hw + 0.45),
        ],
        Vec3::X,
        trim,
    );
    // Wheels inside the arches, dark liners behind them.
    let wz = hw - 0.14;
    for xw in [p.base, -p.base] {
        for s in [1.0f32, -1.0] {
            wheel(&mut m, Vec3::new(xw, p.wheel, s * wz), p.wheel, 0.22, s, lod);
            // Liner: the arch's inside, a dark half-disc in front of the
            // far side and a strip round the arch.
            let zl = s * (hw - 0.3);
            let arch_k = 6;
            let pts: Vec<Vec2> = (0..=arch_k)
                .map(|k| {
                    let a = k as f32 / arch_k as f32 * std::f32::consts::PI;
                    Vec2::new(xw + ra * a.cos(), (p.wheel + ra * a.sin()).max(p.sill))
                })
                .collect();
            let liner = shade(palette::TYRE, 0.7);
            for w in pts.windows(2) {
                m.tri_facing(
                    Vec3::new(xw, p.sill, zl),
                    Vec3::new(w[0].x, w[0].y, zl),
                    Vec3::new(w[1].x, w[1].y, zl),
                    Vec3::Z * s,
                    liner,
                );
                m.quad_facing(
                    [
                        Vec3::new(w[0].x, w[0].y, zl),
                        Vec3::new(w[1].x, w[1].y, zl),
                        Vec3::new(w[1].x, w[1].y, s * hw),
                        Vec3::new(w[0].x, w[0].y, s * hw),
                    ],
                    -Vec3::Y,
                    liner,
                );
            }
        }
    }
    if near {
        // Mirrors at the base of the A pillars.
        let a = p.cabin[0];
        for s in [1.0f32, -1.0] {
            let c = Vec3::new(a.x - 0.22, a.y + 0.08, s * (hw + 0.03));
            m.cuboid2(
                c - Vec3::new(0.06, 0.06, 0.07),
                c + Vec3::new(0.06, 0.07, 0.07),
                body,
                body,
            );
            m.quad_facing(
                [
                    c + Vec3::new(-0.065, -0.05, -0.08),
                    c + Vec3::new(-0.065, -0.05, 0.08),
                    c + Vec3::new(-0.065, 0.06, 0.08),
                    c + Vec3::new(-0.065, 0.06, -0.08),
                ],
                -Vec3::X,
                glass,
            );
        }
        // Door seams and handles on both sides.
        let xb = (p.cabin[1].x + p.cabin[2].x) * 0.5 + 0.1;
        let seams: Vec<f32> = match shape {
            1 => vec![p.cabin[0].x - 0.05, xb],
            2 => vec![p.cabin[0].x - 0.05, xb, -hl + 1.0],
            3 => vec![p.cabin[0].x - 0.05, xb],
            _ => vec![p.cabin[0].x - 0.05, xb, p.cabin[3].x + 0.1],
        };
        for s in [1.0f32, -1.0] {
            let z = s * (hw + 0.003);
            for &x in &seams {
                let (y0, y1) = (sill_at(&p, x) + 0.05, top(x) - 0.03);
                if y1 - y0 < 0.15 {
                    continue;
                }
                m.quad_facing(
                    [
                        Vec3::new(x - 0.012, y0, z),
                        Vec3::new(x + 0.012, y0, z),
                        Vec3::new(x + 0.012, y1, z),
                        Vec3::new(x - 0.012, y1, z),
                    ],
                    Vec3::Z * s,
                    dark,
                );
            }
            for w in seams.windows(2) {
                let x = w[1] + 0.25;
                let y = top(x) - 0.12;
                m.quad_facing(
                    [
                        Vec3::new(x - 0.07, y, z + s * 0.002),
                        Vec3::new(x + 0.07, y, z + s * 0.002),
                        Vec3::new(x + 0.07, y + 0.035, z + s * 0.002),
                        Vec3::new(x - 0.07, y + 0.035, z + s * 0.002),
                    ],
                    Vec3::Z * s,
                    shade(body, 0.6),
                );
            }
            // Side skirt / rubbing strip.
            let y = p.sill + 0.12;
            m.quad_facing(
                [
                    Vec3::new(-p.base + ra, y, z),
                    Vec3::new(p.base - ra, y, z),
                    Vec3::new(p.base - ra, y + 0.05, z),
                    Vec3::new(-p.base + ra, y + 0.05, z),
                ],
                Vec3::Z * s,
                trim,
            );
        }
        // Number plates.
        let plate = Color::srgb_u32(0xEDEBE3);
        let rear_plate = Color::srgb_u32(0xE8C25A);
        for (x, sgn, col) in [(hl + 0.06, 1.0f32, plate), (-hl - 0.13, -1.0, rear_plate)] {
            let y = p.sill + 0.24;
            m.quad_facing(
                [
                    Vec3::new(x, y, -0.25),
                    Vec3::new(x, y, 0.25),
                    Vec3::new(x, y + 0.12, 0.25),
                    Vec3::new(x, y + 0.12, -0.25),
                ],
                Vec3::X * sgn,
                col,
            );
        }
        // Roof rails on SUVs and estates.
        if shape == 5 || shape == 4 {
            let (r0, r1) = (p.cabin[2], p.cabin[1]);
            for s in [1.0f32, -1.0] {
                let z = s * (half(roof_y) - 0.06);
                m.cuboid(
                    Vec3::new(r0.x + 0.1, roof_y, z - 0.03),
                    Vec3::new(r1.x - 0.1, roof_y + 0.06, z + 0.03),
                    trim,
                );
            }
        }
    }
    // Soft contact shadow under the car (a dark quad just above the road).
    let sh = lin(Color::srgba(0.0, 0.0, 0.0, 1.0));
    m.quad_up(
        [
            Vec3::new(-hl + 0.1, 0.012, -hw + 0.05),
            Vec3::new(hl - 0.1, 0.012, -hw + 0.05),
            Vec3::new(hl - 0.1, 0.012, hw - 0.05),
            Vec3::new(-hl + 0.1, 0.012, hw - 0.05),
        ],
        [sh[0] + 0.02, sh[1] + 0.02, sh[2] + 0.02, 1.0],
    );
    m
}

/// Clip a convex polygon to `lo <= x <= hi`.
fn clip_x(poly: &[Vec3], lo: f32, hi: f32) -> Vec<Vec3> {
    let clip = |pts: Vec<Vec3>, keep: &dyn Fn(f32) -> bool, edge: f32| -> Vec<Vec3> {
        let mut out = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let (ka, kb) = (keep(a.x), keep(b.x));
            if ka {
                out.push(a);
            }
            if ka != kb {
                let t = (edge - a.x) / (b.x - a.x);
                out.push(a + (b - a) * t);
            }
        }
        out
    };
    let p = clip(poly.to_vec(), &|x| x >= lo, lo);
    if p.len() < 3 {
        return p;
    }
    clip(p, &|x| x <= hi, hi)
}

/// Delivery truck in its local frame: a cab with a raked windscreen and
/// a body of `kind` (0 a tall cargo box with a livery stripe, 1 a tipper
/// with a load of earth or gravel, 2 a flatbed with a load).
pub fn truck_model(cab: Color, cargo: Color, stripe: Color, kind: u32, lod: Lod) -> MeshBuilder {
    truck_body(
        TRUCK_LEN,
        TRUCK_WIDTH,
        false,
        cab,
        cargo,
        stripe,
        kind % TRUCK_KINDS,
        lod,
    )
}

/// Long-distance lorry (imports and exports between the highway and the
/// town): longer and wider, a sleeper cab with a wind deflector, and a body
/// of `kind` (0 box, 1 tipper, 2 flatbed, 3 curtain-side, 4 tanker).
pub fn lorry_model(cab: Color, cargo: Color, stripe: Color, kind: u32, lod: Lod) -> MeshBuilder {
    truck_body(
        LORRY_LEN,
        LORRY_WIDTH,
        true,
        cab,
        cargo,
        stripe,
        kind % LORRY_KINDS,
        lod,
    )
}

/// Length and width of the long-distance lorry, metres (within
/// `balance::TRUCK_BODY_M`, the length the sim spaces goods vehicles by).
pub const LORRY_LEN: f32 = 5.2;
pub const LORRY_WIDTH: f32 = 2.1;
/// Number of lorry body types.
pub const LORRY_KINDS: u32 = 5;

/// A truck `len` x `width`: cab (a sleeper cab when `sleeper`) and body.
#[allow(clippy::too_many_arguments)]
fn truck_body(
    len: f32,
    width: f32,
    sleeper: bool,
    cab: Color,
    cargo: Color,
    stripe: Color,
    kind: u32,
    lod: Lod,
) -> MeshBuilder {
    let mut m = lod.builder(0.0);
    let (hl, hw) = (len * 0.5, width * 0.5);
    let wr = 0.38;
    let far = lod == Lod::Far;
    let near = lod == Lod::Near;
    let frame = shade(palette::TYRE, 1.4);
    // Chassis rail and bumpers.
    m.cuboid(
        Vec3::new(-hl + 0.05, 0.32, -hw + 0.12),
        Vec3::new(hl - 0.05, 0.62, hw - 0.12),
        frame,
    );
    // Cab: short bonnet, raked windscreen, roof; glass sides.
    let cab_back = hl - if sleeper { 1.6 } else { 1.45 };
    let cab_top = if sleeper { 2.5 } else { 2.12 };
    let prof = [
        Vec2::new(cab_back, 0.42),
        Vec2::new(hl, 0.42),
        Vec2::new(hl, 1.2),
        Vec2::new(hl - 0.3, 2.05),
        Vec2::new(hl - 0.5, cab_top),
        Vec2::new(cab_back, cab_top),
    ];
    let cw = hw - 0.04;
    extrude(&mut m, &prof, -cw, cw, cab, &|i| match i {
        0 => shade(cab, 0.4),
        1 => shade(cab, 0.85),
        2 => palette::CAR_GLASS,
        3 | 4 => shade(cab, 1.08),
        _ => shade(cab, 0.8),
    });
    if !far {
        side_windows(&mut m, hl - 0.38, cab_back + 0.15, 1.3, 1.95, cw);
        if sleeper {
            // Livery band on the cab and a light bar on the roof.
            for z in [cw + 0.012, -cw - 0.012] {
                m.quad_facing(
                    [
                        Vec3::new(cab_back + 0.05, 2.08, z),
                        Vec3::new(hl - 0.45, 2.08, z),
                        Vec3::new(hl - 0.45, 2.3, z),
                        Vec3::new(cab_back + 0.05, 2.3, z),
                    ],
                    Vec3::Z * z.signum(),
                    stripe,
                );
            }
        }
        // Grille, bumper and a sun visor.
        m.quad(
            Vec3::new(hl + 0.005, 0.55, -0.6),
            Vec3::new(hl + 0.005, 0.55, 0.6),
            Vec3::new(hl + 0.005, 1.0, 0.6),
            Vec3::new(hl + 0.005, 1.0, -0.6),
            shade(cab, 0.45),
        );
        m.cuboid2(
            Vec3::new(hl - 0.05, 0.3, -hw),
            Vec3::new(hl + 0.08, 0.5, hw),
            Color::srgb_u32(0x2B2E33),
            Color::srgb_u32(0x3B3E43),
        );
    }
    if near {
        m.cuboid(
            Vec3::new(hl - 0.42, 2.0, -cw + 0.05),
            Vec3::new(hl - 0.22, 2.06, cw - 0.05),
            shade(cab, 0.7),
        );
        // Mirrors on arms.
        for s in [1.0f32, -1.0] {
            let c = Vec3::new(hl - 0.3, 1.6, s * (hw + 0.14));
            m.cuboid(
                c - Vec3::new(0.04, 0.22, 0.04),
                c + Vec3::new(0.04, 0.22, 0.04),
                Color::srgb_u32(0x2B2E33),
            );
            m.cuboid(
                Vec3::new(hl - 0.3, 1.6, s * hw) - Vec3::new(0.02, 0.02, 0.0).with_z(if s > 0.0 { 0.0 } else { 0.14 }),
                Vec3::new(hl - 0.28, 1.62, s * hw) + Vec3::new(0.0, 0.0, if s > 0.0 { 0.14 } else { 0.0 }),
                Color::srgb_u32(0x2B2E33),
            );
        }
        // Step under the door.
        for s in [1.0f32, -1.0] {
            m.cuboid(
                Vec3::new(hl - 1.0, 0.45, s * hw - 0.08),
                Vec3::new(hl - 0.5, 0.52, s * hw + 0.08),
                Color::srgb_u32(0x2B2E33),
            );
        }
    }
    let (bx0, bx1) = (-hl, cab_back - 0.08);
    let xb = bx0 - 0.01;
    if sleeper && !far {
        // Wind deflector from the cab roof up to the body.
        let top = if matches!(kind, 0 | 3) { 2.75 } else { cab_top + 0.15 };
        m.quad_facing(
            [
                Vec3::new(hl - 0.5, cab_top, -cw + 0.05),
                Vec3::new(hl - 0.5, cab_top, cw - 0.05),
                Vec3::new(cab_back + 0.1, top, cw - 0.05),
                Vec3::new(cab_back + 0.1, top, -cw + 0.05),
            ],
            Vec3::new(0.5, 1.0, 0.0).normalize(),
            shade(cab, 1.05),
        );
    }
    let box_top = if sleeper { 2.8 } else { 2.6 };
    match kind {
        // Curtain-sider: a box whose sides are taut curtains in the livery.
        3 => {
            let (by0, by1) = (0.66, box_top);
            m.cuboid2(
                Vec3::new(bx0, by0, -hw),
                Vec3::new(bx1, by1, hw),
                cargo,
                shade(palette::GUARDRAIL, 1.05),
            );
            for z in [hw + 0.01, -hw - 0.01] {
                let (a, b) = if z > 0.0 { (bx0, bx1) } else { (bx1, bx0) };
                m.quad(
                    Vec3::new(a, by0 + 0.1, z),
                    Vec3::new(b, by0 + 0.1, z),
                    Vec3::new(b, by0 + 0.55, z),
                    Vec3::new(a, by0 + 0.55, z),
                    stripe,
                );
                if !far {
                    // Rails at the top and bottom, folds / buckles.
                    let n = if near { 14 } else { 5 };
                    for k in 0..=n {
                        let x = bx0 + (bx1 - bx0) * k as f32 / n as f32;
                        m.quad_facing(
                            [
                                Vec3::new(x - 0.02, by0 + 0.05, z + z.signum() * 0.004),
                                Vec3::new(x + 0.02, by0 + 0.05, z + z.signum() * 0.004),
                                Vec3::new(x + 0.02, by1 - 0.05, z + z.signum() * 0.004),
                                Vec3::new(x - 0.02, by1 - 0.05, z + z.signum() * 0.004),
                            ],
                            Vec3::Z * z.signum(),
                            shade(cargo, 0.82),
                        );
                    }
                    m.cuboid(
                        Vec3::new(bx0, by1 - 0.08, z.signum() * hw - 0.02),
                        Vec3::new(bx1, by1, z.signum() * hw + 0.02),
                        palette::GUARDRAIL,
                    );
                }
            }
        }
        // Tanker: a polished barrel on the chassis, walkway and ladder.
        4 => {
            let r = hw - 0.12;
            let cy = 0.66 + r + 0.08;
            let sides = if far {
                6
            } else if near {
                12
            } else {
                8
            };
            let tank = mix(palette::GUARDRAIL, Color::WHITE, 0.25);
            prism_x(&mut m, cy, 0.0, r, bx0 + 0.05, bx1 - 0.05, sides, tank, true);
            if !far {
                for x in [bx0 + 0.6, (bx0 + bx1) * 0.5, bx1 - 0.6] {
                    prism_x(
                        &mut m,
                        cy,
                        0.0,
                        r + 0.02,
                        x - 0.05,
                        x + 0.05,
                        sides,
                        shade(tank, 0.8),
                        false,
                    );
                }
                // Band in the livery.
                prism_x(&mut m, cy, 0.0, r + 0.012, bx0 + 0.3, bx1 - 0.3, sides, tank, false);
                m.cuboid(
                    Vec3::new(bx0 + 0.3, cy - 0.15, -r - 0.03),
                    Vec3::new(bx1 - 0.3, cy + 0.15, r + 0.03),
                    stripe,
                );
            }
            if near {
                m.cuboid(
                    Vec3::new(bx0 + 0.3, cy + r, -0.25),
                    Vec3::new(bx1 - 0.3, cy + r + 0.05, 0.25),
                    palette::METAL_DARK,
                );
                m.cuboid(
                    Vec3::new(bx0 - 0.05, 0.7, -0.2),
                    Vec3::new(bx0, cy + r, 0.2),
                    palette::METAL_DARK,
                );
            }
        }
        // Tipper: an open steel bin, tapered, with a load.
        1 => {
            let (by0, by1) = (0.66, 1.75);
            let steel = cargo;
            m.cuboid2(
                Vec3::new(bx0, by0, -hw),
                Vec3::new(bx1, by0 + 0.12, hw),
                shade(steel, 0.7),
                shade(steel, 0.8),
            );
            // Walls (sloped sides).
            for s in [1.0f32, -1.0] {
                m.quad_facing(
                    [
                        Vec3::new(bx0, by0, s * (hw - 0.1)),
                        Vec3::new(bx1, by0, s * (hw - 0.1)),
                        Vec3::new(bx1, by1, s * hw),
                        Vec3::new(bx0, by1, s * hw),
                    ],
                    Vec3::Z * s,
                    steel,
                );
                m.quad_facing(
                    [
                        Vec3::new(bx0, by0, s * (hw - 0.14)),
                        Vec3::new(bx1, by0, s * (hw - 0.14)),
                        Vec3::new(bx1, by1, s * (hw - 0.04)),
                        Vec3::new(bx0, by1, s * (hw - 0.04)),
                    ],
                    -Vec3::Z * s,
                    shade(steel, 0.6),
                );
                // Top rim.
                m.quad_up(
                    [
                        Vec3::new(bx0, by1, s * (hw - 0.04)),
                        Vec3::new(bx1, by1, s * (hw - 0.04)),
                        Vec3::new(bx1, by1, s * hw),
                        Vec3::new(bx0, by1, s * hw),
                    ],
                    lin(shade(steel, 1.1)),
                );
            }
            for (x, n) in [(bx0, -Vec3::X), (bx1, Vec3::X)] {
                m.quad_facing(
                    [
                        Vec3::new(x, by0, -hw),
                        Vec3::new(x, by0, hw),
                        Vec3::new(x, by1 + if x == bx1 { 0.25 } else { 0.0 }, hw),
                        Vec3::new(x, by1 + if x == bx1 { 0.25 } else { 0.0 }, -hw),
                    ],
                    n,
                    shade(steel, if x == bx1 { 0.9 } else { 0.85 }),
                );
            }
            // Load heaped in the bin.
            let load = mix(palette::EARTH, Color::srgb_u32(0x8A7558), 0.5);
            let c = Vec3::new((bx0 + bx1) * 0.5, by1 - 0.25, 0.0);
            m.quad_up(
                [
                    Vec3::new(bx0 + 0.05, by1 - 0.2, -hw + 0.08),
                    Vec3::new(bx1 - 0.05, by1 - 0.2, -hw + 0.08),
                    Vec3::new(bx1 - 0.05, by1 - 0.2, hw - 0.08),
                    Vec3::new(bx0 + 0.05, by1 - 0.2, hw - 0.08),
                ],
                lin(load),
            );
            if !far {
                m.cone(c, (bx1 - bx0) * 0.38, 0.45, 6, load, 0.1);
            }
            // Stripe along the cab instead.
            for z in [hw - 0.03, -hw + 0.03] {
                m.quad_facing(
                    [
                        Vec3::new(cab_back + 0.05, 0.75, z * 1.0 + z.signum() * 0.02),
                        Vec3::new(hl - 0.05, 0.75, z + z.signum() * 0.02),
                        Vec3::new(hl - 0.05, 0.9, z + z.signum() * 0.02),
                        Vec3::new(cab_back + 0.05, 0.9, z + z.signum() * 0.02),
                    ],
                    Vec3::Z * z.signum(),
                    stripe,
                );
            }
        }
        // Flatbed with a load: pallets with wrapped goods, or pipes.
        2 => {
            let by0 = 0.66;
            m.cuboid2(
                Vec3::new(bx0, by0, -hw),
                Vec3::new(bx1, by0 + 0.14, hw),
                shade(cargo, 0.6),
                Color::srgb_u32(0x9C7650),
            );
            let y = by0 + 0.14;
            if !far {
                // Headboard behind the cab.
                m.cuboid(
                    Vec3::new(bx1 - 0.08, y, -hw + 0.05),
                    Vec3::new(bx1, y + 0.9, hw - 0.05),
                    shade(stripe, 0.9),
                );
            }
            let pipes = (cab.to_srgba().red * 10.0) as u32 % 2 == 0;
            if pipes {
                let pc = Color::srgb_u32(0x6E7378);
                for (k, z) in [-0.55f32, 0.0, 0.55].into_iter().enumerate() {
                    for yy in [0.0, 0.42] {
                        if yy > 0.0 && k == 1 {
                            continue;
                        }
                        let zz = if yy > 0.0 { z * 0.5 } else { z };
                        let c = Vec3::new(bx0 + 0.1, y + 0.21 + yy, zz);
                        // A pipe along x: an 6-sided prism.
                        let sides = if far { 4 } else { 6 };
                        let ring: Vec<Vec2> = (0..sides)
                            .map(|i| {
                                let a = i as f32 / sides as f32 * std::f32::consts::TAU;
                                Vec2::new(c.y + 0.2 * a.cos(), c.z + 0.2 * a.sin())
                            })
                            .collect();
                        for i in 0..sides {
                            let (a, b) = (ring[i], ring[(i + 1) % sides]);
                            let mid = (a + b) * 0.5 - Vec2::new(c.y, c.z);
                            m.quad_facing(
                                [
                                    Vec3::new(c.x, a.x, a.y),
                                    Vec3::new(bx1 - 0.15, a.x, a.y),
                                    Vec3::new(bx1 - 0.15, b.x, b.y),
                                    Vec3::new(c.x, b.x, b.y),
                                ],
                                Vec3::new(0.0, mid.x, mid.y).normalize(),
                                pc,
                            );
                        }
                    }
                }
            } else {
                let wrap = Color::srgb_u32(0xDCE3E6);
                let n = 2;
                let w = (bx1 - 0.15 - bx0 - 0.1) / n as f32;
                for k in 0..n {
                    let x0 = bx0 + 0.1 + k as f32 * w;
                    m.cuboid2(
                        Vec3::new(x0 + 0.05, y, -hw + 0.12),
                        Vec3::new(x0 + w - 0.05, y + 0.15, hw - 0.12),
                        Color::srgb_u32(0xB89263),
                        Color::srgb_u32(0xB89263),
                    );
                    m.cuboid2(
                        Vec3::new(x0 + 0.1, y + 0.15, -hw + 0.16),
                        Vec3::new(x0 + w - 0.1, y + 1.05, hw - 0.16),
                        wrap,
                        shade(wrap, 1.05),
                    );
                    if near {
                        // Straps.
                        let mid = x0 + w * 0.5;
                        m.cuboid(
                            Vec3::new(mid - 0.04, y + 0.14, -hw + 0.14),
                            Vec3::new(mid + 0.04, y + 1.07, hw - 0.14),
                            stripe,
                        );
                    }
                }
            }
        }
        // Box van: tall cargo box with a livery stripe and rear doors.
        _ => {
            let (by0, by1) = (0.62, box_top);
            m.cuboid2(
                Vec3::new(bx0, by0, -hw),
                Vec3::new(bx1, by1, hw),
                cargo,
                shade(cargo, 1.06),
            );
            for z in [hw + 0.01, -hw - 0.01] {
                let (a, b) = if z > 0.0 { (bx0, bx1) } else { (bx1, bx0) };
                m.quad(
                    Vec3::new(a, 1.25, z),
                    Vec3::new(b, 1.25, z),
                    Vec3::new(b, 1.6, z),
                    Vec3::new(a, 1.6, z),
                    stripe,
                );
            }
            m.quad(
                Vec3::new(xb, 1.25, -hw),
                Vec3::new(xb, 1.25, hw),
                Vec3::new(xb, 1.6, hw),
                Vec3::new(xb, 1.6, -hw),
                stripe,
            );
            if !far {
                // Rear door seam, hinges, a roller-shutter look near.
                let seam = shade(cargo, 0.7);
                m.quad(
                    Vec3::new(xb - 0.002, by0 + 0.1, -0.03),
                    Vec3::new(xb - 0.002, by0 + 0.1, 0.03),
                    Vec3::new(xb - 0.002, by1 - 0.1, 0.03),
                    Vec3::new(xb - 0.002, by1 - 0.1, -0.03),
                    seam,
                );
                // Roof edge trim.
                m.cuboid(
                    Vec3::new(bx0 - 0.01, by1 - 0.06, -hw - 0.01),
                    Vec3::new(bx1 + 0.01, by1 + 0.01, hw + 0.01),
                    shade(cargo, 0.85),
                );
            }
            if near {
                for z in [-hw + 0.15, hw - 0.15] {
                    for y in [by0 + 0.4, by1 - 0.5] {
                        m.quad(
                            Vec3::new(xb - 0.004, y, z - 0.06),
                            Vec3::new(xb - 0.004, y, z + 0.06),
                            Vec3::new(xb - 0.004, y + 0.12, z + 0.06),
                            Vec3::new(xb - 0.004, y + 0.12, z - 0.06),
                            Color::srgb_u32(0x6E7378),
                        );
                    }
                }
                // Side marker lights.
                for x in [bx0 + 0.2, (bx0 + bx1) * 0.5, bx1 - 0.2] {
                    for z in [hw + 0.012, -hw - 0.012] {
                        m.glow(Glow::Sign, 0.0, |m| {
                            m.quad_facing(
                                [
                                    Vec3::new(x - 0.05, by0 + 0.15, z),
                                    Vec3::new(x + 0.05, by0 + 0.15, z),
                                    Vec3::new(x + 0.05, by0 + 0.22, z),
                                    Vec3::new(x - 0.05, by0 + 0.22, z),
                                ],
                                Vec3::Z * z.signum(),
                                Color::srgb_u32(0xF2A33D),
                            )
                        });
                    }
                }
            }
        }
    }
    let head = Color::srgb(0.98, 0.96, 0.85);
    let tail = Color::srgb(0.75, 0.12, 0.1);
    for s in [-1.0f32, 1.0] {
        let z = s * (hw - 0.28);
        m.glow(Glow::Headlight, 0.0, |m| {
            m.quad(
                Vec3::new(hl + 0.006, 0.62, z + 0.18),
                Vec3::new(hl + 0.006, 0.62, z - 0.18),
                Vec3::new(hl + 0.006, 0.8, z - 0.18),
                Vec3::new(hl + 0.006, 0.8, z + 0.18),
                head,
            )
        });
        m.glow(Glow::TailLight, 0.0, |m| {
            m.quad(
                Vec3::new(xb - 0.004, 0.7, z - 0.15),
                Vec3::new(xb - 0.004, 0.7, z + 0.15),
                Vec3::new(xb - 0.004, 0.88, z + 0.15),
                Vec3::new(xb - 0.004, 0.88, z - 0.15),
                tail,
            )
        });
    }
    if far {
        return m;
    }
    for x in [hl - 0.75, -hl + 0.85] {
        for s in [-1.0f32, 1.0] {
            wheel(&mut m, Vec3::new(x, wr, s * (hw - 0.16)), wr, 0.28, s, lod);
        }
    }
    let sh = lin(Color::srgba(0.0, 0.0, 0.0, 1.0));
    m.quad_up(
        [
            Vec3::new(-hl + 0.1, 0.012, -hw + 0.05),
            Vec3::new(hl - 0.1, 0.012, -hw + 0.05),
            Vec3::new(hl - 0.1, 0.012, hw - 0.05),
            Vec3::new(-hl + 0.1, 0.012, hw - 0.05),
        ],
        [sh[0] + 0.02, sh[1] + 0.02, sh[2] + 0.02, 1.0],
    );
    m
}

/// A prism along x (pipe, tank) of radius `r` round `(cy, cz)` from `x0`
/// to `x1`, with end caps when `caps`.
#[allow(clippy::too_many_arguments)]
fn prism_x(m: &mut MeshBuilder, cy: f32, cz: f32, r: f32, x0: f32, x1: f32, sides: usize, color: Color, caps: bool) {
    let ring: Vec<Vec2> = (0..sides)
        .map(|i| {
            let a = (i as f32 + 0.5) / sides as f32 * std::f32::consts::TAU;
            Vec2::new(cy + r * a.cos(), cz + r * a.sin())
        })
        .collect();
    for i in 0..sides {
        let (a, b) = (ring[i], ring[(i + 1) % sides]);
        let mid = (a + b) * 0.5 - Vec2::new(cy, cz);
        m.quad_facing(
            [
                Vec3::new(x0, a.x, a.y),
                Vec3::new(x1, a.x, a.y),
                Vec3::new(x1, b.x, b.y),
                Vec3::new(x0, b.x, b.y),
            ],
            Vec3::new(0.0, mid.x, mid.y).normalize(),
            color,
        );
        if caps {
            for (x, n) in [(x0, -Vec3::X), (x1, Vec3::X)] {
                m.tri_facing(
                    Vec3::new(x, cy, cz),
                    Vec3::new(x, a.x, a.y),
                    Vec3::new(x, b.x, b.y),
                    n,
                    shade(color, 0.9),
                );
            }
        }
    }
}

/// Hauliers' liveries for the long-distance lorries: (cab, body, band).
pub const LORRY_LIVERIES: [(Color, Color, Color); 5] = [
    (
        Color::srgb(0.18, 0.3, 0.52),
        Color::srgb(0.9, 0.91, 0.92),
        Color::srgb(0.18, 0.3, 0.52),
    ),
    (
        Color::srgb(0.75, 0.16, 0.14),
        Color::srgb(0.75, 0.16, 0.14),
        Color::srgb(0.95, 0.94, 0.9),
    ),
    (
        Color::srgb(0.95, 0.94, 0.9),
        Color::srgb(0.25, 0.45, 0.33),
        Color::srgb(0.9, 0.7, 0.2),
    ),
    (
        Color::srgb(0.16, 0.17, 0.19),
        Color::srgb(0.55, 0.58, 0.6),
        Color::srgb(0.85, 0.45, 0.12),
    ),
    (
        Color::srgb(0.92, 0.72, 0.2),
        Color::srgb(0.92, 0.92, 0.9),
        Color::srgb(0.16, 0.17, 0.19),
    ),
];

/// Length and width of the moving van, metres (car length: the sim spaces
/// moving trips as cars).
pub const MOVING_VAN_SIZE: (f32, f32) = (4.36, 1.86);

/// Hire liveries of the moving vans: (cab, box, band).
pub const MOVING_LIVERIES: [(Color, Color, Color); 3] = [
    (
        Color::srgb(0.95, 0.94, 0.9),
        Color::srgb(0.95, 0.94, 0.9),
        Color::srgb(0.9, 0.45, 0.12),
    ),
    (
        Color::srgb(0.93, 0.75, 0.2),
        Color::srgb(0.93, 0.75, 0.2),
        Color::srgb(0.16, 0.17, 0.19),
    ),
    (
        Color::srgb(0.95, 0.94, 0.9),
        Color::srgb(0.95, 0.94, 0.9),
        Color::srgb(0.2, 0.52, 0.32),
    ),
];

/// A moving van (household moving in or out): a car-length hire van with
/// a tall box reaching over the cab ("Luton"), a band and lettering in the
/// hire company's colours, a roller shutter at the back. Within the car
/// body length the sim spaces such trips by (`balance::CAR_BODY_M`).
pub fn moving_van(livery: usize, lod: Lod) -> MeshBuilder {
    let (cab, boxc, band) = MOVING_LIVERIES[livery % MOVING_LIVERIES.len()];
    let mut m = lod.builder(0.0);
    let (len, wid) = MOVING_VAN_SIZE;
    let (hl, hw) = (len * 0.5, wid * 0.5);
    let far = lod == Lod::Far;
    let near = lod == Lod::Near;
    let wr = 0.34;
    let frame = shade(palette::TYRE, 1.4);
    m.cuboid(
        Vec3::new(-hl + 0.05, 0.3, -hw + 0.1),
        Vec3::new(hl - 0.1, 0.55, hw - 0.1),
        frame,
    );
    // Cab: a van nose with a raked windscreen.
    let cab_back = hl - 1.25;
    let prof = [
        Vec2::new(cab_back, 0.4),
        Vec2::new(hl, 0.4),
        Vec2::new(hl, 0.85),
        Vec2::new(hl - 0.45, 1.05),
        Vec2::new(hl - 0.85, 1.7),
        Vec2::new(cab_back, 1.72),
    ];
    let cw = hw - 0.05;
    extrude(&mut m, &prof, -cw, cw, cab, &|i| match i {
        0 => shade(cab, 0.4),
        1 => shade(cab, 0.85),
        2 => shade(cab, 1.05),
        3 => palette::CAR_GLASS,
        _ => shade(cab, 0.9),
    });
    if !far {
        side_windows(&mut m, hl - 0.85, cab_back + 0.1, 1.08, 1.6, cw);
        m.quad(
            Vec3::new(hl + 0.005, 0.5, -0.55),
            Vec3::new(hl + 0.005, 0.5, 0.55),
            Vec3::new(hl + 0.005, 0.8, 0.55),
            Vec3::new(hl + 0.005, 0.8, -0.55),
            Color::srgb_u32(0x2B2E33),
        );
        m.cuboid2(
            Vec3::new(hl - 0.05, 0.28, -hw),
            Vec3::new(hl + 0.06, 0.46, hw),
            Color::srgb_u32(0x2B2E33),
            Color::srgb_u32(0x3B3E43),
        );
    }
    // The box, overhanging the cab roof.
    let (bx0, bx1, by0, by1) = (-hl, hl - 0.75, 0.6, 2.55);
    m.cuboid2(
        Vec3::new(bx0, by0, -hw),
        Vec3::new(cab_back, by1, hw),
        boxc,
        shade(boxc, 1.04),
    );
    m.cuboid2(
        Vec3::new(cab_back, 1.78, -hw),
        Vec3::new(bx1, by1, hw),
        boxc,
        shade(boxc, 1.04),
    );
    if !far {
        // Rounded nose of the overhang.
        m.quad_facing(
            [
                Vec3::new(bx1, 1.78, -hw),
                Vec3::new(bx1, 1.78, hw),
                Vec3::new(bx1 + 0.12, 2.0, hw - 0.02),
                Vec3::new(bx1 + 0.12, 2.0, -hw + 0.02),
            ],
            Vec3::new(1.0, -0.5, 0.0).normalize(),
            shade(boxc, 0.92),
        );
        m.quad_facing(
            [
                Vec3::new(bx1 + 0.12, 2.0, -hw + 0.02),
                Vec3::new(bx1 + 0.12, 2.0, hw - 0.02),
                Vec3::new(bx1, by1, hw),
                Vec3::new(bx1, by1, -hw),
            ],
            Vec3::new(1.0, 0.3, 0.0).normalize(),
            shade(boxc, 0.98),
        );
    }
    // The band and "lettering" on both sides and the back.
    for z in [hw + 0.01, -hw - 0.01] {
        let s = z.signum();
        m.quad_facing(
            [
                Vec3::new(bx0, 1.2, z),
                Vec3::new(bx1, 1.2, z),
                Vec3::new(bx1, 1.55, z),
                Vec3::new(bx0, 1.55, z),
            ],
            Vec3::Z * s,
            band,
        );
        if !far {
            let ink = if band.to_srgba().red < 0.3 && band.to_srgba().green < 0.3 {
                boxc
            } else {
                band
            };
            // A round logo and the company name in thin strokes.
            let lc = Vec3::new(bx0 + 0.55, 1.98, z);
            let ring: Vec<Vec3> = (0..8)
                .map(|i| {
                    let t = i as f32 / 8.0 * std::f32::consts::TAU;
                    lc + Vec3::new(t.cos() * 0.26, t.sin() * 0.26, 0.0)
                })
                .collect();
            for i in 0..8 {
                m.tri_facing(lc, ring[i], ring[(i + 1) % 8], Vec3::Z * s, band);
            }
            let mut x = bx0 + 0.95;
            let mut k = 0;
            while x < bx1 - 0.6 {
                let w = 0.1 + 0.08 * ((k * 7 % 5) as f32 / 4.0);
                m.quad_facing(
                    [
                        Vec3::new(x, 1.9, z),
                        Vec3::new(x + w, 1.9, z),
                        Vec3::new(x + w, 2.08, z),
                        Vec3::new(x, 2.08, z),
                    ],
                    Vec3::Z * s,
                    if band.to_srgba().red < 0.3 { band } else { ink },
                );
                x += w + if k % 5 == 4 { 0.18 } else { 0.035 };
                k += 1;
            }
        }
    }
    let xb = bx0 - 0.01;
    m.quad_facing(
        [
            Vec3::new(xb, 1.2, -hw),
            Vec3::new(xb, 1.2, hw),
            Vec3::new(xb, 1.55, hw),
            Vec3::new(xb, 1.55, -hw),
        ],
        -Vec3::X,
        band,
    );
    if !far {
        // Roller shutter at the back, a loading step under it.
        let step = if near { 0.18 } else { 0.36 };
        let mut y = by0 + 0.15;
        while y < by1 - 0.1 {
            if !(1.15..1.6).contains(&y) {
                m.quad_facing(
                    [
                        Vec3::new(xb - 0.004, y, -hw + 0.08),
                        Vec3::new(xb - 0.004, y, hw - 0.08),
                        Vec3::new(xb - 0.004, y + 0.03, hw - 0.08),
                        Vec3::new(xb - 0.004, y + 0.03, -hw + 0.08),
                    ],
                    -Vec3::X,
                    shade(boxc, 0.75),
                );
            }
            y += step;
        }
        m.cuboid(
            Vec3::new(bx0 - 0.2, 0.38, -0.5),
            Vec3::new(bx0 + 0.02, 0.45, 0.5),
            palette::METAL_DARK,
        );
    }
    if near {
        for s in [1.0f32, -1.0] {
            let c = Vec3::new(hl - 0.95, 1.35, s * (hw + 0.1));
            m.cuboid(
                c - Vec3::new(0.04, 0.12, 0.05),
                c + Vec3::new(0.04, 0.12, 0.05),
                Color::srgb_u32(0x2B2E33),
            );
        }
        let plate = Color::srgb_u32(0xEDEBE3);
        m.quad_facing(
            [
                Vec3::new(hl + 0.065, 0.3, -0.25),
                Vec3::new(hl + 0.065, 0.3, 0.25),
                Vec3::new(hl + 0.065, 0.42, 0.25),
                Vec3::new(hl + 0.065, 0.42, -0.25),
            ],
            Vec3::X,
            plate,
        );
    }
    let head = Color::srgb(0.98, 0.96, 0.85);
    let tail = Color::srgb(0.75, 0.12, 0.1);
    for s in [-1.0f32, 1.0] {
        let z = s * (hw - 0.25);
        m.glow(Glow::Headlight, 0.0, |m| {
            m.quad_facing(
                [
                    Vec3::new(hl + 0.006, 0.62, z - 0.16),
                    Vec3::new(hl + 0.006, 0.62, z + 0.16),
                    Vec3::new(hl + 0.006, 0.78, z + 0.16),
                    Vec3::new(hl + 0.006, 0.78, z - 0.16),
                ],
                Vec3::X,
                head,
            )
        });
        m.glow(Glow::TailLight, 0.0, |m| {
            m.quad_facing(
                [
                    Vec3::new(xb - 0.006, 0.7, z - 0.12),
                    Vec3::new(xb - 0.006, 0.7, z + 0.12),
                    Vec3::new(xb - 0.006, 1.0, z + 0.12),
                    Vec3::new(xb - 0.006, 1.0, z - 0.12),
                ],
                -Vec3::X,
                tail,
            )
        });
    }
    if far {
        return m;
    }
    for x in [hl - 0.7, -hl + 0.8] {
        for s in [-1.0f32, 1.0] {
            wheel(&mut m, Vec3::new(x, wr, s * (hw - 0.15)), wr, 0.24, s, lod);
        }
    }
    let sh = lin(Color::srgba(0.0, 0.0, 0.0, 1.0));
    m.quad_up(
        [
            Vec3::new(-hl + 0.1, 0.012, -hw + 0.05),
            Vec3::new(hl - 0.1, 0.012, -hw + 0.05),
            Vec3::new(hl - 0.1, 0.012, hw - 0.05),
            Vec3::new(-hl + 0.1, 0.012, hw - 0.05),
        ],
        [sh[0] + 0.02, sh[1] + 0.02, sh[2] + 0.02, 1.0],
    );
    m
}

/// Extrude a convex side profile in (x, y) between `z0` and `z1`.
fn extrude(m: &mut MeshBuilder, prof: &[Vec2], z0: f32, z1: f32, side: Color, around: &dyn Fn(usize) -> Color) {
    let n = prof.len();
    let p = |i: usize, z: f32| Vec3::new(prof[i].x, prof[i].y, z);
    // Profile is counter-clockwise seen from +z.
    for i in 1..n - 1 {
        m.tri(p(0, z1), p(i, z1), p(i + 1, z1), side);
        m.tri(p(0, z0), p(i + 1, z0), p(i, z0), side);
    }
    for i in 0..n {
        let j = (i + 1) % n;
        m.quad(p(i, z0), p(j, z0), p(j, z1), p(i, z1), around(i));
    }
}

/// Glass strips on both sides of the cabin between `x1` (front) and `x0`.
fn side_windows(m: &mut MeshBuilder, x1: f32, x0: f32, y0: f32, y1: f32, z: f32) {
    let g = palette::CAR_GLASS;
    let z = z + 0.01;
    m.quad(
        Vec3::new(x0, y0, z),
        Vec3::new(x1, y0, z),
        Vec3::new(x1 - 0.15, y1, z),
        Vec3::new(x0 + 0.1, y1, z),
        g,
    );
    m.quad(
        Vec3::new(x1, y0, -z),
        Vec3::new(x0, y0, -z),
        Vec3::new(x0 + 0.1, y1, -z),
        Vec3::new(x1 - 0.15, y1, -z),
        g,
    );
}

/// Every vehicle model for the model sheet, one row per body type (a few
/// paints each), the trucks last: (near, mid, far).
pub fn sheet_models() -> Vec<Vec<[MeshBuilder; 3]>> {
    let mut rows: Vec<Vec<[MeshBuilder; 3]>> = (0..SHAPES)
        .map(|shape| {
            [0usize, 3, 2, 1, 5, 4]
                .iter()
                .map(|&i| {
                    let c = palette::CAR_BODIES[i];
                    Lod::ALL.map(|l| car_model(c, shape, l))
                })
                .collect()
        })
        .collect();
    rows.push(
        (0..MOVING_LIVERIES.len())
            .map(|i| Lod::ALL.map(|l| moving_van(i, l)))
            .collect(),
    );
    rows.push(
        (0..TRUCK_KINDS * 2)
            .map(|i| {
                let (a, b, c) = palette::TRUCK_LIVERIES[i as usize % palette::TRUCK_LIVERIES.len()];
                Lod::ALL.map(|l| truck_model(a, b, c, i % TRUCK_KINDS, l))
            })
            .collect(),
    );
    rows.push(
        (0..LORRY_KINDS)
            .map(|i| {
                let (a, b, c) = LORRY_LIVERIES[i as usize % LORRY_LIVERIES.len()];
                Lod::ALL.map(|l| lorry_model(a, b, c, i, l))
            })
            .collect(),
    );
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(m: &MeshBuilder) -> (Vec3, Vec3) {
        let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
        for p in &m.pos {
            min = min.min(Vec3::from(*p));
            max = max.max(Vec3::from(*p));
        }
        (min, max)
    }

    #[test]
    fn cars_fit_a_lane_and_sit_on_the_ground() {
        for shape in 0..SHAPES {
            for lod in Lod::ALL {
                let m = car_model(Color::WHITE, shape, lod);
                let (min, max) = bounds(&m);
                assert!(min.y >= -0.001 && min.y < 0.4, "{shape} {lod:?} {min}");
                assert!(
                    max.z - min.z < 2.1 && max.x - min.x <= 4.42,
                    "{shape} {lod:?} {min} {max}"
                );
                assert!((max.x + min.x).abs() < 0.2, "{shape} {lod:?} off centre {min} {max}");
                assert!(m.pos.iter().all(|p| p.iter().all(|v| v.is_finite())));
            }
            // The detailed model has its wheels on the road.
            let (min, _) = bounds(&car_model(Color::WHITE, shape, Lod::Near));
            assert!(min.y < 0.05, "{shape}");
        }
    }

    #[test]
    fn levels_of_detail_get_cheaper() {
        for shape in 0..SHAPES {
            let t = Lod::ALL.map(|l| car_model(Color::WHITE, shape, l).triangle_count());
            assert!(t[0] > t[1] && t[1] > t[2], "{shape}: {t:?}");
            assert!(t[1] <= 260 && t[2] <= 80, "{shape}: {t:?}");
        }
    }

    #[test]
    fn paints_cover_the_palette_with_believable_shares() {
        let mut n = [0u32; 12];
        for h in 0..10_000u32 {
            n[paint(super::super::noise::hash(h))] += 1;
        }
        assert_eq!(n.len(), palette::CAR_BODIES.len());
        assert!(n.iter().all(|&k| k > 0), "{n:?}");
        // Whites, silvers, greys and blacks are the bulk.
        assert!(n[..4].iter().sum::<u32>() > 5_500, "{n:?}");
    }

    #[test]
    fn moving_vans_fit_a_car_and_lorries_the_truck_length() {
        for lod in Lod::ALL {
            for i in 0..MOVING_LIVERIES.len() {
                let (min, max) = bounds(&moving_van(i, lod));
                assert!(
                    max.x - min.x <= citysim_sim::balance::CAR_BODY_M[0] + 0.02,
                    "{min} {max}"
                );
                assert!(
                    max.z - min.z <= citysim_sim::balance::CAR_BODY_M[1] + 0.25,
                    "{min} {max}"
                );
                assert!(min.y >= -0.001 && (max.x + min.x).abs() < 0.2);
            }
            for k in 0..LORRY_KINDS {
                let (a, b, c) = LORRY_LIVERIES[0];
                let (min, max) = bounds(&lorry_model(a, b, c, k, lod));
                assert!(
                    max.x - min.x <= citysim_sim::balance::TRUCK_BODY_M[0] + 0.02,
                    "{k} {min} {max}"
                );
                assert!(
                    max.z - min.z <= citysim_sim::balance::TRUCK_BODY_M[1] + 0.4,
                    "{k} {min} {max}"
                );
                assert!(min.y >= -0.001 && (max.x + min.x).abs() < 0.25, "{k} {min} {max}");
            }
        }
    }

    #[test]
    fn trucks_fit_the_queue_spacing_and_sit_on_the_ground() {
        for kind in 0..TRUCK_KINDS {
            for lod in Lod::ALL {
                let (c, k, s) = palette::TRUCK_LIVERIES[0];
                let m = truck_model(c, k, s, kind, lod);
                let (min, max) = bounds(&m);
                assert!(min.y >= -0.001, "{min}");
                assert!(max.x - min.x <= TRUCK_LEN + 0.2, "{min} {max}");
                assert!(max.z - min.z < 2.3, "{min} {max}");
                // Origin near the middle (LOD cross-fade and queue spacing).
                assert!((max.x + min.x).abs() < 0.2);
            }
        }
    }
}
