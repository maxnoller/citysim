//! Dev aid: `CITYSIM_GFX_STATS=1` logs scene statistics every 5 seconds
//! (entities, meshes, materials, building models, trees, triangles drawn by
//! visible meshes) to check that the scene stays batched.
//! `CITYSIM_GFX_QUALITY=low|medium|high` sets the starting `GraphicsQuality`
//! (dev aid for measuring; at runtime ui owns that resource).

use bevy::mesh::Indices;
use bevy::prelude::*;

use super::buildings::BuildingModels;
use super::trees::TreeCount;
use super::vehicles::VehicleModels;
use crate::shared::GraphicsQuality;

pub struct StatsPlugin;

impl Plugin for StatsPlugin {
    fn build(&self, app: &mut App) {
        let q = match std::env::var("CITYSIM_GFX_QUALITY").as_deref() {
            Ok("low") => Some(GraphicsQuality::Low),
            Ok("medium") => Some(GraphicsQuality::Medium),
            Ok("high") => Some(GraphicsQuality::High),
            _ => None,
        };
        if let Some(q) = q {
            app.add_systems(Startup, move |mut r: ResMut<GraphicsQuality>| *r = q);
        }
        if std::env::var("CITYSIM_GFX_STATS").is_ok_and(|v| !v.is_empty() && v != "0") {
            app.add_systems(Update, log_stats);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn log_stats(
    time: Res<Time<Real>>,
    mut last: Local<f32>,
    entities: Query<Entity>,
    mesh_entities: Query<(&Mesh3d, &ViewVisibility)>,
    meshes: Res<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    buildings: Res<BuildingModels>,
    vehicles: Res<VehicleModels>,
    trees: Res<TreeCount>,
) {
    let now = time.elapsed_secs();
    if now - *last < 5.0 {
        return;
    }
    *last = now;
    let mut tris = 0usize;
    let mut visible = 0usize;
    for (m, vis) in &mesh_entities {
        if !vis.get() {
            continue;
        }
        visible += 1;
        if let Some(mesh) = meshes.get(&m.0) {
            tris += match mesh.indices() {
                Some(Indices::U32(i)) => i.len() / 3,
                Some(Indices::U16(i)) => i.len() / 3,
                None => 0,
            };
        }
    }
    info!(
        "gfx stats: {} entities, {} mesh entities ({} visible, {:.2} M triangles), {} mesh assets, {} materials, \
         {} building models, {} car models, {} trees",
        entities.iter().count(),
        mesh_entities.iter().count(),
        visible,
        tris as f64 / 1e6,
        meshes.len(),
        materials.len(),
        buildings.count(),
        vehicles.count(),
        trees.0,
    );
}
