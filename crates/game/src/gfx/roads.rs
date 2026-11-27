//! Draws `world.roads` (the graph only — never the grid) with the geometry
//! from `road_geom`. The network is split into square chunks; when
//! `versions.roads` changes only chunks whose pieces changed are re-meshed.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::Hasher;

use bevy::prelude::*;

use super::buildings::DrivewayMouths;
use super::mesh_util::MeshBuilder;
use super::road_geom::{self, chunk_of};
use super::scene::{QUALITY_REMESH_MS, QualityApplied, WorldMaterials, drawn_quality};
use crate::shared::{GameSet, GraphicsQuality, SimWorld};

const CHUNK: f32 = 128.0;

#[derive(Default)]
struct RoadChunks {
    key: Option<((u64, u64), u64)>,
    /// Layout of the roads key it was made for.
    layout: Option<((u64, u64), road_geom::RoadLayout)>,
    quality: Option<GraphicsQuality>,
    /// Chunk -> (content hash, road entity, lamp light pools entity).
    /// Per chunk: hash of its pieces, the road mesh and the lamp-light
    /// mesh entities (`None` while that mesh would be empty: Bevy warns
    /// about meshes without vertices).
    chunks: HashMap<IVec2, (u64, Option<Entity>, Option<Entity>)>,
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

/// Give each segment the driveway mouths along its sides.
fn assign_driveways(layout: &mut road_geom::RoadLayout, points: &[Vec2]) {
    const B: f32 = 32.0;
    let mut buckets: HashMap<IVec2, Vec<Vec2>> = HashMap::new();
    for &p in points {
        buckets.entry(chunk_of(p, B)).or_default().push(p);
    }
    for s in &mut layout.segs {
        if s.style.walk <= 0.0 {
            continue;
        }
        let reach = s.style.outer() + 2.0;
        let pts: Vec<Vec2> = s.frame.pts.iter().map(|p| p.1).collect();
        let lo = pts.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p)) - reach;
        let hi = pts.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p)) + reach;
        let (b0, b1) = (chunk_of(lo, B), chunk_of(hi, B));
        for y in b0.y..=b1.y {
            for x in b0.x..=b1.x {
                for &p in buckets.get(&IVec2::new(x, y)).map(Vec::as_slice).unwrap_or(&[]) {
                    let d = pts
                        .windows(2)
                        .map(|w| super::land::point_seg(p, w[0], w[1]))
                        .fold(f32::MAX, f32::min);
                    if d < reach {
                        s.avoid.push(p);
                    }
                }
            }
        }
        s.avoid.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    }
}

pub struct RoadsPlugin;

impl Plugin for RoadsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, init_pools).add_systems(
            Update,
            rebuild_roads
                .in_set(GameSet::Render)
                .after(super::buildings::BuildingsSync),
        );
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

