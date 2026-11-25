//! Draws `world.roads` (the graph only — never the grid) with the geometry
//! from `road_geom`. The network is split into square chunks; when
//! `versions.roads` changes only chunks whose pieces changed are re-meshed.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::Hasher;

use bevy::prelude::*;

use super::mesh_util::MeshBuilder;
use super::road_geom::{self, chunk_of};
use super::scene::WorldMaterials;
use crate::shared::{GameSet, SimWorld};

const CHUNK: f32 = 128.0;

#[derive(Default)]
struct RoadChunks {
    key: Option<(u64, u64)>,
    /// Chunk -> (content hash, road entity, lamp light pools entity).
    chunks: HashMap<IVec2, (u64, Entity, Entity)>,
}

/// Additive material of the street lamps' light pools; its brightness
/// follows the night (set by `daynight`).
#[derive(Resource, Clone)]
pub struct LampPools {
    pub material: Handle<StandardMaterial>,
}

/// Marker for the light pool meshes (shown only after dusk).
#[derive(Component)]
pub struct LampPoolMesh;

/// Radius of a lamp's light pool on the ground, m.
const POOL_R: f32 = 7.5;

pub struct RoadsPlugin;

impl Plugin for RoadsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, init_pools)
            .add_systems(Update, rebuild_roads.in_set(GameSet::Render));
    }
}

fn init_pools(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(LampPools {
        material: materials.add(StandardMaterial {
            base_color: Color::BLACK,
            unlit: true,
            alpha_mode: AlphaMode::Add,
            ..default()
        }),
    });
}

fn rebuild_roads(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mut state: Local<RoadChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mats: Res<WorldMaterials>,
    pools: Res<LampPools>,
) {
    let key = sim.key(sim.world.versions.roads);
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    let layout = road_geom::layout(&sim.world.roads);
    // Bucket pieces and hash each chunk's content.
    let mut nodes: HashMap<IVec2, Vec<usize>> = HashMap::new();
    let mut segs: HashMap<IVec2, Vec<usize>> = HashMap::new();
    let mut hashers: HashMap<IVec2, DefaultHasher> = HashMap::new();
    for (i, n) in layout.nodes.iter().enumerate() {
        let c = chunk_of(n.pos, CHUNK);
        nodes.entry(c).or_default().push(i);
        let h = hashers.entry(c).or_default();
        h.write_u8(1);
        road_geom::hash_node(n, h);
    }
    for (i, s) in layout.segs.iter().enumerate() {
        // The off-map highway goes in the chunk of its map end.
        let c = chunk_of(if s.id.is_none() { s.b } else { s.mid() }, CHUNK);
        segs.entry(c).or_default().push(i);
        let h = hashers.entry(c).or_default();
        h.write_u8(2);
        road_geom::hash_seg(s, h);
    }
    let hashes: HashMap<IVec2, u64> = hashers.into_iter().map(|(c, h)| (c, h.finish())).collect();
    let mut rebuilt = 0;
    // Drop chunks that no longer have roads.
    let gone: HashSet<IVec2> = state
        .chunks
        .keys()
        .filter(|c| !hashes.contains_key(c))
        .copied()
        .collect();
    for c in gone {
        if let Some((_, e, g)) = state.chunks.remove(&c) {
            commands.entity(e).despawn();
            commands.entity(g).despawn();
        }
    }
    for (&c, &hash) in &hashes {
        if state.chunks.get(&c).is_some_and(|(h, _, _)| *h == hash) {
            continue;
        }
        let mut m = MeshBuilder::new();
        let mut glow = MeshBuilder::new();
        for &i in nodes.get(&c).map(Vec::as_slice).unwrap_or(&[]) {
            road_geom::mesh_node(&mut m, &layout.nodes[i]);
        }
        for &i in segs.get(&c).map(Vec::as_slice).unwrap_or(&[]) {
            road_geom::mesh_segment(&mut m, &layout.segs[i]);
            for (_, tip) in road_geom::lamp_spots(&layout.segs[i]) {
                road_geom::light_pool(&mut glow, tip, POOL_R, road_geom::WALK_Y + 0.025, Color::WHITE);
            }
        }
        let mesh = meshes.add(m.build());
        let glow_mesh = meshes.add(glow.build());
        rebuilt += 1;
        match state.chunks.get_mut(&c) {
            Some((h, e, g)) => {
                *h = hash;
                commands.entity(*e).insert(Mesh3d(mesh));
                commands.entity(*g).insert(Mesh3d(glow_mesh));
            }
            None => {
                let e = commands
                    .spawn((
                        Name::new(format!("Roads {c}")),
                        Mesh3d(mesh),
                        MeshMaterial3d(mats.lit.clone()),
                        Transform::default(),
                    ))
                    .id();
                let g = commands
                    .spawn((
                        Name::new(format!("Lamp light {c}")),
                        LampPoolMesh,
                        Mesh3d(glow_mesh),
                        MeshMaterial3d(pools.material.clone()),
                        Transform::default(),
                        Visibility::Hidden,
                        bevy::light::NotShadowCaster,
                    ))
                    .id();
                state.chunks.insert(c, (hash, e, g));
            }
        }
    }
    debug!("roads: rebuilt {rebuilt} of {} chunks", state.chunks.len());
}
