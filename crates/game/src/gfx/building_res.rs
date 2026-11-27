//! Homes: detached houses (level 1), terraces (big level-1 lots and level
//! 2) and blocks of flats (level 3+). See `building_gen` for the frame and
//! the levels of detail.

use bevy::prelude::*;

use super::building_gen::{
    Face, Rect, Roof, Win, body, chimney_c, cornice, dormer, downpipe, fence, flat_patch, flat_roof, garden_wall,
    hedge, lot_ground, pitched, roof_details, roof_height, spread,
};
use super::lot_props;
use super::mesh_util::{Glow, MeshBuilder};
use super::noise::Stream;
use super::palette::{self, lin, mix, shade};
use super::props;

/// Storey height of a house, metres.
const SH: f32 = 2.7;
/// Ground floor level (top of the plinth).
const PLINTH: f32 = 0.35;

/// The look of one house (or one unit of a terrace).
#[derive(Debug, Clone, Copy)]
pub(super) struct HouseStyle {
    pub wall: Color,
    pub roof: Color,
    pub trim: Color,
    pub door: Color,
    /// Surface pattern of the walls (render has none).
    pub finish: Option<Glow>,
    pub shutters: Option<Color>,
    /// Glazing bars of the windows.
    pub bars: u8,
    /// Stone sills (else painted, in the trim colour).
    pub stone: bool,
}

impl HouseStyle {
    pub fn win(&self) -> Win {
        Win {
            frame: self.trim,
            glass: palette::WINDOW,
            sill: Some(if self.stone {
                palette::STONE
            } else {
                shade(self.trim, 0.92)
            }),
            bars: self.bars,
            shutters: self.shutters,
        }
    }
}

pub(super) fn house_style(rng: &mut Stream) -> HouseStyle {
    // Render in the house colours, weatherboard, or brick.
    let finish = rng.below(10);
    let (wall, finish) = match finish {
        0..=5 => (rng.pick(&palette::HOUSE_WALLS), None),
        6..=7 => (rng.pick(&palette::HOUSE_WALLS), Some(Glow::Siding)),
        _ => (rng.pick(&palette::BRICKS), Some(Glow::Brick)),
    };
    let trim = if rng.chance(0.7) {
        palette::WINDOW_FRAME
    } else {
        shade(wall, 0.8)
    };
    let shutters = rng.chance(0.25).then(|| rng.pick(&palette::SHUTTERS));
    HouseStyle {
        wall,
        roof: rng.pick(&palette::HOUSE_ROOFS),
        trim,
        door: rng.pick(&palette::DOORS),
        finish,
        shutters,
        bars: [0, 1, 2, 2, 3, 4][rng.below(6) as usize],
        stone: rng.chance(0.5),
    }
}

/// What a house body gets besides walls, windows and a roof.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct HouseOpts {
    /// Front door at this distance along the front face.
    pub door: Option<f32>,
    /// Hipped roof (else gabled).
    pub hip: bool,
    /// Ridge along x (else along z: the gable faces the street).
    pub along_x: bool,
    /// Roof pitch: rise per half span.
    pub pitch: f32,
    /// Dormers on the front slope (ridge along x only).
    pub dormers: u32,
    /// Porch over the door (posts and a little roof), else a canopy (or
    /// nothing) by chance.
    pub porch: bool,
    /// Faces that get windows (front, right, back, left).
    pub faces: [bool; 4],
    /// Chimney: 0 none, 1 left end, 2 right end.
    pub chimney: u8,
    /// Leave the solar panels off.
    pub no_solar: bool,
    /// No roof (a terrace puts one roof over the whole row).
    pub no_roof: bool,
}

/// What `house_body` built, for things that go round it.
#[derive(Debug, Clone, Copy)]
pub(super) struct HouseBuilt {
    /// Eave height.
    pub top: f32,
    pub rise: f32,
}

