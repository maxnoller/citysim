//! Procedural building models. One model per `(kind, lot size, level,
//! variant)`, cached and shared by every building with that key, so a city
//! of thousands of buildings uses a few hundred meshes and one material.
//!
//! Local frame: the lot is centred on the origin, `x` across, the front
//! (facing the road) towards `+z`. Lot width `w` along x, depth `d` along z.

use bevy::prelude::*;
use citysim_sim::Zone;

use super::mesh_util::{Glow, MeshBuilder};
use super::noise::Stream;
use super::palette::{self, lin, mix, shade};
use super::props;

/// Distinct models per kind / size / level. Facing adds four rotations.
pub const VARIANTS: u32 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelKey {
    pub kind: Zone,
    /// Lot size in cells in the local frame (across, deep).
    pub cells: (u8, u8),
    pub level: u8,
    pub variant: u8,
}

/// Lot-local rectangle.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: f32,
    x1: f32,
    z0: f32,
    z1: f32,
}

impl Rect {
    fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    fn d(&self) -> f32 {
        self.z1 - self.z0
    }
    fn cx(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }
    fn cz(&self) -> f32 {
        (self.z0 + self.z1) * 0.5
    }
    fn inset(&self, k: f32) -> Rect {
        Rect {
            x0: self.x0 + k,
            x1: self.x1 - k,
            z0: self.z0 + k,
            z1: self.z1 - k,
        }
    }
}

/// One side of an axis-aligned body, for putting things on walls.
#[derive(Debug, Clone, Copy)]
struct Face {
    /// Bottom-left corner seen from outside.
    origin: Vec3,
    /// Rightwards along the face seen from outside.
    u: Vec3,
    /// Outward normal.
    n: Vec3,
    width: f32,
}

impl Face {
    fn of(r: Rect, y: f32) -> [Face; 4] {
        [
            // Front (+z), right (+x), back (-z), left (-x).
            Face {
                origin: Vec3::new(r.x0, y, r.z1),
                u: Vec3::X,
                n: Vec3::Z,
                width: r.w(),
            },
            Face {
                origin: Vec3::new(r.x1, y, r.z1),
                u: Vec3::NEG_Z,
                n: Vec3::X,
                width: r.d(),
            },
            Face {
                origin: Vec3::new(r.x1, y, r.z0),
                u: Vec3::NEG_X,
                n: Vec3::NEG_Z,
                width: r.w(),
            },
            Face {
                origin: Vec3::new(r.x0, y, r.z0),
                u: Vec3::Z,
                n: Vec3::NEG_X,
                width: r.d(),
            },
        ]
    }

    /// Point at `a` metres along, `b` metres up, `off` out from the wall.
    fn at(&self, a: f32, b: f32, off: f32) -> Vec3 {
        self.origin + self.u * a + Vec3::Y * b + self.n * off
    }

    /// Rectangle on the face (outward facing).
    #[allow(clippy::too_many_arguments)]
    fn rect(&self, m: &mut MeshBuilder, a0: f32, a1: f32, b0: f32, b1: f32, off: f32, color: Color) {
        m.quad(
            self.at(a0, b0, off),
            self.at(a1, b0, off),
            self.at(a1, b1, off),
            self.at(a0, b1, off),
            color,
        );
    }

    /// Framed window centred at `a`, sill at `b`.
    #[allow(clippy::too_many_arguments)]
    fn window(&self, m: &mut MeshBuilder, a: f32, b: f32, w: f32, h: f32, frame: Color, glass: Color) {
        let f = 0.1;
        let r = wrand(self.at(a, b, 0.0));
        if m.lod {
            m.glow(Glow::Window, r, |m| {
                self.rect(m, a - w * 0.5, a + w * 0.5, b, b + h, 0.05, mix(glass, frame, 0.25))
            });
            return;
        }
        self.rect(m, a - w * 0.5 - f, a + w * 0.5 + f, b - f, b + h + f, 0.03, frame);
        m.glow(Glow::Window, r, |m| {
            self.rect(m, a - w * 0.5, a + w * 0.5, b, b + h, 0.05, glass)
        });
    }

    /// Glass rectangle lit at night as one window (`Window`) or as a shop
    /// front (`Shop`, always lit after dark).
    #[allow(clippy::too_many_arguments)]
    fn lit_rect(&self, m: &mut MeshBuilder, kind: Glow, a0: f32, a1: f32, b0: f32, b1: f32, off: f32, color: Color) {
        let r = wrand(self.at((a0 + a1) * 0.5, b0, 0.0));
        m.glow(kind, r, |m| self.rect(m, a0, a1, b0, b1, off, color));
    }

    /// Box sticking out of the face (canopies, balconies, signs).
    #[allow(clippy::too_many_arguments)]
    fn slab(&self, m: &mut MeshBuilder, a0: f32, a1: f32, b0: f32, b1: f32, out: f32, sides: Color, top: Color) {
        let p0 = self.at(a0, b0, 0.0);
        let p1 = self.at(a1, b0, out);
        let min = p0.min(p1).with_y(b0 + self.origin.y);
        let max = p0.max(p1).with_y(b1 + self.origin.y);
        m.cuboid2(min, max, sides, top);
    }
}

/// A stable pseudo-random value in `0..1` for a point of a model (which
/// windows are lit at night).
fn wrand(p: Vec3) -> f32 {
    let h = (p.x * 12.9898 + p.y * 78.233 + p.z * 37.719).sin() * 43_758.547;
    h - h.floor()
}

/// Evenly spaced positions for `n` items across a face of `width`.
fn spread(width: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| width * (i as f32 + 0.5) / n as f32).collect()
}

fn lot_ground(m: &mut MeshBuilder, lot: Rect, color: Color) {
    let y = 0.03;
    m.quad_up(
        [
            Vec3::new(lot.x0, y, lot.z0),
            Vec3::new(lot.x1, y, lot.z0),
            Vec3::new(lot.x1, y, lot.z1),
            Vec3::new(lot.x0, y, lot.z1),
        ],
        lin(color),
    );
}

fn flat_patch(m: &mut MeshBuilder, r: Rect, y: f32, color: Color) {
    m.quad_up(
        [
            Vec3::new(r.x0, y, r.z0),
            Vec3::new(r.x1, y, r.z0),
            Vec3::new(r.x1, y, r.z1),
            Vec3::new(r.x0, y, r.z1),
        ],
        lin(color),
    );
}

