//! Low-poly people: generated in code like the cars, in the shared
//! vertex-coloured material.
//!
//! A figure is a handful of tapered boxes (feet, legs, hips, torso, arms,
//! hands, neck, head, hair) built for one of [`BUILDS`] body types and one of
//! [`POSES`] poses: [`WALK_POSES`] evenly spaced phases of a walk cycle, then
//! standing. A walker is drawn by swapping its entity's mesh to the pose of
//! its phase (no skeleton per person: a few shared meshes, instanced).
//!
//! Colours are chosen per person, not per mesh: skin, hair, clothes on top
//! and below are tagged slots (`Glow::{PersonSkin, ..}` in `uv.x`), and the
//! city shader takes their colours from the palette by the entity's
//! `MeshTag` ([`person_tag`]); shoes, belts and the like are plain vertex
//! colours. Local frame: the figure faces +x, feet on `y = 0`, about 1.75 m
//! tall (children and the old are the same models scaled and slowed).

use bevy::prelude::*;

use super::mesh_util::{Glow, MeshBuilder};
use super::palette;

/// Walk-cycle poses (evenly spaced phases); pose `WALK_POSES` stands.
pub const WALK_POSES: usize = 8;
/// Poses per body type: the walk cycle, then standing.
pub const POSES: usize = WALK_POSES + 1;
/// Index of the standing pose.
pub const STAND: usize = WALK_POSES;

/// Body types (mesh variants).
pub const BUILDS: usize = 8;

/// Metres covered by one full walk cycle (two steps) of a 1.75 m adult.
pub const STRIDE_M: f32 = 1.55;

/// Height of the model, metres (before the per-person scale).
pub const HEIGHT_M: f32 = 1.76;

/// A body type.
#[derive(Debug, Clone, Copy)]
struct Build {
    /// Width factor (shoulders, hips, limbs).
    girth: f32,
    /// Long sleeves (else forearms bare).
    sleeves: bool,
    /// Hair: 0 short, 1 long (to the shoulders), 2 very short, 3 bun.
    hair: u8,
    /// A skirt / dress (legs bare below it) instead of trousers.
    skirt: bool,
    /// A coat over the hips (the top colour reaches mid-thigh).
    coat: bool,
    /// A bag on one shoulder.
    bag: bool,
    /// Head size factor (children's models are scaled down as a whole; a
    /// bigger head keeps them reading as children).
    head: f32,
    /// Forward lean of the upper body, radians (the old stoop a little).
    lean: f32,
}

const BUILD_LIST: [Build; BUILDS] = [
    Build { girth: 0.92, sleeves: false, hair: 0, skirt: false, coat: false, bag: false, head: 1.0, lean: 0.02 },
    Build { girth: 1.0, sleeves: true, hair: 0, skirt: false, coat: false, bag: true, head: 1.0, lean: 0.03 },
    Build { girth: 1.12, sleeves: true, hair: 2, skirt: false, coat: false, bag: false, head: 1.02, lean: 0.03 },
    Build { girth: 0.9, sleeves: false, hair: 1, skirt: true, coat: false, bag: true, head: 0.98, lean: 0.02 },
    Build { girth: 0.95, sleeves: true, hair: 1, skirt: false, coat: false, bag: false, head: 0.98, lean: 0.02 },
    Build { girth: 1.04, sleeves: true, hair: 0, skirt: false, coat: true, bag: false, head: 1.0, lean: 0.03 },
    // Child: bigger head (the model is scaled down to child height).
    Build { girth: 1.05, sleeves: false, hair: 0, skirt: false, coat: false, bag: false, head: 1.32, lean: 0.0 },
    // Older: a coat, a slight stoop, a bun or short hair.
    Build { girth: 1.02, sleeves: true, hair: 3, skirt: false, coat: true, bag: false, head: 1.0, lean: 0.12 },
];

/// Index of the child build.
pub const CHILD_BUILD: usize = 6;
/// Index of the older person's build.
pub const SENIOR_BUILD: usize = 7;