/// A pitched-roof house body occupying `r` with `storeys` floors (a
/// half-storey in the roof when it has dormers). The door is on the front
/// (+z) at `opts.door` along the front face (if any).
pub(super) fn house_body(
    m: &mut MeshBuilder,
    rng: &mut Stream,
    r: Rect,
    storeys: u32,
    st: &HouseStyle,
    opts: &HouseOpts,
) -> HouseBuilt {
    let top = storeys as f32 * SH + 0.3;
    // Plinth.
    let plinth = shade(
        if st.finish == Some(Glow::Brick) {
            st.wall
        } else {
            mix(st.wall, palette::STONE, 0.3)
        },
        0.72,
    );
    m.cuboid(
        Vec3::new(r.x0 - 0.04, 0.0, r.z0 - 0.04),
        Vec3::new(r.x1 + 0.04, PLINTH, r.z1 + 0.04),
        plinth,
    );
    body(m, r, PLINTH, top, st.wall, st.wall, st.finish);
    let faces = Face::of(r, 0.0);
    let win = st.win();
    let door_a = opts.door;
    for (fi, f) in faces.iter().enumerate() {
        if !opts.faces[fi] {
            continue;
        }
        let per = ((f.width / 2.3).floor() as usize).clamp(1, 4);
        for s in 0..storeys {
            let base = 0.3 + s as f32 * SH + 0.95;
            for a in spread(f.width, per) {
                if fi == 0 && s == 0 && door_a.is_some_and(|d| (a - d).abs() < 1.2) {
                    continue;
                }
                f.window_s(m, a, base, 0.8, 1.15, &win);
            }
        }
        // Corner boards / quoins (near).
        m.fine_only(|m| {
            let q = if st.finish == Some(Glow::Brick) {
                palette::STONE
            } else {
                st.trim
            };
            if st.finish != Some(Glow::Brick) || st.stone {
                for a in [0.0, f.width - 0.16] {
                    f.rect(m, a, a + 0.16, PLINTH, top, 0.012, q);
                }
            }
        });
        // Band between the storeys (mid).
        if storeys > 1 && !m.lod {
            f.rect(
                m,
                0.0,
                f.width,
                0.3 + SH - 0.08,
                0.3 + SH + 0.06,
                0.015,
                shade(st.wall, 0.86),
            );
        }
    }
    if let Some(a) = door_a {
        let f = faces[0];
        f.door(m, a, PLINTH, 0.95, 2.1, st.trim, st.door, plinth);
        if opts.porch {
            porch(m, r, a, st);
        } else if rng.chance(0.55) && !m.lod {
            // Canopy on brackets.
            f.slab(m, a - 0.85, a + 0.85, 2.62, 2.72, 0.75, shade(st.roof, 0.8), st.roof);
            m.fine_only(|m| {
                for x in [a - 0.75, a + 0.75] {
                    f.slab(m, x - 0.04, x + 0.04, 2.3, 2.62, 0.6, st.trim, st.trim);
                }
            });
        } else {
            rng.chance(0.0);
        }
    }
    let span = if opts.along_x { r.d() } else { r.w() };
    let rise = span * 0.5 * opts.pitch;
    if opts.no_roof {
        downpipe(m, r.x0 + 0.12, r.z1 + 0.08, top - 0.05);
        return HouseBuilt { top, rise };
    }
    let roof = Roof {
        along_x: opts.along_x,
        hip: opts.hip,
        over: 0.35,
        color: st.roof,
        wall: st.wall,
        finish: Glow::Roof,
    };
    pitched(m, r, top, rise, &roof);
    // Gable attic window on a street-facing gable.
    if !opts.along_x && !opts.hip && rise > 1.4 && !m.lod {
        let f = faces[0];
        let mut w = st.win();
        w.shutters = None;
        f.window_s(m, f.width * 0.5, top + 0.35, 0.6, 0.75, &w);
    }
    // Dormers on the front slope.
    if opts.dormers > 0 && opts.along_x {
        for a in spread(r.w(), opts.dormers as usize) {
            let mut w = st.win();
            w.shutters = None;
            dormer(m, r, top, rise, r.x0 + a, 1.25, true, st.roof, st.trim, &w);
        }
    }
    let solar = !opts.no_solar && opts.along_x && opts.dormers == 0 && r.w() > 4.2 && rng.chance(0.3);
    if solar {
        // On the front slope, inset from the ends (more on a hipped roof).
        let inset = if opts.hip { 0.5 + span * 0.5 } else { 0.6 };
        let (x0, x1) = (r.x0 + inset, r.x1 - inset);
        if x1 - x0 > 1.2 {
            let e = Vec3::new(x0, top, r.z1);
            let ridge = Vec3::new(x0, top + rise, r.cz());
            let up = ridge - e;
            let n = Vec3::X.cross(up).normalize();
            let n = if n.y < 0.0 { -n } else { n };
            m.detail_only(|m| {
                lot_props::solar_panels(m, e + up * 0.15, Vec3::X * (x1 - x0), up * 0.65, n);
            });
        }
    }
    if opts.chimney > 0 {
        let x = if opts.chimney == 1 { r.x0 + 0.7 } else { r.x1 - 0.7 };
        let z = if opts.along_x { r.cz() - span * 0.12 } else { r.cz() };
        let ridge_y = top + rise;
        let at = roof_height(r, top, rise, opts.along_x, x, z);
        let brick = if st.finish == Some(Glow::Brick) {
            st.wall
        } else {
            mix(palette::BRICK, Color::srgb_u32(0x7A6A60), 0.45)
        };
        chimney_c(m, x, z, at - 0.6, ridge_y + 0.7, brick, 1 + u32::from(r.w() > 6.0));
    }
    // Downpipes at the front corners (near).
    downpipe(m, r.x0 + 0.12, r.z1 + 0.08, top - 0.05);
    downpipe(m, r.x1 - 0.12, r.z1 + 0.08, top - 0.05);
    HouseBuilt { top, rise }
}

/// A porch over the door at `a` on the front of `r`: two posts on a
/// platform and a little gabled roof facing the street.
fn porch(m: &mut MeshBuilder, r: Rect, a: f32, st: &HouseStyle) {
    let x = r.x0 + a;
    let (hw, depth) = (0.95, 1.3);
    let pr = Rect {
        x0: x - hw,
        x1: x + hw,
        z0: r.z1 - 0.3,
        z1: r.z1 + depth,
    };
    let deck = shade(mix(st.wall, palette::STONE, 0.5), 0.85);
    m.cuboid2(
        Vec3::new(pr.x0, 0.0, r.z1),
        Vec3::new(pr.x1, PLINTH, pr.z1),
        shade(deck, 0.85),
        deck,
    );
    for px in [pr.x0 + 0.12, pr.x1 - 0.12] {
        m.cuboid(
            Vec3::new(px - 0.07, PLINTH, pr.z1 - 0.2),
            Vec3::new(px + 0.07, 2.55, pr.z1 - 0.06),
            st.trim,
        );
    }
    pitched(
        m,
        pr,
        2.55,
        0.55,
        &Roof {
            along_x: false,
            hip: false,
            over: 0.1,
            color: st.roof,
            wall: st.trim,
            finish: Glow::Roof,
        },
    );
    m.fine_only(|m| {
        // Railing between the posts and the wall.
        for px in [pr.x0 + 0.12, pr.x1 - 0.12] {
            m.cuboid(
                Vec3::new(px - 0.03, PLINTH + 0.8, r.z1),
                Vec3::new(px + 0.03, PLINTH + 0.86, pr.z1 - 0.13),
                st.trim,
            );
        }
    });
}

/// A bay window on the ground floor of the front of `r` centred at `x`:
/// a shallow box with glass on three sides and a little lead roof.
fn bay(m: &mut MeshBuilder, r: Rect, x: f32, w: f32, st: &HouseStyle) {
    let out = 0.6;
    let b = Rect {
        x0: x - w * 0.5,
        x1: x + w * 0.5,
        z0: r.z1 - 0.1,
        z1: r.z1 + out,
    };
    let base = shade(st.wall, 0.85);
    m.cuboid2(Vec3::new(b.x0, 0.0, b.z0), Vec3::new(b.x1, 0.95, b.z1), base, base);
    m.cuboid2(
        Vec3::new(b.x0, 0.95, b.z0),
        Vec3::new(b.x1, 2.45, b.z1),
        st.trim,
        st.trim,
    );
    let f = Face::of(b, 0.0);
    let mut win = st.win();
    win.shutters = None;
    win.sill = None;
    win.frame = st.trim;
    f[0].window_s(m, b.w() * 0.5, 1.05, b.w() - 0.35, 1.2, &win);
    if !m.lod {
        for side in [1, 3] {
            f[side].window_s(m, f[side].width * 0.5 + 0.05, 1.05, 0.3, 1.2, &win);
        }
    }
    // Lead roof: a shallow hip.
    super::building_gen::pitched_roof(m, b, 2.45, 0.35, true, true, 0.08, palette::METAL_DARK, st.trim);
}