/// Pitched roof over `r` from `eave` with ridge `rise` above it.
/// `along_x`: ridge parallel to x. `hip`: hipped instead of gabled.
#[allow(clippy::too_many_arguments)]
fn pitched_roof(
    m: &mut MeshBuilder,
    r: Rect,
    eave: f32,
    rise: f32,
    along_x: bool,
    hip: bool,
    over: f32,
    roof: Color,
    wall: Color,
) {
    // Work in a frame where the ridge runs along `a` (length la) and the
    // slopes fall along `b` (span lb); map back to x/z at the end.
    let (la, lb) = if along_x { (r.w(), r.d()) } else { (r.d(), r.w()) };
    let (ca, cb) = if along_x { (r.cx(), r.cz()) } else { (r.cz(), r.cx()) };
    let p = |a: f32, b: f32, y: f32| {
        if along_x {
            Vec3::new(ca + a, y, cb + b)
        } else {
            Vec3::new(cb + b, y, ca + a)
        }
    };
    let ha = la * 0.5 + over;
    let hb = lb * 0.5 + over;
    let drop = rise / (lb * 0.5) * over;
    let ye = eave - drop;
    let yr = eave + rise;
    let ridge = if hip { (ha - hb).max(0.0) } else { ha };
    let dark = shade(roof, 0.72);
    // Winding is fixed after mapping because swapping axes mirrors; use
    // `face` to orient every face away from the roof's centre.
    let centre = p(0.0, 0.0, eave + rise * 0.3);
    let face = |m: &mut MeshBuilder, q: &[Vec3], color: Color| {
        let n = (q[1] - q[0]).cross(q[2] - q[0]);
        let mid = q.iter().copied().sum::<Vec3>() / q.len() as f32;
        let flip = n.dot(mid - centre) < 0.0;
        match (q.len(), flip) {
            (3, false) => m.tri(q[0], q[1], q[2], color),
            (3, true) => m.tri(q[0], q[2], q[1], color),
            (_, false) => m.quad(q[0], q[1], q[2], q[3], color),
            (_, true) => m.quad(q[0], q[3], q[2], q[1], color),
        }
    };
    let rf = roof;
    let rb = shade(roof, 0.93);
    // Two long slopes.
    face(
        m,
        &[p(-ha, hb, ye), p(ha, hb, ye), p(ridge, 0.0, yr), p(-ridge, 0.0, yr)],
        rf,
    );
    face(
        m,
        &[p(ha, -hb, ye), p(-ha, -hb, ye), p(-ridge, 0.0, yr), p(ridge, 0.0, yr)],
        rb,
    );
    // Fascia boards along the eaves.
    let t = 0.14;
    for s in [1.0, -1.0] {
        face(
            m,
            &[
                p(-ha, s * hb, ye - t),
                p(ha, s * hb, ye - t),
                p(ha, s * hb, ye),
                p(-ha, s * hb, ye),
            ],
            dark,
        );
    }
    if hip {
        for s in [1.0, -1.0] {
            face(
                m,
                &[p(s * ha, hb, ye), p(s * ha, -hb, ye), p(s * ridge, 0.0, yr)],
                shade(roof, 0.86),
            );
            face(
                m,
                &[
                    p(s * ha, hb, ye - t),
                    p(s * ha, -hb, ye - t),
                    p(s * ha, -hb, ye),
                    p(s * ha, hb, ye),
                ],
                dark,
            );
        }
    } else {
        // Gable walls (in the wall colour) and verge boards.
        let (wa, wb) = (la * 0.5, lb * 0.5);
        for s in [1.0, -1.0] {
            face(
                m,
                &[p(s * wa, wb, eave), p(s * wa, -wb, eave), p(s * wa, 0.0, yr)],
                wall,
            );
            for side in [1.0, -1.0] {
                face(
                    m,
                    &[
                        p(s * ha, side * hb, ye - t),
                        p(s * ha, side * hb, ye),
                        p(s * ha, 0.0, yr),
                        p(s * ha, 0.0, yr - t),
                    ],
                    dark,
                );
            }
        }
    }
    // Underside of the overhang (visible from low angles).
    let soffit = shade(wall, 0.7);
    let y = ye - t;
    let (x0, x1, z0, z1) = (r.x0 - over, r.x1 + over, r.z0 - over, r.z1 + over);
    let ring = [
        Vec3::new(x0, y, z0),
        Vec3::new(x1, y, z0),
        Vec3::new(x1, y, z1),
        Vec3::new(x0, y, z1),
    ];
    // Facing down: wind clockwise seen from above.
    m.quad(ring[0], ring[1], ring[2], ring[3], soffit);
}

/// Flat roof with a parapet.
fn flat_roof(m: &mut MeshBuilder, r: Rect, top: f32, wall: Color, roof: Color, parapet: f32) {
    let t = 0.25;
    let y0 = top;
    let y1 = top + parapet;
    let cap = shade(wall, 0.92);
    for (min, max) in [
        (Vec3::new(r.x0, y0, r.z0), Vec3::new(r.x1, y1, r.z0 + t)),
        (Vec3::new(r.x0, y0, r.z1 - t), Vec3::new(r.x1, y1, r.z1)),
        (Vec3::new(r.x0, y0, r.z0 + t), Vec3::new(r.x0 + t, y1, r.z1 - t)),
        (Vec3::new(r.x1 - t, y0, r.z0 + t), Vec3::new(r.x1, y1, r.z1 - t)),
    ] {
        m.cuboid2(min, max, wall, cap);
    }
    flat_patch(m, r.inset(t), top + 0.02, roof);
}

/// Rooftop clutter: AC boxes, vents, a water tank.
fn roof_details(m: &mut MeshBuilder, rng: &mut Stream, r: Rect, top: f32) {
    m.detail_only(|m| roof_details_inner(m, rng, r, top));
}

fn roof_details_inner(m: &mut MeshBuilder, rng: &mut Stream, r: Rect, top: f32) {
    let n = 1 + rng.below(3);
    for _ in 0..n {
        let w = rng.range(0.8, 1.6);
        let d = rng.range(0.7, 1.3);
        let x = rng.range(r.x0 + 0.6 + w * 0.5, (r.x1 - 0.6 - w * 0.5).max(r.x0 + 0.6 + w * 0.5));
        let z = rng.range(r.z0 + 0.6 + d * 0.5, (r.z1 - 0.6 - d * 0.5).max(r.z0 + 0.6 + d * 0.5));
        m.cuboid2(
            Vec3::new(x - w * 0.5, top, z - d * 0.5),
            Vec3::new(x + w * 0.5, top + rng.range(0.5, 0.9), z + d * 0.5),
            palette::ROOF_DETAIL,
            shade(palette::ROOF_DETAIL, 1.15),
        );
    }
    if rng.chance(0.3) {
        let c = Vec3::new(r.cx() + rng.range(-1.0, 1.0), top, r.z0 + 1.4);
        m.cylinder(c, 0.6, 1.4, 8, palette::TANK, shade(palette::TANK, 1.1));
    }
}

