//! Commercial buildings: shops (levels 1-2), offices (level 3+) and the
//! big-box store with its car park (big level-1 lots). See `building_gen`
//! for the frame and the levels of detail.

use bevy::prelude::*;

use super::building_gen::{
    Face, Rect, Win, body, cornice, downpipe, flat_roof, lot_ground, roof_details, spread, wrand,
};
use super::lot_props;
use super::mesh_util::{Glow, MeshBuilder};
use super::noise::Stream;
use super::palette::{self, lin, mix, shade};
use super::props;

/// "Lettering" on a sign: a row of light blocks of varied width inside
/// `a0..a1` at `b0..b1`, glowing at night.
#[allow(clippy::too_many_arguments)]
fn lettering(m: &mut MeshBuilder, f: &Face, a0: f32, a1: f32, b0: f32, b1: f32, off: f32, ink: Color) {
    if !m.fine {
        m.glow(Glow::Sign, 0.0, |m| {
            f.rect(m, a0, a1, b0 + (b1 - b0) * 0.3, b1 - (b1 - b0) * 0.3, off, ink)
        });
        return;
    }
    let h = b1 - b0;
    let mut a = a0;
    let mut k = 0u32;
    while a < a1 - h * 0.3 {
        let r = wrand(f.at(a, b0 + k as f32, 0.0));
        let w = h * (0.35 + 0.45 * r);
        let end = (a + w).min(a1);
        // A word gap now and then.
        if r > 0.86 && a > a0 {
            a += h * 0.5;
            continue;
        }
        let top = if r < 0.2 { b1 } else { b1 - h * 0.15 };
        m.glow(Glow::Sign, 0.0, |m| f.rect(m, a, end, b0, top, off, ink));
        a = end + h * 0.14;
        k += 1;
    }
}

/// A striped shop awning on face `f` from `a0` to `a1`, top at `y`,
/// sloping out `out` metres; scalloped valance near.
#[allow(clippy::too_many_arguments)]
fn awning(m: &mut MeshBuilder, f: &Face, a0: f32, a1: f32, y: f32, out: f32, color: Color, stripes: bool) {
    let drop = 0.45;
    let light = mix(color, palette::WINDOW_FRAME, 0.85);
    let n = if stripes && !m.lod {
        ((a1 - a0) / 0.45).round().max(1.0) as usize
    } else {
        1
    };
    let w = (a1 - a0) / n as f32;
    for i in 0..n {
        let (s0, s1) = (a0 + w * i as f32, a0 + w * (i + 1) as f32);
        let c = if stripes && i % 2 == 1 { light } else { color };
        let p = [
            f.at(s0, y, 0.05),
            f.at(s1, y, 0.05),
            f.at(s1, y - drop, out),
            f.at(s0, y - drop, out),
        ];
        m.quad(p[3], p[2], p[1], p[0], c);
        m.quad(p[0], p[1], p[2], p[3], shade(c, 0.6));
        // Valance.
        let v = shade(c, 0.88);
        let vh = 0.25;
        m.quad(p[3], p[2], p[2] - Vec3::Y * vh, p[3] - Vec3::Y * vh, v);
        m.quad(p[2], p[3], p[3] - Vec3::Y * vh, p[2] - Vec3::Y * vh, v);
        if m.fine {
            // Scallop: a little triangle under each stripe.
            let mid = (p[2] + p[3]) * 0.5 - Vec3::Y * (vh + 0.1);
            m.tri_facing(p[3] - Vec3::Y * vh, p[2] - Vec3::Y * vh, mid, f.n, v);
        }
    }
    if !m.lod {
        // Side cheeks.
        for s in [a0, a1] {
            let q = [f.at(s, y, 0.05), f.at(s, y - drop, out), f.at(s, y - drop - 0.25, out)];
            m.tri_facing(
                q[0],
                q[1],
                q[2],
                f.u * if s == a0 { -1.0 } else { 1.0 },
                shade(color, 0.8),
            );
        }
    }
}

/// What a shop sells, for the display out front (near).
#[derive(Clone, Copy, PartialEq)]
enum Trade {
    Cafe,
    Grocer,
    Florist,
    Fashion,
    Bakery,
    Hardware,
}

