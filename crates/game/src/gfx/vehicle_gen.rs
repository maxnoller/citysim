//! Low-poly car and delivery-truck models (body, cabin with glass, wheels).
//! Local frame: the vehicle points along +x, centred on the origin, wheels on
//! `y = 0`.

use bevy::prelude::*;

use super::mesh_util::{Glow, MeshBuilder};
use super::palette::{self, lin, shade};

/// Number of body shapes.
pub const SHAPES: u32 = 5;

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

/// Six-sided wheel along z with a cap on the outer side only.
fn wheel(m: &mut MeshBuilder, c: Vec3, r: f32, w: f32) {
    let n = 6;
    let p = |i: usize, z: f32| {
        let a = (i as f32 + 0.5) / n as f32 * std::f32::consts::TAU;
        Vec3::new(c.x + r * a.cos(), c.y + r * a.sin(), z)
    };
    let s = if c.z > 0.0 { 1.0 } else { -1.0 };
    let zo = c.z + s * w * 0.5;
    for i in 0..n {
        let j = (i + 1) % n;
        // Tread, facing away from the axle.
        m.quad(
            p(i, c.z - w * 0.5),
            p(j, c.z - w * 0.5),
            p(j, c.z + w * 0.5),
            p(i, c.z + w * 0.5),
            palette::TYRE,
        );
    }
    let hub = shade(palette::GUARDRAIL, 0.75);
    let cap: Vec<Vec3> = (0..n).map(|i| p(i, zo)).collect();
    for i in 1..n - 1 {
        if s > 0.0 {
            m.tri(cap[0], cap[i], cap[i + 1], hub);
        } else {
            m.tri(cap[0], cap[i + 1], cap[i], hub);
        }
    }
}

/// Length and width of a car shape, metres. Lengths fit the sim's 5.5 m
/// queue spacing (with lane compression).
pub fn car_size(shape: u32) -> (f32, f32) {
    match shape {
        2 => (4.3, 1.85),
        3 => (4.4, 1.85),
        1 => (3.6, 1.72),
        4 => (4.35, 1.8),
        _ => (4.0, 1.78),
    }
}

/// Width of the delivery truck, metres.
pub const TRUCK_WIDTH: f32 = 1.94;

/// A car centred at `c`, rotated `yaw` about +y, into `out`.
pub fn car(out: &mut MeshBuilder, c: Vec3, yaw: f32, body: Color, shape: u32) {
    let m = car_model(body, shape, false);
    out.append(&m, c, yaw, Vec3::ONE, [1.0; 4]);
}