#[allow(clippy::too_many_arguments)]
fn rebuild_roads(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mut state: Local<RoadChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mats: Res<WorldMaterials>,
    pools: Res<LampPools>,
    (requested, applied): (Res<GraphicsQuality>, Option<Res<QualityApplied>>),
    mouths: Res<DrivewayMouths>,
) {
    let quality = drawn_quality(applied.as_deref(), *requested);
    let roads_key = sim.key(sim.world.versions.roads);
    let key = (roads_key, mouths.version);
    if state.key == Some(key) && state.quality == Some(quality) {
        return;
    }
    // Only the quality changed (street furniture): a few chunks per frame.
    let quality_only = state.key == Some(key);
    state.key = Some(key);
    state.quality = Some(quality);
    let furniture: f32 = match quality {
        GraphicsQuality::Low => 0.0,
        GraphicsQuality::Medium => 0.5,
        GraphicsQuality::High => 1.0,
    };
    // The layout changes only with the roads; driveways only move what
    // the sidewalks keep clear.
    if state.layout.as_ref().is_none_or(|(k, _)| *k != roads_key) {
        state.layout = Some((roads_key, road_geom::layout(&sim.world)));
    }
    // Borrowed out of the state while the chunks are updated.
    let (layout_key, mut layout) = state.layout.take().expect("layout");
    for seg in &mut layout.segs {
        seg.avoid.clear();
    }
    assign_driveways(&mut layout, &mouths.points);
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
        let c = chunk_of(if s.id.is_none() { s.b() } else { s.mid() }, CHUNK);
        segs.entry(c).or_default().push(i);
        let h = hashers.entry(c).or_default();
        h.write_u8(2);
        road_geom::hash_seg(s, h);
    }
    let hashes: HashMap<IVec2, u64> = hashers
        .into_iter()
        .map(|(c, mut h)| {
            h.write_u32(furniture.to_bits());
            (c, h.finish())
        })
        .collect();
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
            for e in e.into_iter().chain(g) {
                commands.entity(e).despawn();
            }
        }
    }
    let start = std::time::Instant::now();
    // In a fixed order, so a quality change spread over frames finishes.
    let mut order: Vec<(IVec2, u64)> = hashes.iter().map(|(c, h)| (*c, *h)).collect();
    order.sort_by_key(|(c, _)| (c.y, c.x));
    for (c, hash) in order {
        if state.chunks.get(&c).is_some_and(|(h, _, _)| *h == hash) {
            continue;
        }
        if quality_only && rebuilt > 0 && start.elapsed().as_secs_f64() * 1000.0 > QUALITY_REMESH_MS {
            // The rest next frame.
            state.quality = None;
            break;
        }
        let mut m = MeshBuilder::new();
        let mut glow = MeshBuilder::new();
        for &i in nodes.get(&c).map(Vec::as_slice).unwrap_or(&[]) {
            road_geom::mesh_node(&mut m, &layout.nodes[i]);
            if let Some(r) = &layout.nodes[i].ring {
                for (_, tip) in super::ring_geom::lamp_spots(&layout.nodes[i], r) {
                    road_geom::light_pool(&mut glow, tip, POOL_R, road_geom::WALK_Y + 0.025, Color::WHITE);
                }
            }
        }
        for &i in segs.get(&c).map(Vec::as_slice).unwrap_or(&[]) {
            road_geom::mesh_segment(&mut m, &layout.segs[i]);
            road_geom::road_detail(&mut m, &layout.segs[i], furniture);
            for (_, tip) in road_geom::lamp_spots(&layout.segs[i]) {
                road_geom::light_pool(&mut glow, tip, POOL_R, road_geom::WALK_Y + 0.025, Color::WHITE);
            }
        }
        rebuilt += 1;
        let (h, e, g) = state.chunks.entry(c).or_insert((hash, None, None));
        *h = hash;
        // A chunk can hold a node without a surface of its own, and most
        // chunks hold no lamps: empty meshes get no entity at all.
        match (e.as_ref(), m.is_empty()) {
            (Some(&ent), false) => {
                commands.entity(ent).insert(Mesh3d(meshes.add(m.build())));
            }
            (None, false) => {
                *e = Some(
                    commands
                        .spawn((
                            Name::new(format!("Roads {c}")),
                            Mesh3d(meshes.add(m.build())),
                            MeshMaterial3d(mats.lit.clone()),
                            Transform::default(),
                        ))
                        .id(),
                );
            }
            (Some(&ent), true) => {
                commands.entity(ent).despawn();
                *e = None;
            }
            (None, true) => {}
        }
        match (g.as_ref(), glow.is_empty()) {
            (Some(&ent), false) => {
                commands.entity(ent).insert(Mesh3d(meshes.add(glow.build())));
            }
            (None, false) => {
                *g = Some(
                    commands
                        .spawn((
                            Name::new(format!("Lamp light {c}")),
                            LampPoolMesh,
                            Mesh3d(meshes.add(glow.build())),
                            MeshMaterial3d(pools.material.clone()),
                            Transform::default(),
                            Visibility::Hidden,
                            bevy::light::NotShadowCaster,
                        ))
                        .id(),
                );
            }
            (Some(&ent), true) => {
                commands.entity(ent).despawn();
                *g = None;
            }
            (None, true) => {}
        }
    }
    debug!("roads: rebuilt {rebuilt} of {} chunks", state.chunks.len());
    state.layout = Some((layout_key, layout));
}
