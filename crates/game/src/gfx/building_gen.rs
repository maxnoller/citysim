//! Procedural building models. One model per `(kind, lot size, level,
//! variant)` and level of detail, cached and shared by every building with
//! that key, so a city of thousands of buildings uses a few hundred meshes
//! and one material.
//!
//! Three levels of detail ([`Lod`]): `Near` adds the fine trim that only
//! reads up close (window sills and glazing bars, door panels and steps,
//! gutters, roof clutter, yard props); `Mid` keeps the shapes that read from
//! the play height (roof forms, dormers, chimneys, porches, bays,
//! balconies, awnings, framed windows); `Far` is a simple body with its
//! windows as flat glass. All three draw the same random numbers, so they
//! are the same building.
//!
//! The kinds live in `building_res` (houses, terraces, flats),
//! `building_com` (shops, offices, big-box stores) and `building_ind`
//! (works, warehouses, yards); the shared parts (facades, windows, doors,
//! roofs, roof clutter) are here.
//!
//! Local frame: the lot is centred on the origin, `x` across, the front
//! (facing the road) towards `+z`. Lot width `w` along x, depth `d` along z.

use bevy::prelude::*;
use citysim_sim::Zone;

use super::building_com as com;
use super::building_ind as ind;
use super::building_res as res;
use super::lot_props;
use super::mesh_util::{Glow, MeshBuilder};
use super::noise::Stream;
use super::palette::{self, lin, mix, shade};

/// Distinct models per kind / size / level. Facing adds four rotations.
pub const VARIANTS: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelKey {
    pub kind: Zone,
    /// Lot size in cells in the local frame (across, deep).
    pub cells: (u8, u8),
    pub level: u8,
    pub variant: u8,
}

/// Level of detail of a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lod {
    Near,
    Mid,
    Far,
}

impl Lod {
    pub const ALL: [Lod; 3] = [Lod::Near, Lod::Mid, Lod::Far];

    /// An empty builder for this level of detail.
    pub fn builder(self, ground_ao: f32) -> MeshBuilder {
        let mut m = MeshBuilder::with_ground_ao(ground_ao);
        m.lod = self == Lod::Far;
        m.fine = self == Lod::Near;
        m
    }
}

/// Lot-local rectangle.
#[derive(Debug, Clone, Copy)]
pub(super) struct Rect {
    pub x0: f32,
    pub x1: f32,
    pub z0: f32,
    pub z1: f32,
}

impl Rect {
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn d(&self) -> f32 {
        self.z1 - self.z0
    }
    pub fn cx(&self) -> f32 {
        (self.x0 + self.x1) * 0.5
    }
    pub fn cz(&self) -> f32 {
        (self.z0 + self.z1) * 0.5
    }
    pub fn inset(&self, k: f32) -> Rect {
        Rect {
            x0: self.x0 + k,
            x1: self.x1 - k,
            z0: self.z0 + k,
            z1: self.z1 - k,
        }
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x0 < o.x1 && o.x0 < self.x1 && self.z0 < o.z1 && o.z0 < self.z1
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(o.x0),
            x1: self.x1.max(o.x1),
            z0: self.z0.min(o.z0),
            z1: self.z1.max(o.z1),
        }
    }
}

/// How a window is dressed.
#[derive(Debug, Clone, Copy)]
pub(super) struct Win {
    pub frame: Color,
    pub glass: Color,
    /// Stone / painted sill under the window (and a head over it, near).
    pub sill: Option<Color>,
    /// Glazing bars: 0 none, 1 a centre bar, 2 a cross, 3 two bars and a
    /// transom, 4 a top light (transom only).
    pub bars: u8,
    /// Shutters either side.
    pub shutters: Option<Color>,
}

/// One side of an axis-aligned body, for putting things on walls.
#[derive(Debug, Clone, Copy)]
pub(super) struct Face {
    /// Bottom-left corner seen from outside.
    pub origin: Vec3,
    /// Rightwards along the face seen from outside.
    pub u: Vec3,
    /// Outward normal.
    pub n: Vec3,
    pub width: f32,
}

