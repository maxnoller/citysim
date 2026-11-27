//! Roundabouts (`citysim_sim::roundabout`): the ring drawn as part of its
//! junction. The circulating carriageway joins every arm's cut with kerb
//! fillets onto the outer kerb; a pavement runs round the outside; the
//! central island has a mountable apron (or is a low painted dome on a mini
//! roundabout) and a planting or a small feature chosen by the node's
//! seed; markings: the lane line of a two-lane ring, arrows in the
//! direction of circulation, a zebra on every arm a car length out,
//! splitter islands on street arms; lamps round the pavement. The give-way
//! lines and signs at the entries are drawn with the other junction
//! controls (`signals.rs`). Pure (no ECS).

use std::f32::consts::TAU;

use bevy::prelude::*;
use citysim_sim::roundabout::{RingDims, RoundaboutSize};
use citysim_sim::{NodeId, World};

use super::mesh_util::{Glow, MeshBuilder};
use super::palette::{self, lin, shade};
use super::road_geom::{Arm, MARK_Y, NodeLayout, ROAD_Y, WALK_Y, lift, perp, vface};

/// What a roundabout node needs drawn beyond its arms.
#[derive(Debug, Clone, PartialEq)]
pub struct RingDraw {
    pub size: RoundaboutSize,
    pub center: Vec2,
    pub dims: RingDims,
    /// Stable per node: picks the island's planting.
    pub seed: u32,
    /// Street arms that get a splitter island (by arm order).
    pub splitters: Vec<bool>,
}

impl RingDraw {
    pub fn of(world: &World, n: NodeId) -> Option<RingDraw> {
        let ring = citysim_sim::roundabout::ring(world, n)?;
        let c = Vec2::from(ring.center);
        // A seed from the position: the same island after a save and load.
        let seed = (c.x.to_bits().wrapping_mul(0x9E37_79B1) ^ c.y.to_bits().wrapping_mul(0x85EB_CA77)).rotate_left(7);
        Some(RingDraw {
            size: ring.size,
            center: c,
            dims: ring.dims,
            seed,
            splitters: ring.arms.iter().map(|a| a.splitter).collect(),
        })
    }
}

/// Height of the apron (trucks run over it) and of the island's kerb top.
const APRON_Y: f32 = ROAD_Y + 0.05;
const ISLAND_Y: f32 = WALK_Y + 0.06;

fn dir(a: f32) -> Vec2 {
    Vec2::new(a.cos(), a.sin())
}

fn angle(v: Vec2) -> f32 {
    v.y.atan2(v.x)
}

fn quad_pt(a: Vec2, c: Vec2, b: Vec2, t: f32) -> Vec2 {
    let u = 1.0 - t;
    a * (u * u) + c * (2.0 * u * t) + b * (t * t)
}

/// The kerb from arm `a`'s right edge round the ring to arm `b`'s left
/// edge, `extra_a` / `extra_b` metres outside the carriageway at each end
/// (blending between them): a fillet onto the circle, the circle, a fillet
/// off it. The same number of points whatever the extra, so kerb and
/// pavement pair up.
fn piece(r: &RingDraw, a: &Arm, b: &Arm, extra_a: f32, extra_b: f32) -> Vec<Vec2> {
    let c = r.center;
    let r0 = r.dims.outer_r;
    let start0 = a.center + perp(a.dir) * a.style.h;
    let end0 = b.center - perp(b.dir) * b.style.h;
    let (s_ang, mut e_ang) = (angle(start0 - c), angle(end0 - c));
    while e_ang <= s_ang {
        e_ang += TAU;
    }
    let fillet = 2.0f32.min((e_ang - s_ang) * r0 * 0.3);
    let a0 = s_ang + fillet / r0;
    let a1 = (e_ang - fillet / r0).max(a0);
    const F: usize = 6;
    let arc_steps = (((a1 - a0) * r0) / 1.2).ceil().max(1.0) as usize;
    let total = (2 * F + arc_steps) as f32;
    let mut out = Vec::with_capacity(2 * F + arc_steps + 3);
    let extra = |k: usize| extra_a + (extra_b - extra_a) * (k as f32 / total);
    let mut k = 0;
    // Fillet onto the circle.
    let e = extra(0);
    let start = a.center + perp(a.dir) * (a.style.h + e);
    for i in 0..F {
        let ex = extra(k);
        let t = i as f32 / F as f32;
        out.push(quad_pt(start, c + dir(s_ang) * (r0 + ex), c + dir(a0) * (r0 + ex), t));
        k += 1;
    }
    for i in 0..=arc_steps {
        let ex = extra(k);
        out.push(c + dir(a0 + (a1 - a0) * i as f32 / arc_steps as f32) * (r0 + ex));
        k += 1;
    }
    let e = extra_b;
    let end = b.center - perp(b.dir) * (b.style.h + e);
    for i in 1..=F {
        let ex = extra(k);
        let t = i as f32 / F as f32;
        out.push(quad_pt(c + dir(a1) * (r0 + ex), c + dir(e_ang) * (r0 + ex), end, t));
        k += 1;
    }
    out
}

