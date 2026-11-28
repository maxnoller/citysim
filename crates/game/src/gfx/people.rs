//! People on foot: one entity per person the sim walks on the map
//! (`world.walking.walkers`: a leader and the children with them), drawn
//! with the shared figure models of `person_gen`.
//!
//! Like the cars (`vehicles.rs`), poses are sampled after every fixed
//! update that moved the sim and drawn at the cars' drawn moment
//! (`vehicles::DrawClock`, one update behind with real time), so people move
//! evenly at every speed and whatever the frame times, and stop without a
//! jump on pause. Where the people the interface points at are drawn goes
//! to `shared::DrawnPoses::walkers` (the camera, the leader line, the
//! route's start). The walk cycle follows the metres walked (no
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
use super::vehicles::{DrawClock, DrawnSet, sample_vehicles};
use crate::shared::{
    CameraFollow, DrawnPoses, FollowOn, RouteView, SimStep, SimWorld, WorldCamera, WorldHighlight, WorldTarget,
};

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
}

/// One drawn person.
#[derive(Component)]
pub struct PersonVisual {
    /// The walker (its leader) and the place in its party (0: the leader).
    pub leader: CitizenId,
    pub slot: u8,
    /// (ground point, heading, metres walked, height) and (standing,
    /// hidden) at the last three samples (`DrawClock`), newest last.
    poses: [Sample; 3],
    build: usize,
    scale: f32,
    /// What the entity shows: (pose, far), visible.
    shown: Option<(usize, bool)>,
    far: bool,
    /// Drawn pose this frame (for the selection ring and the camera).
    pub drawn: Vec3,
}

/// A person's state at one sample.
type Sample = ((Vec2, f32, f32, f32), bool, bool);

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
    // People waiting at a kerb stand back from it a little, each their own
    // distance (not all on the corner's one point).
    let p = if standing {
        p - dir * (0.15 + 1.3 * (hash(w.seed ^ 0x51DE) % 100) as f32 / 100.0)
    } else {
        p
    };
    ((p, w.heading, w.walked_m, ground), standing, hidden)
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    a + d * t
}

struct SyncState {
    timing: bool,
    spent: f64,
    frames: u32,
    spent_since: std::time::Instant,
    generation: u64,
}

impl Default for SyncState {
    fn default() -> Self {
        SyncState {
            timing: std::env::var("CITYSIM_PEOPLE_TIMING").is_ok(),
            spent: 0.0,
            frames: 0,
            spent_since: std::time::Instant::now(),
            generation: 0,
        }
    }
}

fn stage_of(world: &World, id: CitizenId) -> LifeStage {
    world.citizens.get(id).map_or(LifeStage::Adult, |c| c.stage)
}

fn seed_of(world: &World, id: CitizenId, fallback: u32) -> u32 {
    world.citizens.get(id).map_or(fallback, |c| c.seed)
}

/// The entity drawing each person on foot: (walker, place in the party).
#[derive(Resource, Default)]
pub struct PeopleEntities(pub HashMap<(CitizenId, u8), Entity>);

/// After every fixed update that moved the sim (`sample_vehicles` counts
/// them): a new sample of every drawn person.
fn sample_people(
    sim: Res<SimWorld>,
    clock: Res<DrawClock>,
    entities: Res<PeopleEntities>,
    mut q: Query<&mut PersonVisual>,
    mut last: Local<u64>,
) {
    if clock.newest() == *last || clock.generation() != sim.generation {
        *last = clock.newest();
        return;
    }
    *last = clock.newest();
    let walkers = &sim.world.walking.walkers;
    for (&(id, slot), &e) in &entities.0 {
        if let Ok(mut vis) = q.get_mut(e)
            && let Some(w) = walkers.get(&id)
        {
            let now = sample(w, slot);
            vis.poses = if vis.poses[2].0.0.distance(now.0.0) > TELEPORT_M {
                [now; 3]
            } else {
                [vis.poses[1], vis.poses[2], now]
            };
        }
    }
}

/// Interpolate two samples at `t`.
fn mix(a: &Sample, b: &Sample, t: f32) -> (Vec2, f32, f32, f32) {
    let (a, b) = (a.0, b.0);
    (
        a.0.lerp(b.0, t),
        lerp_angle(a.1, b.1, t),
        a.2 + (b.2 - a.2) * t,
        a.3 + (b.3 - a.3) * t,
    )
}