fn lawn_color(rng: &mut Stream) -> Color {
    mix(palette::GRASS_LIGHT, palette::GRASS, rng.range(0.0, 0.6))
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
        let species = rng.below(4);
        let size = rng.range(0.55, 0.8);
        let yaw = rng.range(0.0, 6.0);
        if rng.chance(0.45) {
            props::garden_tree(m, p, size, canopy, yaw, species);
        } else {
            props::bush(m, p, size * 1.8, shade(canopy, 1.05), yaw);
        }
    }
}

/// Detached house (level 1): a driveway beside the house with the family
/// car (or a garage or carport), a front path and beds, and on a deep lot
/// a real back garden (patio, shed, washing line, vegetable beds, trees).
///
/// The house takes one of several forms by seed: a plain gabled box, an
/// L-plan with a gabled wing towards the street, a hipped bungalow with a
/// bay window, a narrow house with its gable to the street, or a cottage
/// with dormers in a steep roof.
pub(super) fn detached(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect) {
    lot_ground(m, lot, lawn_color(rng));
    let st = house_style(rng);
    let front_yard = rng.range(2.0, 2.8);
    let drive = lot.w() >= 7.5 && rng.chance(0.75);
    let drive_left = rng.chance(0.5);
    let bw = if drive {
        rng.range(4.3, 4.9).min(lot.w() - 3.4)
    } else {
        rng.range(4.4, 5.6).min(lot.w() - 1.8)
    };
    let bd = rng.range(4.2, 5.2).min(lot.d() - front_yard - 1.2);
    let x0 = if drive {
        if drive_left { lot.x1 - 0.6 - bw } else { lot.x0 + 0.6 }
    } else {
        lot.cx() - bw * 0.5 + rng.range(-0.5, 0.5)
    };
    let body_r = Rect {
        x0,
        x1: x0 + bw,
        z0: lot.z1 - front_yard - bd,
        z1: lot.z1 - front_yard,
    };
    let form = rng.below(5);
    let two = rng.chance(0.25);
    // Cottages keep one storey and live in the roof.
    let storeys = if two && form != 4 { 2 } else { 1 };
    let door = if drive {
        if drive_left { 1.0 } else { bw - 1.0 }
    } else {
        rng.range(0.9, bw - 0.9)
    };
    let pitch = rng.range(0.8, 1.15);
    let porch = rng.chance(0.35);
    let chimney = if rng.chance(0.55) { 1 + rng.below(2) as u8 } else { 0 };
    let mut opts = HouseOpts {
        door: Some(door),
        hip: false,
        along_x: true,
        pitch,
        dormers: 0,
        porch,
        faces: [true; 4],
        chimney,
        no_solar: false,
        no_roof: false,
    };
    // The wing of an L-plan (towards the street, on the side away from
    // the door); a bay window on the bungalow.
    let wing_left = door > bw * 0.5;
    let mut wing: Option<Rect> = None;
    match form {
        1 if bw > 4.4 => {
            let ww = (bw * 0.45).clamp(2.2, 2.8);
            let proj = (front_yard - 0.9).clamp(0.6, 1.5);
            let wx0 = if wing_left { body_r.x0 } else { body_r.x1 - ww };
            wing = Some(Rect {
                x0: wx0,
                x1: wx0 + ww,
                z0: body_r.cz(),
                z1: body_r.z1 + proj,
            });
            opts.porch = false;
        }
        2 => {
            opts.hip = true;
            opts.pitch = pitch * 0.85;
        }
        3 => {
            opts.along_x = false;
            opts.pitch = pitch * 1.1;
        }
        4 if bd > 4.4 => {
            opts.pitch = 1.45;
            opts.dormers = if bw > 5.0 { 2 } else { 1 };
        }
        _ => {}
    }
    let built = house_body(m, rng, body_r, storeys, &st, &opts);
    if let Some(wr) = wing {
        // The wing: same walls, its gable to the street, ridge under the
        // main ridge (its back end is buried in the main roof).
        let wopts = HouseOpts {
            door: None,
            hip: false,
            along_x: false,
            pitch: (built.rise / (wr.w() * 0.5) * 0.85).min(pitch * 1.1),
            dormers: 0,
            porch: false,
            faces: [true, false, false, false],
            chimney: 0,
            no_solar: true,
            no_roof: false,
        };
        house_body(m, rng, wr, storeys, &st, &wopts);
    } else if form == 2 && !m.lod {
        // Bay window on the side of the front away from the door.
        let x = if wing_left { body_r.x0 + 1.4 } else { body_r.x1 - 1.4 };
        if (body_r.x0 + door - x).abs() > 2.0 {
            bay(m, body_r, x, 1.9, &st);
        }
    } else {
        rng.f();
    }
    // Path from the door to the pavement.
    let px = body_r.x0 + door;
    let path = mix(palette::PATH, palette::STONE, rng.f() * 0.5);
    flat_patch(
        m,
        Rect {
            x0: px - 0.5,
            x1: px + 0.5,
            z0: body_r.z1,
            z1: lot.z1,
        },
        0.05,
        path,
    );
    // Driveway with the car, nose to the street (or reversed in); a
    // carport over it, or a garage instead when the car is out.
    let mut drive_rect = None;
    if drive {
        let (dx0, dx1) = if drive_left {
            (lot.x0 + 0.35, body_r.x0 - 0.35)
        } else {
            (body_r.x1 + 0.35, lot.x1 - 0.35)
        };
        let dz0 = (body_r.z0 + 0.5).max(lot.z0 + 0.5);
        lot_props::patch(
            m,
            dx0,
            dz0,
            dx1,
            lot.z1,
            0.045,
            mix(palette::PAVING, palette::ASPHALT, rng.range(0.1, 0.4)),
        );
        drive_rect = Some((dx0, dx1));
        m.driveways.push((dx0, dx1));
        let car = rng.chance(0.8);
        let cover = rng.chance(0.4);
        if car {
            let len = 4.2;
            let cz = (lot.z1 - 0.4 - len * 0.5).min(body_r.z1 + 0.5);
            let yaw = if rng.chance(0.6) {
                -std::f32::consts::FRAC_PI_2
            } else {
                std::f32::consts::FRAC_PI_2
            };
            lot_props::parked_car(m, rng, Vec3::new((dx0 + dx1) * 0.5, 0.045, cz), yaw);
            if cover && dx1 - dx0 > 2.2 {
                carport(m, dx0 + 0.05, dx1 - 0.05, cz - 2.4, cz + 2.0, st.roof, st.trim);
            }
        } else if cover && dx1 - dx0 > 2.2 {
            garage(m, dx0 + 0.05, dx1 - 0.05, dz0, body_r.z1 - 0.4, &st);
        }
        lot_props::bins(
            m,
            rng,
            Vec2::new(if drive_left { dx1 + 0.5 } else { dx0 - 1.2 }, lot.z1 - 0.6),
        );
    }
    // Flower bed along the front of the house.
    if rng.chance(0.55) {
        let (a, b) = if door > bw * 0.5 {
            (body_r.x0 + 0.2, px - 0.7)
        } else {
            (px + 0.7, body_r.x1 - 0.2)
        };
        let clear = wing.is_none_or(|w| b < w.x0 || a > w.x1);
        if b - a > 1.0 && clear {
            lot_props::flower_bed(m, rng, a, b, body_r.z1 + 0.15, body_r.z1 + 0.75);
        }
    }
    // Hedge, picket fence or low wall along the front (not across the
    // driveway).
    let e = 0.3;
    let (fx0, fx1) = match drive_rect {
        Some((_, dx1)) if drive_left => (dx1 + 0.2, lot.x1 - e),
        Some((dx0, _)) => (lot.x0 + e, dx0 - 0.2),
        None => (lot.x0 + e, lot.x1 - e),
    };
    let fz = lot.z1 - e;
    let brick = rng.pick(&palette::BRICKS);
    match rng.below(4) {
        0 => {
            let h = rng.range(0.6, 0.9);
            if px - 0.8 > fx0 {
                hedge(m, Vec2::new(fx0, fz), Vec2::new(px - 0.8, fz), h);
            }
            if fx1 > px + 0.8 {
                hedge(m, Vec2::new(px + 0.8, fz), Vec2::new(fx1, fz), h);
            }
        }
        1 => {
            if px - 0.7 > fx0 {
                fence(m, Vec2::new(fx0, fz), Vec2::new(px - 0.7, fz));
            }
            if fx1 > px + 0.7 {
                fence(m, Vec2::new(px + 0.7, fz), Vec2::new(fx1, fz));
            }
        }
        2 if !m.lod => {
            if px - 0.8 > fx0 {
                garden_wall(m, Vec2::new(fx0, fz), Vec2::new(px - 0.8, fz), brick);
            }
            if fx1 > px + 0.8 {
                garden_wall(m, Vec2::new(px + 0.8, fz), Vec2::new(fx1, fz), brick);
            }
        }
        _ => {}
    }
    // Letterbox by the gate (near).
    m.fine_only(|m| {
        let c = Vec3::new(px + 0.75, 0.0, lot.z1 - 0.45);
        m.cuboid(
            c - Vec3::new(0.03, 0.0, 0.03),
            c + Vec3::new(0.03, 0.95, 0.03),
            palette::METAL_DARK,
        );
        m.cuboid2(
            c + Vec3::new(-0.15, 0.95, -0.12),
            c + Vec3::new(0.15, 1.2, 0.12),
            st.door,
            shade(st.door, 1.1),
        );
    });
    // Back garden: between the house and the back of the lot.
    let garden = Rect {
        x0: lot.x0 + 0.5,
        x1: lot.x1 - 0.5,
        z0: lot.z0 + 0.5,
        z1: body_r.z0 - 0.3,
    };
    let mut used: Vec<Rect> = vec![body_r];
    if garden.d() > 2.2 {
        // Patio right behind the house.
        if rng.chance(0.7) {
            let pz1 = body_r.z0 - 0.05;
            let pz0 = pz1 - rng.range(1.8, 2.6).min(garden.d() - 0.3);
            let pw = rng.range(2.6, 3.6).min(bw);
            let px0 = body_r.cx() - pw * 0.5;
            lot_props::patio(m, rng, px0, pz0, px0 + pw, pz1);
            used.push(Rect {
                x0: px0,
                x1: px0 + pw,
                z0: pz0,
                z1: pz1,
            });
        }
    }
    if garden.d() > 5.0 {
        // Shed in a back corner.
        if rng.chance(0.65) {
            let left = rng.chance(0.5);
            let c = Vec2::new(if left { garden.x0 + 1.3 } else { garden.x1 - 1.3 }, garden.z0 + 1.1);
            lot_props::shed(m, rng, c, 1.0);
            used.push(Rect {
                x0: c.x - 1.4,
                x1: c.x + 1.4,
                z0: c.y - 1.2,
                z1: c.y + 1.4,
            });
        }
        // Washing line across the lawn.
        if rng.chance(0.45) {
            let z = garden.z0 + garden.d() * rng.range(0.45, 0.65);
            let (a, b) = (
                Vec2::new(garden.x0 + 0.6, z),
                Vec2::new(garden.x1 - 0.6, z - rng.range(-0.8, 0.8)),
            );
            if !used
                .iter()
                .any(|r| r.z0 - 0.3 < z && z < r.z1 + 0.3 && r.x1 > a.x && r.x0 < b.x)
            {
                lot_props::washing_line(m, rng, a, b);
            }
        }
        // Vegetable beds.
        if rng.chance(0.45) {
            let left = rng.chance(0.5);
            let w = rng.range(2.0, 3.2);
            let (x0, x1) = if left {
                (garden.x0 + 0.2, garden.x0 + 0.2 + w)
            } else {
                (garden.x1 - 0.2 - w, garden.x1 - 0.2)
            };
            let (z0, z1) = (
                garden.z0 + garden.d() * 0.35,
                garden.z0 + garden.d() * 0.35 + rng.range(1.5, 2.4),
            );
            let r = Rect { x0, x1, z0, z1 };
            if !used.iter().any(|u| u.overlaps(&r)) && z1 < garden.z1 {
                lot_props::veg_beds(m, rng, x0, z0, x1, z1);
                used.push(r);
            }
        }
        // A trampoline or a sandpit for the children (near).
        if rng.chance(0.25) {
            let c = Vec2::new(garden.cx() + rng.range(-1.0, 1.0), garden.z0 + garden.d() * 0.25);
            let r = Rect {
                x0: c.x - 1.3,
                x1: c.x + 1.3,
                z0: c.y - 1.3,
                z1: c.y + 1.3,
            };
            if !used.iter().any(|u| u.overlaps(&r)) {
                lot_props::trampoline(m, Vec3::new(c.x, 0.0, c.y));
                used.push(r);
            }
        }
        // Side and back fences on deep lots.
        if rng.chance(0.6) {
            let (a, b, c, d) = (
                Vec2::new(lot.x0 + e, body_r.z0),
                Vec2::new(lot.x0 + e, lot.z0 + e),
                Vec2::new(lot.x1 - e, lot.z0 + e),
                Vec2::new(lot.x1 - e, body_r.z0),
            );
            fence(m, a, b);
            fence(m, b, c);
            fence(m, c, d);
        }
    }
    let avoid = used.iter().fold(body_r, |acc, r| acc.union(r));
    yard_greenery(m, rng, lot, avoid);
}

