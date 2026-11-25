//! Junction control as the player sees it: traffic lights at every
//! signalled junction (a pole at each approach's stop line, a mast arm over
//! avenues) whose heads show exactly the aspect the sim reports, and
//! give-way markings and signs on the yielding arms of priority junctions.
//!
//! Static furniture (poles, housings, signs, shark teeth) is one mesh per
//! junction, rebuilt only when that junction's control or geometry changes.
//! Each signal head has one small entity for its lit lens, which swaps mesh
//! and material when the aspect changes (checked ten times a second).

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::junction::{self, JunctionControl};
use citysim_sim::{Aspect, NodeId, SegmentId};

use super::city_material::{CityMaterial, city_material};
use super::mesh_util::{Glow, MeshBuilder};
use super::palette::{self, lin};
use super::road_geom::{self, MARK_Y, NodeLayout, WALK_Y};
use super::scene::WorldMaterials;
use crate::shared::{GameSet, SimWorld};

/// Height of a pole-mounted head's centre and of a mast arm's head.
const POLE_HEAD_Y: f32 = WALK_Y + 2.9;
const MAST_HEAD_Y: f32 = WALK_Y + 5.6;
/// Distance between lens centres in a head.
const LENS_STEP: f32 = 0.3;

/// Lit-lens meshes and materials per aspect (red, yellow, green).
#[derive(Resource)]
struct SignalAssets {
    lens: [Handle<Mesh>; 3],
    mats: [Handle<CityMaterial>; 3],
}

fn aspect_index(a: Aspect) -> usize {
    match a {
        Aspect::Red => 0,
        Aspect::Yellow => 1,
        Aspect::Green => 2,
    }
}

/// One signal head showing the aspect of `segment`'s approach (`left`:
/// the head over an avenue's left lane, showing the left-turn aspect).
#[derive(Component)]
struct SignalHead {
    node: NodeId,
    segment: SegmentId,
    left: bool,
    shown: Option<usize>,
}

#[derive(Default)]
struct Junctions {
    roads_key: Option<(u64, u64)>,
    layout: HashMap<NodeId, NodeLayout>,
    /// Node -> (content hash, furniture entity, head entities).
    spawned: HashMap<NodeId, (u64, Entity, Vec<Entity>)>,
    since_aspects: f32,
}

pub struct SignalsPlugin;

impl Plugin for SignalsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, (sync_junctions, update_aspects).chain().in_set(GameSet::Render));
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<CityMaterial>>,
    world_mats: Res<WorldMaterials>,
) {
    let colors = [
        Color::srgb(1.0, 0.16, 0.1),
        Color::srgb(1.0, 0.62, 0.08),
        Color::srgb(0.2, 1.0, 0.45),
    ];
    let lens = [0, 1, 2].map(|i| meshes.add(lens_mesh(i).build()));
    let mats = colors.map(|c| {
        mats.add(city_material(
            StandardMaterial {
                base_color: c,
                perceptual_roughness: 0.4,
                ..default()
            },
            world_mats.day.clone(),
        ))
    });
    commands.insert_resource(SignalAssets { lens, mats });
}

/// Lit lens of aspect `i` (0 red top .. 2 green bottom) in the head's local
/// frame (head centre at the origin, facing +z), plus a lit cap on top of
/// the housing so the aspect also reads from above.
fn lens_mesh(i: usize) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    let y = (1.0 - i as f32) * LENS_STEP;
    m.glow(Glow::Always, 0.0, |m| {
        m.cuboid(
            Vec3::new(-0.09, y - 0.09, 0.125),
            Vec3::new(0.09, y + 0.09, 0.17),
            Color::WHITE,
        );
        m.quad_up(
            [
                Vec3::new(-0.12, 0.53, -0.08),
                Vec3::new(0.12, 0.53, -0.08),
                Vec3::new(0.12, 0.53, 0.1),
                Vec3::new(-0.12, 0.53, 0.1),
            ],
            [1.0; 4],
        );
    });
    m
}

