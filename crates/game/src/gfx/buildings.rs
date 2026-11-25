//! Buildings: one entity per `world.buildings` entry, synced when
//! `versions.buildings` changes. Models come from `building_gen`, cached by
//! `(kind, lot size, level, variant)` and shared, all with one material, so
//! thousands of buildings batch into few draws. New buildings grow in with
//! an ease-out; removed ones shrink out before despawning.

use std::collections::HashMap;

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use citysim_sim::{Building, BuildingId, CELL_SIZE};

use super::building_gen::{self, ModelKey, VARIANTS};
use super::land::{self, Inset, Lot, RoadEdges};
use super::scene::WorldMaterials;
use crate::shared::{GameSet, SimWorld};

const GROW_SECS: f32 = 0.75;
const SHRINK_SECS: f32 = 0.35;
/// Buildings younger than this (sim ticks) animate in; older ones (e.g. from
/// a loaded save) just appear.
const ANIMATE_IF_YOUNGER: u64 = 3 * citysim_sim::TICKS_PER_SECOND as u64;

/// New buildings stand as a building site this long (sim ticks): first
/// foundations, then the frame with scaffolding (and a crane when tall).
pub const SITE_TICKS: u64 = 3 * citysim_sim::balance::TICKS_PER_HOUR as u64;

/// Camera distance (m) where buildings cross-fade to their simplified model.
pub const LOD_DISTANCE: std::ops::Range<f32> = 650.0..700.0;

/// Cache of generated building models: `(detailed, distant)` per key.
#[derive(Resource, Default)]
pub struct BuildingModels {
    meshes: HashMap<ModelKey, (Handle<Mesh>, Handle<Mesh>)>,
    emitters: HashMap<ModelKey, Vec<Vec3>>,
    sites: HashMap<(ModelKey, u8), Handle<Mesh>>,
}

impl BuildingModels {
    /// Number of distinct building models (each has two levels of detail).
    pub fn count(&self) -> usize {
        self.meshes.len()
    }

    fn site(&mut self, key: ModelKey, stage: u8, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        // Sites only depend on the lot, the tier and the stage.
        let k = ModelKey {
            variant: key.variant % 4,
            ..key
        };
        self.sites
            .entry((k, stage))
            .or_insert_with(|| meshes.add(building_gen::site(k, stage).build()))
            .clone()
    }

    pub fn get(&mut self, key: ModelKey, meshes: &mut Assets<Mesh>) -> (Handle<Mesh>, Handle<Mesh>) {
        let emitters = &mut self.emitters;
        self.meshes
            .entry(key)
            .or_insert_with(|| {
                let mut detail = building_gen::generate(key, false);
                emitters.insert(key, std::mem::take(&mut detail.emitters));
                (
                    meshes.add(detail.build()),
                    meshes.add(building_gen::generate(key, true).build()),
                )
            })
            .clone()
    }

    /// Chimney tops that smoke, in model space (after `get`).
    pub fn emitters(&self, key: ModelKey) -> &[Vec3] {
        self.emitters.get(&key).map_or(&[], Vec::as_slice)
    }
}

#[derive(Component)]
pub(super) struct BuildingVisual;

/// Drawn building per sim building: (parent entity, model key, model child
/// transform). Highlights draw over these.
#[derive(Resource, Default)]
pub struct BuildingEntities(pub HashMap<BuildingId, (Entity, ModelKey, Transform)>);

/// A building still under construction (its children show the site).
#[derive(Component)]
struct Site {
    id: BuildingId,
    key: ModelKey,
    model_tf: Transform,
    stage: u8,
    /// Sim tick the works started (built, or rebuilt for a level-up).
    start: u64,
}

/// A level-up big enough to rebuild visibly (a site with scaffolding and,
/// for tall buildings, a crane): to the top levels, or any level-up of a
/// large (2x2) lot.
fn rebuild_shows_site(old: ModelKey, new: ModelKey) -> bool {
    new.kind == old.kind
        && new.cells == old.cells
        && new.level > old.level
        && (new.level >= 3 || new.cells.0 >= 2 && new.cells.1 >= 2)
}