/// Colour slots a person's `MeshTag` picks from (palette sizes).
pub const SKIN_TONES: u32 = palette::PERSON_SKIN.len() as u32;
pub const HAIR_COLOURS: u32 = palette::PERSON_HAIR.len() as u32;
pub const TOP_COLOURS: u32 = palette::PERSON_TOP.len() as u32;
pub const BOTTOM_COLOURS: u32 = palette::PERSON_BOTTOM.len() as u32;

/// The `MeshTag` of a person's colours (bit layout read by `city.wesl`:
/// top bits 1-4, bottom 5-7, skin 8-10, hair 11-13; bit 0 stays free).
pub fn person_tag(top: u32, bottom: u32, skin: u32, hair: u32) -> u32 {
    ((top % TOP_COLOURS) << 1)
        | ((bottom % BOTTOM_COLOURS) << 5)
        | ((skin % SKIN_TONES) << 8)
        | ((hair % HAIR_COLOURS) << 11)
}

/// Joint angles of one pose, radians (positive swings forward, towards +x).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Pose {
    hip: [f32; 2],
    knee: [f32; 2],
    shoulder: [f32; 2],
    elbow: [f32; 2],
    /// Body raised by this much (the stance leg's height), metres.
    lift: f32,
}

fn pose_of(index: usize) -> Pose {
    if index >= WALK_POSES {
        return Pose {
            hip: [0.02, -0.02],
            knee: [0.03, 0.03],
            shoulder: [0.04, -0.03],
            elbow: [0.18, 0.15],
            lift: 0.0,
        };
    }
    let th = index as f32 / WALK_POSES as f32 * std::f32::consts::TAU;
    let leg = |t: f32| 0.42 * t.sin();
    // The swinging leg bends at the knee on its way forward (cos > 0 while
    // the hip angle grows), the stance leg stays nearly straight.
    let knee = |t: f32| 0.08 + 0.55 * t.cos().max(0.0).powf(1.5);
    let hip = [leg(th), leg(th + std::f32::consts::PI)];
    let kn = [knee(th), knee(th + std::f32::consts::PI)];
    Pose {
        hip,
        knee: kn,
        // Arms swing against the legs on the same side.
        shoulder: [-0.75 * hip[0], -0.75 * hip[1]],
        elbow: [0.22 + 0.25 * (-hip[0]).max(0.0), 0.22 + 0.25 * (-hip[1]).max(0.0)],
        lift: 0.0,
    }
}

/// A point `len` metres from `from` at `angle` from straight down, in the
/// x-y plane (positive angles forward, towards +x).
fn down(from: Vec3, angle: f32, len: f32) -> Vec3 {
    from + Vec3::new(angle.sin() * len, -angle.cos() * len, 0.0)
}

/// A tapered box along `a -> b` (in the x-y plane, `a` the upper end), half
/// sizes `(along x, across z)` at each end, offset `z` sideways.
#[allow(clippy::too_many_arguments)]
fn limb(m: &mut MeshBuilder, a: Vec3, b: Vec3, ha: Vec2, hb: Vec2, color: Color) {
    let axis = (b - a).normalize_or(Vec3::NEG_Y);
    // Forward of the limb: perpendicular to it in the x-y plane.
    let fwd = Vec3::new(-axis.y, axis.x, 0.0) * -1.0;
    let fwd = if fwd.x < 0.0 { -fwd } else { fwd };
    let side = Vec3::Z;
    let corners = |c: Vec3, h: Vec2| {
        [
            c + fwd * h.x + side * h.y,
            c - fwd * h.x + side * h.y,
            c - fwd * h.x - side * h.y,
            c + fwd * h.x - side * h.y,
        ]
    };
    let top = corners(a, ha);
    let bot = corners(b, hb);
    let center = (a + b) * 0.5;
    let mut face = |q: [Vec3; 4]| {
        let mid = (q[0] + q[1] + q[2] + q[3]) * 0.25;
        m.quad_facing(q, (mid - center).normalize_or(Vec3::Y), color);
    };
    for i in 0..4 {
        let j = (i + 1) % 4;
        face([top[i], top[j], bot[j], bot[i]]);
    }
    face(top);
    face(bot);
}