/// Car model in its local frame. `lod`: the distant version (no wheels,
/// lights or side glass).
pub fn car_model(body: Color, shape: u32, lod: bool) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    m.lod = lod;
    let glass = palette::CAR_GLASS;
    let dark = shade(body, 0.75);
    let (len, wid) = car_size(shape);
    let (hl, hw) = (len * 0.5, wid * 0.5);
    let wr = 0.34;
    // Lower body with a sloped nose.
    let lower = [
        Vec2::new(-hl, 0.3),
        Vec2::new(hl - 0.05, 0.3),
        Vec2::new(hl, 0.62),
        Vec2::new(hl - 0.35, 0.88),
        Vec2::new(-hl + 0.1, 0.9),
        Vec2::new(-hl, 0.75),
    ];
    let body_c = body;
    extrude(&mut m, &lower, -hw, hw, body_c, &|i| match i {
        0 => shade(body_c, 0.4),
        1 | 5 => dark,
        _ => body_c,
    });
    let cw = hw - 0.08;
    match shape {
        // Van: tall box cabin.
        2 => {
            let cab = [
                Vec2::new(-hl + 0.05, 0.88),
                Vec2::new(hl - 0.45, 0.88),
                Vec2::new(hl - 1.0, 1.85),
                Vec2::new(-hl + 0.05, 1.85),
            ];
            extrude(&mut m, &cab, -cw, cw, body_c, &|i| match i {
                1 => glass,
                2 => shade(body_c, 1.08),
                _ => body_c,
            });
            side_windows(&mut m, hl - 1.1, hl - 2.2, 1.15, 1.6, cw);
        }
        // Pickup: cab plus an open bed.
        3 => {
            let cab = [
                Vec2::new(-0.2, 0.88),
                Vec2::new(hl - 0.55, 0.88),
                Vec2::new(hl - 1.0, 1.55),
                Vec2::new(-0.2, 1.55),
            ];
            extrude(&mut m, &cab, -cw, cw, body_c, &|i| match i {
                1 | 3 => glass,
                2 => shade(body_c, 1.08),
                _ => body_c,
            });
            side_windows(&mut m, hl - 1.05, 0.0, 1.0, 1.42, cw);
            m.cuboid2(
                Vec3::new(-hl + 0.1, 0.9, -cw + 0.05),
                Vec3::new(-0.3, 0.95, cw - 0.05),
                shade(body_c, 0.5),
                shade(body_c, 0.5),
            );
        }
        _ => {
            // Saloon (0), hatchback (1), estate (4: long roof to the back).
            let (back, front) = match shape {
                1 => (-hl + 0.15, hl - 1.1),
                4 => (-hl + 0.38, hl - 1.3),
                _ => (-hl + 0.7, hl - 1.25),
            };
            let cab = [
                Vec2::new(back - 0.25, 0.88),
                Vec2::new(front + 0.45, 0.88),
                Vec2::new(front, 1.42),
                Vec2::new(back + 0.3, 1.42),
            ];
            extrude(&mut m, &cab, -cw + 0.05, cw - 0.05, body_c, &|i| match i {
                1 | 3 => glass,
                2 => shade(body_c, 1.1),
                _ => body_c,
            });
            side_windows(&mut m, front + 0.05, back + 0.1, 0.98, 1.36, cw - 0.05);
        }
    }
    // Lights (also on the distant model: they are what you see at night).
    let head = Color::srgb(0.98, 0.96, 0.85);
    let tail = Color::srgb(0.75, 0.12, 0.1);
    for s in [-1.0, 1.0] {
        let z = s * (hw - 0.3);
        m.glow(Glow::Headlight, 0.0, |m| {
            m.quad(
                Vec3::new(hl + 0.005, 0.5, z + 0.2),
                Vec3::new(hl + 0.005, 0.5, z - 0.2),
                Vec3::new(hl + 0.005, 0.62, z - 0.2),
                Vec3::new(hl + 0.005, 0.62, z + 0.2),
                head,
            )
        });
        m.glow(Glow::TailLight, 0.0, |m| {
            m.quad(
                Vec3::new(-hl - 0.005, 0.55, z - 0.2),
                Vec3::new(-hl - 0.005, 0.55, z + 0.2),
                Vec3::new(-hl - 0.005, 0.68, z + 0.2),
                Vec3::new(-hl - 0.005, 0.68, z - 0.2),
                tail,
            )
        });
    }
    if lod {
        return m;
    }
    let wb = hl - 0.75;
    for x in [-wb, wb] {
        for z in [-(hw - 0.12), hw - 0.12] {
            wheel(&mut m, Vec3::new(x, wr, z), wr, 0.24);
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

/// Length of the delivery truck, metres. The sim queues vehicles 5.5 m
/// centre to centre; `vehicles` keeps lanes from compressing that below
/// what a truck behind a truck needs.
pub const TRUCK_LEN: f32 = 4.8;

/// Small box delivery truck in its local frame: a cab with a raked
/// windscreen in front of a tall cargo box with a livery stripe. `lod`: the
/// distant version (no wheels, lights or glass details).
pub fn truck_model(cab: Color, cargo: Color, stripe: Color, lod: bool) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    m.lod = lod;
    let (hl, hw) = (TRUCK_LEN * 0.5, TRUCK_WIDTH * 0.5);
    let wr = 0.38;
    let frame = shade(palette::TYRE, 1.4);
    // Chassis rail and bumpers.
    m.cuboid(
        Vec3::new(-hl + 0.05, 0.32, -hw + 0.12),
        Vec3::new(hl - 0.05, 0.62, hw - 0.12),
        frame,
    );
    // Cab: short bonnet, raked windscreen, roof.
    let cab_back = hl - 1.45;
    let prof = [
        Vec2::new(cab_back, 0.42),
        Vec2::new(hl, 0.42),
        Vec2::new(hl, 1.2),
        Vec2::new(hl - 0.3, 2.05),
        Vec2::new(cab_back, 2.12),
    ];
    let cw = hw - 0.04;
    extrude(&mut m, &prof, -cw, cw, cab, &|i| match i {
        0 => shade(cab, 0.4),
        1 => shade(cab, 0.85),
        2 => palette::CAR_GLASS,
        3 => shade(cab, 1.08),
        _ => shade(cab, 0.8),
    });
    side_windows(&mut m, hl - 0.38, cab_back + 0.15, 1.3, 1.95, cw);
    // Grille.
    m.detail_only(|m| {
        m.quad(
            Vec3::new(hl + 0.005, 0.55, -0.5),
            Vec3::new(hl + 0.005, 0.55, 0.5),
            Vec3::new(hl + 0.005, 0.95, 0.5),
            Vec3::new(hl + 0.005, 0.95, -0.5),
            shade(cab, 0.45),
        );
    });
    // Cargo box.
    let (bx0, bx1, by0, by1) = (-hl, cab_back - 0.08, 0.62, 2.6);
    m.cuboid2(
        Vec3::new(bx0, by0, -hw),
        Vec3::new(bx1, by1, hw),
        cargo,
        shade(cargo, 1.06),
    );
    // Livery stripe on both sides and the back.
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
    let xb = bx0 - 0.01;
    m.quad(
        Vec3::new(xb, 1.25, -hw),
        Vec3::new(xb, 1.25, hw),
        Vec3::new(xb, 1.6, hw),
        Vec3::new(xb, 1.6, -hw),
        stripe,
    );
    if lod {
        return m;
    }
    // Rear door seam and hinges.
    let seam = shade(cargo, 0.7);
    m.quad(
        Vec3::new(xb - 0.002, by0 + 0.1, -0.03),
        Vec3::new(xb - 0.002, by0 + 0.1, 0.03),
        Vec3::new(xb - 0.002, by1 - 0.1, 0.03),
        Vec3::new(xb - 0.002, by1 - 0.1, -0.03),
        seam,
    );
    let head = Color::srgb(0.98, 0.96, 0.85);
    let tail = Color::srgb(0.75, 0.12, 0.1);
    for s in [-1.0, 1.0] {
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
    for x in [hl - 0.75, -hl + 0.85] {
        for z in [-(hw - 0.16), hw - 0.16] {
            wheel(&mut m, Vec3::new(x, wr, z), wr, 0.28);
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

/// Glass strips on both sides of the cabin between `x1` (front) and `x0`.
fn side_windows(m: &mut MeshBuilder, x1: f32, x0: f32, y0: f32, y1: f32, z: f32) {
    if m.lod {
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cars_fit_a_lane_and_sit_on_the_ground() {
        for shape in 0..SHAPES {
            let m = car_model(Color::WHITE, shape, false);
            let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
            for p in &m.pos {
                min = min.min(Vec3::from(*p));
                max = max.max(Vec3::from(*p));
            }
            assert!(min.y >= -0.001 && min.y < 0.05, "{min}");
            assert!(max.z - min.z < 2.1 && max.x - min.x <= 4.42, "{min} {max}");
        }
    }

    #[test]
    fn trucks_fit_the_queue_spacing_and_sit_on_the_ground() {
        for lod in [false, true] {
            let (c, k, s) = palette::TRUCK_LIVERIES[0];
            let m = truck_model(c, k, s, lod);
            let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
            for p in &m.pos {
                min = min.min(Vec3::from(*p));
                max = max.max(Vec3::from(*p));
            }
            assert!(min.y >= -0.001 && (lod || min.y < 0.05), "{min}");
            assert!(max.x - min.x <= TRUCK_LEN + 0.03, "{min} {max}");
            assert!(max.z - min.z < 2.1, "{min} {max}");
            // Origin near the middle (LOD cross-fade and queue spacing).
            assert!((max.x + min.x).abs() < 0.05);
        }
    }

    #[test]
    fn extruded_faces_point_outward() {
        let mut m = MeshBuilder::new();
        let prof = [
            Vec2::new(-1.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.8, 1.0),
            Vec2::new(-0.8, 1.0),
        ];
        extrude(&mut m, &prof, -0.5, 0.5, Color::WHITE, &|_| Color::WHITE);
        let c = Vec3::new(0.0, 0.5, 0.0);
        for t in m.idx.chunks(3) {
            let p: Vec<Vec3> = t.iter().map(|&i| Vec3::from(m.pos[i as usize])).collect();
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            assert!(n.dot((p[0] + p[1] + p[2]) / 3.0 - c) > 0.0);
        }
    }
}
