//! Draws `world.roads` (the graph only — never the grid) as flat meshes,
//! rebuilt when `versions.roads` changes.

use bevy::prelude::*;
use citysim_sim::{RoadKind, RoadNetwork};

use crate::mesh_util::FlatMeshBuilder;
use crate::sim_driver::SimWorld;
use crate::{GameSet, coords};

pub const STREET_WIDTH: f32 = 6.0;
pub const HIGHWAY_WIDTH: f32 = 7.5;
const STREET_COLOR: Color = Color::srgb(0.22, 0.22, 0.24);
const HIGHWAY_COLOR: Color = Color::srgb(0.15, 0.15, 0.17);

#[derive(Component)]
struct RoadMesh(RoadKind);

pub struct RoadRenderPlugin;

impl Plugin for RoadRenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_road_entities)
            .add_systems(Update, rebuild_roads.in_set(GameSet::Render));
    }
}

fn spawn_road_entities(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    for (kind, color) in [(RoadKind::Street, STREET_COLOR), (RoadKind::Highway, HIGHWAY_COLOR)] {
        commands.spawn((
            Name::new(format!("Roads {kind:?}")),
            RoadMesh(kind),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                perceptual_roughness: 0.9,
                ..default()
            })),
            Transform::default(),
        ));
    }
}

/// One mesh per road kind: a strip per segment plus a disc per node end so
/// joints and diagonals look closed.
pub fn road_mesh(roads: &RoadNetwork, kind: RoadKind) -> Option<Mesh> {
    let (width, y) = match kind {
        RoadKind::Street => (STREET_WIDTH, coords::ROAD_Y),
        RoadKind::Highway => (HIGHWAY_WIDTH, coords::HIGHWAY_Y),
    };
    let mut b = FlatMeshBuilder::new();
    for seg in roads.segments.values().filter(|s| s.kind == kind) {
        let a = coords::sim_to_world(roads.node_pos(seg.a), y);
        let c = coords::sim_to_world(roads.node_pos(seg.b), y);
        b.strip(a, c, width);
        b.disc(a, width * 0.5, 12);
        b.disc(c, width * 0.5, 12);
    }
    (!b.is_empty()).then(|| b.build())
}

fn rebuild_roads(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mut built: Local<Option<(u64, u64)>>,
    mut meshes: ResMut<Assets<Mesh>>,
    q: Query<(Entity, &RoadMesh)>,
) {
    let key = sim.key(sim.world.versions.roads);
    if *built == Some(key) {
        return;
    }
    *built = Some(key);
    for (e, RoadMesh(kind)) in &q {
        match road_mesh(&sim.world.roads, *kind) {
            Some(mesh) => commands.entity(e).insert(Mesh3d(meshes.add(mesh))),
            None => commands.entity(e).remove::<Mesh3d>(),
        };
    }
}