fn chimney(m: &mut MeshBuilder, x: f32, z: f32, y0: f32, y1: f32) {
    m.cuboid2(
        Vec3::new(x - 0.3, y0, z - 0.3),
        Vec3::new(x + 0.3, y1, z + 0.3),
        palette::BRICK,
        shade(palette::BRICK, 0.6),
    );
}

fn hedge(m: &mut MeshBuilder, a: Vec2, b: Vec2, h: f32) {
    let min = a.min(b) - Vec2::splat(0.22);
    let max = a.max(b) + Vec2::splat(0.22);
    m.cuboid2(
        Vec3::new(min.x, 0.0, min.y),
        Vec3::new(max.x, h, max.y),
        palette::HEDGE,
        shade(palette::HEDGE, 1.12),
    );
}

fn fence(m: &mut MeshBuilder, a: Vec2, b: Vec2) {
    m.detail_only(|m| fence_inner(m, a, b));
}

fn fence_inner(m: &mut MeshBuilder, a: Vec2, b: Vec2) {
    let len = a.distance(b);
    if len < 0.3 {
        return;
    }
    let dir = (b - a) / len;
    let n = ((len / 1.2).ceil() as usize).max(1);
    for i in 0..=n {
        let p = a + dir * (len * i as f32 / n as f32);
        m.cuboid(
            Vec3::new(p.x - 0.05, 0.0, p.y - 0.05),
            Vec3::new(p.x + 0.05, 0.8, p.y + 0.05),
            palette::FENCE,
        );
    }
    let min = a.min(b) - Vec2::splat(0.025);
    let max = a.max(b) + Vec2::splat(0.025);
    for y in [0.32, 0.62] {
        m.cuboid(
            Vec3::new(min.x, y, min.y),
            Vec3::new(max.x, y + 0.08, max.y),
            palette::FENCE,
        );
    }
}

// ------------------------------------------------------------------ houses

struct HouseStyle {
    wall: Color,
    roof: Color,
    trim: Color,
}

fn house_style(rng: &mut Stream) -> HouseStyle {
    let wall = rng.pick(&palette::HOUSE_WALLS);
    HouseStyle {
        wall,
        roof: rng.pick(&palette::HOUSE_ROOFS),
        trim: if rng.chance(0.7) {
            palette::WINDOW_FRAME
        } else {
            shade(wall, 0.8)
        },
    }
}

/// A pitched-roof house body occupying `r` with `storeys` floors. The door
/// is on the front (+z) at `door_a` along the front face (if any).
#[allow(clippy::too_many_arguments)]
fn house_body(
    m: &mut MeshBuilder,
    rng: &mut Stream,
    r: Rect,
    storeys: u32,
    st: &HouseStyle,
    door_a: Option<f32>,
    roof_kind: u32,
) {
    let sh = 2.7;
    let top = storeys as f32 * sh + 0.3;
    // Plinth.
    let plinth = shade(st.wall, 0.78);
    m.cuboid(
        Vec3::new(r.x0 - 0.04, 0.0, r.z0 - 0.04),
        Vec3::new(r.x1 + 0.04, 0.35, r.z1 + 0.04),
        plinth,
    );
    m.cuboid2(
        Vec3::new(r.x0, 0.35, r.z0),
        Vec3::new(r.x1, top, r.z1),
        st.wall,
        st.wall,
    );
    let faces = Face::of(r, 0.0);
    for (fi, f) in faces.iter().enumerate() {
        let per = ((f.width / 2.3).floor() as usize).clamp(1, 4);
        for s in 0..storeys {
            let base = 0.3 + s as f32 * sh + 0.95;
            for a in spread(f.width, per) {
                if fi == 0 && s == 0 && door_a.is_some_and(|d| (a - d).abs() < 1.1) {
                    continue;
                }
                f.window(m, a, base, 0.8, 1.15, st.trim, palette::WINDOW);
            }
        }
    }
    if let Some(a) = door_a {
        let f = faces[0];
        f.rect(m, a - 0.6, a + 0.6, 0.3, 2.45, 0.03, st.trim);
        f.rect(m, a - 0.48, a + 0.48, 0.3, 2.3, 0.05, palette::DOOR);
        // Step and a little canopy.
        f.slab(m, a - 0.75, a + 0.75, 0.0, 0.3, 0.6, plinth, shade(plinth, 1.1));
        if rng.chance(0.5) {
            f.slab(m, a - 0.85, a + 0.85, 2.6, 2.72, 0.7, shade(st.roof, 0.8), st.roof);
        }
    }
    let along_x = roof_kind != 1 || r.w() > r.d() * 1.4;
    let span = if along_x { r.d() } else { r.w() };
    let rise = span * rng.range(0.38, 0.55);
    pitched_roof(m, r, top, rise, along_x, roof_kind == 2, 0.35, st.roof, st.wall);
    if rng.chance(0.45) {
        let x = if rng.chance(0.5) { r.x0 + 0.9 } else { r.x1 - 0.9 };
        chimney(m, x, r.cz() - span * 0.12, top, top + rise + 0.9);
    }
}

fn yard_greenery(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, avoid: Rect) {
    // A tree or bushes in the back corners of the lot.
    let spots = [
        Vec2::new(lot.x0 + 1.4, lot.z0 + 1.4),
        Vec2::new(lot.x1 - 1.4, lot.z0 + 1.4),
        Vec2::new(lot.x0 + 1.1, lot.cz()),
        Vec2::new(lot.x1 - 1.1, lot.cz()),
    ];
    let mut placed = 0;
    for s in spots {
        if placed >= 2 {
            break;
        }
        let clear = s.x < avoid.x0 - 0.9 || s.x > avoid.x1 + 0.9 || s.y < avoid.z0 - 0.9 || s.y > avoid.z1 + 0.9;
        if !clear || !rng.chance(0.55) {
            continue;
        }
        placed += 1;
        let p = Vec3::new(s.x, 0.0, s.y);
        let canopy = rng.pick(&palette::CANOPIES);
        if rng.chance(0.45) {
            props::round_tree(m, p, rng.range(0.55, 0.8), canopy, rng.range(0.0, 6.0));
        } else {
            props::bush(m, p, rng.range(0.9, 1.4), shade(canopy, 1.05), rng.range(0.0, 6.0));
        }
    }
}

fn lawn_color(rng: &mut Stream) -> Color {
    mix(palette::GRASS_LIGHT, palette::GRASS, rng.range(0.0, 0.6))
}