/// A carport over a driveway: four posts and a flat roof.
fn carport(m: &mut MeshBuilder, x0: f32, x1: f32, z0: f32, z1: f32, roof: Color, post: Color) {
    if m.lod {
        return;
    }
    for (x, z) in [
        (x0 + 0.08, z0 + 0.1),
        (x1 - 0.08, z0 + 0.1),
        (x0 + 0.08, z1 - 0.1),
        (x1 - 0.08, z1 - 0.1),
    ] {
        m.cuboid(
            Vec3::new(x - 0.06, 0.0, z - 0.06),
            Vec3::new(x + 0.06, 2.3, z + 0.06),
            post,
        );
    }
    m.cuboid2(
        Vec3::new(x0 - 0.1, 2.3, z0 - 0.1),
        Vec3::new(x1 + 0.1, 2.45, z1 + 0.1),
        shade(roof, 0.8),
        shade(roof, 0.95),
    );
}

/// A single garage on the driveway: flat roof with a fascia, an
/// up-and-over door facing the street.
fn garage(m: &mut MeshBuilder, x0: f32, x1: f32, z0: f32, z1: f32, st: &HouseStyle) {
    let r = Rect { x0, x1, z0, z1 };
    if r.d() < 2.5 {
        return;
    }
    body(m, r, 0.0, 2.5, st.wall, st.wall, st.finish);
    m.cuboid2(
        Vec3::new(x0 - 0.08, 2.5, z0 - 0.08),
        Vec3::new(x1 + 0.08, 2.7, z1 + 0.08),
        shade(st.roof, 0.75),
        shade(palette::ROOF_FLAT, 0.8),
    );
    let f = Face::of(r, 0.0)[0];
    let door = mix(st.door, palette::FENCE, 0.6);
    f.rect(m, 0.25, f.width - 0.25, 0.0, 2.1, 0.03, door);
    m.fine_only(|m| {
        let mut y = 0.35;
        while y < 2.0 {
            f.rect(m, 0.25, f.width - 0.25, y, y + 0.03, 0.04, shade(door, 0.82));
            y += 0.35;
        }
    });
}