pub(super) fn shop(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, palette::PAVING);
    let brick = rng.chance(0.3);
    let wall = if brick {
        rng.pick(&palette::BRICKS)
    } else {
        let w = rng.pick(&palette::SHOP_WALLS);
        mix(w, rng.pick(&palette::HOUSE_WALLS), rng.range(0.0, 0.5))
    };
    let finish = brick.then_some(Glow::Brick);
    let accent = rng.pick(&palette::ACCENTS);
    let trade = [
        Trade::Cafe,
        Trade::Grocer,
        Trade::Florist,
        Trade::Fashion,
        Trade::Bakery,
        Trade::Hardware,
    ][rng.below(6) as usize];
    let margin = 0.5;
    let cafe = lot.d() >= 8.0 && (trade == Trade::Cafe || rng.chance(0.25));
    let front = if cafe { 2.6 } else { 0.9 };
    let body_r = Rect {
        x0: lot.x0 + margin,
        x1: lot.x1 - margin,
        z0: lot.z0 + margin + rng.range(0.0, 1.0).min(lot.d() - front - 6.0).max(0.0),
        z1: lot.z1 - front,
    };
    if cafe {
        lot_props::cafe_seating(m, rng, body_r.x0, body_r.x1, lot.z1 - 1.25, accent);
    }
    let ground_h = 3.6;
    let sh = 3.1;
    let top = ground_h + (storeys - 1) as f32 * sh;
    body(m, body_r, 0.0, top, wall, wall, finish);
    let faces = Face::of(body_r, 0.0);
    let f = faces[0];
    // Shop front: a timber surround (pilasters and a fascia) in the accent
    // or a dark colour, stall riser, big glass with mullions, the door.
    let surround = if rng.chance(0.5) {
        shade(accent, 0.75)
    } else {
        mix(palette::METAL_DARK, accent, 0.25)
    };
    let fascia_y = 3.05;
    f.rect(m, 0.15, f.width - 0.15, 0.0, fascia_y + 0.5, 0.025, surround);
    f.rect(m, 0.35, f.width - 0.35, 0.0, 0.45, 0.035, shade(surround, 0.8));
    f.lit_rect(m, Glow::Shop, 0.4, f.width - 0.4, 0.45, 2.9, 0.04, palette::GLASS);
    let door = f.width * rng.range(0.25, 0.75);
    f.rect(m, door - 0.6, door + 0.6, 0.0, 2.5, 0.06, shade(palette::GLASS, 0.75));
    for a in spread(f.width - 0.8, 3) {
        f.rect(m, 0.4 + a - 0.04, 0.4 + a + 0.04, 0.45, 2.9, 0.06, surround);
    }
    m.fine_only(|m| {
        // Transom bar, the door's frame and handle, a display shelf.
        f.rect(m, 0.4, f.width - 0.4, 2.35, 2.42, 0.06, surround);
        f.rect(m, door - 0.62, door + 0.62, 2.5, 2.56, 0.07, surround);
        for x in [door - 0.62, door + 0.56] {
            f.rect(m, x, x + 0.06, 0.0, 2.5, 0.07, surround);
        }
        f.rect(m, door + 0.35, door + 0.4, 0.9, 1.3, 0.08, palette::GUARDRAIL);
        // Goods in the window: a row of coloured blocks behind the glass.
        let mut a = 0.6;
        while a < f.width - 0.6 {
            if (a - door).abs() > 0.8 {
                let c = rng_color(wrand(f.at(a, 0.5, 0.0)));
                f.rect(
                    m,
                    a,
                    a + 0.32,
                    0.5,
                    0.5 + 0.25 + 0.5 * wrand(f.at(a, 1.0, 0.0)),
                    0.041,
                    c,
                );
            }
            a += 0.5;
        }
        // Pilaster capitals.
        for a in [0.15, f.width - 0.45] {
            f.slab(m, a, a + 0.3, fascia_y - 0.1, fascia_y + 0.05, 0.12, surround, surround);
        }
    });
    // Awning: striped on some.
    let stripes = rng.chance(0.55);
    if !(cafe && rng.chance(0.4)) {
        awning(m, &f, 0.3, f.width - 0.3, 3.0, 1.3, accent, stripes);
    }
    // Fascia sign with lettering (glows at night).
    let sign_y = if storeys == 1 { top - 0.75 } else { fascia_y };
    let board = if rng.chance(0.5) { accent } else { surround };
    f.slab(
        m,
        0.35,
        f.width - 0.35,
        sign_y,
        sign_y + 0.5,
        0.1,
        board,
        shade(board, 1.1),
    );
    lettering(
        m,
        &f,
        f.width * 0.18,
        f.width * 0.82,
        sign_y + 0.12,
        sign_y + 0.38,
        0.11,
        palette::WINDOW_FRAME,
    );
    // A hanging sign on a bracket (near).
    let hang = rng.chance(0.5);
    m.fine_only(|m| {
        if hang {
            let a = if door > f.width * 0.5 { 0.5 } else { f.width - 0.5 };
            f.slab(
                m,
                a - 0.03,
                a + 0.03,
                3.65,
                3.7,
                0.85,
                palette::METAL_DARK,
                palette::METAL_DARK,
            );
            let p = f.at(a, 3.05, 0.65);
            m.cuboid2(
                p - Vec3::new(0.03, 0.0, 0.0) + f.u * 0.0 - f.n * 0.0,
                p + Vec3::new(0.03, 0.55, 0.0) + f.n * 0.0,
                accent,
                accent,
            );
            m.box_yaw(
                p,
                Vec3::new(0.06, 0.55, 0.5),
                if f.n.z.abs() > 0.5 {
                    std::f32::consts::FRAC_PI_2
                } else {
                    0.0
                },
                accent,
                accent,
            );
        }
    });
    // Upper floors.
    let win = Win {
        frame: palette::WINDOW_FRAME,
        glass: palette::GLASS,
        sill: Some(if brick { palette::STONE } else { shade(wall, 0.88) }),
        bars: [0, 2, 3][rng.below(3) as usize],
        shutters: None,
    };
    for (fi, f) in faces.iter().enumerate() {
        let per = ((f.width / 2.2).floor() as usize).clamp(1, 6);
        for s in 1..storeys {
            let base = ground_h + (s - 1) as f32 * sh + 0.8;
            for a in spread(f.width, per) {
                f.window_s(m, a, base, 1.0, 1.45, &win);
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
        // Back door for deliveries.
        if fi == 2 && !m.lod {
            f.rect(m, 0.8, 1.8, 0.0, 2.2, 0.03, shade(palette::INDUSTRY_ROOF, 1.1));
        }
    }
    if storeys > 1 {
        cornice(
            m,
            body_r,
            top - 0.3,
            0.25,
            0.15,
            if brick { palette::STONE } else { shade(wall, 0.92) },
        );
    }
    flat_roof(m, body_r, top, wall, palette::ROOF_FLAT, 0.45);
    roof_details(m, rng, body_r.inset(0.3), top);
    downpipe(m, body_r.x0 + 0.12, body_r.z1 + 0.06, top);
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
    } else if !cafe {
        // The trade's display out front (near).
        let seed = rng.f();
        m.fine_only(|m| display(m, &f, trade, door, seed));
    }
}

