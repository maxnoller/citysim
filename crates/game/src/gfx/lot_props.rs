//! Small things that make a lot look lived in: driveways with a parked car,
//! garden sheds, patios with a table and parasol, washing lines, vegetable
//! and flower beds, picnic benches, pallets, bins. All write into a
//! [`MeshBuilder`] in a building model's local frame (front towards +z) and
//! are dropped from the distant level of detail where they are small.

use bevy::prelude::*;

use super::mesh_util::MeshBuilder;
use super::noise::Stream;
use super::palette::{self, lin, mix, shade};
use super::vehicle_gen;

/// Flat paving / earth patch from `(x0, z0)` to `(x1, z1)` at height `y`.
pub fn patch(m: &mut MeshBuilder, x0: f32, z0: f32, x1: f32, z1: f32, y: f32, color: Color) {
    m.quad_up(
        [
            Vec3::new(x0, y, z0),
            Vec3::new(x1, y, z0),
            Vec3::new(x1, y, z1),
            Vec3::new(x0, y, z1),
        ],
        lin(color),
    );
}

/// A parked car (no lights lit) centred at `c` heading along `yaw`.
pub fn parked_car(m: &mut MeshBuilder, rng: &mut Stream, c: Vec3, yaw: f32) {
    let body = rng.pick(&palette::CAR_BODIES);
    let shape = [0, 1, 4, 2, 0, 1][rng.below(6) as usize];
    let car = vehicle_gen::car_model(body, shape, m.lod);
    let start = m.pos.len();
    m.append(&car, c, yaw, Vec3::ONE, [1.0; 4]);
    // Parked: its lights stay dark at night.
    for uv in &mut m.uv[start..] {
        *uv = [0.0, 0.0];
    }
}

/// Garden shed with a lean-to roof, its door facing `face` (+1: +z).
pub fn shed(m: &mut MeshBuilder, rng: &mut Stream, c: Vec2, face: f32) {
    let wood = rng.pick(&[
        Color::srgb_u32(0x8A6A4A),
        Color::srgb_u32(0x6F7F5E),
        Color::srgb_u32(0xA6865E),
        Color::srgb_u32(0x7A8A99),
    ]);
    let (w, d) = (rng.range(1.8, 2.4), rng.range(1.5, 1.9));
    let (x0, x1, z0, z1) = (c.x - w * 0.5, c.x + w * 0.5, c.y - d * 0.5, c.y + d * 0.5);
    m.cuboid2(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 1.9, z1), wood, wood);
    // Lean-to roof: higher at the door side.
    let roof = shade(wood, 0.55);
    let (hf, hb) = (2.25, 1.95);
    let (zf, zb) = if face > 0.0 {
        (z1 + 0.15, z0 - 0.15)
    } else {
        (z0 - 0.15, z1 + 0.15)
    };
    let q = [
        Vec3::new(x0 - 0.15, hb, zb),
        Vec3::new(x1 + 0.15, hb, zb),
        Vec3::new(x1 + 0.15, hf, zf),
        Vec3::new(x0 - 0.15, hf, zf),
    ];
    m.quad_facing(q, Vec3::Y, roof);
    m.quad_facing(q, -Vec3::Y, shade(roof, 0.7));
    // Gable triangles closing the gap under the roof.
    for x in [x0, x1] {
        let n = if x == x0 { -Vec3::X } else { Vec3::X };
        m.tri_facing(
            Vec3::new(x, 1.9, zf.clamp(z0, z1)),
            Vec3::new(x, 1.9, zb.clamp(z0, z1)),
            Vec3::new(x, hf - 0.05, zf.clamp(z0, z1)),
            n,
            wood,
        );
    }
    // Door.
    let zd = if face > 0.0 { z1 + 0.02 } else { z0 - 0.02 };
    let n = Vec3::Z * face;
    m.quad_facing(
        [
            Vec3::new(c.x - 0.4, 0.05, zd),
            Vec3::new(c.x + 0.4, 0.05, zd),
            Vec3::new(c.x + 0.4, 1.7, zd),
            Vec3::new(c.x - 0.4, 1.7, zd),
        ],
        n,
        shade(wood, 0.7),
    );
}