/// Detached house (level 1).
fn detached(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect) {
    lot_ground(m, lot, lawn_color(rng));
    let st = house_style(rng);
    let front_yard = rng.range(1.9, 2.6);
    let bw = rng.range(4.4, 5.6).min(lot.w() - 1.8);
    let bd = rng.range(3.9, 4.8).min(lot.d() - front_yard - 1.2);
    let x0 = lot.cx() - bw * 0.5 + rng.range(-0.5, 0.5);
    let body = Rect {
        x0,
        x1: x0 + bw,
        z0: lot.z1 - front_yard - bd,
        z1: lot.z1 - front_yard,
    };
    let storeys = if rng.chance(0.8) { 1 } else { 2 };
    let door = rng.range(0.9, bw - 0.9);
    let roof_kind = rng.below(3);
    house_body(m, rng, body, storeys, &st, Some(door), roof_kind);
    // Path from the door to the pavement.
    let px = body.x0 + door;
    flat_patch(
        m,
        Rect {
            x0: px - 0.5,
            x1: px + 0.5,
            z0: body.z1,
            z1: lot.z1,
        },
        0.05,
        palette::PATH,
    );
    // Hedge or picket fence along the front and sides.
    let e = 0.3;
    let (a, b, c, d) = (
        Vec2::new(lot.x0 + e, lot.z1 - e),
        Vec2::new(lot.x1 - e, lot.z1 - e),
        Vec2::new(lot.x0 + e, lot.z0 + e),
        Vec2::new(lot.x1 - e, lot.z0 + e),
    );
    match rng.below(3) {
        0 => {
            let h = rng.range(0.6, 0.9);
            hedge(m, a, Vec2::new(px - 0.8, a.y), h);
            hedge(m, Vec2::new(px + 0.8, a.y), b, h);
            hedge(m, a, c, h);
        }
        1 => {
            fence(m, a, Vec2::new(px - 0.7, a.y));
            fence(m, Vec2::new(px + 0.7, a.y), b);
            fence(m, b, d);
        }
        _ => {}
    }
    // Back garden bed.
    if rng.chance(0.35) {
        flat_patch(
            m,
            Rect {
                x0: lot.x1 - 2.6,
                x1: lot.x1 - 0.8,
                z0: lot.z0 + 0.7,
                z1: lot.z0 + 1.6,
            },
            0.05,
            palette::EARTH,
        );
    }
    yard_greenery(m, rng, lot, body);
}

/// A row of narrow townhouses (level 2) with `storeys` floors.
fn terrace(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, lawn_color(rng));
    let units = if lot.w() > 12.0 { 4 } else { 2 };
    let front_yard = rng.range(1.4, 2.0);
    let depth = (lot.d() - front_yard - 1.0).min(6.0);
    let inner = lot.w() - 0.8;
    let uw = inner / units as f32;
    let roof = rng.pick(&palette::HOUSE_ROOFS);
    let front_gables = rng.chance(0.5);
    for u in 0..units {
        let x0 = lot.x0 + 0.4 + uw * u as f32;
        let body = Rect {
            x0,
            x1: x0 + uw,
            z0: lot.z1 - front_yard - depth,
            z1: lot.z1 - front_yard,
        };
        let st = HouseStyle {
            wall: rng.pick(&palette::HOUSE_WALLS),
            roof,
            trim: palette::WINDOW_FRAME,
        };
        let door = if u % 2 == 0 { 0.8 } else { uw - 0.8 };
        house_body(m, rng, body, storeys, &st, Some(door), if front_gables { 1 } else { 0 });
        let px = body.x0 + door;
        flat_patch(
            m,
            Rect {
                x0: px - 0.45,
                x1: px + 0.45,
                z0: body.z1,
                z1: lot.z1,
            },
            0.05,
            palette::PATH,
        );
    }
    let e = 0.3;
    hedge(
        m,
        Vec2::new(lot.x0 + e, lot.z0 + e),
        Vec2::new(lot.x1 - e, lot.z0 + e),
        0.8,
    );
}

/// Small apartment block (level 3+).
/// Apartment block filling the lot: a podium ground floor, balconies on
/// the street side, a set-back attic storey on taller blocks, hipped or
/// flat roof. `storeys` includes the ground floor.
fn apartments(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, mix(lawn_color(rng), palette::PAVING, 0.35));
    let wall = rng.pick(&palette::HOUSE_WALLS);
    let accent = rng.pick(&palette::HOUSE_ROOFS);
    let base_col = mix(shade(wall, 0.82), palette::BRICK, rng.range(0.0, 0.5));
    let big = lot.w() > 12.0;
    let side = if big { 1.0 } else { 0.4 };
    let body = Rect {
        x0: lot.x0 + side,
        x1: lot.x1 - side,
        z0: lot.z0 + side,
        z1: lot.z1 - 1.6,
    };
    let sh = 3.0;
    let ground = 3.4;
    let attic = storeys >= 5 && rng.chance(0.6);
    let main = if attic { storeys - 1 } else { storeys };
    let top = ground + (main - 1) as f32 * sh;
    // Podium ground floor in a darker base colour, then the upper floors.
    m.cuboid2(
        Vec3::new(body.x0, 0.0, body.z0),
        Vec3::new(body.x1, ground, body.z1),
        base_col,
        base_col,
    );
    m.cuboid2(
        Vec3::new(body.x0, ground, body.z0),
        Vec3::new(body.x1, top, body.z1),
        wall,
        wall,
    );
    let faces = Face::of(body, 0.0);
    let trim = if rng.chance(0.5) {
        palette::WINDOW_FRAME
    } else {
        shade(wall, 0.8)
    };
    let balconies = rng.chance(0.75);
    let per_of = |w: f32| ((w / 2.2).floor() as usize).clamp(1, 8);
    for (fi, f) in faces.iter().enumerate() {
        let per = per_of(f.width);
        for (k, a) in spread(f.width, per).into_iter().enumerate() {
            if !(fi == 0 && k == per / 2) {
                f.window(m, a, 0.9, 1.0, 1.5, trim, palette::WINDOW);
            }
        }
        for s in 1..main {
            let base = ground + (s - 1) as f32 * sh + 0.85;
            for (k, a) in spread(f.width, per).into_iter().enumerate() {
                f.window(m, a, base, 0.95, 1.35, trim, palette::WINDOW);
                if balconies && fi == 0 && k % 2 == (s as usize % 2) {
                    f.slab(
                        m,
                        a - 0.95,
                        a + 0.95,
                        base - 0.9,
                        base - 0.75,
                        1.0,
                        shade(wall, 0.85),
                        wall,
                    );
                    f.slab(m, a - 0.95, a + 0.95, base - 0.75, base - 0.15, 1.0, accent, accent);
                }
            }
            // Floor band.
            let y = ground + (s - 1) as f32 * sh;
            f.rect(m, 0.0, f.width, y - 0.1, y + 0.1, 0.02, shade(wall, 0.86));
        }
    }
    // Entrance with a canopy and a path to the pavement.
    let f = faces[0];
    let per = per_of(f.width);
    let a = spread(f.width, per)[per / 2];
    f.lit_rect(m, Glow::Shop, a - 0.75, a + 0.75, 0.0, 2.6, 0.08, palette::GLASS);
    f.slab(m, a - 1.3, a + 1.3, 2.8, 2.98, 1.4, accent, accent);
    flat_patch(
        m,
        Rect {
            x0: body.x0 + a - 0.8,
            x1: body.x0 + a + 0.8,
            z0: body.z1,
            z1: lot.z1,
        },
        0.05,
        palette::PATH,
    );
    let hipped = rng.chance(0.6);
    if attic {
        // Set-back top floor with a roof terrace in front.
        flat_roof(m, body, top, wall, palette::ROOF_FLAT, 1.0);
        let inner = Rect {
            z1: body.z1 - 1.8,
            z0: body.z0 + 0.6,
            x0: body.x0 + 0.6,
            x1: body.x1 - 0.6,
        };
        let atop = top + sh;
        m.cuboid2(
            Vec3::new(inner.x0, top, inner.z0),
            Vec3::new(inner.x1, atop, inner.z1),
            wall,
            wall,
        );
        let f = Face::of(inner, top)[0];
        f.lit_rect(m, Glow::Window, 0.5, f.width - 0.5, 0.3, 2.4, 0.04, palette::GLASS);
        if hipped {
            pitched_roof(m, inner, atop, inner.d() * 0.25, true, true, 0.3, accent, wall);
        } else {
            flat_roof(m, inner, atop, wall, palette::ROOF_FLAT, 0.4);
        }
    } else if hipped {
        pitched_roof(m, body, top, body.d() * 0.28, true, true, 0.35, accent, wall);
    } else {
        flat_roof(m, body, top, wall, palette::ROOF_FLAT, 0.6);
        roof_details(m, rng, body.inset(0.4), top);
    }
    m.detail_only(|m| {
        for x in [lot.x0 + 0.6, lot.x1 - 0.6] {
            props::bush(m, Vec3::new(x, 0.0, lot.z1 - 0.8), 0.8, palette::HEDGE, x);
        }
    });
}