fn site_stage(age: u64) -> Option<u8> {
    if age >= SITE_TICKS {
        None
    } else if age < SITE_TICKS / 2 {
        Some(0)
    } else {
        Some(1)
    }
}

#[derive(Component)]
struct Growing {
    start: f32,
}

#[derive(Component)]
pub(super) struct Shrinking {
    start: f32,
}

#[derive(Default)]
struct Spawned {
    key: Option<((u64, u64), (u64, u64))>,
    entities: HashMap<BuildingId, (Entity, ModelKey, [u32; 3], Inset)>,
    /// Roads key the fitted lots were made for.
    edges_key: Option<(u64, u64)>,
    /// Fitted lot per building (and the pose it was fitted for).
    insets: HashMap<BuildingId, ([u32; 3], Inset)>,
    /// Buildings rebuilt for a big level-up: sim tick the works started
    /// (they show a building site again, with a crane when tall).
    rebuilt: HashMap<BuildingId, u64>,
}

/// A building's lot in its own frame (see `land::Lot`).
pub fn lot_of(b: &Building) -> Lot {
    let (w, d) = b.size();
    Lot {
        c: Vec2::from(b.center()),
        f: Vec2::from(b.front_dir()),
        half: Vec2::new(w as f32, d as f32) * CELL_SIZE * 0.5,
    }
}

pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingModels>()
            .init_resource::<BuildingEntities>()
            .add_systems(
                Update,
                (sync_buildings, advance_sites, animate_buildings)
                    .chain()
                    .in_set(GameSet::Render)
                    .after(super::land::LandUpdate),
            );
    }
}

/// Model key and placement for a building.
pub fn model_key(b: &Building) -> ModelKey {
    let (w, d) = b.size();
    ModelKey {
        kind: b.kind,
        cells: (w.clamp(1, 255) as u8, d.clamp(1, 255) as u8),
        level: b.level.max(1),
        variant: (b.seed % VARIANTS) as u8,
    }
}

/// Rotation turning the model's front (+z) towards `rotation` (the sim's
/// front direction, `atan2` in sim axes).
pub fn front_rotation(rotation: f32) -> Quat {
    Quat::from_rotation_y(std::f32::consts::FRAC_PI_2 - rotation)
}

/// Where a building stands (centre and rotation bits), to notice moves.
fn pose_key(b: &Building) -> [u32; 3] {
    let c = b.center();
    [c[0].to_bits(), c[1].to_bits(), b.rotation().to_bits()]
}

/// Placement of a building: (parent transform, model child transform).
pub fn placement(b: &Building, inset: Inset) -> (Transform, Transform) {
    let c = b.center();
    let tf = Transform::from_xyz(c[0], 0.0, c[1]).with_rotation(front_rotation(b.rotation()));
    let model_tf = Transform::from_xyz(inset.shift.x, 0.0, inset.shift.y).with_scale(inset.scale);
    (tf, model_tf)
}

