//! Vehicles: one entity per `world.vehicles` entry with a shared low-poly
//! car model (shape x colour from the vehicle seed), driving in the
//! right-hand lane of its segment, facing its heading, and interpolated
//! between sim ticks so motion is smooth at any frame rate and speed.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::{RoadKind, Vehicle, VehicleId, World};

use super::noise::hash;
use super::palette;
use super::road_geom::{ROAD_Y, lane_offset};
use super::scene::WorldMaterials;
use super::vehicle_gen::{self, SHAPES};
use crate::shared::{GameSet, SimWorld};

/// If a vehicle moves further than this in one tick it teleported (new trip).
const TELEPORT_M: f32 = 30.0;

#[derive(Resource)]
pub struct VehicleModels {
    /// `[shape * colours + colour]`.
    meshes: Vec<Handle<Mesh>>,
}

impl VehicleModels {
    pub fn count(&self) -> usize {
        self.meshes.len()
    }
}

#[derive(Component)]
struct VehicleVisual {
    prev: (Vec2, f32),
    cur: (Vec2, f32),
}

#[derive(Default)]
struct SyncState {
    entities: HashMap<VehicleId, Entity>,
    last_fixed: f64,
    last_tick: u64,
    generation: u64,
    /// The sim advanced on the most recent fixed update.
    moving: bool,
}

pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, sync_vehicles.in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut handles = Vec::new();
    for shape in 0..SHAPES {
        for &c in &palette::CAR_BODIES {
            handles.push(meshes.add(vehicle_gen::car_model(c, shape).build()));
        }
    }
    commands.insert_resource(VehicleModels { meshes: handles });
}

fn model_index(seed: u32) -> usize {
    let h = hash(seed ^ 0x51ED_270B);
    // Mostly sedans and hatchbacks, some vans and pickups.
    let shape = match h % 10 {
        0..=4 => 0,
        5..=7 => 1,
        8 => 2,
        _ => 3,
    };
    let colours = palette::CAR_BODIES.len();
    shape * colours + (h / 16) as usize % colours
}

/// Lane position (sim metres, heading) for a vehicle.
fn sample(world: &World, v: &Vehicle) -> (Vec2, f32) {
    let kind = v
        .segment
        .and_then(|s| world.roads.segments.get(s))
        .map_or(RoadKind::Street, |s| s.kind);
    let (s, c) = v.heading.sin_cos();
    // Right of travel in sim space is (-sin, cos).
    let off = lane_offset(kind, v.seed);
    (Vec2::new(v.pos[0] - s * off, v.pos[1] + c * off), v.heading)
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    a + d * t
}

#[allow(clippy::too_many_arguments)]
fn sync_vehicles(
    mut commands: Commands,
    sim: Res<SimWorld>,
    fixed: Res<Time<Fixed>>,
    models: Res<VehicleModels>,
    mats: Res<WorldMaterials>,
    mut state: Local<SyncState>,
    mut q: Query<(&mut VehicleVisual, &mut Transform)>,
) {
    let world = &sim.world;
    if state.generation != sim.generation {
        for (_, e) in state.entities.drain() {
            commands.entity(e).despawn();
        }
        state.generation = sim.generation;
    }
    let vehicles = &world.vehicles;
    state.entities.retain(|id, e| {
        let keep = vehicles.contains_key(*id);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    // Did a fixed update run since last frame, and did the sim advance?
    let fixed_now = fixed.elapsed_secs_f64();
    let stepped = fixed_now != state.last_fixed;
    let advanced = world.time.tick != state.last_tick;
    state.last_fixed = fixed_now;
    state.last_tick = world.time.tick;
    if stepped {
        state.moving = advanced;
    }
    let alpha = if state.moving {
        fixed.overstep_fraction().clamp(0.0, 1.0)
    } else {
        1.0
    };
    for (id, v) in vehicles.iter() {
        let now = sample(world, v);
        match state.entities.get(&id) {
            Some(&e) => {
                let Ok((mut vis, mut t)) = q.get_mut(e) else { continue };
                if stepped {
                    vis.prev = if advanced { vis.cur } else { now };
                    vis.cur = now;
                    if vis.prev.0.distance(vis.cur.0) > TELEPORT_M {
                        vis.prev = vis.cur;
                    }
                }
                let p = vis.prev.0.lerp(vis.cur.0, alpha);
                let h = lerp_angle(vis.prev.1, vis.cur.1, alpha);
                let new = Transform::from_xyz(p.x, ROAD_Y, p.y).with_rotation(Quat::from_rotation_y(-h));
                if *t != new {
                    *t = new;
                }
            }
            None => {
                let e = commands
                    .spawn((
                        VehicleVisual { prev: now, cur: now },
                        Mesh3d(models.meshes[model_index(v.seed)].clone()),
                        MeshMaterial3d(mats.lit.clone()),
                        Transform::from_xyz(now.0.x, ROAD_Y, now.0.y).with_rotation(Quat::from_rotation_y(-now.1)),
                    ))
                    .id();
                state.entities.insert(id, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_is_to_the_right_of_travel() {
        // Heading +x in sim (east). Right-hand traffic: the lane is on the
        // driver's right, i.e. (heading x up) in 3D -> +z in Bevy.
        let heading = 0.0f32;
        let (s, c) = heading.sin_cos();
        let right = Vec2::new(-s, c);
        let forward3 = Vec3::new(c, 0.0, s);
        let right3 = forward3.cross(Vec3::Y);
        assert!(right3.distance(Vec3::new(right.x, 0.0, right.y)) < 1e-6);
    }

    #[test]
    fn angle_lerp_takes_the_short_way() {
        let a = lerp_angle(3.0, -3.0, 0.5);
        assert!((a.abs() - std::f32::consts::PI).abs() < 0.3, "{a}");
    }
}
