//! Buildings: one entity per `world.buildings` entry, synced when
//! `versions.buildings` changes. Models come from `building_gen`, cached by
//! `(kind, lot size, level, variant)` and shared, all with one material, so
//! thousands of buildings batch into few draws. New buildings grow in with
//! an ease-out; removed ones shrink out before despawning.

use std::collections::HashMap;

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use citysim_sim::{Building, BuildingId, CELL_SIZE};

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::tasks::{ComputeTaskPool, TaskPool};

use super::building_gen::{self, Lod, ModelKey, VARIANTS};
use super::land::{self, Inset, Lot, RoadEdges};
use super::scene::{QualityApplied, WorldMaterials};
use crate::shared::{GameSet, GraphicsQuality, SimWorld, WorldCamera};

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

/// Cache of generated building models: `[near, mid, far]` per key.
#[derive(Resource, Default)]
pub struct BuildingModels {
    meshes: HashMap<ModelKey, [Handle<Mesh>; 3]>,
    emitters: HashMap<ModelKey, Vec<Vec3>>,
    driveways: HashMap<ModelKey, Vec<(f32, f32)>>,
    sites: HashMap<(ModelKey, u8), Handle<Mesh>>,
}

/// One generated model: its three levels of detail, smoke emitters and
/// driveways.
type Generated = (ModelKey, [Mesh; 3], Vec<Vec3>, Vec<(f32, f32)>);

fn generate_all(key: ModelKey) -> Generated {
    let mut near = building_gen::generate(key, Lod::Near);
    let emitters = std::mem::take(&mut near.emitters);
    let driveways = std::mem::take(&mut near.driveways);
    (
        key,
        [
            near.build(),
            building_gen::generate(key, Lod::Mid).build(),
            building_gen::generate(key, Lod::Far).build(),
        ],
        emitters,
        driveways,
    )
}

impl BuildingModels {
    /// Number of distinct building models (each has three levels of
    /// detail).
    pub fn count(&self) -> usize {
        self.meshes.len()
    }

    pub fn site(&mut self, key: ModelKey, stage: u8, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
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

    /// Generate the models of `keys` not yet cached, side by side on the
    /// compute threads (a loaded city needs hundreds at once).
    pub fn prepare(&mut self, keys: impl IntoIterator<Item = ModelKey>, meshes: &mut Assets<Mesh>) {
        let mut todo: Vec<ModelKey> = keys.into_iter().filter(|k| !self.meshes.contains_key(k)).collect();
        todo.sort_by_key(|k| (k.kind as u8, k.cells, k.level, k.variant));
        todo.dedup();
        if todo.is_empty() {
            return;
        }
        let built: Vec<Generated> = if todo.len() == 1 {
            vec![generate_all(todo[0])]
        } else {
            let pool = ComputeTaskPool::get_or_init(TaskPool::default);
            pool.scope(|s| {
                for k in todo {
                    s.spawn(async move { generate_all(k) });
                }
            })
        };
        for (key, [near, mid, far], emitters, driveways) in built {
            self.emitters.insert(key, emitters);
            self.driveways.insert(key, driveways);
            self.meshes
                .insert(key, [meshes.add(near), meshes.add(mid), meshes.add(far)]);
        }
    }

    pub fn get(&mut self, key: ModelKey, meshes: &mut Assets<Mesh>) -> [Handle<Mesh>; 3] {
        self.prepare([key], meshes);
        self.meshes[&key].clone()
    }

    /// Driveway x ranges at the lot front, in model space (after `get`).
    pub fn driveways(&self, key: ModelKey) -> &[(f32, f32)] {
        self.driveways.get(&key).map_or(&[], Vec::as_slice)
    }

    /// Chimney tops that smoke, in model space (after `get`).
    pub fn emitters(&self, key: ModelKey) -> &[Vec3] {
        self.emitters.get(&key).map_or(&[], Vec::as_slice)
    }
}

/// Which level of detail a building's child entity shows (the near and
/// middle ranges follow the graphics quality).
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum BuildingPart {
    Near,
    Mid,
}

/// Camera distance (m) up to which buildings show their near model, per
/// quality (Low: never).
pub fn near_distance(q: GraphicsQuality) -> f32 {
    match q {
        GraphicsQuality::Low => 0.0,
        GraphicsQuality::Medium => 120.0,
        GraphicsQuality::High => 180.0,
    }
}

/// Cross-fade half width at the near / middle switch, metres.
const NEAR_FADE: f32 = 12.0;

fn part_range(part: BuildingPart, near: f32) -> (VisibilityRange, Visibility) {
    let fade = (near - NEAR_FADE).max(0.0)..near + NEAR_FADE;
    match part {
        BuildingPart::Near => (
            VisibilityRange {
                start_margin: 0.0..0.0,
                end_margin: fade,
                use_aabb: false,
            },
            if near > 0.0 {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        ),
        BuildingPart::Mid => (
            VisibilityRange {
                start_margin: if near > 0.0 { fade } else { 0.0..0.0 },
                end_margin: LOD_DISTANCE,
                use_aabb: false,
            },
            Visibility::Inherited,
        ),
    }
}

/// Spawn a finished building's three models under its entity.
fn spawn_models(
    p: &mut ChildSpawnerCommands,
    model_tf: Transform,
    handles: &[Handle<Mesh>; 3],
    mats: &WorldMaterials,
    near: f32,
) {
    for (part, mesh) in [(BuildingPart::Near, &handles[0]), (BuildingPart::Mid, &handles[1])] {
        let (range, vis) = part_range(part, near);
        p.spawn((
            model_tf,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mats.lit.clone()),
            range,
            vis,
            part,
        ));
    }
    p.spawn((
        model_tf,
        Mesh3d(handles[2].clone()),
        MeshMaterial3d(mats.lit.clone()),
        VisibilityRange {
            start_margin: LOD_DISTANCE,
            end_margin: 1e6..1e6,
            use_aabb: false,
        },
    ));
}

/// A quality switch moves the near / middle boundary of every building.
fn apply_quality(
    applied: Option<Res<QualityApplied>>,
    mut shown: Local<Option<GraphicsQuality>>,
    mut parts: Query<(&BuildingPart, &mut VisibilityRange, &mut Visibility)>,
) {
    let Some(q) = applied.and_then(|a| a.0) else { return };
    if *shown == Some(q) {
        return;
    }
    *shown = Some(q);
    let near = near_distance(q);
    for (part, mut range, mut vis) in &mut parts {
        let (r, v) = part_range(*part, near);
        if *range != r {
            *range = r;
        }
        vis.set_if_neq(v);
    }
}

/// The near distance for buildings spawned now.
fn current_near(applied: Option<&QualityApplied>, requested: GraphicsQuality) -> f32 {
    near_distance(super::scene::drawn_quality(applied, requested))
}

#[derive(Component)]
pub(super) struct BuildingVisual;

/// Drawn building per sim building: (parent entity, model key, model child
/// transform). Highlights draw over these.
#[derive(Resource, Default)]
pub struct BuildingEntities(pub HashMap<BuildingId, (Entity, ModelKey, Transform)>);

/// Where driveways meet the road side (sim metres): street furniture keeps
/// clear of them. `version` changes whenever the set changes.
#[derive(Resource, Default)]
pub struct DrivewayMouths {
    pub version: u64,
    pub points: Vec<Vec2>,
}

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
    /// A new world (a load) is being filled in, `FILL_PER_FRAME` buildings
    /// a frame nearest the camera first, without grow animations or sites.
    filling: bool,
}