/// Dark housing with unlit lenses, centred at `c`, facing `face` (unit, xz).
fn housing(m: &mut MeshBuilder, c: Vec3, face: Vec2) {
    let yaw = -face.y.atan2(face.x) + std::f32::consts::FRAC_PI_2;
    let body = Color::srgb_u32(0x2A2E33);
    m.box_yaw(c - Vec3::Y * 0.5, Vec3::new(0.34, 1.02, 0.24), yaw, body, body);
    let rot = Quat::from_rotation_y(yaw);
    let fwd = Vec3::new(face.x, 0.0, face.y);
    for k in 0..3 {
        let y = (1 - k) as f32 * LENS_STEP;
        let p = c + Vec3::Y * y + fwd * 0.121;
        let r = rot * Vec3::X * 0.1;
        m.quad_facing(
            [
                p - r - Vec3::Y * 0.1,
                p + r - Vec3::Y * 0.1,
                p + r + Vec3::Y * 0.1,
                p - r + Vec3::Y * 0.1,
            ],
            fwd,
            Color::srgb_u32(0x15181B),
        );
        // Visor.
        m.box_yaw(
            p + Vec3::Y * 0.1 + fwd * 0.05,
            Vec3::new(0.24, 0.03, 0.12),
            yaw,
            body,
            body,
        );
    }
}

/// Transform of a head entity at `c` facing `face`.
fn head_transform(c: Vec3, face: Vec2) -> Transform {
    Transform::from_translation(c).with_rotation(Quat::from_rotation_y(
        -face.y.atan2(face.x) + std::f32::consts::FRAC_PI_2,
    ))
}

/// Upward triangle helper in the xz plane.
fn tri2(m: &mut MeshBuilder, a: Vec2, b: Vec2, c: Vec2, y: f32, col: [f32; 4]) {
    m.tri_up(
        Vec3::new(a.x, y, a.y),
        Vec3::new(b.x, y, b.y),
        Vec3::new(c.x, y, c.y),
        col,
    );
}