/// A row of narrow townhouses (big level-1 lots and level 2) with
/// `storeys` floors: units in their own colours under one roofline, bay
/// windows on some, chimney stacks on the party walls, small front
/// gardens behind low walls or hedges.
pub(super) fn terrace(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, lawn_color(rng));
    let units = if lot.w() > 12.0 { 4 } else { 2 };
    let front_yard = rng.range(1.4, 2.0);
    let depth = (lot.d() - front_yard - 1.0).min(6.0);
    let inner = lot.w() - 0.8;
    let uw = inner / units as f32;
    let roof = rng.pick(&palette::HOUSE_ROOFS);
    let front_gables = rng.chance(0.4);
    let hip_ends = !front_gables && rng.chance(0.4);
    let pitch = rng.range(0.75, 1.0);
    // The whole row shares a finish: all brick, or rendered in colours.
    let brick_row = rng.chance(0.35);
    let row_brick = rng.pick(&palette::BRICKS);
    let bays = rng.chance(0.5);
    let dormers = storeys == 2 && !front_gables && rng.chance(0.35);
    let boundary = rng.below(3);
    for u in 0..units {
        let x0 = lot.x0 + 0.4 + uw * u as f32;
        let body_r = Rect {
            x0,
            x1: x0 + uw,
            z0: lot.z1 - front_yard - depth,
            z1: lot.z1 - front_yard,
        };
        let mut st = house_style(rng);
        st.roof = roof;
        if brick_row {
            st.wall = row_brick;
            st.finish = Some(Glow::Brick);
        } else if st.finish == Some(Glow::Brick) {
            st.finish = None;
            st.wall = rng.pick(&palette::HOUSE_WALLS);
        }
        let door = if u % 2 == 0 { 0.8 } else { uw - 0.8 };
        let end = u == 0 || u == units - 1;
        let _ = end;
        let opts = HouseOpts {
            door: Some(door),
            hip: false,
            along_x: !front_gables,
            pitch,
            dormers: 0,
            porch: false,
            faces: [true, u == units - 1, true, u == 0],
            chimney: 0,
            no_solar: true,
            // One roof over the row unless each unit has its own gable.
            no_roof: !front_gables,
        };
        let built = house_body(m, rng, body_r, storeys, &st, &opts);
        if !front_gables && u == units - 1 {
            // The row's roof: hipped or gabled ends, dormers per unit,
            // chimney stacks on the party walls.
            let row = Rect {
                x0: lot.x0 + 0.4,
                x1: lot.x0 + 0.4 + inner,
                ..body_r
            };
            let rise = row.d() * 0.5 * pitch;
            pitched(
                m,
                row,
                built.top,
                rise,
                &Roof {
                    along_x: true,
                    hip: hip_ends,
                    over: 0.35,
                    color: roof,
                    wall: st.wall,
                    finish: Glow::Roof,
                },
            );
            if dormers {
                let mut w = st.win();
                w.shutters = None;
                for k in 0..units {
                    let cx = row.x0 + uw * (k as f32 + 0.5);
                    if hip_ends && (k == 0 || k == units - 1) && uw < rise * 1.6 {
                        continue;
                    }
                    super::building_gen::dormer(m, row, built.top, rise, cx, 1.2, true, roof, st.trim, &w);
                }
            }
            let stack = shade(mix(palette::BRICK, Color::srgb_u32(0x7A6A60), 0.45), 0.95);
            for k in (1..units).step_by(2) {
                let x = row.x0 + uw * k as f32;
                let z = row.cz() - 0.3;
                let at = roof_height(row, built.top, rise, true, x, z);
                chimney_c(m, x, z, at - 0.5, built.top + rise + 0.7, stack, 2);
            }
        }
        if bays && !m.lod && uw > 3.4 {
            let bx = if u % 2 == 0 {
                body_r.x1 - uw * 0.38
            } else {
                body_r.x0 + uw * 0.38
            };
            bay(m, body_r, bx, (uw * 0.45).min(2.0), &st);
        }
        let px = body_r.x0 + door;
        flat_patch(
            m,
            Rect {
                x0: px - 0.45,
                x1: px + 0.45,
                z0: body_r.z1,
                z1: lot.z1,
            },
            0.05,
            mix(palette::PATH, palette::STONE, 0.3),
        );
        // Front boundary of this unit, with a gap for the path.
        let fz = lot.z1 - 0.25;
        let (a0, a1) = (x0 + 0.1, x0 + uw - 0.1);
        let segs = [(a0, px - 0.6), (px + 0.6, a1)];
        for (s0, s1) in segs {
            if s1 - s0 < 0.4 {
                continue;
            }
            match boundary {
                0 => garden_wall(m, Vec2::new(s0, fz), Vec2::new(s1, fz), row_brick),
                1 => hedge(m, Vec2::new(s0, fz), Vec2::new(s1, fz), 0.7),
                _ => fence(m, Vec2::new(s0, fz), Vec2::new(s1, fz)),
            }
        }
        lot_props::bins(
            m,
            rng,
            Vec2::new(if u % 2 == 0 { px + 0.7 } else { px - 1.35 }, lot.z1 - 0.85),
        );
    }
    // Back gardens per unit: patio, sometimes a shed, fences between.
    let back = lot.z1 - front_yard - depth;
    if back - lot.z0 > 3.0 {
        for u in 0..units {
            let x0 = lot.x0 + 0.4 + uw * u as f32;
            let x1 = x0 + uw;
            lot_props::patch(
                m,
                x0 + 0.3,
                back - 1.8,
                x1 - 0.3,
                back - 0.05,
                0.05,
                mix(palette::PAVING, palette::PATH, 0.5),
            );
            if back - lot.z0 > 6.0 && rng.chance(0.4) {
                lot_props::shed(m, rng, Vec2::new((x0 + x1) * 0.5, lot.z0 + 1.4), 1.0);
            } else if rng.chance(0.4) {
                lot_props::washing_line(
                    m,
                    rng,
                    Vec2::new(x0 + 0.5, (lot.z0 + back) * 0.5),
                    Vec2::new(x1 - 0.5, (lot.z0 + back) * 0.5 + 0.4),
                );
            }
            if u > 0 {
                fence(m, Vec2::new(x0, back), Vec2::new(x0, lot.z0 + 0.3));
            }
        }
    }
    let e = 0.3;
    hedge(
        m,
        Vec2::new(lot.x0 + e, lot.z0 + e),
        Vec2::new(lot.x1 - e, lot.z0 + e),
        0.8,
    );
}