fn rng_color(r: f32) -> Color {
    let c = [
        Color::srgb_u32(0xE0564A),
        Color::srgb_u32(0xF2B33D),
        Color::srgb_u32(0x3FA66B),
        Color::srgb_u32(0x2F8FCB),
        Color::srgb_u32(0xF7F1E5),
        Color::srgb_u32(0xB07CD8),
    ];
    c[((r * c.len() as f32) as usize).min(c.len() - 1)]
}

/// Goods set out in front of a shop (near only).
fn display(m: &mut MeshBuilder, f: &Face, trade: Trade, door: f32, seed: f32) {
    let (a0, a1) = if door > f.width * 0.5 {
        (0.6, door - 1.0)
    } else {
        (door + 1.0, f.width - 0.6)
    };
    if a1 - a0 < 1.0 {
        return;
    }
    match trade {
        Trade::Grocer => {
            // Crates of fruit on a stand.
            let stand = Color::srgb_u32(0x8A6A4A);
            f.slab(m, a0, a1, 0.0, 0.7, 0.7, stand, shade(stand, 1.1));
            let mut a = a0 + 0.05;
            let mut k = 0;
            while a < a1 - 0.4 {
                let c = [
                    Color::srgb_u32(0xE0564A),
                    Color::srgb_u32(0xF2B33D),
                    Color::srgb_u32(0x6FAE3E),
                    Color::srgb_u32(0xE88A3A),
                ][k % 4];
                f.slab(m, a, a + 0.4, 0.7, 0.85, 0.65, Color::srgb_u32(0xB89263), c);
                a += 0.45;
                k += 1;
            }
        }
        Trade::Florist => {
            let mut a = a0 + 0.2;
            let mut k = 0;
            while a < a1 - 0.2 {
                let p = f.at(a, 0.0, 0.45);
                m.cylinder(p, 0.15, 0.35, 6, Color::srgb_u32(0x5C6670), Color::srgb_u32(0x3B4046));
                let c = [
                    Color::srgb_u32(0xE86A8A),
                    Color::srgb_u32(0xF2C94C),
                    Color::srgb_u32(0xB07CD8),
                    Color::srgb_u32(0xF7F1E5),
                ][(k + (seed * 4.0) as usize) % 4];
                props::blob(m, p + Vec3::Y * 0.5, Vec3::new(0.2, 0.18, 0.2), c, a);
                a += 0.4;
                k += 1;
            }
        }
        Trade::Fashion | Trade::Bakery => {
            // A sandwich board.
            let p = f.at((a0 + a1) * 0.5, 0.0, 0.7);
            let c = if trade == Trade::Bakery {
                Color::srgb_u32(0x2A2D31)
            } else {
                palette::WINDOW_FRAME
            };
            m.box_yaw(
                p,
                Vec3::new(0.55, 0.9, 0.12),
                if f.n.z.abs() > 0.5 {
                    0.0
                } else {
                    std::f32::consts::FRAC_PI_2
                },
                c,
                c,
            );
        }
        Trade::Hardware => {
            // Stacked goods: buckets and a ladder against the wall.
            let mut a = a0 + 0.2;
            while a < a1 - 0.2 {
                let p = f.at(a, 0.0, 0.35);
                m.cylinder(p, 0.16, 0.3, 6, Color::srgb_u32(0x2F8FCB), Color::srgb_u32(0x26282B));
                a += 0.45;
            }
            f.slab(
                m,
                a1 - 0.5,
                a1 - 0.45,
                0.0,
                2.2,
                0.15,
                Color::srgb_u32(0xC9CCCF),
                Color::srgb_u32(0xC9CCCF),
            );
            f.slab(
                m,
                a1 - 0.1,
                a1 - 0.05,
                0.0,
                2.2,
                0.15,
                Color::srgb_u32(0xC9CCCF),
                Color::srgb_u32(0xC9CCCF),
            );
        }
        Trade::Cafe => {}
    }
}