/// Buildings spawned per frame while a loaded world fills in (spawning all
/// of a big city's at once took one frame 30-60 ms).
const FILL_PER_FRAME: usize = 160;

/// Rows of a lot the building itself stands on (the rest is its yard).
fn built_rows(b: &Building) -> i32 {
    b.size().1.min(citysim_sim::balance::BUILT_DEPTH as i32)
}

/// The part of a building's lot it stands on, in its own frame (see
/// `land::Lot`): the front rows; a deeper lot's yard lies behind it.
pub fn lot_of(b: &Building) -> Lot {
    let (w, d) = b.size();
    let built = built_rows(b);
    let f = Vec2::from(b.front_dir());
    Lot {
        c: Vec2::from(b.center()) + f * (d - built) as f32 * CELL_SIZE * 0.5,
        f,
        half: Vec2::new(w as f32, built as f32) * CELL_SIZE * 0.5,
    }
}

/// How a building is fitted on its lot: clear of the roads and, on a
/// curve, inside the narrowing or widening lot with a little room to its
/// neighbours.
/// Beyond the lot's half diagonal, how far road edges can change its fit
/// (generous: strip lots fit inside their lattice quad).
const FIT_REACH_M: f32 = 12.0;

fn fit_building(world: &citysim_sim::World, b: &Building, edges: &RoadEdges) -> Inset {
    let lot = lot_of(b);
    match world.zoning.lattice(b.footprint.lattice) {
        Some(l) if l.is_strip() => {
            let Some(q) = world.zoning.lot_quad(&b.footprint) else {
                return land::fit(lot, edges);
            };
            // The built part of the quad: from the front edge back by the
            // built rows' share of the lot.
            let (_, d) = b.size();
            let t = built_rows(b) as f32 / d.max(1) as f32;
            let q = q.map(Vec2::from);
            let back_l = q[0] + (q[3] - q[0]) * t;
            let back_r = q[1] + (q[2] - q[1]) * t;
            land::fit_in_quad(lot, [q[0], q[1], back_r, back_l], 0.4, edges)
        }
        _ => land::fit(lot, edges),
    }
}

/// The system that syncs buildings (and the driveway mouths).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BuildingsSync;

pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingModels>()
            .init_resource::<BuildingEntities>()
            .init_resource::<DrivewayMouths>()
            .add_systems(
                Update,
                (
                    apply_quality,
                    sync_buildings.in_set(BuildingsSync),
                    advance_sites,
                    animate_buildings,
                )
                    .chain()
                    .in_set(GameSet::Render)
                    .after(super::land::LandUpdate),
            );
    }
}