/// An axis-aligned box from `min` to `max` (all six faces).
fn block(m: &mut MeshBuilder, min: Vec3, max: Vec3, color: Color) {
    let c = (min + max) * 0.5;
    let h = (max - min) * 0.5;
    limb(
        m,
        Vec3::new(c.x, max.y, c.z),
        Vec3::new(c.x, min.y, c.z),
        Vec2::new(h.x, h.z),
        Vec2::new(h.x, h.z),
        color,
    );
}

/// White, shaded by `k` (tinted slots take their colour in the shader).
fn white(k: f32) -> Color {
    palette::shade(Color::WHITE, k)
}

/// One figure: body type `build`, pose `pose` (`0..POSES`); `far`: the
/// distant level of detail (legs, body and head only).
pub fn person_model(build: usize, pose: usize, far: bool) -> MeshBuilder {
    let b = BUILD_LIST[build % BUILDS];
    let p = pose_of(pose);
    let mut m = MeshBuilder::new();
    let g = b.girth;
    // Proportions of a 1.76 m adult (a child's build has a bigger head on
    // the same body; the whole model is scaled to the person's height).
    let leg_up = 0.43;
    let leg_lo = 0.41;
    let foot_h = 0.07;
    let hip_y = leg_up + leg_lo + foot_h;
    // Stance foot on the ground: the lower of the two feet touches y = 0.
    let foot_y = |i: usize| {
        let knee = down(Vec3::new(0.0, hip_y, 0.0), p.hip[i], leg_up);
        down(knee, p.hip[i] - p.knee[i], leg_lo).y - foot_h
    };
    let drop = foot_y(0).min(foot_y(1));
    let hip = Vec3::new(0.0, hip_y - drop + p.lift, 0.0);
    let shoe = palette::PERSON_SHOE;
    let bottom = |k: f32| white(k);
    let skin = white(1.0);
    for (i, zs) in [(0usize, 1.0f32), (1, -1.0)] {
        let z = zs * 0.095 * g;
        let h0 = hip + Vec3::new(0.0, 0.0, z);
        let knee = down(h0, p.hip[i], leg_up);
        let ankle = down(knee, p.hip[i] - p.knee[i], leg_lo);
        if far {
            m.glow(Glow::PersonBottom, 0.0, |m| {
                limb(m, h0, ankle, Vec2::new(0.085, 0.075) * g, Vec2::new(0.06, 0.055) * g, bottom(0.95))
            });
        } else {
            // Thigh in the trousers' (or the skirt's) colour, shins bare
            // under a skirt.
            m.glow(Glow::PersonBottom, 0.0, |m| {
                limb(m, h0, knee, Vec2::new(0.085, 0.08) * g, Vec2::new(0.065, 0.062) * g, bottom(0.96))
            });
            if b.skirt {
                m.glow(Glow::PersonSkin, 0.0, |m| {
                    limb(m, knee, ankle, Vec2::new(0.05, 0.048) * g, Vec2::new(0.04, 0.04) * g, skin)
                });
            } else {
                m.glow(Glow::PersonBottom, 0.0, |m| {
                    limb(m, knee, ankle, Vec2::new(0.063, 0.06) * g, Vec2::new(0.055, 0.052) * g, bottom(0.9))
                });
            }
        }
        // Foot: flat on the ground under the ankle, pointing forward (the
        // toe lifts with the swinging leg a little).
        let toe_lift = (p.hip[i] - p.knee[i]).min(0.0) * -0.15;
        let heel = ankle + Vec3::new(-0.06, -foot_h, 0.0);
        let toe = ankle + Vec3::new(0.17, -foot_h + toe_lift, 0.0);
        let mid = (heel + toe) * 0.5 + Vec3::Y * (foot_h * 0.5);
        limb(
            &mut m,
            mid + Vec3::new(-0.115, 0.0, 0.0),
            mid + Vec3::new(0.115, 0.0, 0.0),
            Vec2::new(foot_h * 0.5, 0.05 * g),
            Vec2::new(foot_h * 0.45, 0.048 * g),
            shoe,
        );
    }
    // Upper body, leaning a little forward from the hips.
    let lean = Vec3::new(b.lean.sin(), b.lean.cos(), 0.0);
    let waist = hip + Vec3::Y * 0.04;
    let chest = waist + lean * 0.5;
    let neck = waist + lean * 0.56;
    // Hips (the trousers' colour, a skirt flaring out over the thighs).
    if b.skirt {
        m.glow(Glow::PersonBottom, 0.0, |m| {
            limb(
                m,
                waist + Vec3::Y * 0.06,
                hip - Vec3::Y * 0.36,
                Vec2::new(0.12, 0.17) * g,
                Vec2::new(0.2, 0.23) * g,
                bottom(0.98),
            )
        });
    } else {
        m.glow(Glow::PersonBottom, 0.0, |m| {
            limb(
                m,
                waist + Vec3::Y * 0.04,
                hip - Vec3::Y * 0.1,
                Vec2::new(0.11, 0.165) * g,
                Vec2::new(0.11, 0.17) * g,
                bottom(1.0),
            )
        });
    }
    // Torso (a coat reaches down over the hips).
    let torso_lo = if b.coat { hip - Vec3::Y * 0.3 } else { waist - Vec3::Y * 0.02 };
    m.glow(Glow::PersonTop, 0.0, |m| {
        limb(
            m,
            chest,
            torso_lo,
            Vec2::new(0.115, 0.2) * g,
            Vec2::new(if b.coat { 0.14 } else { 0.11 }, if b.coat { 0.19 } else { 0.165 }) * g,
            white(1.0),
        )
    });
    if !far {
        // Belt.
        if !b.coat && !b.skirt {
            limb(
                &mut m,
                waist + Vec3::Y * 0.03,
                waist - Vec3::Y * 0.02,
                Vec2::new(0.117, 0.172) * g,
                Vec2::new(0.117, 0.172) * g,
                palette::PERSON_BELT,
            );
        }
        // Shoulders: rounded off with a narrower block on top.
        m.glow(Glow::PersonTop, 0.0, |m| {
            limb(
                m,
                neck,
                chest - Vec3::Y * 0.02,
                Vec2::new(0.09, 0.16) * g,
                Vec2::new(0.115, 0.2) * g,
                white(0.97),
            )
        });
        // Arms.
        for (i, zs) in [(0usize, 1.0f32), (1, -1.0)] {
            let sh = chest + Vec3::new(0.0, -0.03, zs * 0.235 * g);
            let elbow = down(sh, p.shoulder[i] + b.lean, 0.28);
            let wrist = down(elbow, p.shoulder[i] + b.lean + p.elbow[i], 0.25);
            m.glow(Glow::PersonTop, 0.0, |m| {
                limb(m, sh, elbow, Vec2::new(0.052, 0.05) * g, Vec2::new(0.045, 0.044) * g, white(0.95))
            });
            if b.sleeves {
                m.glow(Glow::PersonTop, 0.0, |m| {
                    limb(m, elbow, wrist, Vec2::new(0.045, 0.043) * g, Vec2::new(0.04, 0.038) * g, white(0.93))
                });
            } else {
                m.glow(Glow::PersonSkin, 0.0, |m| {
                    limb(m, elbow, wrist, Vec2::new(0.038, 0.036) * g, Vec2::new(0.033, 0.032) * g, skin)
                });
            }
            let hand = down(wrist, p.shoulder[i] + b.lean + p.elbow[i], 0.08);
            m.glow(Glow::PersonSkin, 0.0, |m| {
                limb(m, wrist, hand, Vec2::new(0.035, 0.025), Vec2::new(0.03, 0.022), skin)
            });
        }
        // A shoulder bag on the left, its strap across the chest.
        if b.bag {
            let side = chest + Vec3::new(-0.02, -0.38, -0.25 * g);
            block(
                &mut m,
                side + Vec3::new(-0.11, -0.1, -0.035),
                side + Vec3::new(0.11, 0.1, 0.035),
                palette::PERSON_BAG,
            );
            limb(
                &mut m,
                chest + Vec3::new(0.118 * g, 0.0, 0.17 * g),
                side + Vec3::new(0.0, 0.1, 0.0),
                Vec2::new(0.006, 0.02),
                Vec2::new(0.006, 0.02),
                palette::PERSON_BAG,
            );
        }
    }
    // Neck and head.
    let hs = b.head;
    let head_c = neck + Vec3::Y * (0.06 + 0.115 * hs);
    m.glow(Glow::PersonSkin, 0.0, |m| {
        limb(m, neck + Vec3::Y * 0.07, neck - Vec3::Y * 0.02, Vec2::splat(0.045), Vec2::splat(0.05), skin)
    });
    let hx = 0.1 * hs;
    let hz = 0.088 * hs;
    let hy = 0.115 * hs;
    m.glow(Glow::PersonSkin, 0.0, |m| {
        // A slightly narrower chin.
        limb(
            m,
            head_c + Vec3::Y * hy,
            head_c - Vec3::Y * hy,
            Vec2::new(hx, hz),
            Vec2::new(hx * 0.82, hz * 0.8),
            skin,
        )
    });
    if !far {
        // Nose, so the head shows which way it faces.
        m.glow(Glow::PersonSkin, 0.0, |m| {
            block(
                m,
                head_c + Vec3::new(hx - 0.005, -0.04 * hs, -0.017),
                head_c + Vec3::new(hx + 0.03 * hs, 0.01 * hs, 0.017),
                white(0.92),
            )
        });
    }
    // Hair: a cap over the top and back of the head.
    let hair_c = head_c + Vec3::new(-0.012, 0.0, 0.0);
    m.glow(Glow::PersonHair, 0.0, |m| match b.hair {
        2 => block(
            m,
            hair_c + Vec3::new(-hx - 0.012, hy * 0.55, -hz - 0.012),
            hair_c + Vec3::new(hx * 0.8, hy + 0.018, hz + 0.012),
            white(1.0),
        ),
        _ => {
            block(
                m,
                hair_c + Vec3::new(-hx - 0.018, hy * 0.35, -hz - 0.016),
                hair_c + Vec3::new(hx + 0.008, hy + 0.035, hz + 0.016),
                white(1.0),
            );
            // Back of the head (long hair falls to the shoulders).
            let low = if b.hair == 1 { -hy - 0.13 } else { -hy * 0.45 };
            block(
                m,
                hair_c + Vec3::new(-hx - 0.03, low, -hz - 0.014),
                hair_c + Vec3::new(-hx * 0.25, hy * 0.5, hz + 0.014),
                white(0.94),
            );
            if b.hair == 3 && !far {
                block(
                    m,
                    hair_c + Vec3::new(-hx - 0.085, hy * 0.3, -0.05),
                    hair_c + Vec3::new(-hx - 0.01, hy * 0.95, 0.05),
                    white(0.97),
                );
            }
        }
    });
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn figures_stand_on_the_ground_and_are_small() {
        for build in 0..BUILDS {
            for pose in 0..POSES {
                let m = person_model(build, pose, false);
                let lo = m.pos.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
                let hi = m.pos.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
                assert!(lo.abs() < 0.03, "build {build} pose {pose} floats or sinks: {lo}");
                assert!((1.6..1.95).contains(&hi), "build {build} pose {pose} height {hi}");
                assert!(m.triangle_count() < 400, "{}", m.triangle_count());
                let far = person_model(build, pose, true);
                assert!(far.triangle_count() * 2 < m.triangle_count());
            }
        }
    }

    #[test]
    fn walk_poses_differ_and_the_cycle_is_symmetric() {
        let a = pose_of(0);
        let b = pose_of(WALK_POSES / 2);
        assert!((a.hip[0] + b.hip[0]).abs() < 1e-5 && (a.hip[0] - b.hip[1]).abs() < 1e-5);
        assert_ne!(pose_of(1), pose_of(3));
    }

    #[test]
    fn tags_round_trip_through_the_bit_layout() {
        let t = person_tag(TOP_COLOURS - 1, BOTTOM_COLOURS - 1, SKIN_TONES - 1, HAIR_COLOURS - 1);
        assert_eq!(t & 1, 0);
        assert_eq!((t >> 1) & 15, TOP_COLOURS - 1);
        assert_eq!((t >> 5) & 7, BOTTOM_COLOURS - 1);
        assert_eq!((t >> 8) & 7, SKIN_TONES - 1);
        assert_eq!((t >> 11) & 7, HAIR_COLOURS - 1);
    }
}