// ------------------------------------------------------------------ shops & offices

fn shop(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, palette::PAVING);
    let wall = rng.pick(&palette::SHOP_WALLS);
    let wall = mix(wall, rng.pick(&palette::HOUSE_WALLS), rng.range(0.0, 0.5));
    let accent = rng.pick(&palette::ACCENTS);
    let margin = 0.5;
    let front = 0.9;
    let body = Rect {
        x0: lot.x0 + margin,
        x1: lot.x1 - margin,
        z0: lot.z0 + margin + rng.range(0.0, 1.0),
        z1: lot.z1 - front,
    };
    let ground_h = 3.6;
    let sh = 3.1;
    let top = ground_h + (storeys - 1) as f32 * sh;
    m.cuboid2(
        Vec3::new(body.x0, 0.0, body.z0),
        Vec3::new(body.x1, top, body.z1),
        wall,
        wall,
    );
    let faces = Face::of(body, 0.0);
    let f = faces[0];
    // Shop front: big glass, door, awning and sign.
    f.rect(m, 0.35, f.width - 0.35, 0.0, 0.45, 0.03, shade(wall, 0.7));
    f.lit_rect(m, Glow::Shop, 0.4, f.width - 0.4, 0.45, 2.9, 0.04, palette::GLASS);
    let door = f.width * rng.range(0.25, 0.75);
    f.rect(m, door - 0.6, door + 0.6, 0.0, 2.5, 0.06, shade(palette::GLASS, 0.75));
    for a in spread(f.width - 0.8, 3) {
        f.rect(m, 0.4 + a - 0.04, 0.4 + a + 0.04, 0.45, 2.9, 0.06, shade(wall, 0.85));
    }
    // Sloped awning.
    let (a0, a1) = (0.3, f.width - 0.3);
    let p = [
        f.at(a0, 3.2, 0.05),
        f.at(a1, 3.2, 0.05),
        f.at(a1, 2.75, 1.3),
        f.at(a0, 2.75, 1.3),
    ];
    m.quad(p[3], p[2], p[1], p[0], accent);
    m.quad(p[0], p[1], p[2], p[3], shade(accent, 0.6));
    let valance = shade(accent, 0.85);
    m.quad(p[3], p[2], p[2] - Vec3::Y * 0.25, p[3] - Vec3::Y * 0.25, valance);
    m.quad(p[2], p[3], p[3] - Vec3::Y * 0.25, p[2] - Vec3::Y * 0.25, valance);
    // Sign board.
    let sign_y = if storeys == 1 { top - 0.15 } else { 3.25 };
    f.slab(
        m,
        f.width * 0.2,
        f.width * 0.8,
        sign_y,
        sign_y + 0.55,
        0.18,
        accent,
        shade(accent, 1.1),
    );
    m.glow(Glow::Sign, 0.0, |m| {
        f.rect(
            m,
            f.width * 0.27,
            f.width * 0.73,
            sign_y + 0.18,
            sign_y + 0.37,
            0.19,
            palette::WINDOW_FRAME,
        )
    });
    // Upper floors.
    for (fi, f) in faces.iter().enumerate() {
        let per = ((f.width / 2.2).floor() as usize).clamp(1, 6);
        for s in 1..storeys {
            let base = ground_h + (s - 1) as f32 * sh + 0.8;
            for a in spread(f.width, per) {
                f.window(m, a, base, 1.1, 1.4, palette::WINDOW_FRAME, palette::GLASS);
            }
        }
        if fi > 0 && storeys == 1 {
            f.lit_rect(
                m,
                Glow::Shop,
                0.6,
                f.width - 0.6,
                1.0,
                2.2,
                0.03,
                shade(palette::GLASS, 0.9),
            );
        }
    }
    flat_roof(m, body, top, wall, palette::ROOF_FLAT, 0.45);
    roof_details(m, rng, body.inset(0.3), top);
    // Bollards / planters on the pavement.
    if rng.chance(0.5) {
        for a in [0.6, f.width - 0.6] {
            let c = f.at(a, 0.0, 0.5);
            props::bush(m, c.with_y(0.25), 0.7, palette::HEDGE, a);
            m.cuboid(
                Vec3::new(c.x - 0.4, 0.0, c.z - 0.3),
                Vec3::new(c.x + 0.4, 0.4, c.z + 0.3),
                shade(palette::PAVING, 0.8),
            );
        }
    }
}

