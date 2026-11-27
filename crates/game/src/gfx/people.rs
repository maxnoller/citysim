//! People on foot: one entity per person the sim walks on the map
//! (`world.walking.walkers`: a leader and the children with them), drawn
//! with the shared figure models of `person_gen`.
//!
//! Like the cars (`vehicles.rs`), poses are sampled once per sim tick and
//! interpolated between ticks, one fixed update behind, so people move
//! evenly at every speed. The walk cycle follows the metres walked (no
//! sliding feet); people waiting at a kerb stand. Children walk beside the
//! adult. Up close the detailed model, from `LOD_FAR` the simple one, beyond
//! `CUT_OFF_M` nobody is drawn; at the door people step in and out (hidden
//! in the last half metre, on the lot's ground on the door path).

use std::collections::HashMap;

use bevy::mesh::MeshTag;
use bevy::prelude::*;
use citysim_sim::walk::{WalkState, Walker};
use citysim_sim::{CitizenId, LifeStage, World};

use super::noise::hash;
use super::person_gen::{self, BUILDS, POSES, STAND, WALK_POSES};
use super::road_geom::{ROAD_Y, WALK_Y};
use super::scene::WorldMaterials;
use crate::shared::{GameSet, SimWorld, WorldCamera};

/// Camera distance beyond which people use the simple model, and beyond
/// which they are not drawn at all (with a little hysteresis).
const LOD_FAR: f32 = 130.0;
const LOD_NEAR: f32 = 120.0;
const CUT_OFF_M: f32 = 420.0;
/// Hidden this close to the door (stepping in or out: about the lot's
/// front edge, `balance::DOOR_INSET_M` from the door).
const DOOR_HIDE_M: f32 = 1.45;
/// Over this distance from the door the ground rises from the lot to the
/// pavement.
const DOOR_RAMP_M: f32 = 1.6;
/// A person moving further than this in a tick was put somewhere else.
const TELEPORT_M: f32 = 20.0;

/// Figure meshes: `[build][pose]` -> (detailed, distant).
#[derive(Resource)]
pub struct PeopleModels {
    meshes: Vec<(Handle<Mesh>, Handle<Mesh>)>,
}

impl PeopleModels {
    pub fn mesh(&self, build: usize, pose: usize, far: bool) -> Handle<Mesh> {
        let (near, distant) = &self.meshes[(build % BUILDS) * POSES + pose % POSES];
        if far { distant.clone() } else { near.clone() }
    }

    pub fn count(&self) -> usize {
        self.meshes.len()
    }
}

/// One drawn person.
#[derive(Component)]
pub struct PersonVisual {
    /// The walker (its leader) and the place in its party (0: the leader).
    pub leader: CitizenId,
    pub slot: u8,
    /// (ground point, heading, metres walked, height) at the previous and
    /// the latest sim tick.
    prev: (Vec2, f32, f32, f32),
    cur: (Vec2, f32, f32, f32),
    standing: bool,
    hidden: bool,
    build: usize,
    scale: f32,
    /// What the entity shows: (pose, far), visible.
    shown: Option<(usize, bool)>,
    far: bool,
    /// Drawn pose this frame (for the selection ring and the camera).
    pub drawn: Vec3,
}

/// Where a walker's person `slot` stands this tick: ground point, heading,
/// metres walked, height of the ground; hidden in the doorway.
fn sample(w: &Walker, slot: u8) -> ((Vec2, f32, f32, f32), bool, bool) {
    let p = Vec2::from(w.pos);
    let dir = Vec2::from_angle(w.heading);
    // Children beside the adult, on the side away from the road, a little
    // behind.
    let p = if slot > 0 {
        p - dir.perp() * (0.55 * slot as f32) - dir * 0.3
    } else {
        p
    };
    let (done, left) = (w.done_m(), w.left_m());
    let on_zebra = w.crossing().is_some() && w.state != WalkState::Waiting;
    let ground = if on_zebra {
        ROAD_Y
    } else {
        WALK_Y * (done.min(left) / DOOR_RAMP_M).clamp(0.0, 1.0)
    };
    let hidden = done < DOOR_HIDE_M || left < DOOR_HIDE_M;
    let standing = w.state == WalkState::Waiting;
    ((p, w.heading, w.walked_m, ground), standing, hidden)
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    a + d * t
}

#[derive(Default)]
struct SyncState {
    entities: HashMap<(CitizenId, u8), Entity>,
    last_fixed: f64,
    last_tick: u64,
    generation: u64,
    moving: bool,
    span: u32,
}

fn stage_of(world: &World, id: CitizenId) -> LifeStage {
    world.citizens.get(id).map_or(LifeStage::Adult, |c| c.stage)
}

