//! Placeholder vehicles: one box per `world.vehicles` entry, moved every
//! frame from the vehicle's position and heading.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::VehicleId;

use crate::shared::{GameSet, SimWorld, sim_to_world};

/// Lateral offset from the segment centre line to the driving lane, metres.
const LANE_OFFSET: f32 = 1.5;

#[derive(Resource)]
struct VehicleAssets {
    mesh: Handle<Mesh>,
    materials: Vec<Handle<StandardMaterial>>,
}

pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, sync_vehicles.in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let colors = [
        Color::srgb(0.85, 0.2, 0.2),
        Color::srgb(0.95, 0.95, 0.95),
        Color::srgb(0.2, 0.3, 0.8),
        Color::srgb(0.15, 0.15, 0.15),
        Color::srgb(0.9, 0.75, 0.2),
    ];
    commands.insert_resource(VehicleAssets {
        mesh: meshes.add(Cuboid::new(4.2, 1.5, 1.9)),
        materials: colors.iter().map(|&c| materials.add(c)).collect(),
    });
}

fn sync_vehicles(
    mut commands: Commands,
    sim: Res<SimWorld>,
    assets: Res<VehicleAssets>,
    mut spawned: Local<HashMap<VehicleId, Entity>>,
    mut transforms: Query<&mut Transform>,
) {
    let vehicles = &sim.world.vehicles;
    spawned.retain(|id, e| {
        let keep = vehicles.contains_key(*id);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    for (id, v) in vehicles.iter() {
        let (s, c) = v.heading.sin_cos();
        let pos = [v.pos[0] - s * LANE_OFFSET, v.pos[1] + c * LANE_OFFSET];
        let tf = Transform::from_translation(sim_to_world(pos, 0.9)).with_rotation(Quat::from_rotation_y(-v.heading));
        match spawned.get(&id) {
            Some(&e) => {
                if let Ok(mut t) = transforms.get_mut(e) {
                    *t = tf;
                }
            }
            None => {
                let mat = assets.materials[v.seed as usize % assets.materials.len()].clone();
                let e = commands
                    .spawn((Mesh3d(assets.mesh.clone()), MeshMaterial3d(mat), tf))
                    .id();
                spawned.insert(id, e);
            }
        }
    }
}
