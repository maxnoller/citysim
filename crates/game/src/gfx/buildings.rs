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
use super::road_geom;
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
    /// Road edges for fitting lots, per roads version.
    edges: Option<((u64, u64), RoadEdges)>,
    /// Fitted lot per building (and the pose it was fitted for).
    insets: HashMap<BuildingId, ([u32; 3], Inset)>,
    /// Buildings rebuilt for a big level-up: sim tick the works started
    /// (they show a building site again, with a crane when tall).
    rebuilt: HashMap<BuildingId, u64>,
}

/// How a building model is fitted clear of the roads beside its lot, in
/// the model's frame (front towards `+z`, width along `x`): the lot is
/// squeezed (`scale.x` width, `scale.z` depth) and its centre moved by
/// `shift` (model x, z). Lots are laid out on cells whose centres keep the
/// sim's road reserve clear, but wide roads (avenues), curves and junction
/// corners can still reach into the drawn lot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inset {
    pub scale: Vec3,
    pub shift: Vec2,
}

impl Inset {
    pub const NONE: Inset = Inset {
        scale: Vec3::ONE,
        shift: Vec2::ZERO,
    };
}

/// Clearance kept between a building lot and a road's sidewalk edge, m.
const ROAD_CLEARANCE: f32 = 0.3;
/// A lot is never shrunk below this share of its depth / width.
const MIN_INSET_SCALE: f32 = 0.45;

/// Road edges near lots: polylines (sim metres) with the distance that
/// must stay clear of them (half the drawn road width incl. sidewalks, or 0
/// for a junction's pavement outline), and their bounding boxes.
#[derive(Default)]
pub struct RoadEdges {
    lines: Vec<(Vec<Vec2>, f32, Vec2, Vec2)>,
}

impl RoadEdges {
    pub fn new(world: &citysim_sim::World) -> RoadEdges {
        let mut lines = Vec::new();
        for (_, seg) in world.roads.segments.iter() {
            let pts: Vec<Vec2> = seg.curve.flatten(0.25).into_iter().map(Vec2::from).collect();
            let keep = road_geom::style(seg.kind).outer();
            lines.push(bounded(pts, keep));
        }
        for (n, _) in world.roads.nodes.iter() {
            let walk = world.roads.nodes[n]
                .segments
                .iter()
                .filter_map(|&s| world.roads.segments.get(s))
                .map(|s| road_geom::style(s.kind).walk)
                .fold(0.0, f32::max);
            if let Some(shape) = citysim_sim::junction::junction_shape(world, n, walk) {
                let mut pts: Vec<Vec2> = shape.outline.iter().map(|p| Vec2::from(*p)).collect();
                if let Some(&first) = pts.first() {
                    pts.push(first);
                }
                lines.push(bounded(pts, 0.0));
            }
        }
        RoadEdges { lines }
    }
}

fn bounded(pts: Vec<Vec2>, keep: f32) -> (Vec<Vec2>, f32, Vec2, Vec2) {
    let lo = pts.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p)) - Vec2::splat(keep);
    let hi = pts.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p)) + Vec2::splat(keep);
    (pts, keep, lo, hi)
}