/// Office block: three facade systems by seed (ribbon windows over
/// spandrel bands; punched windows in stone between pilasters; a curtain
/// wall with mullions and spandrels), a two-storey podium with the upper
/// floors set back on taller blocks, an entrance colonnade with a canopy,
/// a crown or plant screen on the roof.
pub(super) fn office(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, palette::PAVING);
    let style = rng.below(3);
    let wall = rng.pick(&palette::OFFICE_WALLS);
    let accent = rng.pick(&palette::ACCENTS);
    let margin = if lot.w() > 12.0 { 1.0 } else { 0.35 };
    let body_r = Rect {
        x0: lot.x0 + margin,
        x1: lot.x1 - margin,
        z0: lot.z0 + margin,
        z1: lot.z1 - margin - 0.4,
    };
    let sh = 3.3;
    let base_h = 4.0;
    // Four floors and up: a two-storey podium over the whole lot with the
    // upper floors set back on top of it.
    let setback = storeys >= 4;
    let low_storeys = if setback { 2 } else { storeys };
    let glass = mix(palette::GLASS, palette::WINDOW, rng.range(0.0, 0.6));
    let mullion = if rng.chance(0.5) {
        palette::METAL_DARK
    } else {
        palette::ROOF_DETAIL
    };
    let brick = style == 1 && rng.chance(0.4);
    let section = |m: &mut MeshBuilder, r: Rect, s0: u32, s1: u32| -> f32 {
        let y0 = if s0 == 0 { 0.0 } else { base_h + (s0 - 1) as f32 * sh };
        let top = base_h + (s1 - 1) as f32 * sh;
        let body_col = if style == 2 { shade(glass, 0.8) } else { wall };
        let finish = if brick { Some(Glow::Brick) } else { None };
        body(m, r, y0, top, body_col, body_col, finish);
        let faces = Face::of(r, 0.0);
        for f in &faces {
            if s0 == 0 {
                // Lobby glass on the ground floor, with mullions.
                f.lit_rect(m, Glow::Shop, 0.5, f.width - 0.5, 0.3, 3.3, 0.04, shade(glass, 0.85));
                if !m.lod {
                    let n = ((f.width - 1.0) / 1.6).floor().max(1.0) as usize;
                    for k in 0..=n {
                        let a = 0.5 + (f.width - 1.0) * k as f32 / n as f32;
                        f.rect(m, a - 0.05, a + 0.05, 0.3, 3.3, 0.05, mullion);
                    }
                    f.rect(m, 0.5, f.width - 0.5, 3.25, 3.4, 0.05, mullion);
                }
            }
            for s in s0.max(1)..s1 {
                let y = base_h + (s - 1) as f32 * sh;
                match style {
                    0 => {
                        // Ribbon windows over spandrel bands.
                        f.lit_rect(m, Glow::Window, 0.3, f.width - 0.3, y + 0.75, y + 2.7, 0.04, glass);
                        if !m.lod {
                            let n = ((f.width - 0.6) / 1.5).floor().max(1.0) as usize;
                            for k in 1..n {
                                let a = 0.3 + (f.width - 0.6) * k as f32 / n as f32;
                                f.rect(m, a - 0.04, a + 0.04, y + 0.75, y + 2.7, 0.05, mullion);
                            }
                            f.ledge(m, 0.2, f.width - 0.2, y + 0.62, y + 0.75, 0.08, shade(wall, 0.92));
                        }
                    }
                    1 => {
                        // Punched windows in a stone or brick facade.
                        let n = ((f.width / 1.7).floor() as usize).max(1);
                        let w = Win {
                            frame: shade(wall, 0.82),
                            glass,
                            sill: Some(palette::STONE),
                            bars: 4,
                            shutters: None,
                        };
                        for a in spread(f.width, n) {
                            f.window_s(m, a, y + 0.8, 0.95, 1.8, &w);
                        }
                    }
                    _ => {
                        // Curtain wall: opaque spandrel panel at each floor.
                        f.rect(m, 0.0, f.width, y - 0.1, y + 0.55, 0.04, shade(mullion, 1.15));
                        // Lit floors (the glass behind reads as windows).
                        f.lit_rect(
                            m,
                            Glow::Window,
                            0.05,
                            f.width - 0.05,
                            y + 0.6,
                            y + sh - 0.15,
                            0.02,
                            glass,
                        );
                    }
                }
            }
            if style == 1 && !m.lod {
                // Pilasters between the bays.
                let n = ((f.width / 1.7).floor() as usize).max(1);
                for k in 0..=n {
                    let a = f.width * k as f32 / n as f32;
                    let y_a = y0.max(base_h - 0.3);
                    f.rect(m, a - 0.12, a + 0.12, y_a - y0 + y0, top - 0.2, 0.06, shade(wall, 1.06));
                }
            }
            if style == 2 {
                let n = ((f.width / 1.5).floor() as usize).max(1);
                for k in 1..n {
                    let a = f.width * k as f32 / n as f32;
                    f.rect(m, a - 0.05, a + 0.05, y0.max(3.4) - y0 + y0, top, 0.05, mullion);
                }
                // Fins (near): shallow vertical blades.
                m.fine_only(|m| {
                    for k in 1..n {
                        let a = f.width * k as f32 / n as f32;
                        f.slab(m, a - 0.03, a + 0.03, y0.max(3.4), top, 0.25, mullion, mullion);
                    }
                });
            }
        }
        top
    };
    let mut top = section(m, body_r, 0, low_storeys);
    let roof_wall = if style == 2 { palette::ROOF_DETAIL } else { wall };
    let upper = if setback { body_r.inset(0.9) } else { body_r };
    if setback {
        flat_roof(m, body_r, top, roof_wall, palette::ROOF_FLAT, 0.4);
        // Planters on the podium roof (near).
        let t = top;
        m.fine_only(|m| {
            for x in [body_r.x0 + 0.5, body_r.x1 - 0.5] {
                m.cuboid2(
                    Vec3::new(x - 0.25, t, body_r.z1 - 0.65),
                    Vec3::new(x + 0.25, t + 0.5, body_r.z1 - 0.3),
                    palette::STONE,
                    shade(palette::STONE, 0.9),
                );
                props::bush(m, Vec3::new(x, t + 0.35, body_r.z1 - 0.47), 0.5, palette::HEDGE, x);
            }
        });
        top = section(m, upper, low_storeys, storeys);
        flat_roof(m, upper, top, roof_wall, palette::ROOF_FLAT, 0.6);
    } else {
        flat_roof(m, body_r, top, roof_wall, palette::ROOF_FLAT, 0.6);
    }
    if style == 1 {
        cornice(m, upper, top - 0.4, 0.3, 0.2, palette::STONE);
    }
    // Entrance: a canopy in the accent colour on columns, revolving door.
    let f = Face::of(body_r, 0.0)[0];
    f.slab(m, f.width * 0.3, f.width * 0.7, 3.5, 3.7, 1.6, accent, accent);
    m.fine_only(|m| {
        for a in [f.width * 0.32, f.width * 0.68] {
            let p = f.at(a, 0.0, 1.4);
            m.cylinder(p, 0.09, 3.5, 8, palette::METAL_DARK, palette::METAL_DARK);
        }
        let c = f.at(f.width * 0.5, 0.0, 0.5);
        m.cylinder(c, 0.85, 2.4, 10, shade(glass, 0.9), mullion);
        // Company name on the canopy edge.
        lettering(
            m,
            &f,
            f.width * 0.36,
            f.width * 0.64,
            3.53,
            3.67,
            1.61,
            palette::WINDOW_FRAME,
        );
    });
    if rng.chance(0.5) {
        let crown = Face::of(upper, 0.0);
        for f in &crown {
            f.rect(m, 0.0, f.width, top - 0.7, top - 0.1, 0.06, accent);
        }
    }
    // Plant room with a louvred screen.
    let plant = upper.inset(upper.w().min(upper.d()) * 0.3);
    m.cuboid2(
        Vec3::new(plant.x0, top, plant.z0),
        Vec3::new(plant.x1, top + 1.8, plant.z1),
        shade(wall, 0.85),
        palette::ROOF_DETAIL,
    );
    if !m.lod {
        for pf in Face::of(plant, top) {
            let n = ((pf.width / 0.35) as usize).max(1);
            if m.fine {
                for k in 0..n {
                    let a = (k as f32 + 0.5) * pf.width / n as f32;
                    pf.rect(m, a - 0.04, a + 0.04, 0.2, 1.6, 0.02, shade(wall, 0.7));
                }
            } else {
                pf.rect(m, 0.15, pf.width - 0.15, 0.2, 1.6, 0.02, shade(wall, 0.75));
            }
        }
    }
    let mut rr = upper.inset(0.4);
    rr.z1 = plant.z0 - 0.2;
    if rr.d() > 1.5 {
        roof_details(m, rng, rr, top);
    } else {
        roof_details(m, rng, upper.inset(0.4), top + 1.8);
    }
    if rng.chance(0.4) {
        let c = Vec3::new(plant.x1 - 0.4, top + 1.8, plant.z0 + 0.4);
        m.detail_only(|m| m.cylinder(c, 0.06, 3.5, 4, palette::ROOF_DETAIL, palette::ROOF_DETAIL));
        m.glow(Glow::Always, 0.0, |m| {
            if !m.lod {
                m.cuboid(
                    c + Vec3::new(-0.08, 3.5, -0.08),
                    c + Vec3::new(0.08, 3.65, 0.08),
                    Color::srgb(0.9, 0.1, 0.08),
                )
            }
        });
    }
    // Trees in planters by the entrance (mid).
    if lot.w() > 12.0 && !m.lod {
        for a in [f.width * 0.12, f.width * 0.88] {
            let p = f.at(a, 0.0, 0.9);
            m.cuboid2(
                p - Vec3::new(0.5, 0.0, 0.5),
                p + Vec3::new(0.5, 0.45, 0.5),
                palette::STONE,
                shade(palette::EARTH, 0.6),
            );
            props::garden_tree(m, p + Vec3::Y * 0.4, 0.5, palette::CANOPIES[1], a, 0);
        }
    }
}