/// Static furniture of one junction, and where its heads go:
/// (segment, centre, facing, left-lane head).
#[allow(clippy::type_complexity)]
fn furniture(
    node: &NodeLayout,
    control: JunctionControl,
    major: &HashMap<SegmentId, bool>,
) -> (MeshBuilder, Vec<(SegmentId, Vec3, Vec2, bool)>) {
    let mut m = MeshBuilder::new();
    let mut heads = Vec::new();
    let pole = palette::LAMP;
    for arm in &node.arms {
        let st = arm.style;
        if st.highway {
            continue;
        }
        // Travel towards the node is -dir; its right is (dir.y, -dir.x)
        // in sim space (traffic drives on the right).
        let right = Vec2::new(arm.dir.y, -arm.dir.x);
        // The sim's stop line, and the pole just behind it on the
        // sidewalk (beyond the kerb corner).
        let stop = if arm.stop > 0.0 { arm.stop } else { arm.setback };
        let base = arm.center + arm.dir * ((stop + 0.9).max(arm.setback + 0.4) - arm.setback) + right * (st.h + 0.55);
        let white = lin(palette::MARKING);
        let lane_lo = st.median + 0.15;
        let lane_hi = st.h - 0.25;
        let across = |m: &mut MeshBuilder, t0: f32, t1: f32, o0: f32, o1: f32| {
            let q = [
                arm.center + arm.dir * (t0 - arm.setback) + right * o0,
                arm.center + arm.dir * (t1 - arm.setback) + right * o0,
                arm.center + arm.dir * (t1 - arm.setback) + right * o1,
                arm.center + arm.dir * (t0 - arm.setback) + right * o1,
            ];
            m.quad_up(q.map(|p| Vec3::new(p.x, MARK_Y + 0.004, p.y)), white);
        };
        match control {
            JunctionControl::Signal => {
                // Solid stop line across the arriving lanes.
                across(&mut m, stop, stop + 0.4, lane_lo, lane_hi);
                let foot = Vec3::new(base.x, WALK_Y, base.y);
                let avenue = st.median > 0.0;
                let top = if avenue { MAST_HEAD_Y + 0.7 } else { POLE_HEAD_Y + 0.65 };
                m.cylinder(foot, 0.09, top - WALK_Y, 6, pole, pole);
                // Faces the traffic arriving along -dir.
                let face = arm.dir;
                let c = Vec3::new(base.x, POLE_HEAD_Y, base.y) + Vec3::new(face.x, 0.0, face.y) * 0.18;
                housing(&mut m, c, face);
                heads.push((arm.seg, c, face, false));
                if avenue {
                    // Mast arm over the approach lanes.
                    let tip = arm.center
                        + arm.dir * ((stop + 0.9).max(arm.setback + 0.4) - arm.setback)
                        + right * (st.h * 0.42);
                    let mid = (base + tip) * 0.5;
                    let d = tip - base;
                    let yaw = -d.y.atan2(d.x);
                    m.box_yaw(
                        Vec3::new(mid.x, MAST_HEAD_Y + 0.55, mid.y),
                        Vec3::new(d.length() + 0.2, 0.14, 0.14),
                        yaw,
                        pole,
                        pole,
                    );
                    let c = Vec3::new(tip.x, MAST_HEAD_Y, tip.y);
                    housing(&mut m, c, face);
                    heads.push((arm.seg, c, face, true));
                }
            }
            JunctionControl::Priority => {
                if major.get(&arm.seg).copied().unwrap_or(true) {
                    continue;
                }
                // Give-way line (dashed) with triangles ("shark teeth")
                // behind it, across the arriving lanes.
                let mut o = lane_lo;
                while o + 0.5 <= lane_hi + 1e-3 {
                    across(&mut m, stop, stop + 0.3, o, o + 0.5);
                    o += 0.9;
                }
                let (lo, hi) = (lane_lo + 0.1, lane_hi - 0.1);
                let mut o = lo;
                while o + 0.45 <= hi + 1e-3 {
                    let a = arm.center + arm.dir * (stop + 0.6 - arm.setback) + right * o;
                    let b = a + right * 0.45;
                    let tip = (a + b) * 0.5 + arm.dir * 0.65;
                    // Point towards the arriving driver.
                    tri2(&mut m, a, b, tip, MARK_Y + 0.004, white);
                    o += 0.7;
                }
                // Give-way sign: red-bordered white triangle, point down.
                let foot = Vec3::new(base.x, WALK_Y, base.y);
                m.cylinder(foot, 0.05, 2.35, 5, palette::GUARDRAIL, palette::GUARDRAIL);
                let face = Vec3::new(arm.dir.x, 0.0, arm.dir.y);
                let side = Vec3::new(right.x, 0.0, right.y);
                let c = foot + Vec3::Y * 2.2 + face * 0.06;
                let (r, w) = (0.42, 0.28);
                let red = Color::srgb_u32(0xC8322B);
                // Front (towards the driver) and back faces.
                let (p0, p1, p2) = (
                    c + Vec3::Y * 0.24 + side * r,
                    c + Vec3::Y * 0.24 - side * r,
                    c - Vec3::Y * 0.48,
                );
                m.glow(Glow::Sign, 0.0, |m| {
                    m.tri_facing(p0, p1, p2, face, red);
                });
                m.tri_facing(p0, p1, p2, -face, palette::GUARDRAIL);
                let (q0, q1, q2) = (
                    c + Vec3::Y * 0.17 + side * w + face * 0.01,
                    c + Vec3::Y * 0.17 - side * w + face * 0.01,
                    c - Vec3::Y * 0.32 + face * 0.01,
                );
                m.tri_facing(q0, q1, q2, face, palette::MARKING);
            }
        }
    }
    (m, heads)
}