fn office(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, palette::PAVING);
    let style = rng.below(3);
    let wall = rng.pick(&palette::OFFICE_WALLS);
    let accent = rng.pick(&palette::ACCENTS);
    let margin = if lot.w() > 12.0 { 1.0 } else { 0.35 };
    let body = Rect {
        x0: lot.x0 + margin,
        x1: lot.x1 - margin,
        z0: lot.z0 + margin,
        z1: lot.z1 - margin - 0.4,
    };
    let sh = 3.3;
    let base_h = 4.0;
    // Taller blocks sometimes step back for their top floors.
    // Four floors and up: a two-storey podium over the whole lot with the
    // upper floors set back on top of it.
    let setback = storeys >= 4;
    let low_storeys = if setback { 2 } else { storeys };
    let glass = mix(palette::GLASS, palette::WINDOW, rng.range(0.0, 0.6));
    let section = |m: &mut MeshBuilder, r: Rect, s0: u32, s1: u32| -> f32 {
        let y0 = if s0 == 0 { 0.0 } else { base_h + (s0 - 1) as f32 * sh };
        let top = base_h + (s1 - 1) as f32 * sh;
        let body_col = if style == 2 { shade(glass, 0.8) } else { wall };
        m.cuboid2(
            Vec3::new(r.x0, y0, r.z0),
            Vec3::new(r.x1, top, r.z1),
            body_col,
            body_col,
        );
        let faces = Face::of(r, 0.0);
        for f in &faces {
            if s0 == 0 {
                // Lobby glass on the ground floor.
                f.lit_rect(m, Glow::Shop, 0.5, f.width - 0.5, 0.3, 3.3, 0.04, shade(glass, 0.85));
            }
            for s in s0.max(1)..s1 {
                let y = base_h + (s - 1) as f32 * sh;
                match style {
                    0 => {
                        // Ribbon windows.
                        f.lit_rect(m, Glow::Window, 0.3, f.width - 0.3, y + 0.75, y + 2.7, 0.04, glass);
                    }
                    1 => {
                        // Punched windows in a stone or brick facade.
                        let n = ((f.width / 1.7).floor() as usize).max(1);
                        for a in spread(f.width, n) {
                            f.window(m, a, y + 0.8, 0.95, 1.8, shade(wall, 0.82), glass);
                        }
                    }
                    _ => {
                        // Curtain wall: floor lines over glass.
                        f.rect(m, 0.0, f.width, y - 0.1, y + 0.12, 0.04, palette::ROOF_DETAIL);
                    }
                }
            }
            if style == 2 {
                let n = ((f.width / 1.5).floor() as usize).max(1);
                for k in 1..n {
                    let a = f.width * k as f32 / n as f32;
                    f.rect(
                        m,
                        a - 0.05,
                        a + 0.05,
                        y0.max(3.4) - y0,
                        top - y0,
                        0.05,
                        palette::ROOF_DETAIL,
                    );
                }
            }
        }
        top
    };
    let mut top = section(m, body, 0, low_storeys);
    let roof_wall = if style == 2 { palette::ROOF_DETAIL } else { wall };
    if setback {
        let inner = body.inset(0.9);
        flat_roof(m, body, top, roof_wall, palette::ROOF_FLAT, 0.4);
        top = section(m, inner, low_storeys, storeys);
        flat_roof(m, inner, top, roof_wall, palette::ROOF_FLAT, 0.6);
    } else {
        flat_roof(m, body, top, roof_wall, palette::ROOF_FLAT, 0.6);
    }
    // Entrance canopy in the accent colour and a crown band.
    let f = Face::of(body, 0.0)[0];
    f.slab(m, f.width * 0.3, f.width * 0.7, 3.5, 3.7, 1.2, accent, accent);
    if rng.chance(0.5) {
        let crown = Face::of(if setback { body.inset(0.9) } else { body }, 0.0);
        for f in &crown {
            f.rect(m, 0.0, f.width, top - 0.7, top - 0.1, 0.06, accent);
        }
    }
    let plant = body.inset(body.w().min(body.d()) * 0.3);
    m.cuboid2(
        Vec3::new(plant.x0, top, plant.z0),
        Vec3::new(plant.x1, top + 1.8, plant.z1),
        shade(wall, 0.85),
        palette::ROOF_DETAIL,
    );
    roof_details(m, rng, body.inset(0.4), top);
    if rng.chance(0.4) {
        let c = Vec3::new(plant.x1 - 0.4, top + 1.8, plant.z0 + 0.4);
        m.detail_only(|m| m.cylinder(c, 0.06, 3.5, 4, palette::ROOF_DETAIL, palette::ROOF_DETAIL));
    }
}

/// Big-box store with a car park (commercial, large lot, low level).
fn supermarket(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect) {
    lot_ground(m, lot, palette::ASPHALT);
    let wall = rng.pick(&palette::SHOP_WALLS);
    let accent = rng.pick(&palette::ACCENTS);
    let body = Rect {
        x0: lot.x0 + 0.8,
        x1: lot.x1 - 0.8,
        z0: lot.z0 + 0.8,
        z1: lot.z0 + lot.d() * 0.55,
    };
    let top = 5.0;
    m.cuboid2(
        Vec3::new(body.x0, 0.0, body.z0),
        Vec3::new(body.x1, top, body.z1),
        wall,
        wall,
    );
    let f = Face::of(body, 0.0)[0];
    f.rect(m, 0.0, f.width, top - 1.1, top, 0.03, accent);
    f.lit_rect(
        m,
        Glow::Shop,
        f.width * 0.3,
        f.width * 0.7,
        0.0,
        3.0,
        0.04,
        palette::GLASS,
    );
    f.slab(
        m,
        f.width * 0.28,
        f.width * 0.72,
        3.0,
        3.25,
        2.0,
        shade(wall, 0.9),
        wall,
    );
    f.slab(
        m,
        f.width * 0.35,
        f.width * 0.65,
        top,
        top + 1.2,
        0.2,
        accent,
        shade(accent, 1.1),
    );
    flat_roof(m, body, top, wall, palette::ROOF_FLAT, 0.4);
    roof_details(m, rng, body.inset(1.0), top);
    // Car park bays and a few parked cars.
    let white = lin(palette::MARKING);
    let bay_z0 = body.z1 + 2.2;
    let bay_z1 = lot.z1 - 1.0;
    let n = ((lot.w() - 2.0) / 2.6) as usize;
    for i in 0..=n {
        let x = lot.x0 + 1.0 + i as f32 * 2.6;
        m.quad_up(
            [
                Vec3::new(x - 0.06, 0.05, bay_z0),
                Vec3::new(x + 0.06, 0.05, bay_z0),
                Vec3::new(x + 0.06, 0.05, bay_z1),
                Vec3::new(x - 0.06, 0.05, bay_z1),
            ],
            white,
        );
        if i < n && rng.chance(0.55) {
            let body_col = rng.pick(&palette::CAR_BODIES);
            let c = Vec3::new(x + 1.3, 0.03, (bay_z0 + bay_z1) * 0.5);
            let shape = rng.below(3);
            m.detail_only(|m| super::vehicle_gen::car(m, c, std::f32::consts::FRAC_PI_2, body_col, shape));
        }
    }
}