/// Big-box store with a car park (commercial, large lot, low level).
pub(super) fn supermarket(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect) {
    lot_ground(m, lot, palette::ASPHALT);
    let wall = rng.pick(&palette::SHOP_WALLS);
    let accent = rng.pick(&palette::ACCENTS);
    let body_r = Rect {
        x0: lot.x0 + 0.8,
        x1: lot.x1 - 0.8,
        z0: lot.z0 + 0.8,
        z1: lot.z0 + lot.d() * 0.55,
    };
    let top = 5.0;
    body(m, body_r, 0.0, top, wall, wall, Some(Glow::Cladding));
    let f = Face::of(body_r, 0.0)[0];
    // A band of the brand colour along the top, the glazed entrance under
    // a deep canopy on posts, a tall sign over it.
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
    if !m.lod {
        let n = 6;
        for k in 0..=n {
            let a = f.width * (0.3 + 0.4 * k as f32 / n as f32);
            f.rect(m, a - 0.05, a + 0.05, 0.0, 3.0, 0.05, palette::METAL_DARK);
        }
        for a in [f.width * 0.29, f.width * 0.71] {
            f.slab(
                m,
                a - 0.08,
                a + 0.08,
                0.0,
                3.0,
                1.9,
                palette::METAL_DARK,
                palette::METAL_DARK,
            );
        }
    }
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
    lettering(
        m,
        &f,
        f.width * 0.38,
        f.width * 0.62,
        top + 0.3,
        top + 0.9,
        0.21,
        palette::WINDOW_FRAME,
    );
    flat_roof(m, body_r, top, wall, palette::ROOF_FLAT, 0.4);
    roof_details(m, rng, body_r.inset(1.0), top);
    // Car park bays and a few parked cars, lamp posts, a trolley shelter.
    let white = lin(palette::MARKING);
    let bay_z0 = body_r.z1 + 2.2;
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
            let c = Vec3::new(x + 1.3, 0.03, (bay_z0 + bay_z1) * 0.5);
            let yaw = if rng.chance(0.5) { 1.0 } else { -1.0 } * std::f32::consts::FRAC_PI_2;
            m.detail_only(|m| lot_props::parked_car(m, rng, c, yaw));
        }
    }
    m.fine_only(|m| {
        // Walkway paint in front of the doors.
        flat_patch_lin(
            m,
            f.width * 0.3 + body_r.x0,
            body_r.z1 + 0.2,
            f.width * 0.7 + body_r.x0,
            body_r.z1 + 2.0,
            lin(mix(palette::ASPHALT, palette::MARKING, 0.25)),
        );
        // Trolley shelter.
        let x = body_r.x0 + f.width * 0.15;
        let z = body_r.z1 + 1.2;
        m.cuboid2(
            Vec3::new(x - 1.2, 2.0, z - 0.6),
            Vec3::new(x + 1.2, 2.1, z + 0.6),
            accent,
            accent,
        );
        for dx in [-1.1, 1.1] {
            m.cuboid(
                Vec3::new(x + dx - 0.04, 0.0, z - 0.04),
                Vec3::new(x + dx + 0.04, 2.0, z + 0.04),
                palette::METAL_DARK,
            );
        }
        for k in 0..4 {
            let p = Vec3::new(x - 0.8 + k as f32 * 0.5, 0.0, z);
            m.cuboid(
                p - Vec3::new(0.2, 0.0, 0.35),
                p + Vec3::new(0.2, 0.9, 0.35),
                palette::GUARDRAIL,
            );
        }
    });
    // Lamp posts between the rows.
    if !m.lod {
        let z = (bay_z0 + bay_z1) * 0.5;
        for x in [lot.x0 + lot.w() * 0.25, lot.x0 + lot.w() * 0.75] {
            m.cuboid(
                Vec3::new(x - 0.07, 0.0, bay_z1 + 0.3),
                Vec3::new(x + 0.07, 6.0, bay_z1 + 0.44),
                palette::LAMP,
            );
            m.glow(Glow::Lamp, 0.0, |m| {
                m.cuboid(
                    Vec3::new(x - 0.3, 5.85, bay_z1 + 0.22),
                    Vec3::new(x + 0.3, 6.0, bay_z1 + 0.52),
                    palette::LAMP_GLASS,
                )
            });
            let _ = z;
        }
    }
    downpipe(m, body_r.x0 + 0.15, body_r.z1 + 0.06, top);
}

fn flat_patch_lin(m: &mut MeshBuilder, x0: f32, z0: f32, x1: f32, z1: f32, c: [f32; 4]) {
    m.quad_up(
        [
            Vec3::new(x0, 0.055, z0),
            Vec3::new(x1, 0.055, z0),
            Vec3::new(x1, 0.055, z1),
            Vec3::new(x0, 0.055, z1),
        ],
        c,
    );
}