#[allow(clippy::too_many_arguments)]
fn sync_people(
    mut commands: Commands,
    sim: Res<SimWorld>,
    clock: Res<DrawClock>,
    models: Res<PeopleModels>,
    mats: Res<WorldMaterials>,
    mut state: Local<SyncState>,
    mut entities: ResMut<PeopleEntities>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&mut PersonVisual, &mut Transform, &mut Mesh3d, &mut Visibility)>,
) {
    let started = std::time::Instant::now();
    let cam = cams.single().map_or(Vec3::ZERO, |c| c.translation());
    let world = &sim.world;
    if state.generation != sim.generation {
        for (_, e) in entities.0.drain() {
            commands.entity(e).despawn();
        }
        state.generation = sim.generation;
    }
    let walkers = &world.walking.walkers;
    entities.0.retain(|(id, slot), e| {
        let keep = walkers.get(id).is_some_and(|w| (*slot as usize) <= w.party.len());
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    // The drawn moment: samples after the oldest kept (0..=2).
    let k = clock.k();
    for (&id, w) in walkers.iter() {
        for slot in 0..=w.party.len().min(4) as u8 {
            if entities.0.contains_key(&(id, slot)) {
                continue;
            }
            let person = if slot == 0 { id } else { w.party[slot as usize - 1] };
            let (build, scale, tag) = looks(seed_of(world, person, w.seed ^ slot as u32), stage_of(world, person));
            let now = sample(w, slot);
            let e = commands
                .spawn((
                    PersonVisual {
                        leader: id,
                        slot,
                        poses: [now; 3],
                        build,
                        scale,
                        shown: None,
                        far: false,
                        drawn: Vec3::new(now.0.0.x, now.0.3, now.0.0.y),
                    },
                    Mesh3d(models.mesh(build, STAND, false)),
                    MeshMaterial3d(mats.lit.clone()),
                    MeshTag::new(tag),
                    Transform::from_xyz(now.0.0.x, now.0.3, now.0.0.y),
                    Visibility::Hidden,
                ))
                .id();
            entities.0.insert((id, slot), e);
        }
    }
    for (mut vis, mut t, mut mesh, mut visibility) in &mut q {
        let (from, to, f) = if k <= 1.0 { (0, 1, k) } else { (1, 2, k - 1.0) };
        let (p, h, walked, y) = mix(&vis.poses[from], &vis.poses[to], f);
        // Standing or in the doorway as at the newer of the two samples.
        let (_, standing, hidden) = vis.poses[to];
        let moved = (vis.poses[to].0.2 - vis.poses[from].0.2).abs() >= 1e-4;
        let pos = Vec3::new(p.x, y, p.y);
        vis.drawn = pos;
        let d = cam.distance(pos);
        let show = !hidden && d < CUT_OFF_M;
        visibility.set_if_neq(if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
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
        let pose = if standing || !moved && clock.moving() {
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
    // `CITYSIM_PEOPLE_TIMING=1`: this system's CPU time, logged every 5 s.
    if state.timing {
        state.spent += started.elapsed().as_secs_f64();
        state.frames += 1;
        if state.spent_since.elapsed().as_secs_f32() > 5.0 {
            info!(
                "people: {} walkers, {} figures, sync {:.0} us a frame",
                walkers.len(),
                entities.0.len(),
                state.spent / state.frames.max(1) as f64 * 1e6
            );
            state.spent = 0.0;
            state.frames = 0;
            state.spent_since = std::time::Instant::now();
        }
    }
}

/// Where the people on foot the interface points at are drawn this frame
/// (`DrawnPoses::walkers`).
fn publish_walkers(
    sim: Res<SimWorld>,
    hl: Res<WorldHighlight>,
    follow: Res<CameraFollow>,
    route: Res<RouteView>,
    entities: Res<PeopleEntities>,
    q: Query<&PersonVisual>,
    mut drawn: ResMut<DrawnPoses>,
) {
    let mut ids: Vec<CitizenId> = Vec::with_capacity(4);
    for t in [&hl.selected, &hl.hovered] {
        if let Some(WorldTarget::Walker(c)) = t {
            ids.push(*c);
        }
    }
    if let FollowOn::Walker(c) = follow.on {
        ids.push(c);
    }
    ids.extend(route.walker);
    let mut out = Vec::with_capacity(ids.len());
    for c in ids {
        if out.iter().any(|(x, _)| *x == c) {
            continue;
        }
        let Some((leader, w)) = citysim_sim::walk::walker_of(&sim.world, c) else {
            continue;
        };
        let slot = if c == leader {
            0
        } else {
            w.party.iter().position(|&p| p == c).map_or(0, |i| i as u8 + 1)
        };
        if let Some(v) = entities.0.get(&(leader, slot)).and_then(|e| q.get(*e).ok()) {
            out.push((c, [v.drawn.x, v.drawn.z]));
        }
    }
    if drawn.walkers != out {
        drawn.walkers = out;
    }
}

pub struct PeoplePlugin;

impl Plugin for PeoplePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PeopleEntities>()
            .add_systems(Startup, setup)
            .add_systems(FixedUpdate, sample_people.after(SimStep).after(sample_vehicles))
            .add_systems(Update, (sync_people, publish_walkers).chain().in_set(DrawnSet::People));
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
            if h.is_multiple_of(3) {
                (h >> 3) as usize % 6
            } else {
                person_gen::SENIOR_BUILD
            },
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
