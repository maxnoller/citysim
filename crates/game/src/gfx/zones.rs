//! Tints zoned cells (green / blue / yellow), rebuilt when `versions.grid`
//! changes. Stronger in `OverlayMode::Zones`, hidden in `Traffic`.

use bevy::prelude::*;
use citysim_sim::{Cell, MAP_CELLS, Zone};

use super::heights;
use super::mesh_util::FlatMeshBuilder;
use crate::shared::{GameSet, OverlayMode, SimWorld, cell_corners};

pub fn zone_color(zone: Zone) -> Color {
    match zone {
        Zone::Residential => Color::srgb(0.30, 0.78, 0.30),
        Zone::Commercial => Color::srgb(0.25, 0.50, 0.95),
        Zone::Industrial => Color::srgb(0.95, 0.80, 0.20),
    }
}

#[derive(Component)]
struct ZoneMesh(Zone);

pub struct ZonesPlugin;

impl Plugin for ZonesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_zone_entities)
            .add_systems(Update, (rebuild_zones, apply_overlay).in_set(GameSet::Render));
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
            Visibility::default(),
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
                    b.quad(cell_corners(c, 0.6, heights::ZONE));
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

fn apply_overlay(
    mode: Res<OverlayMode>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(&ZoneMesh, &MeshMaterial3d<StandardMaterial>, &mut Visibility)>,
) {
    if !mode.is_changed() {
        return;
    }
    for (ZoneMesh(zone), mat, mut vis) in &mut q {
        *vis = if *mode == OverlayMode::Traffic {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let alpha = if *mode == OverlayMode::Zones { 0.9 } else { 0.55 };
        if let Some(mut m) = materials.get_mut(&mat.0) {
            m.base_color = zone_color(*zone).with_alpha(alpha);
        }
    }
}