/// Model key and placement for a building.
pub fn model_key(b: &Building) -> ModelKey {
    let (w, _) = b.size();
    let d = built_rows(b);
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
    let c = lot_of(b).c;
    let tf = Transform::from_xyz(c.x, 0.0, c.y).with_rotation(front_rotation(b.rotation()));
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
    mut mouths: ResMut<DrivewayMouths>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    quality: (Option<Res<QualityApplied>>, Res<GraphicsQuality>),
) {
    let near = current_near(quality.0.as_deref(), *quality.1);
    let key = (sim.key(sim.world.versions.buildings), sim.key(sim.world.versions.roads));
    if spawned.key == Some(key) && !spawned.filling {
        return;
    }
    let reloaded = spawned.key.is_some_and(|((g, _), _)| g != sim.generation);
    spawned.key = Some(key);
    if reloaded {
        spawned.filling = true;
    }
    // Spawning as if loaded: no grow animation, no building sites.
    let fresh = spawned.filling;
    let sp = &mut *spawned;
    let now = time.elapsed_secs();
    let world = &sim.world;
    let buildings = &world.buildings;
    // Each building's fit changes only with the roads or its own lot.
    if sp.edges_key != edges.key {
        sp.edges_key = edges.key;
        // Refit only the buildings near roads that changed.
        sp.insets.retain(|id, _| {
            buildings.get(*id).is_some_and(|b| {
                let lot = lot_of(b);
                let r = Vec2::splat(lot.half.length() + FIT_REACH_M);
                edges.unchanged_in(lot.c - r, lot.c + r)
            })
        });
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
        let i = fit_building(world, b, &edges);
        insets.insert(id, (pose, i));
        i
    };
    let tick = world.time.tick;
    let mut rebuilt: Vec<BuildingId> = Vec::new();
    sp.entities.retain(|id, (e, k, f, i)| {
        let keep = buildings
            .get(*id)
            .is_some_and(|b| !reloaded && model_key(b) == *k && pose_key(b) == *f && inset_of(*id, b) == *i);
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
    let mut missing: Vec<(BuildingId, &Building)> = buildings
        .iter()
        .filter(|(id, _)| !sp.entities.contains_key(id))
        .collect();
    if fresh {
        let eye = cams
            .single()
            .map_or(Vec2::ZERO, |t| Vec2::new(t.translation().x, t.translation().z));
        missing.sort_by(|a, b| {
            let d = |b: &Building| Vec2::from(b.center()).distance_squared(eye);
            d(a.1).total_cmp(&d(b.1))
        });
        if missing.len() <= FILL_PER_FRAME {
            sp.filling = false;
        }
        missing.truncate(FILL_PER_FRAME);
    }
    // New models for this frame, generated side by side.
    models.prepare(missing.iter().map(|(_, b)| model_key(b)), &mut meshes);
    for (id, b) in missing {
        let mk = model_key(b);
        let young = world.time.tick.saturating_sub(b.created_tick) < ANIMATE_IF_YOUNGER && !fresh;
        let inset = inset_of(id, b);
        // The inset lives on the model children, so the grow animation can
        // scale the parent freely.
        let (tf, model_tf) = placement(b, inset);
        let start = sp.rebuilt.get(&id).copied().unwrap_or(b.created_tick);
        let age = world.time.tick.saturating_sub(start);
        if !fresh && let Some(stage) = site_stage(age) {
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
        let handles = models.get(mk, &mut meshes);
        let mut ec = commands.spawn((BuildingVisual, Visibility::default()));
        ec.with_children(|p| spawn_models(p, model_tf, &handles, &mats, near));
        if young {
            ec.insert((tf.with_scale(Vec3::new(0.9, 0.01, 0.9)), Growing { start: now }));
        } else {
            ec.insert(tf);
        }
        index.0.insert(id, (ec.id(), mk, model_tf));
        sp.entities.insert(id, (ec.id(), mk, pose_key(b), inset));
    }
    sp.insets = insets;
    // Published once a loaded world has filled in (the roads re-mesh for
    // them).
    if sp.filling {
        return;
    }
    // Driveway mouths of every finished (or growing) building.
    let mut points = Vec::new();
    for (id, (_, key, _, inset)) in &sp.entities {
        let Some(b) = buildings.get(*id) else { continue };
        let lot = lot_of(b);
        for &(x0, x1) in models.driveways(*key) {
            let x = (x0 + x1) * 0.5;
            points.push(lot.world(Vec2::new(
                inset.shift.x + x * inset.scale.x,
                inset.shift.y + lot.half.y * inset.scale.z,
            )));
        }
    }
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    if points != mouths.points {
        mouths.points = points;
        mouths.version += 1;
    }
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
    quality: (Option<Res<QualityApplied>>, Res<GraphicsQuality>),
) {
    let near = current_near(quality.0.as_deref(), *quality.1);
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
                let handles = models.get(site.key, &mut meshes);
                tf.scale = Vec3::new(0.9, 0.01, 0.9);
                commands
                    .entity(e)
                    .remove::<Site>()
                    .insert(Growing {
                        start: time.elapsed_secs(),
                    })
                    .with_children(|p| spawn_models(p, model_tf, &handles, &mats, near));
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