/// Pavement width of the ring (the sidewalks of the arms blend into it).
fn pavement(r: &RingDraw) -> f32 {
    r.dims.pavement_m
}

/// Sidewalk between paired kerb and outer points (kerb stone, paving,
/// faces), as the junctions have.
fn walk(m: &mut MeshBuilder, kerb: &[Vec2], outer: &[Vec2]) {
    let walk = lin(palette::SIDEWALK);
    let kerb_col = lin(palette::KERB);
    for k in 0..kerb.len().min(outer.len()).saturating_sub(1) {
        let (k0, k1, o0, o1) = (kerb[k], kerb[k + 1], outer[k], outer[k + 1]);
        let w0 = (o0 - k0).length().max(1e-4);
        let w1 = (o1 - k1).length().max(1e-4);
        let m0 = k0 + (o0 - k0) * (0.22 / w0).min(1.0);
        let m1 = k1 + (o1 - k1) * (0.22 / w1).min(1.0);
        m.quad_up([k0, k1, m1, m0].map(|p| lift(p, WALK_Y)), kerb_col);
        m.quad_up([m0, m1, o1, o0].map(|p| lift(p, WALK_Y)), walk);
        let to_road = ((k0 + k1) - (o0 + o1)).normalize_or_zero();
        vface(m, k0, k1, ROAD_Y - 0.02, WALK_Y, to_road, palette::KERB);
        vface(m, o0, o1, 0.0, WALK_Y, -to_road, shade(palette::KERB, 0.92));
    }
}

/// A ring of quads between radii `r0` and `r1` at height `y`.
fn annulus(m: &mut MeshBuilder, c: Vec2, r0: f32, r1: f32, y: f32, col: [f32; 4]) {
    let n = ((r1 * TAU) / 1.5).ceil().max(16.0) as usize;
    for i in 0..n {
        let (a, b) = (i as f32 / n as f32 * TAU, (i + 1) as f32 / n as f32 * TAU);
        let q = [c + dir(a) * r0, c + dir(b) * r0, c + dir(b) * r1, c + dir(a) * r1];
        m.quad_up(q.map(|p| lift(p, y)), col);
    }
}

/// Vertical face round a circle from `y0` to `y1`, facing out.
fn round_face(m: &mut MeshBuilder, c: Vec2, r: f32, y0: f32, y1: f32, color: Color) {
    let n = ((r * TAU) / 1.5).ceil().max(16.0) as usize;
    for i in 0..n {
        let (a, b) = (i as f32 / n as f32 * TAU, (i + 1) as f32 / n as f32 * TAU);
        let (p, q) = (c + dir(a) * r, c + dir(b) * r);
        vface(m, p, q, y0, y1, (p + q) * 0.5 - c, color);
    }
}

/// Circulation direction at angle `a` (the island on the left).
fn tangent(a: f32) -> Vec2 {
    Vec2::new(a.sin(), -a.cos())
}

/// A painted arrow at `p` pointing along `d`.
fn arrow(m: &mut MeshBuilder, p: Vec2, d: Vec2, len: f32, c: [f32; 4]) {
    let n = perp(d);
    let tail = p - d * len * 0.5;
    let neck = p + d * len * 0.15;
    let tip = p + d * len * 0.5;
    let w = 0.12;
    m.quad_up(
        [tail - n * w, neck - n * w, neck + n * w, tail + n * w].map(|q| lift(q, MARK_Y + 0.003)),
        c,
    );
    m.tri_up(
        lift(neck - n * 0.42, MARK_Y + 0.003),
        lift(tip, MARK_Y + 0.003),
        lift(neck + n * 0.42, MARK_Y + 0.003),
        c,
    );
}