/// Paved patio with a table, two chairs and sometimes a parasol.
pub fn patio(m: &mut MeshBuilder, rng: &mut Stream, x0: f32, z0: f32, x1: f32, z1: f32) {
    let stone = mix(palette::PAVING, palette::PATH, rng.f());
    patch(m, x0, z0, x1, z1, 0.06, stone);
    let c = Vec2::new((x0 + x1) * 0.5, (z0 + z1) * 0.5);
    m.detail_only(|m| {
        let wood = Color::srgb_u32(0x9C7650);
        m.cylinder(Vec3::new(c.x, 0.06, c.y), 0.06, 0.7, 5, wood, wood);
        m.cylinder(Vec3::new(c.x, 0.72, c.y), 0.55, 0.06, 8, wood, shade(wood, 1.1));
        for s in [-1.0f32, 1.0] {
            let p = Vec3::new(c.x + s * 0.85, 0.06, c.y);
            m.cuboid(
                p - Vec3::new(0.22, 0.0, 0.22),
                p + Vec3::new(0.22, 0.45, 0.22),
                palette::FENCE,
            );
            m.cuboid(
                p + Vec3::new(s * 0.18, 0.45, -0.22),
                p + Vec3::new(s * 0.24, 0.9, 0.22),
                palette::FENCE,
            );
        }
        if rng.chance(0.55) {
            let cloth = rng.pick(&[
                Color::srgb_u32(0xE0564A),
                Color::srgb_u32(0xF2E6CF),
                Color::srgb_u32(0x2F8FCB),
                Color::srgb_u32(0x3FA66B),
            ]);
            m.cylinder(Vec3::new(c.x, 0.78, c.y), 0.03, 1.5, 4, palette::FENCE, palette::FENCE);
            m.cone(Vec3::new(c.x, 2.05, c.y), 1.25, 0.45, 8, cloth, 0.0);
        }
    });
}

/// Washing line between two posts with a few clothes on it.
pub fn washing_line(m: &mut MeshBuilder, rng: &mut Stream, a: Vec2, b: Vec2) {
    m.detail_only(|m| {
        let post = Color::srgb_u32(0xC9CCCF);
        for p in [a, b] {
            m.cylinder(Vec3::new(p.x, 0.0, p.y), 0.04, 1.9, 4, post, post);
        }
        let d = b - a;
        let len = d.length();
        let yaw = -d.y.atan2(d.x);
        let mid = (a + b) * 0.5;
        m.box_yaw(
            Vec3::new(mid.x, 1.82, mid.y),
            Vec3::new(len, 0.02, 0.02),
            yaw,
            post,
            post,
        );
        let n = 3 + rng.below(3);
        let colors = [
            Color::srgb_u32(0xF7F1E5),
            Color::srgb_u32(0x2F8FCB),
            Color::srgb_u32(0xE0564A),
            Color::srgb_u32(0xF2B33D),
            Color::srgb_u32(0xB8CAD6),
            Color::srgb_u32(0xE7C4B5),
        ];
        for k in 0..n {
            let t = (k as f32 + 0.7) / (n as f32 + 0.4);
            let p = a + d * t;
            let w = rng.range(0.35, 0.6);
            let h = rng.range(0.4, 0.7);
            let col = rng.pick(&colors);
            m.box_yaw(Vec3::new(p.x, 1.8 - h, p.y), Vec3::new(w, h, 0.03), yaw, col, col);
        }
    });
}

/// Raised vegetable beds: earth with rows of green.
pub fn veg_beds(m: &mut MeshBuilder, rng: &mut Stream, x0: f32, z0: f32, x1: f32, z1: f32) {
    let earth = shade(palette::EARTH, 0.62);
    let rows = ((x1 - x0) / 1.1).floor().max(1.0) as usize;
    let w = (x1 - x0) / rows as f32;
    for r in 0..rows {
        let (a, b) = (x0 + w * r as f32 + 0.1, x0 + w * (r + 1) as f32 - 0.1);
        m.cuboid2(
            Vec3::new(a, 0.0, z0),
            Vec3::new(b, 0.22, z1),
            Color::srgb_u32(0x8C6A48),
            earth,
        );
        m.detail_only(|m| {
            let green = mix(palette::HEDGE, palette::GRASS_LIGHT, rng.f());
            let mut z = z0 + 0.35;
            while z < z1 - 0.25 {
                m.cuboid(
                    Vec3::new((a + b) * 0.5 - 0.18, 0.22, z - 0.12),
                    Vec3::new((a + b) * 0.5 + 0.18, 0.42, z + 0.12),
                    green,
                );
                z += 0.5;
            }
        });
    }
}

