//! Buildings: one entity per `world.buildings` entry, synced when
//! `versions.buildings` changes. Models come from `building_gen`, cached by
//! `(kind, lot size, level, variant)` and shared, all with one material, so
//! thousands of buildings batch into few draws. New buildings grow in with
//! an ease-out; removed ones shrink out before despawning.

use std::collections::HashMap;

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use citysim_sim::{Building, BuildingId, CELL_SIZE, Facing};

use super::building_gen::{self, ModelKey, VARIANTS};
use super::scene::WorldMaterials;
use crate::shared::{GameSet, SimWorld};

const GROW_SECS: f32 = 0.75;
const SHRINK_SECS: f32 = 0.35;
/// Buildings younger than this (sim ticks) animate in; older ones (e.g. from
/// a loaded save) just appear.
const ANIMATE_IF_YOUNGER: u64 = 3 * citysim_sim::TICKS_PER_SECOND as u64;

/// Camera distance (m) where buildings cross-fade to their simplified model.
pub const LOD_DISTANCE: std::ops::Range<f32> = 650.0..700.0;

/// Cache of generated building models: `(detailed, distant)` per key.
#[derive(Resource, Default)]
pub struct BuildingModels {
    meshes: HashMap<ModelKey, (Handle<Mesh>, Handle<Mesh>)>,
}

impl BuildingModels {
    /// Number of distinct building models (each has two levels of detail).
    pub fn count(&self) -> usize {
        self.meshes.len()
    }

    fn get(&mut self, key: ModelKey, meshes: &mut Assets<Mesh>) -> (Handle<Mesh>, Handle<Mesh>) {
        self.meshes
            .entry(key)
            .or_insert_with(|| {
                (
                    meshes.add(building_gen::generate(key, false).build()),
                    meshes.add(building_gen::generate(key, true).build()),
                )
            })
            .clone()
    }
}

#[derive(Component)]
struct BuildingVisual;

#[derive(Component)]
struct Growing {
    start: f32,
}

#[derive(Component)]
struct Shrinking {
    start: f32,
}

#[derive(Default)]
struct Spawned {
    key: Option<(u64, u64)>,
    entities: HashMap<BuildingId, (Entity, ModelKey, Facing)>,
}

pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingModels>().add_systems(
            Update,
            (sync_buildings, animate_buildings).chain().in_set(GameSet::Render),
        );
    }
}

/// Model key and placement for a building.
pub fn model_key(b: &Building) -> ModelKey {
    let (w, d) = b.size();
    let (w, d) = match b.facing {
        Facing::PlusY | Facing::MinusY => (w, d),
        Facing::PlusX | Facing::MinusX => (d, w),
    };
    ModelKey {
        kind: b.kind,
        cells: (w.clamp(1, 255) as u8, d.clamp(1, 255) as u8),
        level: b.level.max(1),
        variant: (b.seed % VARIANTS) as u8,
    }
}

/// Rotation turning the model's front (+z) towards the building's facing.
pub fn facing_rotation(f: Facing) -> Quat {
    use std::f32::consts::{FRAC_PI_2, PI};
    Quat::from_rotation_y(match f {
        Facing::PlusY => 0.0,
        Facing::PlusX => FRAC_PI_2,
        Facing::MinusY => PI,
        Facing::MinusX => -FRAC_PI_2,
    })
}

fn footprint_center(b: &Building) -> Vec3 {
    let (w, d) = b.size();
    let min = b.footprint.min;
    Vec3::new(
        (min.x as f32 + w as f32 * 0.5) * CELL_SIZE,
        0.0,
        (min.y as f32 + d as f32 * 0.5) * CELL_SIZE,
    )
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
) {
    let key = sim.key(sim.world.versions.buildings);
    if spawned.key == Some(key) {
        return;
    }
    let reloaded = spawned.key.is_some_and(|(g, _)| g != sim.generation);
    spawned.key = Some(key);
    let now = time.elapsed_secs();
    let world = &sim.world;
    let buildings = &world.buildings;
    spawned.entities.retain(|id, (e, k, f)| {
        let keep = buildings
            .get(*id)
            .is_some_and(|b| model_key(b) == *k && b.facing == *f && !reloaded);
        if !keep {
            commands.entity(*e).remove::<Growing>().insert(Shrinking { start: now });
        }
        keep
    });
    for (id, b) in buildings.iter() {
        if spawned.entities.contains_key(&id) {
            continue;
        }
        let mk = model_key(b);
        let young = world.time.tick.saturating_sub(b.created_tick) < ANIMATE_IF_YOUNGER && !reloaded;
        let tf = Transform::from_translation(footprint_center(b)).with_rotation(facing_rotation(b.facing));
        let (detail, far) = models.get(mk, &mut meshes);
        let mut ec = commands.spawn((BuildingVisual, Visibility::default()));
        ec.with_children(|p| {
            p.spawn((
                Mesh3d(detail),
                MeshMaterial3d(mats.lit.clone()),
                VisibilityRange {
                    start_margin: 0.0..0.0,
                    end_margin: LOD_DISTANCE,
                    use_aabb: false,
                },
            ));
            p.spawn((
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
        spawned.entities.insert(id, (ec.id(), mk, b.facing));
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
}
