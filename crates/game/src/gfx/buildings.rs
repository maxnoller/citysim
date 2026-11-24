//! Placeholder buildings: one box entity per `world.buildings` entry,
//! synced when `versions.buildings` changes.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::{BuildingId, CELL_SIZE, Zone};

use super::zones::zone_color;
use crate::shared::{GameSet, SimWorld};

#[derive(Resource)]
struct BuildingAssets {
    cube: Handle<Mesh>,
    /// `[zone][shade]`.
    materials: [[Handle<StandardMaterial>; 3]; 3],
}

#[derive(Default)]
struct Spawned {
    key: Option<(u64, u64)>,
    entities: HashMap<BuildingId, Entity>,
}

pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, sync_buildings.in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let materials = Zone::ALL.map(|z| {
        [0.55, 0.7, 0.85].map(|shade| {
            let base = zone_color(z).to_srgba();
            materials.add(StandardMaterial {
                base_color: Color::srgb(
                    base.red * shade + 0.1,
                    base.green * shade + 0.1,
                    base.blue * shade + 0.1,
                ),
                perceptual_roughness: 0.8,
                ..default()
            })
        })
    });
    commands.insert_resource(BuildingAssets {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        materials,
    });
}

fn sync_buildings(
    mut commands: Commands,
    sim: Res<SimWorld>,
    assets: Res<BuildingAssets>,
    mut spawned: Local<Spawned>,
) {
    let key = sim.key(sim.world.versions.buildings);
    if spawned.key == Some(key) {
        return;
    }
    spawned.key = Some(key);
    let buildings = &sim.world.buildings;
    spawned.entities.retain(|id, e| {
        let keep = buildings.contains_key(*id);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    for (id, b) in buildings.iter() {
        if spawned.entities.contains_key(&id) {
            continue;
        }
        let (w, d) = b.size();
        let min = b.footprint.min;
        let center = Vec3::new(
            (min.x as f32 + w as f32 * 0.5) * CELL_SIZE,
            0.0,
            (min.y as f32 + d as f32 * 0.5) * CELL_SIZE,
        );
        let height = 3.0 + b.level as f32 * 4.0 + (b.seed % 5) as f32;
        let zone = Zone::ALL.iter().position(|&z| z == b.kind).unwrap_or(0);
        let e = commands
            .spawn((
                Mesh3d(assets.cube.clone()),
                MeshMaterial3d(assets.materials[zone][(b.seed % 3) as usize].clone()),
                Transform::from_translation(center + Vec3::Y * height * 0.5).with_scale(Vec3::new(
                    w as f32 * CELL_SIZE - 1.6,
                    height,
                    d as f32 * CELL_SIZE - 1.6,
                )),
            ))
            .id();
        spawned.entities.insert(id, e);
    }
}
