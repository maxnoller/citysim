//! Industry: works, warehouses and process plants with their yards. See
//! `building_gen` for the frame and the levels of detail.
//!
//! By seed a lot gets one of three forms: a works (sawtooth roof on the
//! bigger ones, roller doors, a stack), a warehouse (a long low shed with a
//! row of dock doors and an office annex), or a process plant (a smaller
//! shed with silos or tanks, a pipe rack and a banded stack). Walls and
//! roofs are profiled metal (`Glow::Cladding`, drawn by the shader).

use bevy::prelude::*;

use super::building_gen::{Face, Rect, Roof, Win, body, flat_roof, lot_ground, pitched, roof_details, spread};
use super::lot_props;
use super::mesh_util::{Glow, MeshBuilder};
use super::noise::Stream;
use super::palette::{self, mix, shade};
use super::props;

#[derive(Clone, Copy, PartialEq)]
enum Form {
    Works,
    Warehouse,
    Plant,
}

pub(super) fn works(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, level: u8) {
    lot_ground(m, lot, mix(palette::YARD_CONCRETE, palette::EARTH, rng.range(0.0, 0.5)));
    let wall = mix(
        rng.pick(&palette::CLADDING),
        palette::SHOP_WALLS[1],
        rng.range(0.0, 0.25),
    );
    let roof = mix(palette::INDUSTRY_ROOF, wall, rng.range(0.0, 0.25));
    let big = lot.w() > 12.0;
    let form = match rng.below(10) {
        0..=3 => Form::Works,
        4..=6 => Form::Warehouse,
        _ => Form::Plant,
    };
    let stripe = rng.pick(&[
        palette::INDUSTRY_STRIPE,
        Color::srgb_u32(0x2F6FB5),
        Color::srgb_u32(0x3FA66B),
        Color::srgb_u32(0xC8322B),
    ]);
    let margin = 0.6;
    let plant_w = if form == Form::Plant {
        (lot.w() * 0.38).clamp(3.0, 7.0)
    } else {
        0.0
    };
    let body_r = Rect {
        x0: lot.x0 + margin,
        x1: lot.x1 - margin - plant_w - if big { 0.0 } else { rng.range(0.0, 1.5) },
        z0: lot.z0 + margin,
        z1: lot.z1 - if big { 5.0 } else { 2.2 },
    };
    let h = if level >= 2 {
        rng.range(6.0, 7.5)
    } else {
        rng.range(4.2, 5.4)
    } * if form == Form::Warehouse { 1.08 } else { 1.0 };
    body(m, body_r, 0.0, h, wall, wall, Some(Glow::Cladding));
    // A low concrete plinth round the shed.
    let plinth = shade(palette::YARD_CONCRETE, 0.85);
    m.cuboid(
        Vec3::new(body_r.x0 - 0.05, 0.0, body_r.z0 - 0.05),
        Vec3::new(body_r.x1 + 0.05, 0.6, body_r.z1 + 0.05),
        plinth,
    );
    let faces = Face::of(body_r, 0.0);
    // Accent stripe right round the building, a strip of high windows.
    for f in &faces {
        f.rect(m, 0.0, f.width, h - 0.9, h - 0.55, 0.02, stripe);
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
        // Corner flashings (mid).
        if !m.lod {
            for a in [0.0, f.width - 0.12] {
                f.rect(m, a, a + 0.12, 0.6, h, 0.02, shade(wall, 0.8));
            }
        }
    }
    let f = faces[0];
    match form {
        Form::Warehouse => {
            // A row of dock doors with rubber shelters and bumpers, a
            // canopy over them, the office annex at one end.
            let annex_w = (f.width * 0.28).clamp(3.0, 5.0);
            let annex_left = rng.chance(0.5);
            let (d0, d1) = if annex_left {
                (annex_w + 0.4, f.width)
            } else {
                (0.0, f.width - annex_w - 0.4)
            };
            let n = (((d1 - d0) / 3.4).floor() as usize).max(1);
            for a in spread(d1 - d0, n) {
                let a = d0 + a;
                dock_door(m, &f, a, stripe);
            }
            f.slab(
                m,
                d0 + 0.2,
                d1 - 0.2,
                3.9,
                4.05,
                1.4,
                shade(wall, 0.8),
                shade(roof, 1.1),
            );
            let ax0 = if annex_left { body_r.x0 } else { body_r.x1 - annex_w };
            annex(
                m,
                rng,
                Rect {
                    x0: ax0,
                    x1: ax0 + annex_w,
                    z0: body_r.z1 - 3.0,
                    z1: body_r.z1 + 0.6,
                },
                stripe,
            );
        }
        _ => {
            // Loading doors and the office door with the company sign.
            let doors = if big { 2 } else { 1 };
            for (i, a) in spread(f.width, doors + 1).into_iter().enumerate() {
                if i == doors {
                    f.door(m, a, 0.0, 0.9, 2.2, shade(wall, 0.7), palette::DOOR, plinth);
                    f.slab(m, a - 1.3, a + 1.3, 2.6, 3.3, 0.12, stripe, shade(stripe, 1.1));
                    m.glow(Glow::Sign, 0.0, |m| {
                        f.rect(m, a - 1.0, a + 1.0, 2.85, 3.05, 0.13, palette::WINDOW_FRAME)
                    });
                    continue;
                }
                roller_door(m, &f, a, 1.6, 3.35, stripe);
            }
        }
    }
    // Side door and a vent louvre (near).
    m.fine_only(|m| {
        let s = faces[1];
        s.rect(m, 1.0, 1.9, 0.0, 2.1, 0.03, shade(wall, 0.7));
        s.rect(
            m,
            s.width - 2.0,
            s.width - 1.0,
            h - 2.5,
            h - 1.7,
            0.03,
            shade(wall, 0.6),
        );
    });
    // Roof: sawtooth for the bigger works, shallow gable otherwise (a
    // warehouse: very shallow, with roof lights).
    let saw = form == Form::Works && (level >= 2 || (big && rng.chance(0.5)));
    if saw {
        let teeth = ((body_r.w() / 3.5).round() as usize).max(2);
        let tw = body_r.w() / teeth as f32;
        for t in 0..teeth {
            let x0 = body_r.x0 + t as f32 * tw;
            let x1 = x0 + tw;
            let rh = 1.6;
            // Vertical glazed face towards -x, slope down towards +x.
            let (a, b, c, d) = (
                Vec3::new(x0, h, body_r.z0),
                Vec3::new(x0, h, body_r.z1),
                Vec3::new(x0, h + rh, body_r.z1),
                Vec3::new(x0, h + rh, body_r.z0),
            );
            m.glow(Glow::Shop, (t as f32 * 0.37).fract(), |m| {
                m.quad(a, b, c, d, palette::GLASS)
            });
            m.surface(Glow::Cladding, |m| {
                m.quad(d, c, Vec3::new(x1, h, body_r.z1), Vec3::new(x1, h, body_r.z0), roof)
            });
            m.tri(a, d, Vec3::new(x1, h, body_r.z0), wall);
            m.tri(b, Vec3::new(x1, h, body_r.z1), c, wall);
            m.fine_only(|m| {
                // Glazing bars on the north lights.
                let n = ((body_r.d() / 1.2) as usize).max(1);
                for k in 1..n {
                    let z = body_r.z0 + body_r.d() * k as f32 / n as f32;
                    m.quad_facing(
                        [
                            Vec3::new(x0 - 0.01, h, z - 0.04),
                            Vec3::new(x0 - 0.01, h, z + 0.04),
                            Vec3::new(x0 - 0.01, h + rh, z + 0.04),
                            Vec3::new(x0 - 0.01, h + rh, z - 0.04),
                        ],
                        -Vec3::X,
                        palette::METAL_DARK,
                    );
                }
            });
        }
    } else {
        let pitch = if form == Form::Warehouse { 0.1 } else { 0.16 };
        pitched(
            m,
            body_r,
            h,
            body_r.d() * pitch,
            &Roof {
                along_x: true,
                hip: false,
                over: 0.25,
                color: roof,
                wall,
                finish: Glow::Cladding,
            },
        );
        // Roof lights along the slopes (mid).
        if !m.lod && body_r.w() > 6.0 {
            let n = ((body_r.w() - 2.0) / 3.0) as usize;
            for a in spread(body_r.w() - 2.0, n) {
                let x = body_r.x0 + 1.0 + a;
                for (z0, z1) in [
                    (body_r.cz() + 0.6, body_r.cz() + body_r.d() * 0.3),
                    (body_r.cz() - body_r.d() * 0.3, body_r.cz() - 0.6),
                ] {
                    let y0 = super::building_gen::roof_height(body_r, h, body_r.d() * pitch, true, x, z0) + 0.04;
                    let y1 = super::building_gen::roof_height(body_r, h, body_r.d() * pitch, true, x, z1) + 0.04;
                    let n = Vec3::new(0.0, 1.0, if z0 > body_r.cz() { pitch * 2.0 } else { -pitch * 2.0 }).normalize();
                    m.glow(Glow::Shop, 0.9, |m| {
                        m.quad_facing(
                            [
                                Vec3::new(x - 0.5, y0, z0),
                                Vec3::new(x + 0.5, y0, z0),
                                Vec3::new(x + 0.5, y1, z1),
                                Vec3::new(x - 0.5, y1, z1),
                            ],
                            n,
                            mix(palette::GLASS, Color::WHITE, 0.3),
                        )
                    });
                }
            }
        }
        // Roof vents (near).
        let vents = rng.below(3);
        m.fine_only(|m| {
            for k in 0..vents {
                let x = body_r.x0 + 1.5 + k as f32 * 2.5;
                if x > body_r.x1 - 1.0 {
                    break;
                }
                let y = h + body_r.d() * pitch * 0.9;
                m.cylinder(
                    Vec3::new(x, y - 0.3, body_r.cz()),
                    0.3,
                    0.9,
                    8,
                    palette::TANK,
                    shade(palette::TANK, 0.7),
                );
                m.cone(
                    Vec3::new(x, y + 0.6, body_r.cz()),
                    0.4,
                    0.3,
                    8,
                    shade(palette::TANK, 1.1),
                    0.0,
                );
            }
        });
    }
    // Yard to the side and in front.
    let yard = Rect {
        x0: body_r.x1 + 0.3,
        x1: lot.x1 - 0.3,
        z0: body_r.z1 + 0.3,
        z1: lot.z1 - 0.3,
    };
    if form == Form::Plant {
        plant(
            m,
            rng,
            Rect {
                x0: body_r.x1 + 0.4,
                x1: lot.x1 - 0.4,
                z0: lot.z0 + 0.5,
                z1: body_r.z1,
            },
            level,
            stripe,
        );
    }
    let stack = rng.chance(if level >= 2 { 0.55 } else { 0.25 }) || form == Form::Plant;
    if stack {
        let x = if rng.chance(0.5) {
            body_r.x0 + 1.0
        } else {
            body_r.x1 - 1.0
        };
        let c = Vec3::new(x, h - 0.5, body_r.z0 + 1.0);
        // Small works get a stubby flue; the tall stacks come with level 2.
        let ch = if level >= 2 {
            rng.range(5.0, 9.0) + if big { 3.0 } else { 0.0 }
        } else {
            rng.range(2.2, 3.6)
        };
        let brick = rng.chance(0.5);
        let (col, cap) = if brick {
            (palette::BRICK, shade(palette::BRICK, 0.45))
        } else {
            (palette::TANK, shade(palette::TANK, 0.4))
        };
        if brick {
            m.surface(Glow::Brick, |m| m.cylinder(c, 0.5, ch, 8, col, cap));
        } else {
            m.cylinder(c, 0.45, ch, 8, col, cap);
            // Red and white bands near the top (mid).
            if !m.lod && ch > 4.0 {
                for k in 0..2 {
                    let y = ch - 1.6 + k as f32 * 0.8;
                    m.cylinder(
                        c + Vec3::Y * y,
                        0.47,
                        0.4,
                        8,
                        Color::srgb_u32(0xC8322B),
                        Color::srgb_u32(0xC8322B),
                    );
                }
            }
        }
        if level >= 2 {
            m.emitters.push(c + Vec3::Y * ch);
        }
        m.cylinder(c + Vec3::Y * (ch - 0.8), 0.58, 0.35, 8, shade(col, 0.8), col);
        // Ladder up the stack (near).
        m.fine_only(|m| {
            m.cuboid(
                c + Vec3::new(-0.2, 0.0, 0.47),
                c + Vec3::new(0.2, ch - 0.9, 0.52),
                palette::METAL_DARK,
            );
        });
    }
    if form != Form::Plant && rng.chance(0.55) {
        // Tanks beside the shed (or in front if there is no side yard).
        let n = 1 + rng.below(2);
        for i in 0..n {
            let r = rng.range(0.8, 1.2);
            let th = rng.range(2.0, 3.2);
            let pos = if yard.w() > 2.0 * r + 0.2 {
                Vec3::new(yard.cx(), 0.0, body_r.z0 + 1.5 + i as f32 * 2.8)
            } else {
                Vec3::new(body_r.x0 + 1.4 + i as f32 * 2.6, 0.0, (body_r.z1 + lot.z1) * 0.5 - 0.2)
            };
            if pos.z + r > lot.z1 - 0.2 {
                continue;
            }
            tank(m, pos, r, th);
        }
    } else {
        // Containers or crates in the front yard.
        let n = 2 + rng.below(4);
        let containers = big && rng.chance(0.6);
        if containers && lot.z1 - body_r.z1 > 3.2 {
            for i in 0..n.min(3) {
                let c = rng.pick(&palette::CONTAINERS);
                let x = body_r.x1 - 3.4 - i as f32 * 2.6;
                if x - 1.3 < lot.x0 + 0.3 {
                    break;
                }
                lot_props::container(m, Vec3::new(x, 0.0, (body_r.z1 + lot.z1) * 0.5), c, true);
            }
        } else {
            let x = body_r.x1 - 1.0;
            for i in 0..n {
                let base = Vec3::new(x - (i % 3) as f32 * 0.95, (i / 3) as f32 * 0.85, body_r.z1 + 1.0);
                props::crate_box(m, base, 0.85, rng.range(-0.15, 0.15));
            }
        }
    }
    // Pallets, a skip, a forklift, and a parked lorry where the yard allows.
    let front = lot.z1 - body_r.z1;
    if front > 2.6 {
        m.detail_only(|m| {
            for k in 0..1 + rng.below(3) {
                let c = Vec2::new(body_r.x0 + 1.0 + k as f32 * 1.5, lot.z1 - 1.0);
                lot_props::pallets(m, rng, c);
            }
        });
        let skip = rng.chance(0.4);
        let fork = rng.chance(0.5);
        m.fine_only(|m| {
            if skip && yard.w() > 2.4 {
                lot_props::skip(
                    m,
                    Vec3::new(yard.cx(), 0.0, lot.z1 - 1.3),
                    rng.pick(&[
                        palette::INDUSTRY_STRIPE,
                        Color::srgb_u32(0x3F8A5A),
                        Color::srgb_u32(0x2F6FB5),
                    ]),
                );
            }
            if fork {
                lot_props::forklift(m, Vec3::new(body_r.x0 + 3.0, 0.0, body_r.z1 + 1.4), 0.4);
            }
        });
    }
    if lot.d() > 12.0 && front > 5.5 && rng.chance(0.6) {
        let (cab, cargo, stripe) = rng.pick(&palette::TRUCK_LIVERIES);
        let body_kind = rng.below(3);
        let lod = if m.lod {
            super::building_gen::Lod::Far
        } else if m.fine {
            super::building_gen::Lod::Near
        } else {
            super::building_gen::Lod::Mid
        };
        let t = super::vehicle_gen::truck_model(cab, cargo, stripe, body_kind, lod);
        let start = m.pos.len();
        let c = Vec3::new(body_r.cx() + rng.range(-1.0, 1.0), 0.03, body_r.z1 + front * 0.55);
        m.append(
            &t,
            c,
            std::f32::consts::FRAC_PI_2 * if rng.chance(0.5) { 1.0 } else { -1.0 },
            Vec3::ONE,
            [1.0; 4],
        );
        for uv in &mut m.uv[start..] {
            *uv = [0.0, 0.0];
        }
    }
    // Security fence along the side and back (near).
    m.fine_only(|m| {
        let post = shade(palette::GUARDRAIL, 0.8);
        let e = 0.2;
        let runs = [
            (Vec2::new(lot.x0 + e, lot.z0 + e), Vec2::new(lot.x1 - e, lot.z0 + e)),
            (Vec2::new(lot.x1 - e, lot.z0 + e), Vec2::new(lot.x1 - e, lot.z1 - 2.0)),
        ];
        for (a, b) in runs {
            let len = a.distance(b);
            let n = (len / 2.5).ceil().max(1.0) as usize;
            for i in 0..=n {
                let p = a + (b - a) * (i as f32 / n as f32);
                m.cuboid(
                    Vec3::new(p.x - 0.03, 0.0, p.y - 0.03),
                    Vec3::new(p.x + 0.03, 1.9, p.y + 0.03),
                    post,
                );
            }
            let (lo, hi) = (a.min(b) - Vec2::splat(0.015), a.max(b) + Vec2::splat(0.015));
            for y in [0.1, 1.85] {
                m.cuboid(Vec3::new(lo.x, y, lo.y), Vec3::new(hi.x, y + 0.04, hi.y), post);
            }
        }
    });
}

