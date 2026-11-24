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
    /// Chunk -> (content hash, entity).
    chunks: HashMap<IVec2, (u64, Entity)>,
}

pub struct RoadsPlugin;

impl Plugin for RoadsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, rebuild_roads.in_set(GameSet::Render));
    }
}

fn rebuild_roads(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mut state: Local<RoadChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mats: Res<WorldMaterials>,
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
        if let Some((_, e)) = state.chunks.remove(&c) {
            commands.entity(e).despawn();
        }
    }
    for (&c, &hash) in &hashes {
        if state.chunks.get(&c).is_some_and(|(h, _)| *h == hash) {
            continue;
        }
        let mut m = MeshBuilder::new();
        for &i in nodes.get(&c).map(Vec::as_slice).unwrap_or(&[]) {
            road_geom::mesh_node(&mut m, &layout.nodes[i]);
        }
        for &i in segs.get(&c).map(Vec::as_slice).unwrap_or(&[]) {
            road_geom::mesh_segment(&mut m, &layout.segs[i]);
        }
        let mesh = meshes.add(m.build());
        rebuilt += 1;
        match state.chunks.get_mut(&c) {
            Some((h, e)) => {
                *h = hash;
                commands.entity(*e).insert(Mesh3d(mesh));
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
                state.chunks.insert(c, (hash, e));
            }
        }
    }
    debug!("roads: rebuilt {rebuilt} of {} chunks", state.chunks.len());
}