/// The ring of node `node`: carriageway, pavement, island, markings and
/// lamps.
pub fn mesh_ring(m: &mut MeshBuilder, node: &NodeLayout, r: &RingDraw) {
    let arms = &node.arms;
    let k = arms.len();
    if k == 0 {
        return;
    }
    let c = r.center;
    let d = &r.dims;
    let asphalt = lin(palette::ASPHALT);
    // Carriageway: the arm cuts joined round the outer kerb.
    let mut outline: Vec<Vec2> = Vec::new();
    for i in 0..k {
        let (a, b) = (&arms[i], &arms[(i + 1) % k]);
        outline.push(a.center - perp(a.dir) * a.style.h);
        let kerb = piece(r, a, b, 0.0, 0.0);
        outline.extend(&kerb);
        let (wa, wb) = (a.style.walk.max(0.5), b.style.walk.max(0.5));
        let mid = pavement(r);
        // Pavement round the outside, the arms' sidewalks blending into it.
        let outer_a = piece(r, a, b, wa, mid);
        let outer_b = piece(r, a, b, mid, wb);
        let n = kerb.len();
        let half = n / 2;
        let mut outer: Vec<Vec2> = outer_a[..half].to_vec();
        outer.extend(&outer_b[half..]);
        walk(m, &kerb, &outer);
    }
    let ring3: Vec<Vec3> = outline.iter().map(|&q| lift(q, ROAD_Y)).collect();
    m.fan_up(lift(c, ROAD_Y), &ring3, asphalt);
    let white = lin(palette::MARKING);
    // Lane line of a two-lane ring: dashes round it.
    if d.lanes == 2 {
        let rl = d.outer_r - d.lane_w;
        let n = ((rl * TAU) / 4.0).floor().max(8.0) as usize;
        for i in 0..n {
            let a0 = i as f32 / n as f32 * TAU;
            let a1 = a0 + (2.0 / rl);
            let q = [
                c + dir(a0) * (rl - 0.07),
                c + dir(a1) * (rl - 0.07),
                c + dir(a1) * (rl + 0.07),
                c + dir(a0) * (rl + 0.07),
            ];
            m.quad_up(q.map(|p| lift(p, MARK_Y)), white);
        }
    }
    // Edge line along the apron.
    {
        let n = ((d.inner_r * TAU) / 1.0).ceil().max(24.0) as usize;
        for i in 0..n {
            let (a0, a1) = (i as f32 / n as f32 * TAU, (i + 1) as f32 / n as f32 * TAU);
            let rr = d.inner_r + 0.25;
            let q = [
                c + dir(a0) * (rr - 0.08),
                c + dir(a1) * (rr - 0.08),
                c + dir(a1) * (rr + 0.08),
                c + dir(a0) * (rr + 0.08),
            ];
            m.quad_up(q.map(|p| lift(p, MARK_Y)), white);
        }
    }
    // Arrows round the ring, just past each entry, in every lane.
    for a in arms {
        let ang = angle(a.center - c);
        for lane in 0..d.lanes {
            let rl = d.lane_r[lane as usize];
            let at = ang - (a.style.h + 4.0) / rl;
            arrow(m, c + dir(at) * rl, tangent(at), 2.6, white);
        }
    }
    // The island: the apron trucks may run over, then the raised island.
    if d.mountable {
        // A low painted dome: white edge, paved middle.
        annulus(m, c, d.island_r - 0.45, d.island_r, ROAD_Y + 0.03, white);
        m.cylinder(
            lift(c, ROAD_Y),
            d.island_r - 0.45,
            0.06,
            28,
            palette::PAVING,
            palette::PAVING,
        );
        m.cylinder(
            lift(c, ROAD_Y + 0.06),
            (d.island_r - 0.45) * 0.55,
            0.04,
            20,
            palette::PAVING,
            shade(palette::PAVING, 1.04),
        );
    } else {
        // Cobbled apron: setts in two tones.
        let (sett_a, sett_b) = (lin(shade(palette::PAVING, 0.78)), lin(shade(palette::PAVING, 0.7)));
        let n = ((d.inner_r * TAU) / 0.9).ceil() as usize;
        for i in 0..n {
            let (a0, a1) = (i as f32 / n as f32 * TAU, (i + 1) as f32 / n as f32 * TAU);
            let q = [
                c + dir(a0) * d.island_r,
                c + dir(a1) * d.island_r,
                c + dir(a1) * d.inner_r,
                c + dir(a0) * d.inner_r,
            ];
            m.quad_up(q.map(|p| lift(p, APRON_Y)), if i % 2 == 0 { sett_a } else { sett_b });
        }
        round_face(m, c, d.inner_r, ROAD_Y - 0.02, APRON_Y, shade(palette::KERB, 0.9));
        annulus(m, c, d.island_r - 0.3, d.island_r, ISLAND_Y, lin(palette::KERB));
        round_face(m, c, d.island_r, APRON_Y - 0.02, ISLAND_Y, palette::KERB);
        island(m, r);
    }
    // Splitter islands on street arms and a zebra a car length out.
    for (i, a) in arms.iter().enumerate() {
        if a.style.highway {
            continue;
        }
        if r.splitters.get(i).copied().unwrap_or(false) && a.style.median <= 0.0 {
            splitter(m, a);
        }
        zebra(m, a);
    }
}