/// A roller shutter door centred at `a` on face `f`, half width `dw`.
fn roller_door(m: &mut MeshBuilder, f: &Face, a: f32, dw: f32, h: f32, stripe: Color) {
    f.rect(m, a - dw - 0.12, a + dw + 0.12, 0.0, h + 0.15, 0.03, stripe);
    f.rect(m, a - dw, a + dw, 0.0, h, 0.05, shade(palette::INDUSTRY_ROOF, 1.1));
    if m.lod {
        return;
    }
    let slat = shade(palette::INDUSTRY_ROOF, 0.8);
    let step = if m.fine { 0.21 } else { 0.42 };
    let mut y = 0.4;
    while y < h - 0.1 {
        f.rect(m, a - dw, a + dw, y, y + 0.05, 0.06, slat);
        y += step;
    }
    // Housing box over it and yellow-black bollards either side (near).
    f.slab(
        m,
        a - dw - 0.1,
        a + dw + 0.1,
        h,
        h + 0.4,
        0.3,
        shade(palette::INDUSTRY_ROOF, 0.9),
        palette::INDUSTRY_ROOF,
    );
    m.fine_only(|m| {
        for x in [a - dw - 0.35, a + dw + 0.35] {
            let p = f.at(x, 0.0, 0.3);
            m.cylinder(p, 0.1, 1.0, 6, Color::srgb_u32(0xE8C25A), Color::srgb_u32(0x26282B));
            m.cylinder(
                p + Vec3::Y * 0.5,
                0.105,
                0.15,
                6,
                Color::srgb_u32(0x26282B),
                Color::srgb_u32(0x26282B),
            );
        }
    });
}

