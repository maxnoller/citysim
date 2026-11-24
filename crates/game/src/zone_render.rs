//! Tints zoned cells (green / blue / yellow), rebuilt when `versions.grid`
//! changes.

use bevy::prelude::*;
use citysim_sim::{Cell, MAP_CELLS, Zone};

use crate::mesh_util::FlatMeshBuilder;
use crate::sim_driver::SimWorld;
use crate::{GameSet, coords};

pub fn zone_color(zone: Zone) -> Color {
    match zone {
        Zone::Residential => Color::srgb(0.30, 0.78, 0.30),
        Zone::Commercial => Color::srgb(0.25, 0.50, 0.95),
        Zone::Industrial => Color::srgb(0.95, 0.80, 0.20),
    }
}

#[derive(Component)]
struct ZoneMesh(Zone);

pub struct ZoneRenderPlugin;

impl Plugin for ZoneRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_zone_entities)
            .add_systems(Update, rebuild_zones.in_set(GameSet::Render));
    }
}

fn spawn_zone_entities(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    for zone in Zone::ALL {
        commands.spawn((
            Name::new(format!("Zones {zone:?}")),
            ZoneMesh(zone),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: zone_color(zone).with_alpha(0.55),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 1.0,
                ..default()
            })),
            Transform::default(),
        ));
    }
}

fn rebuild_zones(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mut built: Local<Option<(u64, u64)>>,
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(Entity, &ZoneMesh)>,
) {
    let key = sim.key(sim.world.versions.grid);
    if *built == Some(key) {
        return;
    }
    *built = Some(key);
    let grid = &sim.world.grid;
    for (e, ZoneMesh(zone)) in &q {
        let mut b = FlatMeshBuilder::new();
        for y in 0..MAP_CELLS {
            for x in 0..MAP_CELLS {
                let c = Cell::new(x, y);
                if grid.get(c).and_then(|cd| cd.zone) == Some(*zone) {
                    b.quad(coords::cell_corners(c, 0.6, coords::ZONE_Y));
                }
            }
        }
        if b.is_empty() {
            commands.entity(e).remove::<Mesh3d>();
        } else {
            commands.entity(e).insert(Mesh3d(meshes.add(b.build())));
        }
    }
}