/// Apartment block (level 3+) with `storeys` floors including the ground
/// floor: a base (a darker podium, the entrance with a canopy), the middle
/// (windows with sills, balconies with railings on the street side, a
/// glazed stair core on a side), and a top (a cornice under a flat roof
/// with plant, a set-back attic storey on taller blocks, or a hipped roof
/// with dormers). Deep lots park the residents' cars behind.
pub(super) fn apartments(m: &mut MeshBuilder, rng: &mut Stream, lot: Rect, storeys: u32) {
    lot_ground(m, lot, mix(lawn_color(rng), palette::PAVING, 0.35));
    let brick = rng.chance(0.3);
    let wall = if brick {
        rng.pick(&palette::BRICKS)
    } else {
        rng.pick(&palette::HOUSE_WALLS)
    };
    let finish = brick.then_some(Glow::Brick);
    let accent = rng.pick(&palette::HOUSE_ROOFS);
    let base_col = mix(shade(wall, 0.82), palette::BRICK, rng.range(0.0, 0.5));
    let big = lot.w() > 12.0;
    let side = if big { 1.0 } else { 0.4 };
    // Deep lots: a block of normal depth with residents' parking behind.
    let deep = lot.d() > 12.0;
    let body_r = Rect {
        x0: lot.x0 + side,
        x1: lot.x1 - side,
        z0: if deep {
            lot.z1 - 1.6 - rng.range(9.5, 11.0)
        } else {
            lot.z0 + side
        },
        z1: lot.z1 - 1.6,
    };
    if deep {
        let park = Rect {
            x0: lot.x0 + 0.4,
            x1: lot.x1 - 0.4,
            z0: lot.z0 + 0.4,
            z1: body_r.z0 - 0.6,
        };
        lot_props::patch(
            m,
            park.x0,
            park.z0,
            park.x1,
            park.z1,
            0.04,
            mix(palette::ASPHALT, palette::PAVING, 0.3),
        );
        let white = lin(palette::MARKING);
        let bays = ((park.w() - 0.4) / 2.6).floor() as usize;
        for k in 0..=bays {
            let x = park.x0 + 0.2 + k as f32 * 2.6;
            m.quad_up(
                [
                    Vec3::new(x - 0.05, 0.05, park.z0 + 0.2),
                    Vec3::new(x + 0.05, 0.05, park.z0 + 0.2),
                    Vec3::new(x + 0.05, 0.05, park.z0 + 4.8),
                    Vec3::new(x - 0.05, 0.05, park.z0 + 4.8),
                ],
                white,
            );
        }
        for k in 0..bays {
            if rng.chance(0.6) {
                let x = park.x0 + 0.2 + (k as f32 + 0.5) * 2.6;
                lot_props::parked_car(m, rng, Vec3::new(x, 0.04, park.z0 + 2.5), std::f32::consts::FRAC_PI_2);
            }
        }
    }
    let sh = 3.0;
    let ground = 3.4;
    let attic = storeys >= 5 && rng.chance(0.6);
    let main = if attic { storeys - 1 } else { storeys };
    let top = ground + (main - 1) as f32 * sh;
    // Podium ground floor in a darker base colour, then the upper floors.
    m.cuboid2(
        Vec3::new(body_r.x0, 0.0, body_r.z0),
        Vec3::new(body_r.x1, ground, body_r.z1),
        base_col,
        base_col,
    );
    body(m, body_r, ground, top, wall, wall, finish);
    let faces = Face::of(body_r, 0.0);
    let trim = if rng.chance(0.5) {
        palette::WINDOW_FRAME
    } else {
        shade(wall, 0.8)
    };
    let win = Win {
        frame: trim,
        glass: palette::WINDOW,
        sill: Some(if brick { palette::STONE } else { shade(wall, 0.9) }),
        bars: [0, 1, 2, 4][rng.below(4) as usize],
        shutters: None,
    };
    let balconies = rng.chance(0.75);
    let rail_glass = rng.chance(0.5);
    let stair_side = rng.below(2) as usize * 2 + 1;
    let per_of = |w: f32| ((w / 2.2).floor() as usize).clamp(1, 8);
    for (fi, f) in faces.iter().enumerate() {
        let per = per_of(f.width);
        // A glazed stair core on one side (mid): a vertical strip of glass.
        let core = fi == stair_side && f.width > 7.0 && main >= 3;
        let core_a = f.width * 0.5;
        for (k, a) in spread(f.width, per).into_iter().enumerate() {
            if !(fi == 0 && k == per / 2) {
                f.window_s(m, a, 0.9, 1.0, 1.5, &Win { sill: None, ..win });
            }
        }
        for s in 1..main {
            let base = ground + (s - 1) as f32 * sh + 0.85;
            for (k, a) in spread(f.width, per).into_iter().enumerate() {
                if core && (a - core_a).abs() < 1.2 {
                    continue;
                }
                f.window_s(m, a, base, 0.95, 1.35, &win);
                if balconies && fi == 0 && k % 2 == (s as usize % 2) {
                    balcony(m, f, a, base, wall, accent, rail_glass);
                }
            }
            // Floor band.
            let y = ground + (s - 1) as f32 * sh;
            f.rect(m, 0.0, f.width, y - 0.1, y + 0.1, 0.02, shade(wall, 0.86));
        }
        if core && !m.lod {
            f.lit_rect(
                m,
                Glow::Window,
                core_a - 0.6,
                core_a + 0.6,
                ground + 0.4,
                top - 0.4,
                0.04,
                palette::GLASS,
            );
            m.fine_only(|m| {
                for s in 1..main {
                    let y = ground + (s - 1) as f32 * sh;
                    f.rect(m, core_a - 0.62, core_a + 0.62, y - 0.06, y + 0.06, 0.05, trim);
                }
            });
        }
    }
    // Entrance: glass doors, a canopy, house number, a path.
    let f = faces[0];
    let per = per_of(f.width);
    let a = spread(f.width, per)[per / 2];
    f.lit_rect(m, Glow::Shop, a - 0.75, a + 0.75, 0.0, 2.6, 0.08, palette::GLASS);
    f.slab(m, a - 1.3, a + 1.3, 2.8, 2.98, 1.4, accent, accent);
    m.fine_only(|m| {
        f.rect(m, a - 0.04, a + 0.04, 0.0, 2.6, 0.09, palette::METAL_DARK);
        f.rect(m, a - 0.75, a + 0.75, 2.2, 2.26, 0.09, palette::METAL_DARK);
        // Number plate and a wall light either side.
        f.rect(m, a + 0.9, a + 1.25, 2.2, 2.45, 0.03, palette::STONE);
        for x in [a - 1.0, a + 1.0] {
            m.glow(Glow::Sign, 0.0, |m| {
                f.rect(m, x - 0.07, x + 0.07, 2.0, 2.3, 0.06, Color::srgb_u32(0xF6D9A0))
            });
        }
        // Steps.
        f.slab(m, a - 1.0, a + 1.0, 0.0, 0.12, 0.8, palette::STONE, palette::STONE);
    });
    flat_patch(
        m,
        Rect {
            x0: body_r.x0 + a - 0.8,
            x1: body_r.x0 + a + 0.8,
            z0: body_r.z1,
            z1: lot.z1,
        },
        0.05,
        palette::PATH,
    );
    let hipped = rng.chance(0.6);
    if !hipped || attic {
        cornice(
            m,
            body_r,
            top - 0.35,
            0.3,
            0.18,
            if brick { palette::STONE } else { shade(wall, 0.92) },
        );
    }
    if attic {
        // Set-back top floor with a roof terrace in front.
        flat_roof(m, body_r, top, wall, palette::ROOF_FLAT, 1.0);
        let inner = Rect {
            z1: body_r.z1 - 1.8,
            z0: body_r.z0 + 0.6,
            x0: body_r.x0 + 0.6,
            x1: body_r.x1 - 0.6,
        };
        let atop = top + sh;
        m.cuboid2(
            Vec3::new(inner.x0, top, inner.z0),
            Vec3::new(inner.x1, atop, inner.z1),
            mix(wall, palette::WINDOW_FRAME, 0.4),
            wall,
        );
        let f = Face::of(inner, top)[0];
        f.lit_rect(m, Glow::Window, 0.5, f.width - 0.5, 0.3, 2.4, 0.04, palette::GLASS);
        m.fine_only(|m| {
            for a in spread(f.width - 1.0, ((f.width - 1.0) / 1.4) as usize + 1) {
                f.rect(m, 0.5 + a - 0.04, 0.5 + a + 0.04, 0.3, 2.4, 0.05, trim);
            }
            // Terrace: planters and a parasol.
            let z = (inner.z1 + body_r.z1) * 0.5;
            for x in [body_r.x0 + 1.0, body_r.x1 - 1.0] {
                m.cuboid2(
                    Vec3::new(x - 0.4, top, z - 0.3),
                    Vec3::new(x + 0.4, top + 0.5, z + 0.3),
                    palette::STONE,
                    shade(palette::STONE, 0.9),
                );
                props::bush(m, Vec3::new(x, top + 0.4, z), 0.6, palette::HEDGE, x);
            }
        });
        if hipped {
            pitched(
                m,
                inner,
                atop,
                inner.d() * 0.25,
                &Roof {
                    along_x: true,
                    hip: true,
                    over: 0.3,
                    color: accent,
                    wall,
                    finish: Glow::Roof,
                },
            );
        } else {
            flat_roof(m, inner, atop, wall, palette::ROOF_FLAT, 0.4);
            roof_details(m, rng, inner.inset(0.3), atop);
        }
    } else if hipped {
        let rise = body_r.d() * 0.28;
        pitched(
            m,
            body_r,
            top,
            rise,
            &Roof {
                along_x: true,
                hip: true,
                over: 0.35,
                color: accent,
                wall,
                finish: Glow::Roof,
            },
        );
        // Dormers along the street side, a stack at each end.
        let n = ((body_r.w() - 3.0) / 3.2).floor().max(0.0) as usize;
        let mut dw = win;
        dw.sill = None;
        if rise > 2.2 {
            for a in spread(body_r.w() - 2.0 * rise, n.min(4)) {
                super::building_gen::dormer(m, body_r, top, rise, body_r.x0 + rise + a, 1.3, true, accent, trim, &dw);
            }
        }
        for x in [body_r.x0 + rise * 0.9, body_r.x1 - rise * 0.9] {
            let z = body_r.cz() - 0.4;
            let at = roof_height(body_r, top, rise, true, x, z).min(top + rise);
            chimney_c(m, x, z, at - 0.5, top + rise + 0.6, shade(base_col, 0.9), 2);
        }
    } else {
        flat_roof(m, body_r, top, wall, palette::ROOF_FLAT, 0.6);
        // Lift overrun and the usual plant.
        let lift = Rect {
            x0: body_r.cx() - 1.2,
            x1: body_r.cx() + 1.2,
            z0: body_r.cz() - 1.0,
            z1: body_r.cz() + 1.0,
        };
        m.cuboid2(
            Vec3::new(lift.x0, top, lift.z0),
            Vec3::new(lift.x1, top + 2.0, lift.z1),
            shade(wall, 0.85),
            palette::ROOF_FLAT,
        );
        let mut rr = body_r.inset(0.4);
        rr.z0 = lift.z1 + 0.2;
        roof_details(m, rng, rr, top);
    }
    m.detail_only(|m| {
        for x in [lot.x0 + 0.6, lot.x1 - 0.6] {
            props::bush(m, Vec3::new(x, 0.0, lot.z1 - 0.8), 0.8, palette::HEDGE, x);
        }
    });
    // Cycle stands and bins by the entrance (near).
    m.fine_only(|m| {
        let x = body_r.x0 + a + 2.2;
        if x + 1.6 < lot.x1 {
            for k in 0..3 {
                let c = Vec3::new(x + k as f32 * 0.5, 0.0, lot.z1 - 0.9);
                m.cuboid(
                    c - Vec3::new(0.02, 0.0, 0.3),
                    c + Vec3::new(0.02, 0.8, 0.3),
                    palette::METAL_DARK,
                );
            }
        }
    });
    downpipe(m, body_r.x0 + 0.15, body_r.z1 + 0.06, top);
    downpipe(m, body_r.x1 - 0.15, body_r.z1 + 0.06, top);
}