/// A dock door at `a`: a raised door with a dark shelter round it, the
/// leveller plate and two bumpers.
fn dock_door(m: &mut MeshBuilder, f: &Face, a: f32, stripe: Color) {
    let dw = 1.3;
    let seal = Color::srgb_u32(0x2A2D31);
    f.rect(m, a - dw - 0.3, a + dw + 0.3, 1.0, 3.9, 0.04, seal);
    f.rect(m, a - dw, a + dw, 1.2, 3.6, 0.06, shade(palette::INDUSTRY_ROOF, 1.12));
    if m.lod {
        return;
    }
    f.rect(m, a - dw - 0.3, a + dw + 0.3, 3.6, 3.9, 0.07, stripe);
    let slat = shade(palette::INDUSTRY_ROOF, 0.82);
    let mut y = 1.5;
    while y < 3.5 {
        f.rect(m, a - dw, a + dw, y, y + 0.05, 0.07, slat);
        y += if m.fine { 0.22 } else { 0.44 };
    }
    m.fine_only(|m| {
        // Dock platform edge, leveller, bumpers.
        f.slab(
            m,
            a - dw,
            a + dw,
            0.0,
            1.0,
            0.35,
            shade(palette::YARD_CONCRETE, 0.8),
            shade(palette::YARD_CONCRETE, 0.95),
        );
        for x in [a - dw + 0.2, a + dw - 0.4] {
            f.slab(m, x, x + 0.2, 0.55, 0.95, 0.5, seal, seal);
        }
        f.slab(
            m,
            a - dw + 0.4,
            a + dw - 0.4,
            0.95,
            1.0,
            0.4,
            Color::srgb_u32(0xE8C25A),
            Color::srgb_u32(0xE8C25A),
        );
    });
}