/// Distance from point `p` to segment `a..b`.
fn point_seg(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Distance between segment `a..b` and the rectangle `min..max` (0 when
/// they intersect).
fn seg_rect(a: Vec2, b: Vec2, min: Vec2, max: Vec2) -> f32 {
    // Liang-Barsky clip: does the segment enter the rectangle?
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    let mut inside = true;
    for (p, q) in [
        (-d.x, a.x - min.x),
        (d.x, max.x - a.x),
        (-d.y, a.y - min.y),
        (d.y, max.y - a.y),
    ] {
        if p.abs() < 1e-9 {
            if q < 0.0 {
                inside = false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    if inside && t0 <= t1 {
        return 0.0;
    }
    let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
    let to_rect = |p: Vec2| p.clamp(min, max).distance(p);
    corners
        .iter()
        .map(|&c| point_seg(c, a, b))
        .chain([to_rect(a), to_rect(b)])
        .fold(f32::MAX, f32::min)
}

/// The lot of a building in its own frame: centre `c`, front `f` (unit, sim
/// axes), `half` = (half width, half depth).
#[derive(Debug, Clone, Copy)]
pub struct Lot {
    pub c: Vec2,
    pub f: Vec2,
    pub half: Vec2,
}

impl Lot {
    pub fn of(b: &Building) -> Lot {
        let (w, d) = b.size();
        Lot {
            c: Vec2::from(b.center()),
            f: Vec2::from(b.front_dir()),
            half: Vec2::new(w as f32, d as f32) * CELL_SIZE * 0.5,
        }
    }

    /// Sim point -> lot frame (x = model x, y = model z = towards the front).
    fn local(&self, p: Vec2) -> Vec2 {
        let d = p - self.c;
        Vec2::new(d.x * self.f.y - d.y * self.f.x, d.dot(self.f))
    }
}

/// Fit a lot clear of the roads: first squeeze its depth away from the
/// front (anchored at the back), then, if a road still reaches in (a side
/// road at a corner, a junction), squeeze its width away from that side.
pub fn road_inset(lot: Lot, edges: &RoadEdges) -> Inset {
    let r = lot.half.length() + 1.0;
    let near: Vec<(Vec<Vec2>, f32)> = edges
        .lines
        .iter()
        .filter(|(_, _, lo, hi)| {
            lot.c.x + r >= lo.x && lot.c.x - r <= hi.x && lot.c.y + r >= lo.y && lot.c.y - r <= hi.y
        })
        .map(|(pts, keep, _, _)| (pts.iter().map(|p| lot.local(*p)).collect(), *keep))
        .collect();
    if near.is_empty() {
        return Inset::NONE;
    }
    let clear = |min: Vec2, max: Vec2| {
        near.iter().all(|(pts, keep)| {
            pts.windows(2)
                .all(|w| seg_rect(w[0], w[1], min, max) >= keep + ROAD_CLEARANCE)
        })
    };
    let (min, max) = (-lot.half, lot.half);
    if clear(min, max) {
        return Inset::NONE;
    }
    // Largest share of the depth (from the back) that is clear.
    let fit = |ok: &dyn Fn(f32) -> bool| {
        if ok(1.0) {
            return 1.0;
        }
        let (mut lo, mut hi) = (MIN_INSET_SCALE, 1.0f32);
        if !ok(lo) {
            return lo;
        }
        for _ in 0..12 {
            let mid = (lo + hi) * 0.5;
            if ok(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    };
    let depth = fit(&|s| clear(min, Vec2::new(max.x, min.y + 2.0 * lot.half.y * s)));
    let top = min.y + 2.0 * lot.half.y * depth;
    // Then the width, from whichever side is clear.
    let from_left = fit(&|s| clear(min, Vec2::new(min.x + 2.0 * lot.half.x * s, top)));
    let from_right = fit(&|s| clear(Vec2::new(max.x - 2.0 * lot.half.x * s, min.y), Vec2::new(max.x, top)));
    let (x0, x1) = if from_left >= from_right {
        (min.x, min.x + 2.0 * lot.half.x * from_left)
    } else {
        (max.x - 2.0 * lot.half.x * from_right, max.x)
    };
    let (lo, hi) = (Vec2::new(x0, min.y), Vec2::new(x1, top));
    Inset {
        scale: Vec3::new((hi.x - lo.x) / (max.x - min.x), 1.0, (hi.y - lo.y) / (max.y - min.y)),
        shift: (lo + hi) * 0.5,
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
                    .in_set(GameSet::Render),
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
    // Road edges change only with the roads; each building's fit only
    // with the roads or its own lot.
    let roads_key = sim.key(world.versions.roads);
    if sp.edges.as_ref().is_none_or(|(k, _)| *k != roads_key) {
        sp.edges = Some((roads_key, RoadEdges::new(world)));
        sp.insets.clear();
    }
    let edges = &sp.edges.as_ref().expect("edges").1;
    let mut insets = std::mem::take(&mut sp.insets);
    insets.retain(|id, _| buildings.contains_key(*id));
    let mut inset_of = |id: BuildingId, b: &Building| {
        let pose = pose_key(b);
        if let Some((k, i)) = insets.get(&id)
            && *k == pose
        {
            return *i;
        }
        let i = road_inset(Lot::of(b), edges);
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
    fn facing_rotation_turns_front_towards_facing() {
        for f in Facing::ALL {
            let front = facing_rotation(f) * Vec3::Z;
            let (dx, dy) = f.offset();
            assert!(front.distance(Vec3::new(dx as f32, 0.0, dy as f32)) < 1e-5, "{f:?}");
        }
    }

    #[test]
    fn grow_easing_ends_at_one() {
        assert!(ease_out_back(0.0).abs() < 1e-6);
        assert!((ease_out_back(1.0) - 1.0).abs() < 1e-6);
    }

    /// A town with a 45 degree street and housing zoned on both sides.
    fn diagonal_town() -> citysim_sim::World {
        use citysim_sim::{Cell, Command, World, Zone};
        let mut w = World::new(3);
        w.economy.money = 1_000_000;
        for (a, b) in [((11, 128), (40, 128)), ((40, 128), (60, 108)), ((25, 120), (25, 136))] {
            w.apply(Command::BuildRoad {
                from: Cell::new(a.0, a.1),
                to: Cell::new(b.0, b.1),
                kind: citysim_sim::RoadKind::Street,
            })
            .expect("road");
        }
        w.apply(Command::SetZone {
            min: Cell::new(38, 104),
            max: Cell::new(64, 130),
            zone: Some(Zone::Residential),
        })
        .expect("zone");
        w.apply(Command::SetZone {
            min: Cell::new(12, 120),
            max: Cell::new(37, 127),
            zone: Some(Zone::Commercial),
        })
        .expect("zone");
        for _ in 0..citysim_sim::balance::TICKS_PER_DAY * 2 {
            w.step();
        }
        w
    }

    #[test]
    fn buildings_beside_a_diagonal_road_are_inset_clear_of_it() {
        let w = diagonal_town();
        let diagonals = diagonal_roads(&w.roads);
        assert!(!diagonals.is_empty());
        let mut inset_count = 0;
        let mut beside = 0;
        for (_, b) in w.buildings.iter() {
            let (sw, sd) = b.size();
            let min = Vec2::new(b.footprint.min.x as f32, b.footprint.min.y as f32) * CELL_SIZE;
            let max = min + Vec2::new(sw as f32, sd as f32) * CELL_SIZE;
            let raw = diagonals
                .iter()
                .map(|&(a, c, _)| seg_rect(a, c, min, max))
                .fold(f32::MAX, f32::min);
            if raw < 4.0 {
                beside += 1;
            }
            let i = diagonal_inset(min, max, &diagonals);
            if i.scale < 1.0 {
                inset_count += 1;
            }
            // The drawn lot after the inset.
            let c = (min + max) * 0.5 + i.shift;
            let half = (max - min) * 0.5 * i.scale;
            for &(a, c2, hw) in &diagonals {
                let d = seg_rect(a, c2, c - half, c + half);
                assert!(
                    d >= hw + ROAD_CLEARANCE - 0.05,
                    "building at {min} clips a diagonal road: {d}"
                );
            }
        }
        println!(
            "{} buildings, {beside} beside the diagonal, {inset_count} inset",
            w.buildings.len()
        );
        assert!(
            beside > 0 && inset_count > 0,
            "the test town grew no buildings beside the diagonal"
        );
    }

    #[test]
    fn inset_keeps_the_far_corner_and_shrinks_toward_it() {
        // A cell whose corner touches a 45 degree centre line.
        let road = [(Vec2::new(0.0, 16.0), Vec2::new(16.0, 0.0), 4.0)];
        let (min, max) = (Vec2::new(8.0, 8.0), Vec2::new(16.0, 16.0));
        let i = diagonal_inset(min, max, &road);
        assert!(i.scale < 0.8 && i.scale > MIN_INSET_SCALE, "{i:?}");
        // Anchored at the far corner (16, 16): the centre moves towards it.
        assert!(i.shift.x > 0.0 && i.shift.y > 0.0);
        // A lot far away is untouched.
        assert_eq!(diagonal_inset(min + 40.0, max + 40.0, &road), Inset::NONE);
    }
}