/// Flower bed strip along x with coloured blooms.
pub fn flower_bed(m: &mut MeshBuilder, rng: &mut Stream, x0: f32, x1: f32, z0: f32, z1: f32) {
    patch(m, x0, z0, x1, z1, 0.05, shade(palette::EARTH, 0.7));
    m.detail_only(|m| {
        let blooms = [
            Color::srgb_u32(0xE86A8A),
            Color::srgb_u32(0xF2C94C),
            Color::srgb_u32(0xB07CD8),
            Color::srgb_u32(0xF7F1E5),
            Color::srgb_u32(0xE0564A),
        ];
        let mut x = x0 + 0.25;
        while x < x1 - 0.2 {
            let z = rng.range(z0 + 0.15, z1 - 0.15);
            let col = rng.pick(&blooms);
            let h = rng.range(0.2, 0.4);
            m.cuboid2(
                Vec3::new(x - 0.12, 0.05, z - 0.12),
                Vec3::new(x + 0.12, h, z + 0.12),
                shade(palette::HEDGE, 0.9),
                col,
            );
            x += rng.range(0.3, 0.5);
        }
    });
}

/// Wheelie bins by the front of a lot.
pub fn bins(m: &mut MeshBuilder, rng: &mut Stream, p: Vec2) {
    m.detail_only(|m| {
        let n = 1 + rng.below(2);
        for k in 0..n {
            let col = [
                Color::srgb_u32(0x2F5D49),
                Color::srgb_u32(0x3F4A55),
                Color::srgb_u32(0x2F6FB5),
            ][k as usize];
            let c = Vec3::new(p.x + k as f32 * 0.65, 0.0, p.y);
            m.cuboid2(
                c - Vec3::new(0.28, 0.0, 0.3),
                c + Vec3::new(0.28, 1.0, 0.3),
                col,
                shade(col, 1.2),
            );
        }
    });
}

/// Stack of pallets (industrial yards).
pub fn pallets(m: &mut MeshBuilder, rng: &mut Stream, c: Vec2) {
    let wood = Color::srgb_u32(0xB89263);
    let n = 1 + rng.below(4);
    for k in 0..n {
        let y = k as f32 * 0.16;
        m.cuboid2(
            Vec3::new(c.x - 0.6, y, c.y - 0.5),
            Vec3::new(c.x + 0.6, y + 0.14, c.y + 0.5),
            shade(wood, 0.85),
            wood,
        );
    }
    if rng.chance(0.5) {
        let y = n as f32 * 0.16;
        let wrap = Color::srgb_u32(0xDCE3E6);
        m.cuboid(
            Vec3::new(c.x - 0.55, y, c.y - 0.45),
            Vec3::new(c.x + 0.55, y + 0.9, c.y + 0.45),
            wrap,
        );
    }
}

/// Café tables with parasols in front of a shop.
pub fn cafe_seating(m: &mut MeshBuilder, rng: &mut Stream, x0: f32, x1: f32, z: f32, color: Color) {
    m.detail_only(|m| {
        let mut x = x0 + 0.8;
        while x < x1 - 0.6 {
            let c = Vec3::new(x, 0.0, z);
            m.cylinder(c, 0.35, 0.72, 6, palette::FENCE, shade(palette::FENCE, 1.05));
            for s in [-1.0f32, 1.0] {
                let p = c + Vec3::new(s * 0.55, 0.0, 0.0);
                m.cuboid(
                    p - Vec3::new(0.17, 0.0, 0.17),
                    p + Vec3::new(0.17, 0.45, 0.17),
                    shade(color, 0.8),
                );
            }
            if rng.chance(0.7) {
                m.cylinder(c + Vec3::Y * 0.72, 0.03, 1.4, 4, palette::FENCE, palette::FENCE);
                m.cone(c + Vec3::Y * 2.0, 1.0, 0.4, 8, color, 0.0);
            }
            x += 2.0;
        }
    });
}

/// Solar panels on a roof plane: `origin` at the lower-left corner, `u`
/// along the eave, `v` up the slope (both with length), normal `n`.
pub fn solar_panels(m: &mut MeshBuilder, origin: Vec3, u: Vec3, v: Vec3, n: Vec3) {
    let cols = ((u.length() / 1.05).floor() as usize).max(1);
    let rows = ((v.length() / 1.7).floor() as usize).clamp(1, 2);
    let (du, dv) = (u / cols as f32, v / rows as f32);
    let panel = Color::srgb_u32(0x23314A);
    let frame = Color::srgb_u32(0xA9B2BA);
    for i in 0..cols {
        for j in 0..rows {
            let o = origin + du * i as f32 + dv * j as f32 + n * 0.06;
            let (a, b) = (du * 0.06, dv * 0.06);
            m.quad_facing([o, o + du, o + du + dv, o + dv], n, frame);
            let o2 = o + a + b + n * 0.01;
            let (u2, v2) = (du - a * 2.0, dv - b * 2.0);
            m.quad_facing([o2, o2 + u2, o2 + u2 + v2, o2 + v2], n, panel);
        }
    }
}