/// Two-storey office annex of a warehouse.
fn annex(m: &mut MeshBuilder, rng: &mut Stream, r: Rect, stripe: Color) {
    let wall = mix(palette::OFFICE_WALLS[rng.below(6) as usize], palette::WINDOW_FRAME, 0.3);
    let top = 6.4;
    body(m, r, 0.0, top, wall, wall, None);
    let win = Win {
        frame: palette::METAL_DARK,
        glass: palette::GLASS,
        sill: None,
        bars: 1,
        shutters: None,
    };
    let faces = Face::of(r, 0.0);
    for (fi, f) in faces.iter().enumerate() {
        if fi == 2 {
            continue;
        }
        let n = ((f.width / 1.6) as usize).max(1);
        for a in spread(f.width, n) {
            for y in [0.9, 4.0] {
                if fi == 0 && y < 1.0 && (a - f.width * 0.5).abs() < 0.8 {
                    continue;
                }
                f.window_s(m, a, y, 1.0, 1.4, &win);
            }
        }
    }
    let f = faces[0];
    f.door(
        m,
        f.width * 0.5,
        0.0,
        1.1,
        2.3,
        palette::METAL_DARK,
        shade(palette::GLASS, 0.7),
        palette::YARD_CONCRETE,
    );
    f.slab(
        m,
        f.width * 0.5 - 1.0,
        f.width * 0.5 + 1.0,
        2.6,
        2.75,
        1.0,
        stripe,
        stripe,
    );
    flat_roof(m, r, top, wall, palette::ROOF_FLAT, 0.4);
    if !m.lod {
        f.rect(m, 0.0, f.width, top - 0.6, top - 0.2, 0.03, stripe);
    }
}