impl Face {
    pub fn of(r: Rect, y: f32) -> [Face; 4] {
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
    pub fn at(&self, a: f32, b: f32, off: f32) -> Vec3 {
        self.origin + self.u * a + Vec3::Y * b + self.n * off
    }

    /// Rectangle on the face (outward facing).
    #[allow(clippy::too_many_arguments)]
    pub fn rect(&self, m: &mut MeshBuilder, a0: f32, a1: f32, b0: f32, b1: f32, off: f32, color: Color) {
        m.quad(
            self.at(a0, b0, off),
            self.at(a1, b0, off),
            self.at(a1, b1, off),
            self.at(a0, b1, off),
            color,
        );
    }

    /// Dressed window centred at `a`, glass from `b` up `h`.
    #[allow(clippy::too_many_arguments)]
    pub fn window_s(&self, m: &mut MeshBuilder, a: f32, b: f32, w: f32, h: f32, st: &Win) {
        let f = 0.09;
        let r = wrand(self.at(a, b, 0.0));
        let (a0, a1) = (a - w * 0.5, a + w * 0.5);
        if m.lod {
            m.glow(Glow::Window, r, |m| {
                self.rect(m, a0, a1, b, b + h, 0.05, mix(st.glass, st.frame, 0.25))
            });
            return;
        }
        // Frame, then the glass just proud of it.
        self.rect(m, a0 - f, a1 + f, b - f, b + h + f, 0.03, st.frame);
        m.glow(Glow::Window, r, |m| self.rect(m, a0, a1, b, b + h, 0.045, st.glass));
        if let Some(s) = st.shutters {
            let sw = (w * 0.48).min(0.6);
            for (x0, x1) in [(a0 - f - sw, a0 - f), (a1 + f, a1 + f + sw)] {
                // Louvres: the siding pattern draws the slats.
                m.surface(Glow::Siding, |m| self.rect(m, x0, x1, b - 0.02, b + h + 0.02, 0.035, s));
            }
        }
        if let Some(sill) = st.sill {
            self.ledge(m, a0 - f - 0.08, a1 + f + 0.08, b - f - 0.09, b - f, 0.13, sill);
            m.fine_only(|m| self.ledge(m, a0 - f - 0.05, a1 + f + 0.05, b + h + f, b + h + f + 0.1, 0.05, sill));
        }
        m.fine_only(|m| {
            let bar = 0.055;
            let off = 0.058;
            let mut vbar = |x: f32| self.rect(m, x - bar * 0.5, x + bar * 0.5, b, b + h, off, st.frame);
            match st.bars {
                1 | 2 => vbar(a),
                3 => {
                    vbar(a0 + w / 3.0);
                    vbar(a0 + w * 2.0 / 3.0);
                }
                _ => {}
            }
            if matches!(st.bars, 2..=4) {
                let y = b + h * 0.68;
                self.rect(m, a0, a1, y - bar * 0.5, y + bar * 0.5, off, st.frame);
            }
        });
    }

    /// A ledge sticking out of the face: top and front (and ends, near).
    #[allow(clippy::too_many_arguments)]
    pub fn ledge(&self, m: &mut MeshBuilder, a0: f32, a1: f32, b0: f32, b1: f32, out: f32, color: Color) {
        let top = shade(color, 1.06);
        let under = shade(color, 0.7);
        let (p0, p1) = (self.at(a0, b1, 0.0), self.at(a1, b1, 0.0));
        let (q0, q1) = (self.at(a0, b1, out), self.at(a1, b1, out));
        m.quad_facing([p0, p1, q1, q0], Vec3::Y, top);
        self.rect(m, a0, a1, b0, b1, out, color);
        if m.fine {
            let (r0, r1) = (self.at(a0, b0, 0.0), self.at(a1, b0, 0.0));
            let (s0, s1) = (self.at(a0, b0, out), self.at(a1, b0, out));
            m.quad_facing([r0, r1, s1, s0], -Vec3::Y, under);
            m.quad_facing([r0, s0, q0, p0], -self.u, shade(color, 0.85));
            m.quad_facing([r1, s1, q1, p1], self.u, shade(color, 0.85));
        }
    }

    /// Glass rectangle lit at night as one window (`Window`) or as a shop
    /// front (`Shop`, always lit after dark).
    #[allow(clippy::too_many_arguments)]
    pub fn lit_rect(
        &self,
        m: &mut MeshBuilder,
        kind: Glow,
        a0: f32,
        a1: f32,
        b0: f32,
        b1: f32,
        off: f32,
        color: Color,
    ) {
        let r = wrand(self.at((a0 + a1) * 0.5, b0, 0.0));
        m.glow(kind, r, |m| self.rect(m, a0, a1, b0, b1, off, color));
    }

    /// Box sticking out of the face (canopies, balconies, signs).
    #[allow(clippy::too_many_arguments)]
    pub fn slab(&self, m: &mut MeshBuilder, a0: f32, a1: f32, b0: f32, b1: f32, out: f32, sides: Color, top: Color) {
        let p0 = self.at(a0, b0, 0.0);
        let p1 = self.at(a1, b0, out);
        let min = p0.min(p1).with_y(b0 + self.origin.y);
        let max = p0.max(p1).with_y(b1 + self.origin.y);
        m.cuboid2(min, max, sides, top);
    }

    /// Box on the face between `o0` and `o1` metres out (rails, fins).
    #[allow(clippy::too_many_arguments)]
    pub fn slab_out(&self, m: &mut MeshBuilder, a0: f32, a1: f32, b0: f32, b1: f32, o0: f32, o1: f32, color: Color) {
        let p0 = self.at(a0, b0, o0);
        let p1 = self.at(a1, b0, o1);
        let min = p0.min(p1).with_y(b0 + self.origin.y);
        let max = p0.max(p1).with_y(b1 + self.origin.y);
        m.cuboid2(min, max, color, shade(color, 1.08));
    }

    /// A front door centred at `a`: frame, leaf (panelled near, with a
    /// fanlight over it), a step and a small lamp beside it.
    #[allow(clippy::too_many_arguments)]
    pub fn door(&self, m: &mut MeshBuilder, a: f32, y0: f32, w: f32, h: f32, frame: Color, leaf: Color, step: Color) {
        let hw = w * 0.5;
        if m.lod {
            self.rect(m, a - hw, a + hw, y0, y0 + h, 0.05, leaf);
            return;
        }
        self.rect(m, a - hw - 0.1, a + hw + 0.1, y0, y0 + h + 0.1, 0.03, frame);
        let fan = 0.38;
        self.rect(m, a - hw, a + hw, y0, y0 + h - fan, 0.05, leaf);
        self.lit_rect(
            m,
            Glow::Window,
            a - hw,
            a + hw,
            y0 + h - fan + 0.06,
            y0 + h,
            0.045,
            palette::WINDOW,
        );
        // Step (two on a raised floor).
        self.slab(
            m,
            a - hw - 0.25,
            a + hw + 0.25,
            0.0,
            y0.max(0.12),
            0.45,
            step,
            shade(step, 1.08),
        );
        m.fine_only(|m| {
            if y0 > 0.3 {
                self.slab(
                    m,
                    a - hw - 0.3,
                    a + hw + 0.3,
                    0.0,
                    y0 * 0.5,
                    0.75,
                    step,
                    shade(step, 1.08),
                );
            }
            // Panels and a handle.
            let dark = shade(leaf, 0.8);
            let top = y0 + h - fan;
            for (b0, b1) in [(y0 + 0.15, y0 + (h - fan) * 0.45), (y0 + (h - fan) * 0.52, top - 0.12)] {
                self.rect(m, a - hw + 0.1, a - 0.04, b0, b1, 0.058, dark);
                self.rect(m, a + 0.04, a + hw - 0.1, b0, b1, 0.058, dark);
            }
            self.rect(
                m,
                a + hw - 0.17,
                a + hw - 0.1,
                y0 + 0.95,
                y0 + 1.07,
                0.065,
                palette::GUARDRAIL,
            );
            // Wall lamp.
            let lx = a + hw + 0.28;
            self.slab(
                m,
                lx - 0.07,
                lx + 0.07,
                y0 + h - 0.35,
                y0 + h - 0.12,
                0.12,
                palette::METAL_DARK,
                palette::METAL_DARK,
            );
            m.glow(Glow::Sign, 0.0, |m| {
                self.rect(
                    m,
                    lx - 0.05,
                    lx + 0.05,
                    y0 + h - 0.32,
                    y0 + h - 0.16,
                    0.125,
                    Color::srgb_u32(0xF6D9A0),
                )
            });
        });
    }
}

/// A stable pseudo-random value in `0..1` for a point of a model (which
/// windows are lit at night).
pub(super) fn wrand(p: Vec3) -> f32 {
    let h = (p.x * 12.9898 + p.y * 78.233 + p.z * 37.719).sin() * 43_758.547;
    h - h.floor()
}

/// Evenly spaced positions for `n` items across a face of `width`.
pub(super) fn spread(width: f32, n: usize) -> Vec<f32> {
    (0..n).map(|i| width * (i as f32 + 0.5) / n as f32).collect()
}

pub(super) fn lot_ground(m: &mut MeshBuilder, lot: Rect, color: Color) {
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

pub(super) fn flat_patch(m: &mut MeshBuilder, r: Rect, y: f32, color: Color) {
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

/// A box body whose walls carry a surface pattern (`finish`), top in `top`.
pub(super) fn body(m: &mut MeshBuilder, r: Rect, y0: f32, y1: f32, wall: Color, top: Color, finish: Option<Glow>) {
    let (min, max) = (Vec3::new(r.x0, y0, r.z0), Vec3::new(r.x1, y1, r.z1));
    match finish {
        Some(k) => m.surface(k, |m| m.cuboid2(min, max, wall, top)),
        None => m.cuboid2(min, max, wall, top),
    }
}

/// What a pitched roof looks like.
#[derive(Debug, Clone, Copy)]
pub(super) struct Roof {
    /// Ridge parallel to x (else to z).
    pub along_x: bool,
    /// Hipped (else gabled).
    pub hip: bool,
    pub over: f32,
    pub color: Color,
    pub wall: Color,
    /// Surface pattern of the slopes (tiles, or metal on works).
    pub finish: Glow,
}

/// Height of a pitched roof's surface over `(x, z)` (the slope plane of
/// the side the point is on), for things set into it.
pub(super) fn roof_height(r: Rect, eave: f32, rise: f32, along_x: bool, x: f32, z: f32) -> f32 {
    let (off, half) = if along_x {
        ((z - r.cz()).abs(), r.d() * 0.5)
    } else {
        ((x - r.cx()).abs(), r.w() * 0.5)
    };
    eave + rise * (1.0 - off / half)
}

/// Pitched roof over `r` from `eave` with ridge `rise` above it.
#[allow(clippy::too_many_arguments)]
pub(super) fn pitched_roof(
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
    pitched(
        m,
        r,
        eave,
        rise,
        &Roof {
            along_x,
            hip,
            over,
            color: roof,
            wall,
            finish: Glow::Roof,
        },
    );
}

/// Pitched roof over `r` from `eave` with ridge `rise` above it: tiled
/// slopes, fascia and verge boards, a ridge cap, gutters (near) and the
/// soffit under the overhang.
pub(super) fn pitched(m: &mut MeshBuilder, r: Rect, eave: f32, rise: f32, s: &Roof) {
    let (along_x, hip, over, roof, wall) = (s.along_x, s.hip, s.over, s.color, s.wall);
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
    m.surface(s.finish, |m| {
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
        if hip {
            for sgn in [1.0, -1.0] {
                face(
                    m,
                    &[p(sgn * ha, hb, ye), p(sgn * ha, -hb, ye), p(sgn * ridge, 0.0, yr)],
                    shade(roof, 0.86),
                );
            }
        }
    });
    if m.lod {
        // The distant model: closed underneath by the walls; no trim.
        if !hip {
            let (wa, wb) = (la * 0.5, lb * 0.5);
            for sgn in [1.0, -1.0] {
                face(
                    m,
                    &[p(sgn * wa, wb, eave), p(sgn * wa, -wb, eave), p(sgn * wa, 0.0, yr)],
                    wall,
                );
            }
        }
        return;
    }
    // Fascia boards along the eaves.
    let t = 0.14;
    for sgn in [1.0, -1.0] {
        face(
            m,
            &[
                p(-ha, sgn * hb, ye - t),
                p(ha, sgn * hb, ye - t),
                p(ha, sgn * hb, ye),
                p(-ha, sgn * hb, ye),
            ],
            dark,
        );
    }
    if hip {
        for sgn in [1.0, -1.0] {
            face(
                m,
                &[
                    p(sgn * ha, hb, ye - t),
                    p(sgn * ha, -hb, ye - t),
                    p(sgn * ha, -hb, ye),
                    p(sgn * ha, hb, ye),
                ],
                dark,
            );
        }
    } else {
        // Gable walls (in the wall colour) and verge boards.
        let (wa, wb) = (la * 0.5, lb * 0.5);
        for sgn in [1.0, -1.0] {
            face(
                m,
                &[p(sgn * wa, wb, eave), p(sgn * wa, -wb, eave), p(sgn * wa, 0.0, yr)],
                wall,
            );
            for side in [1.0, -1.0] {
                face(
                    m,
                    &[
                        p(sgn * ha, side * hb, ye - t),
                        p(sgn * ha, side * hb, ye),
                        p(sgn * ha, 0.0, yr),
                        p(sgn * ha, 0.0, yr - t),
                    ],
                    dark,
                );
            }
        }
    }
    // Ridge cap: a little rounded-looking strip along the ridge.
    if ridge > 0.05 {
        let cap = shade(roof, 0.8);
        let (w, h) = (0.13, 0.09);
        for sgn in [1.0, -1.0] {
            face(
                m,
                &[
                    p(-ridge - 0.02, sgn * w, yr - 0.03),
                    p(ridge + 0.02, sgn * w, yr - 0.03),
                    p(ridge + 0.02, 0.0, yr + h),
                    p(-ridge - 0.02, 0.0, yr + h),
                ],
                cap,
            );
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
    // Gutters along the eaves (near).
    m.fine_only(|m| {
        let g = palette::METAL_DARK;
        for sgn in [1.0, -1.0] {
            let a = p(-ha, sgn * (hb + 0.06), ye - 0.12);
            let b = p(ha, sgn * (hb - 0.04), ye - 0.02);
            m.cuboid(a.min(b), a.max(b), g);
        }
    });
}

/// A dormer in a pitched roof (ridge along x, over `r`, eave `eave`, rise
/// `rise`) on the front (`front`) or back slope, centred at `x`: a small
/// gabled box with a window, set into the slope.
#[allow(clippy::too_many_arguments)]
pub(super) fn dormer(
    m: &mut MeshBuilder,
    r: Rect,
    eave: f32,
    rise: f32,
    x: f32,
    w: f32,
    front: bool,
    roof: Color,
    wall: Color,
    win: &Win,
) {
    let half = r.d() * 0.5;
    // The dormer's face stands 40 % of the way up the slope.
    let k = 0.38;
    let yf = eave + rise * k;
    let h = (rise * 0.5).clamp(0.9, 1.5);
    let y1 = yf + h;
    // Its own ridge must stay under the main ridge.
    let drise = (w * 0.45).min(eave + rise - y1 - 0.12);
    if drise < 0.2 {
        return;
    }
    let sgn = if front { 1.0 } else { -1.0 };
    let zf = r.cz() + sgn * half * (1.0 - k);
    let rect = if front {
        Rect {
            x0: x - w * 0.5,
            x1: x + w * 0.5,
            z0: r.cz(),
            z1: zf,
        }
    } else {
        Rect {
            x0: x - w * 0.5,
            x1: x + w * 0.5,
            z0: zf,
            z1: r.cz(),
        }
    };
    m.cuboid2(
        Vec3::new(rect.x0, yf - 0.3, rect.z0),
        Vec3::new(rect.x1, y1, rect.z1),
        wall,
        wall,
    );
    pitched_roof(m, rect, y1, drise, false, false, 0.12, roof, wall);
    let f = Face::of(rect, yf)[if front { 0 } else { 2 }];
    let ww = (w - 0.5).max(0.5);
    f.window_s(m, f.width * 0.5, 0.12, ww, h - 0.32, win);
}

/// Flat roof with a parapet and its coping.
pub(super) fn flat_roof(m: &mut MeshBuilder, r: Rect, top: f32, wall: Color, roof: Color, parapet: f32) {
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
    // Coping stones over the parapet, a little proud of both sides.
    if !m.lod {
        let c = palette::STONE;
        let e = 0.06;
        for (min, max) in [
            (
                Vec3::new(r.x0 - e, y1, r.z0 - e),
                Vec3::new(r.x1 + e, y1 + 0.08, r.z0 + t + e),
            ),
            (
                Vec3::new(r.x0 - e, y1, r.z1 - t - e),
                Vec3::new(r.x1 + e, y1 + 0.08, r.z1 + e),
            ),
            (
                Vec3::new(r.x0 - e, y1, r.z0 + t + e),
                Vec3::new(r.x0 + t + e, y1 + 0.08, r.z1 - t - e),
            ),
            (
                Vec3::new(r.x1 - t - e, y1, r.z0 + t + e),
                Vec3::new(r.x1 + e, y1 + 0.08, r.z1 - t - e),
            ),
        ] {
            m.cuboid2(min, max, shade(c, 0.9), c);
        }
    }
    m.surface(Glow::Membrane, |m| flat_patch(m, r.inset(t), top + 0.02, roof));
}

/// A cornice: a moulded band right round a body at `y` (a stepped ledge
/// on every face).
pub(super) fn cornice(m: &mut MeshBuilder, r: Rect, y: f32, h: f32, out: f32, color: Color) {
    for f in Face::of(r, 0.0) {
        f.ledge(m, -out, f.width + out, y, y + h, out, color);
        if m.fine {
            f.ledge(
                m,
                -out * 0.5,
                f.width + out * 0.5,
                y - h * 0.5,
                y,
                out * 0.5,
                shade(color, 0.92),
            );
        }
    }
}

/// Rooftop clutter on a flat roof: AC units with fan grilles, vents, a
/// water tank on legs, skylights, an aerial or a satellite dish. Mid keeps
/// the bigger boxes; near adds the small things.
pub(super) fn roof_details(m: &mut MeshBuilder, rng: &mut Stream, r: Rect, top: f32) {
    if m.lod {
        // Still draw the same random numbers.
        let mut scratch = MeshBuilder::new();
        roof_details_inner(&mut scratch, rng, r, top);
        return;
    }
    roof_details_inner(m, rng, r, top);
}

fn roof_details_inner(m: &mut MeshBuilder, rng: &mut Stream, r: Rect, top: f32) {
    let unit = palette::ROOF_DETAIL;
    let n = 1 + rng.below(3);
    let mut placed: Vec<Rect> = Vec::new();
    for _ in 0..n {
        let w = rng.range(0.9, 1.6);
        let d = rng.range(0.8, 1.2);
        let h = rng.range(0.6, 0.95);
        let x = rng.range(r.x0 + 0.6 + w * 0.5, (r.x1 - 0.6 - w * 0.5).max(r.x0 + 0.6 + w * 0.5));
        let z = rng.range(r.z0 + 0.6 + d * 0.5, (r.z1 - 0.6 - d * 0.5).max(r.z0 + 0.6 + d * 0.5));
        let b = Rect {
            x0: x - w * 0.5,
            x1: x + w * 0.5,
            z0: z - d * 0.5,
            z1: z + d * 0.5,
        };
        if placed.iter().any(|p| p.overlaps(&b)) {
            continue;
        }
        placed.push(b);
        m.cuboid2(
            Vec3::new(b.x0, top, b.z0),
            Vec3::new(b.x1, top + h, b.z1),
            unit,
            shade(unit, 1.15),
        );
        // Fan grille on top and a louvre band on the side (near).
        m.fine_only(|m| {
            let c = Vec3::new(x, top + h + 0.01, z);
            let rr = (w.min(d) * 0.32).min(0.4);
            let ring: Vec<Vec3> = (0..8)
                .map(|i| {
                    let a = i as f32 / 8.0 * std::f32::consts::TAU;
                    c + Vec3::new(a.cos() * rr, 0.0, a.sin() * rr)
                })
                .collect();
            m.fan_up(c, &ring, lin(shade(unit, 0.55)));
            let f = Face::of(b, top)[0];
            for k in 0..3 {
                let y = 0.15 + k as f32 * 0.14;
                f.rect(m, 0.1, f.width - 0.1, y, y + 0.06, 0.01, shade(unit, 0.75));
            }
        });
    }
    // Vents: little pipes with caps (near).
    let vents = rng.below(4);
    let vent_spots: Vec<Vec2> = (0..vents)
        .map(|_| Vec2::new(rng.range(r.x0 + 0.4, r.x1 - 0.4), rng.range(r.z0 + 0.4, r.z1 - 0.4)))
        .collect();
    m.fine_only(|m| {
        for p in vent_spots {
            let c = Vec3::new(p.x, top, p.y);
            m.cylinder(c, 0.09, 0.55, 6, unit, unit);
            m.cylinder(c + Vec3::Y * 0.55, 0.16, 0.08, 6, shade(unit, 0.8), shade(unit, 1.1));
        }
    });
    if rng.chance(0.3) {
        let c = Vec3::new(r.cx() + rng.range(-1.0, 1.0), top, r.z0 + 1.4);
        let legs = 0.5;
        m.cylinder(
            c + Vec3::Y * legs,
            0.65,
            1.3,
            10,
            palette::TANK,
            shade(palette::TANK, 1.1),
        );
        m.cone(
            c + Vec3::Y * (legs + 1.3),
            0.66,
            0.3,
            10,
            shade(palette::TANK, 1.05),
            0.0,
        );
        for (dx, dz) in [(-0.4, -0.4), (0.4, -0.4), (-0.4, 0.4), (0.4, 0.4)] {
            let p = c + Vec3::new(dx, 0.0, dz);
            m.cuboid(
                p - Vec3::new(0.05, 0.0, 0.05),
                p + Vec3::new(0.05, legs, 0.05),
                palette::METAL_DARK,
            );
        }
    }
    let skylights = rng.chance(0.4);
    let sky_x = rng.range(r.x0 + 0.8, (r.x1 - 2.0).max(r.x0 + 0.8));
    if skylights && r.w() > 4.0 && r.d() > 3.0 {
        let z = r.z1 - 1.4;
        for k in 0..2 {
            let x = sky_x + k as f32 * 1.1;
            let b = Rect {
                x0: x,
                x1: x + 0.8,
                z0: z - 0.6,
                z1: z + 0.6,
            };
            if placed.iter().any(|p| p.overlaps(&b)) || b.x1 > r.x1 - 0.3 {
                continue;
            }
            m.cuboid2(
                Vec3::new(b.x0, top, b.z0),
                Vec3::new(b.x1, top + 0.25, b.z1),
                palette::ROOF_DETAIL,
                palette::ROOF_DETAIL,
            );
            m.glow(Glow::Window, wrand(Vec3::new(x, top, z)), |m| {
                flat_patch(m, b.inset(0.08), top + 0.26, palette::GLASS)
            });
        }
    }
    let aerial = rng.chance(0.5);
    let ax = rng.range(r.x0 + 0.5, r.x1 - 0.5);
    let dish = rng.chance(0.35);
    m.fine_only(|m| {
        if aerial {
            let c = Vec3::new(ax, top, r.z0 + 0.5);
            m.cuboid(
                c - Vec3::new(0.03, 0.0, 0.03),
                c + Vec3::new(0.03, 2.4, 0.03),
                palette::METAL_DARK,
            );
            for (y, w) in [(2.0, 0.7), (2.25, 0.5)] {
                m.cuboid(
                    c + Vec3::new(-w * 0.5, y, -0.02),
                    c + Vec3::new(w * 0.5, y + 0.04, 0.02),
                    palette::METAL_DARK,
                );
            }
        }
        if dish {
            let c = Vec3::new(r.x1 - 0.7, top + 0.4, r.z1 - 0.7);
            m.cuboid(
                Vec3::new(c.x - 0.04, top, c.z - 0.04),
                Vec3::new(c.x + 0.04, c.y, c.z + 0.04),
                palette::METAL_DARK,
            );
            m.quad_facing(
                [
                    c + Vec3::new(-0.3, -0.25, 0.05),
                    c + Vec3::new(0.3, -0.25, 0.05),
                    c + Vec3::new(0.3, 0.3, 0.2),
                    c + Vec3::new(-0.3, 0.3, 0.2),
                ],
                Vec3::new(0.0, 0.4, 1.0).normalize(),
                palette::FENCE,
            );
        }
    });
}

/// Chimney stack in `brick` with `pots` pots.
pub(super) fn chimney_c(m: &mut MeshBuilder, x: f32, z: f32, y0: f32, y1: f32, brick: Color, pots: u32) {
    let hw = 0.24 + 0.17 * (pots.max(1) - 1) as f32;
    m.surface(Glow::Brick, |m| {
        m.cuboid2(
            Vec3::new(x - hw, y0, z - 0.3),
            Vec3::new(x + hw, y1, z + 0.3),
            brick,
            shade(brick, 0.5),
        );
    });
    // Cap.
    m.cuboid2(
        Vec3::new(x - hw - 0.06, y1, z - 0.36),
        Vec3::new(x + hw + 0.06, y1 + 0.1, z + 0.36),
        shade(brick, 0.85),
        shade(brick, 0.45),
    );
    m.fine_only(|m| {
        let pot = Color::srgb_u32(0xB0684A);
        for k in 0..pots.max(1) {
            let px = x - hw + 0.24 + k as f32 * 0.34;
            m.cylinder(Vec3::new(px, y1 + 0.1, z), 0.09, 0.32, 6, pot, shade(pot, 0.4));
        }
    });
}

pub(super) fn hedge(m: &mut MeshBuilder, a: Vec2, b: Vec2, h: f32) {
    let min = a.min(b) - Vec2::splat(0.22);
    let max = a.max(b) + Vec2::splat(0.22);
    let top = shade(palette::HEDGE, 1.12);
    m.cuboid2(
        Vec3::new(min.x, 0.0, min.y),
        Vec3::new(max.x, h, max.y),
        palette::HEDGE,
        top,
    );
    // A softer, rounded top (near).
    m.fine_only(|m| {
        let e = 0.08;
        m.cuboid2(
            Vec3::new(min.x + e, h, min.y + e),
            Vec3::new(max.x - e, h + 0.1, max.y - e),
            shade(palette::HEDGE, 1.05),
            shade(palette::HEDGE, 1.2),
        );
    });
}

/// Low garden wall with a coping and piers at its ends.
pub(super) fn garden_wall(m: &mut MeshBuilder, a: Vec2, b: Vec2, brick: Color) {
    let min = a.min(b) - Vec2::splat(0.12);
    let max = a.max(b) + Vec2::splat(0.12);
    m.surface(Glow::Brick, |m| {
        m.cuboid2(
            Vec3::new(min.x, 0.0, min.y),
            Vec3::new(max.x, 0.55, max.y),
            brick,
            brick,
        )
    });
    m.cuboid2(
        Vec3::new(min.x - 0.04, 0.55, min.y - 0.04),
        Vec3::new(max.x + 0.04, 0.63, max.y + 0.04),
        palette::STONE,
        palette::STONE,
    );
    m.fine_only(|m| {
        for p in [a, b] {
            m.surface(Glow::Brick, |m| {
                m.cuboid2(
                    Vec3::new(p.x - 0.2, 0.0, p.y - 0.2),
                    Vec3::new(p.x + 0.2, 0.8, p.y + 0.2),
                    brick,
                    brick,
                )
            });
            m.cuboid2(
                Vec3::new(p.x - 0.24, 0.8, p.y - 0.24),
                Vec3::new(p.x + 0.24, 0.88, p.y + 0.24),
                palette::STONE,
                palette::STONE,
            );
        }
    });
}

pub(super) fn fence(m: &mut MeshBuilder, a: Vec2, b: Vec2) {
    m.detail_only(|m| fence_inner(m, a, b));
}

fn fence_inner(m: &mut MeshBuilder, a: Vec2, b: Vec2) {
    let len = a.distance(b);
    if len < 0.3 {
        return;
    }
    let dir = (b - a) / len;
    // Posts every 1.2 m (mid), pickets between them (near).
    let n = ((len / 1.2).ceil() as usize).max(1);
    for i in 0..=n {
        let p = a + dir * (len * i as f32 / n as f32);
        m.cuboid(
            Vec3::new(p.x - 0.05, 0.0, p.y - 0.05),
            Vec3::new(p.x + 0.05, 0.85, p.y + 0.05),
            palette::FENCE,
        );
    }
    let min = a.min(b) - Vec2::splat(0.025);
    let max = a.max(b) + Vec2::splat(0.025);
    for y in [0.32, 0.62] {
        m.cuboid(
            Vec3::new(min.x, y, min.y),
            Vec3::new(max.x, y + 0.07, max.y),
            palette::FENCE,
        );
    }
    m.fine_only(|m| {
        // Pickets: thin boards facing both ways, every 0.25 m.
        let k = ((len / 0.25).floor() as usize).max(1);
        let n = Vec3::new(-dir.y, 0.0, dir.x);
        let col = shade(palette::FENCE, 0.96);
        for i in 0..k {
            let p = a + dir * (len * (i as f32 + 0.5) / k as f32) + Vec2::new(n.x, n.z) * 0.04;
            let (l, r) = (p - dir * 0.04, p + dir * 0.04);
            let q = [
                Vec3::new(l.x, 0.08, l.y),
                Vec3::new(r.x, 0.08, r.y),
                Vec3::new(r.x, 0.78, r.y),
                Vec3::new(l.x, 0.78, l.y),
            ];
            m.quad_facing(q, n, col);
            m.quad_facing(q, -n, col);
        }
    });
}

/// Downpipe from the eaves at `(x, z)` on a wall (near).
pub(super) fn downpipe(m: &mut MeshBuilder, x: f32, z: f32, top: f32) {
    m.fine_only(|m| {
        m.cuboid(
            Vec3::new(x - 0.05, 0.1, z - 0.05),
            Vec3::new(x + 0.05, top, z + 0.05),
            palette::METAL_DARK,
        )
    });
}

// ------------------------------------------------------------------ entry

/// The model for `key` at a level of detail (for the model sheet).
pub fn generate_lod(key: ModelKey, lod: usize) -> Option<MeshBuilder> {
    Lod::ALL.get(lod).map(|&l| generate(key, l))
}

/// Build the model for `key`.
pub fn generate(key: ModelKey, lod: Lod) -> MeshBuilder {
    let mut m = lod.builder(0.32);
    if lod == Lod::Far {
        m.tag = [Glow::Body as u32 as f32, 0.0];
    }
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
            0 | 1 if big => res::terrace(&mut m, &mut rng, lot, 1),
            0 | 1 => res::detached(&mut m, &mut rng, lot),
            2 => res::terrace(&mut m, &mut rng, lot, 2),
            3 => {
                let n = tall(3, &mut rng);
                res::apartments(&mut m, &mut rng, lot, n)
            }
            4 => {
                let n = tall(5, &mut rng);
                res::apartments(&mut m, &mut rng, lot, n)
            }
            _ => {
                let n = tall(6, &mut rng) + u32::from(big);
                res::apartments(&mut m, &mut rng, lot, n.min(8))
            }
        },
        Zone::Commercial => match key.level {
            0 | 1 if big => com::supermarket(&mut m, &mut rng, lot),
            0 | 1 => com::shop(&mut m, &mut rng, lot, 1),
            2 => com::shop(&mut m, &mut rng, lot, 2),
            3 => {
                let n = tall(3, &mut rng);
                com::office(&mut m, &mut rng, lot, n)
            }
            4 => {
                let n = tall(5, &mut rng);
                com::office(&mut m, &mut rng, lot, n)
            }
            _ => {
                let n = tall(6, &mut rng) + u32::from(big);
                com::office(&mut m, &mut rng, lot, n.min(8))
            }
        },
        Zone::Industrial => ind::works(&mut m, &mut rng, lot, key.level),
    }
    m
}

// ------------------------------------------------------------------ building sites

/// Storeys a finished building of `key` will roughly have (for the frame).
fn site_storeys(key: ModelKey) -> u32 {
    let big = key.cells.0 >= 2 && key.cells.1 >= 2;
    match (key.kind, key.level) {
        (Zone::Industrial, _) => 1,
        (_, 0 | 1) => 1,
        (_, 2) => 2,
        _ => 4 + u32::from(big),
    }
}

/// A building site on the lot of `key`: hoarding round the lot, a
/// foundation slab, a portacabin and materials (`stage` 0); then the bare
/// frame with scaffolding and, for tall buildings, a tower crane (1).
pub fn site(key: ModelKey, stage: u8) -> MeshBuilder {
    let mut m = MeshBuilder::with_ground_ao(0.25);
    let mut rng = Stream::new(key.variant as u32 * 31 + key.level as u32 * 7 + key.cells.0 as u32 * 3 + stage as u32);
    let w = key.cells.0 as f32 * citysim_sim::CELL_SIZE;
    let d = key.cells.1 as f32 * citysim_sim::CELL_SIZE;
    let lot = Rect {
        x0: -w * 0.5,
        x1: w * 0.5,
        z0: -d * 0.5,
        z1: d * 0.5,
    };
    let mud = mix(palette::EARTH, Color::srgb_u32(0x8A7558), 0.5);
    lot_ground(&mut m, lot, mud);
    let body = Rect {
        x0: lot.x0 + 1.2,
        x1: lot.x1 - 1.2,
        z0: lot.z0 + 1.2,
        z1: lot.z1 - 2.2,
    };
    let concrete = Color::srgb_u32(0xB9B6AE);
    m.cuboid2(
        Vec3::new(body.x0, 0.0, body.z0),
        Vec3::new(body.x1, 0.35, body.z1),
        shade(concrete, 0.85),
        concrete,
    );
    // Hoarding: white panels with an orange band, a gap for the gate.
    let white = Color::srgb_u32(0xEEEDE8);
    let orange = palette::INDUSTRY_STRIPE;
    let e = 0.25;
    let gate = (lot.cx() - 1.6, lot.cx() + 1.6);
    let mut panel = |a: Vec2, b: Vec2| {
        let mid = (a + b) * 0.5;
        let dvec = b - a;
        let yaw = -dvec.y.atan2(dvec.x);
        let len = dvec.length();
        if len < 0.1 {
            return;
        }
        m.surface(Glow::Cladding, |m| {
            m.box_yaw(
                Vec3::new(mid.x, 0.0, mid.y),
                Vec3::new(len, 1.9, 0.06),
                yaw,
                white,
                white,
            )
        });
        m.box_yaw(
            Vec3::new(mid.x, 1.3, mid.y),
            Vec3::new(len + 0.01, 0.25, 0.08),
            yaw,
            orange,
            orange,
        );
    };
    panel(Vec2::new(lot.x0 + e, lot.z0 + e), Vec2::new(lot.x1 - e, lot.z0 + e));
    panel(Vec2::new(lot.x0 + e, lot.z0 + e), Vec2::new(lot.x0 + e, lot.z1 - e));
    panel(Vec2::new(lot.x1 - e, lot.z0 + e), Vec2::new(lot.x1 - e, lot.z1 - e));
    panel(Vec2::new(lot.x0 + e, lot.z1 - e), Vec2::new(gate.0, lot.z1 - e));
    panel(Vec2::new(gate.1, lot.z1 - e), Vec2::new(lot.x1 - e, lot.z1 - e));
    // Portacabin and a sand heap.
    let cabin = Color::srgb_u32(0x3F7FB8);
    let cx = if rng.chance(0.5) { lot.x0 + 1.9 } else { lot.x1 - 1.9 };
    if key.cells.0 >= 2 || stage == 0 {
        m.surface(Glow::Cladding, |m| {
            m.cuboid2(
                Vec3::new(cx - 1.3, 0.0, lot.z1 - 2.0),
                Vec3::new(cx + 1.3, 2.4, lot.z1 - 0.6),
                cabin,
                shade(cabin, 1.2),
            )
        });
        // Windows and a door on the cabin's long side.
        let f = Face::of(
            Rect {
                x0: cx - 1.3,
                x1: cx + 1.3,
                z0: lot.z1 - 2.0,
                z1: lot.z1 - 0.6,
            },
            0.0,
        )[2];
        f.lit_rect(&mut m, Glow::Window, 0.3, 1.1, 1.1, 1.8, 0.02, palette::WINDOW);
        f.rect(&mut m, 1.6, 2.2, 0.1, 2.0, 0.02, shade(cabin, 0.7));
    }
    let sand = Color::srgb_u32(0xD8C08A);
    m.cone(
        Vec3::new(lot.cx() + rng.range(-1.5, 1.5), 0.0, lot.z1 - 1.4),
        0.9,
        0.8,
        7,
        sand,
        0.1,
    );
    lot_props::pallets(&mut m, &mut rng, Vec2::new(lot.x0 + 1.4, body.z0 + 1.0));
    if stage == 0 {
        // Footings and rebar stubs.
        let steel = Color::srgb_u32(0x6E5A4A);
        let mut x = body.x0 + 0.3;
        while x < body.x1 {
            for z in [body.z0 + 0.3, body.z1 - 0.3] {
                m.cuboid(
                    Vec3::new(x - 0.05, 0.35, z - 0.05),
                    Vec3::new(x + 0.05, 1.3, z + 0.05),
                    steel,
                );
            }
            x += 1.6;
        }
        return m;
    }
    // Bare frame: columns and floor slabs.
    let storeys = site_storeys(key);
    let sh = if key.kind == Zone::Industrial { 5.0 } else { 3.0 };
    let top = 0.35 + storeys as f32 * sh;
    let grey = Color::srgb_u32(0xA7A49C);
    let step = if body.w() > 10.0 {
        3.5
    } else {
        (body.w() / 2.0).max(1.8)
    };
    let mut x = body.x0;
    while x <= body.x1 + 0.01 {
        for z in [body.z0, body.cz(), body.z1] {
            m.cuboid(
                Vec3::new(x - 0.15, 0.35, z - 0.15),
                Vec3::new(x + 0.15, top, z + 0.15),
                grey,
            );
        }
        x += step;
    }
    for k in 1..=storeys {
        let y = 0.35 + k as f32 * sh;
        m.cuboid2(
            Vec3::new(body.x0 - 0.1, y - 0.25, body.z0 - 0.1),
            Vec3::new(body.x1 + 0.1, y, body.z1 + 0.1),
            shade(concrete, 0.8),
            concrete,
        );
    }
    // Scaffold along the front: poles, boards and a safety net.
    let pole = Color::srgb_u32(0xC9CCCF);
    let board = Color::srgb_u32(0xC49A5A);
    let zf = body.z1 + 0.9;
    let mut x = body.x0 - 0.2;
    while x <= body.x1 + 0.3 {
        for z in [body.z1 + 0.25, zf] {
            m.cuboid(
                Vec3::new(x - 0.04, 0.0, z - 0.04),
                Vec3::new(x + 0.04, top + 1.0, z + 0.04),
                pole,
            );
        }
        x += 2.0;
    }
    for k in 1..=storeys {
        let y = 0.35 + k as f32 * sh - 0.3;
        m.cuboid(
            Vec3::new(body.x0 - 0.2, y, body.z1 + 0.25),
            Vec3::new(body.x1 + 0.3, y + 0.06, zf),
            board,
        );
    }
    let net = Color::srgba(0.32, 0.62, 0.42, 1.0);
    m.detail_only(|m| {
        m.quad_facing(
            [
                Vec3::new(body.x0 - 0.2, 1.0, zf + 0.05),
                Vec3::new(body.x1 + 0.3, 1.0, zf + 0.05),
                Vec3::new(body.x1 + 0.3, top * 0.6, zf + 0.05),
                Vec3::new(body.x0 - 0.2, top * 0.6, zf + 0.05),
            ],
            Vec3::Z,
            net,
        );
    });
    // Tower crane for tall buildings.
    if storeys >= 4 {
        let yellow = palette::ACCENTS[2];
        let c = Vec3::new(body.x0 - 0.2, 0.0, body.z0 + 0.6);
        let h = top + 9.0;
        let mast = 0.9;
        m.cuboid2(
            c - Vec3::new(1.0, 0.0, 1.0),
            c + Vec3::new(1.0, 0.6, 1.0),
            concrete,
            concrete,
        );
        for (dx, dz) in [(0.0, 0.0), (mast, 0.0), (0.0, mast), (mast, mast)] {
            let p = c + Vec3::new(dx - mast * 0.5, 0.0, dz - mast * 0.5);
            m.cuboid(p - Vec3::new(0.07, 0.0, 0.07), p + Vec3::new(0.07, h, 0.07), yellow);
        }
        let mut y = 1.5;
        while y < h {
            m.cuboid(
                c + Vec3::new(-mast * 0.5, y, -mast * 0.5),
                c + Vec3::new(mast * 0.5, y + 0.08, mast * 0.5),
                yellow,
            );
            // Diagonal bracing on two faces of the mast.
            for (dir, n) in [(Vec3::X, Vec3::Z), (Vec3::Z, Vec3::X)] {
                let o = c + n * (mast * 0.5 + 0.01) - dir * mast * 0.5 + Vec3::Y * (y - 1.5);
                let (a, b) = (o, o + dir * mast + Vec3::Y * 1.5);
                let side = n * 0.04;
                m.quad_facing(
                    [a - side, b - side, b + side + Vec3::Y * 0.08, a + side + Vec3::Y * 0.08],
                    n,
                    yellow,
                );
            }
            y += 1.5;
        }
        // Jib over the building, counter-jib with its weight, the cab.
        let jib = (body.w() + 6.0).min(26.0);
        m.cuboid(
            c + Vec3::new(-mast * 0.5, h, -0.4),
            c + Vec3::new(jib, h + 0.8, 0.4),
            yellow,
        );
        m.cuboid(
            c + Vec3::new(-7.0, h, -0.45),
            c + Vec3::new(-mast * 0.5, h + 0.7, 0.45),
            yellow,
        );
        m.cuboid2(
            c + Vec3::new(-6.8, h - 1.2, -0.7),
            c + Vec3::new(-5.0, h, 0.7),
            concrete,
            concrete,
        );
        m.cuboid2(
            c + Vec3::new(-0.6, h - 1.4, -0.7),
            c + Vec3::new(0.6, h, 0.7),
            yellow,
            shade(yellow, 1.1),
        );
        m.cuboid(
            c + Vec3::new(-0.2, h + 0.8, -0.2),
            c + Vec3::new(0.2, h + 3.2, 0.2),
            yellow,
        );
        // Hook line.
        let hx = jib * 0.6;
        m.cuboid(
            c + Vec3::new(hx - 0.02, top + 1.0, -0.02),
            c + Vec3::new(hx + 0.02, h, 0.02),
            Color::srgb_u32(0x3A3D40),
        );
        m.cuboid(
            c + Vec3::new(hx - 0.25, top + 0.6, -0.25),
            c + Vec3::new(hx + 0.25, top + 1.0, 0.25),
            palette::INDUSTRY_STRIPE,
        );
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
                            [Lod::Near, Lod::Mid, Lod::Far][variant as usize % 3],
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
                    Lod::Far,
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

    /// Triangles and generation time per kind, level, lot and level of
    /// detail: `cargo test -p citysim --release report_triangles -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn report_triangles() {
        let mut total_ms = 0.0;
        for kind in Zone::ALL {
            for level in 1..=3u8 {
                for cells in [(1, 1), (1, 2), (2, 2)] {
                    let mut line = format!("{kind:?} L{level} {cells:?}:");
                    for lod in Lod::ALL {
                        let t = std::time::Instant::now();
                        let total: usize = (0..VARIANTS as u8)
                            .map(|variant| {
                                generate(
                                    ModelKey {
                                        kind,
                                        cells,
                                        level,
                                        variant,
                                    },
                                    lod,
                                )
                                .triangle_count()
                            })
                            .sum();
                        let ms = t.elapsed().as_secs_f64() * 1000.0 / VARIANTS as f64;
                        total_ms += ms;
                        line.push_str(&format!(" {lod:?} {} tris {ms:.2} ms;", total / VARIANTS as usize));
                    }
                    println!("{line}");
                }
            }
        }
        println!("all three levels of one model of each key: {total_ms:.1} ms");
    }
}