/// A balcony on face `f` under the window at `a` (window sill at `base`):
/// a slab, a railing (bars near, a panel mid) and a top rail.
fn balcony(m: &mut MeshBuilder, f: &Face, a: f32, base: f32, wall: Color, accent: Color, glass: bool) {
    let (a0, a1) = (a - 0.95, a + 0.95);
    let (y0, y1) = (base - 0.9, base - 0.75);
    f.slab(m, a0, a1, y0, y1, 1.0, shade(wall, 0.85), wall);
    if m.lod {
        return;
    }
    let rail = if glass {
        mix(palette::GLASS, Color::WHITE, 0.45)
    } else {
        palette::METAL_DARK
    };
    let top = base - 0.2;
    if !m.fine {
        // Middle model: the front panel with its rail.
        f.slab_out(m, a0, a1, y1, top + 0.07, 0.95, 1.02, if glass { rail } else { accent });
        return;
    }
    if glass {
        // A panel along the front and the two sides.
        f.slab_out(m, a0, a1, y1, top, 0.95, 1.0, rail);
        f.slab_out(m, a0, a0 + 0.05, y1, top, 0.0, 0.95, rail);
        f.slab_out(m, a1 - 0.05, a1, y1, top, 0.0, 0.95, rail);
    } else {
        // Bars along the front.
        let mut x = a0 + 0.06;
        while x < a1 - 0.03 {
            f.slab_out(m, x - 0.02, x + 0.02, y1, top, 0.95, 0.99, rail);
            x += 0.16;
        }
        for x in [a0, a1 - 0.05] {
            f.slab_out(m, x, x + 0.05, y1, top, 0.0, 1.0, rail);
        }
    }
    // Hand rail in the accent colour.
    f.slab_out(m, a0 - 0.02, a1 + 0.02, top, top + 0.07, 0.93, 1.03, accent);
    m.fine_only(|m| {
        // A plant pot or a chair on some.
        if super::building_gen::wrand(f.at(a, base, 0.0)) < 0.4 {
            let p = f.at(a + 0.5, y1, 0.6);
            m.cylinder(p, 0.15, 0.3, 6, Color::srgb_u32(0xB0684A), Color::srgb_u32(0x5A4030));
            props::bush(m, p + Vec3::Y * 0.25, 0.4, palette::HEDGE, a);
        }
    });
}