/// A vertical tank with a domed top and (near) a ladder.
fn tank(m: &mut MeshBuilder, pos: Vec3, r: f32, h: f32) {
    m.cylinder(pos, r, h, 10, palette::TANK, shade(palette::TANK, 1.1));
    m.cone(pos + Vec3::Y * h, r, r * 0.35, 10, shade(palette::TANK, 1.06), 0.0);
    if !m.lod {
        // Weld bands.
        m.cylinder(
            pos + Vec3::Y * (h * 0.5),
            r + 0.02,
            0.06,
            10,
            shade(palette::TANK, 0.85),
            shade(palette::TANK, 0.85),
        );
    }
    m.fine_only(|m| {
        m.cuboid(
            pos + Vec3::new(-0.18, 0.0, r - 0.02),
            pos + Vec3::new(0.18, h + 0.2, r + 0.05),
            palette::METAL_DARK,
        );
    });
}

/// The process part of a plant in `r`: silos with conical tops or tanks,
/// a pipe rack joining them to the shed.
fn plant(m: &mut MeshBuilder, rng: &mut Stream, r: Rect, level: u8, stripe: Color) {
    if r.w() < 2.4 || r.d() < 3.0 {
        return;
    }
    let silos = rng.chance(0.5);
    let rad = (r.w() * 0.5 - 0.2).min(1.5);
    let n = ((r.d() / (rad * 2.0 + 0.4)) as usize).clamp(1, 3);
    let hgt = if level >= 2 {
        rng.range(7.0, 10.0)
    } else {
        rng.range(4.5, 6.5)
    };
    let cx = r.cx();
    let mut tops = Vec::new();
    for i in 0..n {
        let cz = r.z0 + (rad + 0.2) + i as f32 * (rad * 2.0 + 0.4);
        let base = Vec3::new(cx, 0.0, cz);
        if silos {
            // Silo on legs with a conical hopper and a cone roof.
            let legs = 1.8;
            m.cylinder(
                base + Vec3::Y * legs,
                rad,
                hgt - legs,
                12,
                palette::TANK,
                shade(palette::TANK, 1.1),
            );
            m.cone(
                base + Vec3::Y * (hgt),
                rad,
                rad * 0.6,
                12,
                shade(palette::TANK, 1.08),
                0.0,
            );
            if !m.lod {
                // Hopper (an inverted cone drawn as a short taper).
                m.cylinder(
                    base + Vec3::Y * (legs - 0.6),
                    rad * 0.4,
                    0.6,
                    8,
                    shade(palette::TANK, 0.85),
                    palette::TANK,
                );
                for (dx, dz) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    let p = base + Vec3::new(dx * rad * 0.7, 0.0, dz * rad * 0.7);
                    m.cuboid(
                        p - Vec3::new(0.08, 0.0, 0.08),
                        p + Vec3::new(0.08, legs + 0.2, 0.08),
                        palette::METAL_DARK,
                    );
                }
                m.cylinder(base + Vec3::Y * (hgt * 0.75), rad + 0.02, 0.25, 12, stripe, stripe);
            }
        } else {
            tank(m, base, rad, hgt * 0.6);
        }
        tops.push(base + Vec3::Y * (if silos { hgt } else { hgt * 0.6 }));
    }
    // Pipe rack from the tanks to the shed wall (mid).
    if !m.lod {
        let y = 3.2;
        let z0 = r.z0 + 0.4;
        let z1 = r.z1 - 0.4;
        let px = r.x0 + 0.15;
        for z in [z0, z1] {
            m.cuboid(
                Vec3::new(px - 0.08, 0.0, z - 0.08),
                Vec3::new(px + 0.08, y, z + 0.08),
                palette::METAL_DARK,
            );
        }
        for (k, col) in [palette::TANK, palette::INDUSTRY_STRIPE, Color::srgb_u32(0x3F8A5A)]
            .into_iter()
            .enumerate()
        {
            let yy = y + 0.1 + k as f32 * 0.22;
            m.cuboid(Vec3::new(px - 0.09, yy, z0), Vec3::new(px + 0.09, yy + 0.16, z1), col);
        }
        // A walkway across the silo tops (near).
        m.fine_only(|m| {
            if tops.len() > 1 {
                let t = tops[0].y + 0.3;
                m.cuboid(
                    Vec3::new(cx - 0.4, t, tops[0].z),
                    Vec3::new(cx + 0.4, t + 0.08, tops[tops.len() - 1].z),
                    palette::METAL_DARK,
                );
                for z in [tops[0].z, tops[tops.len() - 1].z] {
                    m.cuboid(
                        Vec3::new(cx + 0.36, t, z - 0.02),
                        Vec3::new(cx + 0.4, t + 1.0, z + 0.02),
                        palette::INDUSTRY_STRIPE,
                    );
                }
            }
        });
    }
    let _ = roof_details;
}