/// A narrow raised island on a street arm's centre line from the ring
/// outward, with a painted approach.
fn splitter(m: &mut MeshBuilder, a: &Arm) {
    let len = 7.0;
    let w = 0.3;
    let p0 = a.at(-0.3, 0.0);
    let p1 = a.at(len, 0.0);
    let n = perp(a.dir);
    let outline = [p0 - n * w, p1 - n * w * 0.4, p1 + n * w * 0.4, p0 + n * w];
    m.prism(&outline, ROAD_Y, WALK_Y, palette::KERB, Some(palette::MEDIAN_GRASS));
    // A keep-right bollard at its nose.
    let nose = a.at(0.2, 0.0);
    m.cylinder(lift(nose, WALK_Y), 0.12, 0.85, 8, palette::MARKING, palette::MARKING);
    m.glow(Glow::Lamp, 0.0, |m| {
        m.cylinder(
            lift(nose, WALK_Y + 0.85),
            0.13,
            0.12,
            8,
            palette::LAMP_GLASS,
            palette::LAMP_GLASS,
        )
    });
    // Painted taper beyond it.
    let white = lin(palette::MARKING);
    let t0 = len;
    let t1 = len + 6.0;
    for s in [-1.0f32, 1.0] {
        let q = [
            a.at(t0, s * w * 0.4),
            a.at(t1, s * 0.06),
            a.at(t1, s * 0.06 + s * 0.12),
            a.at(t0, s * w * 0.4 + s * 0.12),
        ];
        m.quad_up(q.map(|p| lift(p, MARK_Y + 0.002)), white);
    }
}

/// Zebra crossing across an arm, one car length out from the ring.
fn zebra(m: &mut MeshBuilder, a: &Arm) {
    let white = lin(palette::MARKING);
    let (t0, t1) = (5.0, 7.4);
    let stripe = 0.5;
    let mut o = -a.style.h + 0.4;
    while o + stripe <= a.style.h - 0.4 + 1e-3 {
        // Leave the splitter / median free.
        let mid = o + stripe * 0.5;
        if mid.abs() > a.style.median.max(0.35) {
            let q = [a.at(t0, o), a.at(t1, o), a.at(t1, o + stripe), a.at(t0, o + stripe)];
            m.quad_up(q.map(|p| lift(p, MARK_Y + 0.004)), white);
        }
        o += stripe * 2.0;
    }
}