fn seed_of(world: &World, id: CitizenId, fallback: u32) -> u32 {
    world.citizens.get(id).map_or(fallback, |c| c.seed)
}

#[allow(clippy::too_many_arguments)]
fn sync_people(
    mut commands: Commands,
    sim: Res<SimWorld>,
    fixed: Res<Time<Fixed>>,
    models: Res<PeopleModels>,
    mats: Res<WorldMaterials>,
    mut state: Local<SyncState>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&mut PersonVisual, &mut Transform, &mut Mesh3d, &mut Visibility)>,
) {
    let cam = cams.single().map_or(Vec3::ZERO, |c| c.translation());
    let world = &sim.world;
    if state.generation != sim.generation {
        for (_, e) in state.entities.drain() {
            commands.entity(e).despawn();
        }
        state.generation = sim.generation;
    }
    let walkers = &world.walking.walkers;
    state.entities.retain(|(id, slot), e| {
        let keep = walkers.get(id).is_some_and(|w| (*slot as usize) <= w.party.len());
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    let fixed_now = fixed.elapsed_secs_f64();
    let stepped = fixed_now != state.last_fixed;
    let tick = world.time.tick;
    let advanced = tick != state.last_tick;
    if stepped {
        let period = fixed.timestep().as_secs_f64().max(1e-6);
        state.span = (((fixed_now - state.last_fixed) / period).round() as u32).clamp(1, 16);
        state.moving = advanced;
    }
    state.last_fixed = fixed_now;
    state.last_tick = tick;
    let alpha = if state.moving {
        let span = state.span.max(1) as f32;
        ((span - 1.0 + fixed.overstep_fraction().clamp(0.0, 1.0)) / span).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let tick_now = stepped && advanced;
    for (&id, w) in walkers.iter() {
        for slot in 0..=w.party.len().min(4) as u8 {
            match state.entities.get(&(id, slot)) {
                Some(&e) => {
                    let Ok((mut vis, _, _, _)) = q.get_mut(e) else { continue };
                    if tick_now {
                        let (now, standing, hidden) = sample(w, slot);
                        vis.prev = vis.cur;
                        vis.cur = now;
                        vis.standing = standing;
                        vis.hidden = hidden;
                        if vis.prev.0.distance(vis.cur.0) > TELEPORT_M {
                            vis.prev = vis.cur;
                        }
                    } else if stepped {
                        vis.prev = vis.cur;
                    }
                }
                None => {
                    let person = if slot == 0 { id } else { w.party[slot as usize - 1] };
                    let (build, scale, tag) =
                        looks(seed_of(world, person, w.seed ^ slot as u32), stage_of(world, person));
                    let (now, standing, hidden) = sample(w, slot);
                    let e = commands
                        .spawn((
                            PersonVisual {
                                leader: id,
                                slot,
                                prev: now,
                                cur: now,
                                standing,
                                hidden,
                                build,
                                scale,
                                shown: None,
                                far: false,
                                drawn: Vec3::new(now.0.x, now.3, now.0.y),
                            },
                            Mesh3d(models.mesh(build, STAND, false)),
                            MeshMaterial3d(mats.lit.clone()),
                            MeshTag::new(tag),
                            Transform::from_xyz(now.0.x, now.3, now.0.y),
                            Visibility::Hidden,
                        ))
                        .id();
                    state.entities.insert((id, slot), e);
                }
            }
        }
    }
    for (mut vis, mut t, mut mesh, mut visibility) in &mut q {
        let p = vis.prev.0.lerp(vis.cur.0, alpha);
        let h = lerp_angle(vis.prev.1, vis.cur.1, alpha);
        let walked = vis.prev.2 + (vis.cur.2 - vis.prev.2) * alpha;
        let y = vis.prev.3 + (vis.cur.3 - vis.prev.3) * alpha;
        let pos = Vec3::new(p.x, y, p.y);
        vis.drawn = pos;
        let d = cam.distance(pos);
        let show = !vis.hidden && d < CUT_OFF_M;
        visibility.set_if_neq(if show { Visibility::Inherited } else { Visibility::Hidden });
        if !show {
            continue;
        }
        let new = Transform::from_translation(pos)
            .with_rotation(Quat::from_rotation_y(-h))
            .with_scale(Vec3::splat(vis.scale));
        if *t != new {
            *t = new;
        }
        let far = if vis.far { d > LOD_NEAR } else { d > LOD_FAR };
        vis.far = far;
        // The walk cycle by the metres walked (shorter legs, shorter
        // strides); standing at the kerb.
        let pose = if vis.standing || (vis.cur.2 - vis.prev.2).abs() < 1e-4 && state.moving {
            STAND
        } else {
            let stride = person_gen::STRIDE_M * vis.scale;
            let phase = (walked / stride + vis.slot as f32 * 0.37).rem_euclid(1.0);
            ((phase * WALK_POSES as f32) as usize).min(WALK_POSES - 1)
        };
        // Far away the walk cycle is not worth a mesh swap every few frames.
        let pose = if far && pose != STAND { pose / 2 * 2 } else { pose };
        if vis.shown != Some((pose, far)) {
            vis.shown = Some((pose, far));
            mesh.0 = models.mesh(vis.build, pose, far);
        }
    }
}

pub struct PeoplePlugin;

impl Plugin for PeoplePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, sync_people.in_set(GameSet::Render));
        if let Some(at) = std::env::var("CITYSIM_GFX_PEOPLE_SHOWCASE").ok().and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() >= 2).then(|| Vec2::new(v[0], v[1]))
        }) {
            app.add_systems(
                PostStartup,
                move |c: Commands, m: Res<PeopleModels>, w: Res<WorldMaterials>| showcase(c, m, w, at),
            );
        }
    }
}