#[allow(clippy::too_many_arguments)]
fn sync_buildings(
    mut commands: Commands,
    sim: Res<SimWorld>,
    time: Res<Time<Real>>,
    mats: Res<WorldMaterials>,
    mut models: ResMut<BuildingModels>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut spawned: Local<Spawned>,
    mut index: ResMut<BuildingEntities>,
    edges: Res<RoadEdges>,
) {
    let key = (sim.key(sim.world.versions.buildings), sim.key(sim.world.versions.roads));
    if spawned.key == Some(key) {
        return;
    }
    let reloaded = spawned.key.is_some_and(|((g, _), _)| g != sim.generation);
    spawned.key = Some(key);
    let sp = &mut *spawned;
    let now = time.elapsed_secs();
    let world = &sim.world;
    let buildings = &world.buildings;
    // Each building's fit changes only with the roads or its own lot.
    if sp.edges_key != edges.key {
        sp.edges_key = edges.key;
        sp.insets.clear();
    }
    let mut insets = std::mem::take(&mut sp.insets);
    insets.retain(|id, _| buildings.contains_key(*id));
    let mut inset_of = |id: BuildingId, b: &Building| {
        let pose = pose_key(b);
        if let Some((k, i)) = insets.get(&id)
            && *k == pose
        {
            return *i;
        }
        let i = land::fit(lot_of(b), &edges);
        insets.insert(id, (pose, i));
        i
    };
    let tick = world.time.tick;
    let mut rebuilt: Vec<BuildingId> = Vec::new();
    sp.entities.retain(|id, (e, k, f, i)| {
        let keep = buildings
            .get(*id)
            .is_some_and(|b| model_key(b) == *k && pose_key(b) == *f && inset_of(*id, b) == *i && !reloaded);
        if !keep
            && !reloaded
            && let Some(b) = buildings.get(*id)
            && rebuild_shows_site(*k, model_key(b))
        {
            rebuilt.push(*id);
        }
        if !keep {
            commands.entity(*e).remove::<Growing>().insert(Shrinking { start: now });
            index.0.remove(id);
        }
        keep
    });
    for id in rebuilt {
        sp.rebuilt.insert(id, tick);
    }
    sp.rebuilt
        .retain(|id, t| buildings.contains_key(*id) && tick.saturating_sub(*t) < SITE_TICKS && !reloaded);
    for (id, b) in buildings.iter() {
        if sp.entities.contains_key(&id) {
            continue;
        }
        let mk = model_key(b);
        let young = world.time.tick.saturating_sub(b.created_tick) < ANIMATE_IF_YOUNGER && !reloaded;
        let inset = inset_of(id, b);
        // The inset lives on the model children, so the grow animation can
        // scale the parent freely.
        let (tf, model_tf) = placement(b, inset);
        let start = sp.rebuilt.get(&id).copied().unwrap_or(b.created_tick);
        let age = world.time.tick.saturating_sub(start);
        if !reloaded && let Some(stage) = site_stage(age) {
            let mesh = models.site(mk, stage, &mut meshes);
            let mut ec = commands.spawn((
                BuildingVisual,
                Visibility::default(),
                Site {
                    id,
                    key: mk,
                    model_tf,
                    stage,
                    start,
                },
                tf,
            ));
            ec.with_children(|p| {
                p.spawn((model_tf, Mesh3d(mesh), MeshMaterial3d(mats.lit.clone())));
            });
            index.0.insert(id, (ec.id(), mk, model_tf));
            sp.entities.insert(id, (ec.id(), mk, pose_key(b), inset));
            continue;
        }
        let (detail, far) = models.get(mk, &mut meshes);
        let mut ec = commands.spawn((BuildingVisual, Visibility::default()));
        ec.with_children(|p| {
            p.spawn((
                model_tf,
                Mesh3d(detail),
                MeshMaterial3d(mats.lit.clone()),
                VisibilityRange {
                    start_margin: 0.0..0.0,
                    end_margin: LOD_DISTANCE,
                    use_aabb: false,
                },
            ));
            p.spawn((
                model_tf,
                Mesh3d(far),
                MeshMaterial3d(mats.lit.clone()),
                VisibilityRange {
                    start_margin: LOD_DISTANCE,
                    end_margin: 1e6..1e6,
                    use_aabb: false,
                },
            ));
        });
        if young {
            ec.insert((tf.with_scale(Vec3::new(0.9, 0.01, 0.9)), Growing { start: now }));
        } else {
            ec.insert(tf);
        }
        index.0.insert(id, (ec.id(), mk, model_tf));
        sp.entities.insert(id, (ec.id(), mk, pose_key(b), inset));
    }
    sp.insets = insets;
}