/// A small deterministic random number from a seed.
fn rnd(seed: u32, k: u32) -> f32 {
    let mut x = seed ^ k.wrapping_mul(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    (x & 0xFFFF) as f32 / 65535.0
}

/// A round deciduous tree: trunk and two stacked canopies.
fn tree(m: &mut MeshBuilder, p: Vec2, scale: f32, tint: u32) {
    let base = lift(p, ISLAND_Y);
    m.cylinder(base, 0.16 * scale, 1.8 * scale, 7, palette::TRUNK, palette::TRUNK);
    let col = palette::CANOPIES[tint as usize % palette::CANOPIES.len()];
    m.cone(base + Vec3::Y * 1.3 * scale, 1.6 * scale, 2.6 * scale, 9, col, 0.1);
    m.cone(
        base + Vec3::Y * 2.5 * scale,
        1.2 * scale,
        2.1 * scale,
        9,
        shade(col, 1.08),
        0.35,
    );
}

/// The island's planting or feature, by size and seed.
fn island(m: &mut MeshBuilder, r: &RingDraw) {
    let c = r.center;
    let ir = r.dims.island_r - 0.3;
    // Grass.
    m.cylinder(
        lift(c, ISLAND_Y - 0.02),
        ir,
        0.02,
        40,
        palette::MEDIAN_GRASS,
        palette::MEDIAN_GRASS,
    );
    let style = r.seed % 3;
    let flowers = [palette::ACCENTS[0], palette::ACCENTS[2], c_rgb(0xD878B8)];
    match (r.size, style) {
        // Small: one big tree, or a stone feature with a flower bed.
        (RoundaboutSize::Small, 0) | (RoundaboutSize::Small, 2) => {
            tree(m, c, 1.3, r.seed);
            ring_of_shrubs(m, c, ir * 0.7, 9, r.seed);
        }
        (RoundaboutSize::Small, _) => {
            flower_bed(m, c, ir * 0.6, flowers[(r.seed / 3) as usize % 3]);
            stone(m, c, 0.9, 2.2);
        }
        // Medium and large: a circle of trees, a feature in the middle.
        (_, s) => {
            let trees = if r.size == RoundaboutSize::Large { 9 } else { 5 };
            let tr = ir * 0.68;
            for i in 0..trees {
                let a = i as f32 / trees as f32 * TAU + rnd(r.seed, 1) * TAU;
                tree(m, c + dir(a) * tr, 0.9 + 0.35 * rnd(r.seed, 10 + i), r.seed + i);
            }
            match s {
                0 => {
                    flower_bed(m, c, ir * 0.35, flowers[(r.seed / 3) as usize % 3]);
                    stone(m, c, 1.1, 3.2);
                }
                1 => {
                    // A paved circle with a low fountain basin.
                    m.cylinder(
                        lift(c, ISLAND_Y - 0.01),
                        ir * 0.38,
                        0.03,
                        32,
                        palette::PAVING,
                        palette::PAVING,
                    );
                    m.cylinder(lift(c, ISLAND_Y), 1.8, 0.5, 24, palette::KERB, c_rgb(0x5E8FB0));
                    m.cylinder(lift(c, ISLAND_Y + 0.5), 0.35, 1.0, 10, palette::KERB, palette::KERB);
                }
                _ => {
                    tree(m, c, 1.7, r.seed + 99);
                    ring_of_shrubs(m, c, ir * 0.42, 12, r.seed);
                }
            }
        }
    }
}

const fn c_rgb(hex: u32) -> Color {
    Color::srgb(
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
    )
}

fn ring_of_shrubs(m: &mut MeshBuilder, c: Vec2, rad: f32, n: u32, seed: u32) {
    for i in 0..n {
        let a = i as f32 / n as f32 * TAU;
        let s = 0.45 + 0.2 * rnd(seed, 50 + i);
        let p = c + dir(a) * rad;
        m.cone(
            lift(p, ISLAND_Y - 0.05),
            s * 1.1,
            s * 1.4,
            7,
            palette::HEDGE,
            rnd(seed, 70 + i),
        );
    }
}

fn flower_bed(m: &mut MeshBuilder, c: Vec2, rad: f32, col: Color) {
    m.cylinder(lift(c, ISLAND_Y - 0.01), rad, 0.12, 28, palette::EARTH, col);
}

/// A plain stone column (a small monument).
fn stone(m: &mut MeshBuilder, c: Vec2, w: f32, h: f32) {
    m.box_yaw(
        lift(c, ISLAND_Y + 0.1),
        Vec3::new(w * 1.4, 0.2, w * 1.4),
        0.4,
        palette::KERB,
        palette::KERB,
    );
    m.box_yaw(
        lift(c, ISLAND_Y + 0.2 + h * 0.5),
        Vec3::new(w, h, w),
        0.4,
        shade(palette::PAVING, 0.95),
        palette::PAVING,
    );
}

/// Lamp posts round the pavement: `(base, tip over the ring)`, clear of the
/// arms.
pub fn lamp_spots(node: &NodeLayout, r: &RingDraw) -> Vec<(Vec2, Vec2)> {
    let c = r.center;
    let base_r = r.dims.outer_r + pavement(r) * 0.7;
    let n = ((base_r * TAU) / 20.0).round().max(4.0) as usize;
    let mut out = Vec::new();
    for i in 0..n {
        let a = (i as f32 + 0.5) / n as f32 * TAU;
        let p = c + dir(a) * base_r;
        let clear = node.arms.iter().all(|arm| {
            let rel = p - arm.center;
            let along = rel.dot(arm.dir);
            let side = rel.dot(perp(arm.dir)).abs();
            !(along > -4.0 && side < arm.style.outer() + 1.5)
        });
        if clear {
            out.push((p, c + dir(a) * (base_r - 1.6)));
        }
    }
    out
}

/// The lamp posts themselves (same look as the streets').
pub fn mesh_lamps(m: &mut MeshBuilder, node: &NodeLayout, r: &RingDraw) {
    let pole = palette::LAMP;
    for (base, tip) in lamp_spots(node, r) {
        m.box_yaw(lift(base, WALK_Y), Vec3::new(0.14, 4.6, 0.14), 0.0, pole, pole);
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