// ------------------------------------------------------------------ industry

fn shed(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, level: u8) {
    lot_ground(m, lot, mix(palette::YARD_CONCRETE, palette::EARTH, rng.range(0.0, 0.5)));
    let wall = mix(palette::INDUSTRY_WALLS, palette::SHOP_WALLS[1], rng.range(0.0, 0.4));
    let roof = palette::INDUSTRY_ROOF;
    let big = lot.w() > 12.0;
    let margin = 0.6;
    let body = Rect {
        x0: lot.x0 + margin,
        x1: lot.x1 - margin - if big { 0.0 } else { rng.range(0.0, 1.5) },
        z0: lot.z0 + margin,
        z1: lot.z1 - if big { 5.0 } else { 2.2 },
    };
    let h = if level >= 2 {
        rng.range(6.0, 7.5)
    } else {
        rng.range(4.2, 5.4)
    };
    m.cuboid2(
        Vec3::new(body.x0, 0.0, body.z0),
        Vec3::new(body.x1, h, body.z1),
        wall,
        wall,
    );
    let faces = Face::of(body, 0.0);
    // Accent stripe right round the building.
    for f in &faces {
        f.rect(m, 0.0, f.width, h - 0.9, h - 0.55, 0.02, palette::INDUSTRY_STRIPE);
        let per = ((f.width / 3.0).floor() as usize).max(1);
        for a in spread(f.width, per) {
            f.lit_rect(
                m,
                Glow::Window,
                a - 0.7,
                a + 0.7,
                h - 2.0,
                h - 1.2,
                0.03,
                palette::WINDOW,
            );
        }
    }
    // Loading doors.
    let f = faces[0];
    let doors = if big { 2 } else { 1 };
    for (i, a) in spread(f.width, doors + 1).into_iter().enumerate() {
        if i == doors {
            // Small office door.
            f.rect(m, a - 0.45, a + 0.45, 0.0, 2.2, 0.04, palette::DOOR);
            continue;
        }
        let dw = 1.6;
        f.rect(
            m,
            a - dw - 0.12,
            a + dw + 0.12,
            0.0,
            3.5,
            0.03,
            palette::INDUSTRY_STRIPE,
        );
        f.rect(m, a - dw, a + dw, 0.0, 3.35, 0.05, shade(palette::INDUSTRY_ROOF, 1.1));
        for k in 0..7 {
            let y = 0.4 + k as f32 * 0.42;
            f.rect(m, a - dw, a + dw, y, y + 0.06, 0.06, shade(palette::INDUSTRY_ROOF, 0.8));
        }
    }
    // Roof: sawtooth for the bigger works, shallow gable otherwise.
    if level >= 2 || (big && rng.chance(0.5)) {
        let teeth = ((body.w() / 3.5).round() as usize).max(2);
        let tw = body.w() / teeth as f32;
        for t in 0..teeth {
            let x0 = body.x0 + t as f32 * tw;
            let x1 = x0 + tw;
            let rh = 1.6;
            // Vertical glazed face towards -x, slope down towards +x.
            let (a, b, c, d) = (
                Vec3::new(x0, h, body.z0),
                Vec3::new(x0, h, body.z1),
                Vec3::new(x0, h + rh, body.z1),
                Vec3::new(x0, h + rh, body.z0),
            );
            m.quad(a, b, c, d, palette::GLASS);
            m.quad(d, c, Vec3::new(x1, h, body.z1), Vec3::new(x1, h, body.z0), roof);
            m.tri(a, d, Vec3::new(x1, h, body.z0), wall);
            m.tri(b, Vec3::new(x1, h, body.z1), c, wall);
        }
    } else {
        pitched_roof(m, body, h, body.d() * 0.16, true, false, 0.25, roof, wall);
    }
    // Yard props.
    let yard = Rect {
        x0: body.x1 + 0.3,
        x1: lot.x1 - 0.3,
        z0: body.z1 + 0.3,
        z1: lot.z1 - 0.3,
    };
    if rng.chance(if level >= 2 { 0.55 } else { 0.25 }) {
        let x = if rng.chance(0.5) { body.x0 + 1.0 } else { body.x1 - 1.0 };
        let c = Vec3::new(x, h - 0.5, body.z0 + 1.0);
        // Small works get a stubby flue; the tall stacks come with level 2.
        let ch = if level >= 2 {
            rng.range(5.0, 9.0) + if big { 3.0 } else { 0.0 }
        } else {
            rng.range(2.2, 3.6)
        };
        m.cylinder(c, 0.5, ch, 8, palette::BRICK, shade(palette::BRICK, 0.45));
        m.cylinder(
            c + Vec3::Y * (ch - 0.8),
            0.58,
            0.35,
            8,
            shade(palette::BRICK, 0.8),
            palette::BRICK,
        );
    }
    if rng.chance(0.55) {
        // Tanks beside the shed (or in front if there is no side yard).
        let n = 1 + rng.below(2);
        for i in 0..n {
            let r = rng.range(0.8, 1.2);
            let pos = if yard.w() > 2.0 * r + 0.2 {
                Vec3::new(yard.cx(), 0.0, body.z0 + 1.5 + i as f32 * 2.8)
            } else {
                Vec3::new(body.x0 + 1.4 + i as f32 * 2.6, 0.0, (body.z1 + lot.z1) * 0.5 - 0.2)
            };
            if pos.z + r > lot.z1 - 0.2 {
                continue;
            }
            m.cylinder(
                pos,
                r,
                rng.range(2.0, 3.2),
                10,
                palette::TANK,
                shade(palette::TANK, 1.1),
            );
        }
    } else {
        // Crate stacks in the front yard.
        let n = 2 + rng.below(4);
        let x = body.x1 - 1.0;
        for i in 0..n {
            let base = Vec3::new(x - (i % 3) as f32 * 0.95, (i / 3) as f32 * 0.85, body.z1 + 1.0);
            props::crate_box(m, base, 0.85, rng.range(-0.15, 0.15));
        }
    }
}

// ------------------------------------------------------------------ entry