/// Move building sites on: foundations -> frame -> the finished building
/// (which then grows in).
#[allow(clippy::too_many_arguments)]
fn advance_sites(
    mut commands: Commands,
    sim: Res<SimWorld>,
    time: Res<Time<Real>>,
    mats: Res<WorldMaterials>,
    mut models: ResMut<BuildingModels>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut sites: Query<(Entity, &mut Site, &mut Transform, &Children), Without<Shrinking>>,
) {
    let world = &sim.world;
    for (e, mut site, mut tf, children) in &mut sites {
        if !world.buildings.contains_key(site.id) {
            continue;
        }
        let stage = site_stage(world.time.tick.saturating_sub(site.start));
        if stage == Some(site.stage) {
            continue;
        }
        for c in children.iter() {
            commands.entity(c).despawn();
        }
        let model_tf = site.model_tf;
        match stage {
            Some(st) => {
                site.stage = st;
                let mesh = models.site(site.key, st, &mut meshes);
                commands.entity(e).with_children(|p| {
                    p.spawn((model_tf, Mesh3d(mesh), MeshMaterial3d(mats.lit.clone())));
                });
            }
            None => {
                let (detail, far) = models.get(site.key, &mut meshes);
                tf.scale = Vec3::new(0.9, 0.01, 0.9);
                commands
                    .entity(e)
                    .remove::<Site>()
                    .insert(Growing {
                        start: time.elapsed_secs(),
                    })
                    .with_children(|p| {
                        p.spawn((
                            model_tf,
                            Mesh3d(detail),
                            MeshMaterial3d(mats.lit.clone()),
                            VisibilityRange {
                                start_margin: 0.0..0.0,
                                end_margin: LOD_DISTANCE,
                                use_aabb: false,
                            },
                        ));
                        p.spawn((
                            model_tf,
                            Mesh3d(far),
                            MeshMaterial3d(mats.lit.clone()),
                            VisibilityRange {
                                start_margin: LOD_DISTANCE,
                                end_margin: 1e6..1e6,
                                use_aabb: false,
                            },
                        ));
                    });
            }
        }
    }
}

/// Ease-out with a slight overshoot.
fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.4;
    let c3 = c1 + 1.0;
    let u = t - 1.0;
    1.0 + c3 * u * u * u + c1 * u * u
}

fn animate_buildings(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut growing: Query<(Entity, &Growing, &mut Transform), Without<Shrinking>>,
    mut shrinking: Query<(Entity, &Shrinking, &mut Transform), Without<Growing>>,
) {
    let now = time.elapsed_secs();
    for (e, g, mut t) in &mut growing {
        let k = ((now - g.start) / GROW_SECS).clamp(0.0, 1.0);
        if k >= 1.0 {
            t.scale = Vec3::ONE;
            commands.entity(e).remove::<Growing>();
        } else {
            let y = ease_out_back(k).max(0.01);
            let xz = 0.9 + 0.1 * (k * 2.0).min(1.0);
            t.scale = Vec3::new(xz, y, xz);
        }
    }
    for (e, s, mut t) in &mut shrinking {
        let k = ((now - s.start) / SHRINK_SECS).clamp(0.0, 1.0);
        if k >= 1.0 {
            commands.entity(e).despawn();
        } else {
            let y = (1.0 - k * k).max(0.01);
            t.scale = Vec3::new(1.0 - 0.1 * k, y, 1.0 - 0.1 * k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_rotation_turns_the_model_front_towards_the_sim_front() {
        for k in 0..16 {
            let r = k as f32 * 0.4 - 3.0;
            let front = front_rotation(r) * Vec3::Z;
            assert!(front.distance(Vec3::new(r.cos(), 0.0, r.sin())) < 1e-5, "{r}");
            // The lot frame's x is the model's x.
            let lot = Lot {
                c: Vec2::ZERO,
                f: Vec2::new(r.cos(), r.sin()),
                half: Vec2::ONE,
            };
            let x = front_rotation(r) * Vec3::X;
            assert!(lot.world(Vec2::X).distance(Vec2::new(x.x, x.z)) < 1e-5, "{r}");
        }
    }

    #[test]
    fn grow_easing_ends_at_one() {
        assert!(ease_out_back(0.0).abs() < 1e-6);
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-6);
    }
}