/// What a person looks like, from their seed and stage of life: body
/// type, height scale and colours (`MeshTag`).
pub fn looks(seed: u32, stage: LifeStage) -> (usize, f32, u32) {
    let h = hash(seed ^ 0x9E0_1E5);
    let (build, scale) = match stage {
        LifeStage::Child => (person_gen::CHILD_BUILD, 0.56 + 0.14 * ((h >> 20) % 100) as f32 / 100.0),
        LifeStage::Teen => ((h % 6) as usize, 0.88 + 0.1 * ((h >> 20) % 100) as f32 / 100.0),
        LifeStage::Senior => (
            if h % 3 == 0 { (h >> 3) as usize % 6 } else { person_gen::SENIOR_BUILD },
            0.92 + 0.1 * ((h >> 20) % 100) as f32 / 100.0,
        ),
        LifeStage::Adult => ((h % 6) as usize, 0.93 + 0.14 * ((h >> 20) % 100) as f32 / 100.0),
    };
    let hair = if stage == LifeStage::Senior {
        6 + (h >> 13) % 2
    } else {
        (h >> 13) % 6
    };
    let tag = person_gen::person_tag(h >> 1, h >> 5, (h >> 9) % person_gen::SKIN_TONES, hair);
    (build, scale, tag)
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut handles = Vec::with_capacity(BUILDS * POSES);
    for build in 0..BUILDS {
        for pose in 0..POSES {
            handles.push((
                meshes.add(person_gen::person_model(build, pose, false).build()),
                meshes.add(person_gen::person_model(build, pose, true).build()),
            ));
        }
    }
    commands.insert_resource(PeopleModels { meshes: handles });
}

/// Dev aid (`CITYSIM_GFX_PEOPLE_SHOWCASE=x,z`): every body type in rows at
/// sim point `(x, z)`, standing and at several phases of the walk, in
/// varied colours and sizes (children and the old at their heights).
fn showcase(mut commands: Commands, models: Res<PeopleModels>, mats: Res<WorldMaterials>, at: Vec2) {
    let poses = [person_gen::STAND, 0, 1, 2, 3, 5, 6];
    for build in 0..BUILDS {
        for (k, &pose) in poses.iter().enumerate() {
            let n = (build * poses.len() + k) as u32;
            let h = super::noise::hash(n.wrapping_mul(0x9E37_79B9) ^ 0x5EED);
            let scale = match build {
                person_gen::CHILD_BUILD => 0.62 + 0.08 * (k % 3) as f32,
                person_gen::SENIOR_BUILD => 0.95,
                _ => 0.94 + 0.12 * ((h >> 20) % 100) as f32 / 100.0,
            };
            let hair = if build == person_gen::SENIOR_BUILD {
                6 + (h >> 13) % 2
            } else {
                (h >> 13) % 6
            };
            let tag = person_gen::person_tag(h % 16, (h >> 4) % 8, (h >> 8) % 6, hair);
            let p = at + Vec2::new(k as f32 * 1.5, build as f32 * 2.0);
            commands.spawn((
                Mesh3d(models.mesh(build, pose, false)),
                MeshMaterial3d(mats.lit.clone()),
                MeshTag::new(tag),
                Transform::from_xyz(p.x, WALK_Y, p.y)
                    .with_rotation(Quat::from_rotation_y(0.0))
                    .with_scale(Vec3::splat(scale)),
            ));
        }
    }
}