/// Build the model for `key`.
pub fn generate(key: ModelKey, lod: bool) -> MeshBuilder {
    let mut m = MeshBuilder::with_ground_ao(0.32);
    m.lod = lod;
    let w = key.cells.0 as f32 * citysim_sim::CELL_SIZE;
    let d = key.cells.1 as f32 * citysim_sim::CELL_SIZE;
    let lot = Rect {
        x0: -w * 0.5,
        x1: w * 0.5,
        z0: -d * 0.5,
        z1: d * 0.5,
    };
    let kind_salt = match key.kind {
        Zone::Residential => 1,
        Zone::Commercial => 2,
        Zone::Industrial => 3,
    };
    let mut rng = Stream::new(
        key.variant as u32 * 7919 + key.level as u32 * 131 + kind_salt * 1_000_003 + key.cells.0 as u32 * 17,
    );
    let big = key.cells.0 >= 2 && key.cells.1 >= 2;
    // Heights grow only with level (the sim's top levels are
    // `balance::MAX_LEVEL` = R 3, C 3, I 2), so a young town is low-rise
    // and the skyline is earned by level-ups:
    // * level 1: one-storey houses (a few with a second floor), one-storey
    //   shops or a big-box store, small sheds (~4-7 m);
    // * level 2: two-storey townhouses and shops, taller works (~7-9 m);
    // * level 3: the tallest tier, 3-4 storey apartments and offices (one
    //   more on a big lot, ~11-17 m), so even a 5,000-resident city keeps a
    //   modest skyline. Levels 4-5 (unused) step up from there.
    let tall = |base: u32, rng: &mut Stream| (base + rng.below(2) + u32::from(big)).min(8);
    match key.kind {
        Zone::Residential => match key.level {
            0 | 1 if big => terrace(&mut m, &mut rng, lot, 1),
            0 | 1 => detached(&mut m, &mut rng, lot),
            2 => terrace(&mut m, &mut rng, lot, 2),
            3 => {
                let n = tall(3, &mut rng);
                apartments(&mut m, &mut rng, lot, n)
            }
            4 => {
                let n = tall(5, &mut rng);
                apartments(&mut m, &mut rng, lot, n)
            }
            _ => {
                let n = tall(6, &mut rng) + u32::from(big);
                apartments(&mut m, &mut rng, lot, n.min(8))
            }
        },
        Zone::Commercial => match key.level {
            0 | 1 if big => supermarket(&mut m, &mut rng, lot),
            0 | 1 => shop(&mut m, &mut rng, lot, 1),
            2 => shop(&mut m, &mut rng, lot, 2),
            3 => {
                let n = tall(3, &mut rng);
                office(&mut m, &mut rng, lot, n)
            }
            4 => {
                let n = tall(5, &mut rng);
                office(&mut m, &mut rng, lot, n)
            }
            _ => {
                let n = tall(6, &mut rng) + u32::from(big);
                office(&mut m, &mut rng, lot, n.min(8))
            }
        },
        Zone::Industrial => shed(&mut m, &mut rng, lot, key.level),
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_level_and_size_generates_sane_geometry() {
        for kind in Zone::ALL {
            for level in 1..=5u8 {
                for cells in [(1, 1), (2, 2), (1, 2), (2, 1), (3, 3)] {
                    for variant in 0..VARIANTS as u8 {
                        let m = generate(
                            ModelKey {
                                kind,
                                cells,
                                level,
                                variant,
                            },
                            variant % 2 == 0,
                        );
                        assert!(m.triangle_count() > 20, "{kind:?} {level} {cells:?}");
                        let half = cells.0.max(cells.1) as f32 * 4.0 + 1.0;
                        for p in &m.pos {
                            assert!(p.iter().all(|v| v.is_finite()));
                            assert!(p[0].abs() <= half && p[2].abs() <= half, "outside lot: {p:?} {kind:?}");
                            assert!(p[1] >= -0.01 && p[1] < 60.0, "height {p:?}");
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod height_tests {
    use super::*;

    /// (min, mean, max) model height over all variants.
    fn heights(kind: Zone, level: u8, cells: (u8, u8)) -> (f32, f32, f32) {
        let hs: Vec<f32> = (0..VARIANTS as u8)
            .map(|variant| {
                let m = generate(
                    ModelKey {
                        kind,
                        cells,
                        level,
                        variant,
                    },
                    true,
                );
                m.pos.iter().map(|p| p[1]).fold(0.0, f32::max)
            })
            .collect();
        let min = hs.iter().copied().fold(f32::MAX, f32::min);
        let max = hs.iter().copied().fold(0.0, f32::max);
        (min, hs.iter().sum::<f32>() / hs.len() as f32, max)
    }

    #[test]
    fn skyline_grows_with_level_and_level_one_is_low_rise() {
        for (kind, max_level) in Zone::ALL.into_iter().zip(citysim_sim::balance::MAX_LEVEL) {
            for cells in [(1, 1), (2, 2)] {
                let per: Vec<_> = (1..=max_level).map(|l| heights(kind, l, cells)).collect();
                for (l, h) in per.iter().enumerate() {
                    println!(
                        "{kind:?} {cells:?} L{}: min {:.1} mean {:.1} max {:.1}",
                        l + 1,
                        h.0,
                        h.1,
                        h.2
                    );
                }
                for w in per.windows(2) {
                    assert!(
                        w[1].1 > w[0].1 + 0.8,
                        "{kind:?} {cells:?}: mean height must grow per level"
                    );
                }
                // Level 1 is low-rise: houses, small shops, sheds.
                assert!(per[0].1 < 8.0, "{kind:?} {cells:?} level 1 too tall: {:?}", per[0]);
                // The top level is clearly the tallest tier (no overlap with
                // level 1 and the mean well above level 2's tallest).
                if max_level == 3 {
                    assert!(per[2].0 > per[0].2, "{kind:?} {cells:?}");
                    assert!(per[2].1 > per[1].2, "{kind:?} {cells:?}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tri_report {
    use super::*;

    #[test]
    #[ignore]
    fn report_triangles() {
        for kind in Zone::ALL {
            for level in 1..=4u8 {
                let total: usize = (0..VARIANTS as u8)
                    .map(|variant| {
                        generate(
                            ModelKey {
                                kind,
                                cells: (1, 1),
                                level,
                                variant,
                            },
                            true,
                        )
                        .triangle_count()
                    })
                    .sum();
                println!("{kind:?} L{level}: avg {} tris", total / VARIANTS as usize);
            }
        }
        let mut m = MeshBuilder::new();
        props::round_tree(&mut m, Vec3::ZERO, 1.0, Color::WHITE, 0.0);
        println!("round tree {}", m.triangle_count());
        let mut m = MeshBuilder::new();
        props::pine_tree(&mut m, Vec3::ZERO, 1.0, Color::WHITE, 0.0);
        println!("pine {}", m.triangle_count());
        println!(
            "car {}",
            super::super::vehicle_gen::car_model(Color::WHITE, 0, false).triangle_count()
        );
    }
}