#[allow(clippy::too_many_arguments)]
fn sync_junctions(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mut state: Local<Junctions>,
    mut meshes: ResMut<Assets<Mesh>>,
    mats: Res<WorldMaterials>,
    assets: Res<SignalAssets>,
    time: Res<Time<Real>>,
) {
    let world = &sim.world;
    let key = sim.key(world.versions.roads);
    let roads_changed = state.roads_key != Some(key);
    // Control can change without a road edit (player override, the
    // sim's daily auto choice): check a few times a second.
    state.since_aspects += time.delta_secs();
    if !roads_changed && state.since_aspects < 0.25 {
        return;
    }
    state.since_aspects = 0.0;
    let state = &mut *state;
    if roads_changed {
        state.roads_key = Some(key);
        let layout = road_geom::layout(world);
        state.layout = layout.nodes.into_iter().map(|n| (n.id, n)).collect();
    }
    let mut seen: Vec<NodeId> = Vec::new();
    for n in junction::junction_nodes(world) {
        let Some(j) = junction::junction(world, n) else {
            continue;
        };
        let Some(node) = state.layout.get(&n) else { continue };
        let spawned = &mut state.spawned;
        let major: HashMap<SegmentId, bool> = j.arms.iter().map(|a| (a.segment, a.major)).collect();
        let mut h = DefaultHasher::new();
        (j.control == JunctionControl::Signal).hash(&mut h);
        road_geom::hash_node(node, &mut h);
        for a in &j.arms {
            a.major.hash(&mut h);
        }
        let hash = h.finish();
        seen.push(n);
        if spawned.get(&n).is_some_and(|(old, _, _)| *old == hash) {
            continue;
        }
        if let Some((_, e, heads)) = spawned.remove(&n) {
            commands.entity(e).despawn();
            for h in heads {
                commands.entity(h).despawn();
            }
        }
        let (m, head_spots) = furniture(node, j.control, &major);
        let e = commands
            .spawn((
                Name::new("Junction furniture"),
                Mesh3d(meshes.add(m.build())),
                MeshMaterial3d(mats.lit.clone()),
                Transform::default(),
            ))
            .id();
        let heads = head_spots
            .into_iter()
            .map(|(seg, c, face, left)| {
                commands
                    .spawn((
                        SignalHead {
                            node: n,
                            segment: seg,
                            left,
                            shown: Some(0),
                        },
                        Mesh3d(assets.lens[0].clone()),
                        MeshMaterial3d(assets.mats[0].clone()),
                        head_transform(c, face),
                        NotShadowCaster,
                    ))
                    .id()
            })
            .collect();
        spawned.insert(n, (hash, e, heads));
    }
    let gone: Vec<NodeId> = state.spawned.keys().filter(|n| !seen.contains(n)).copied().collect();
    for n in gone {
        if let Some((_, e, heads)) = state.spawned.remove(&n) {
            commands.entity(e).despawn();
            for h in heads {
                commands.entity(h).despawn();
            }
        }
    }
}

/// Show each head's current aspect (main or left-turn), as the sim
/// reports it per approach.
fn update_aspects(
    sim: Res<SimWorld>,
    assets: Res<SignalAssets>,
    mut heads: Query<(&mut SignalHead, &mut Mesh3d, &mut MeshMaterial3d<CityMaterial>)>,
    mut views: Local<HashMap<NodeId, junction::JunctionView>>,
    mut tick: Local<u64>,
) {
    let world = &sim.world;
    if *tick != world.time.tick {
        *tick = world.time.tick;
        views.clear();
    }
    for (mut head, mut mesh, mut mat) in &mut heads {
        let view = views.entry(head.node).or_insert_with(|| {
            junction::junction_view(world, head.node).unwrap_or(junction::JunctionView {
                node: head.node,
                control: JunctionControl::Priority,
                setting: Default::default(),
                approaches: Vec::new(),
                phase: None,
                phase_count: 0,
                peak_per_minute: 0,
            })
        });
        let a = view
            .approaches
            .iter()
            .find(|a| a.segment == head.segment)
            .and_then(|a| if head.left { a.left_aspect } else { a.aspect })
            .map(aspect_index);
        if a == head.shown {
            continue;
        }
        head.shown = a;
        let i = a.unwrap_or(0);
        mesh.0 = assets.lens[i].clone();
        mat.0 = assets.mats[i].clone();
    }
}
