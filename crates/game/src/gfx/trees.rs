//! Trees and small props scattered deterministically over open land.
//!
//! * Inside the map: woods and copses from a noise field, cleared from
//!   cells that are zoned, built on, roads, or next to a road. Merged into
//!   one mesh per 32x32-cell chunk; a chunk is re-meshed only when its
//!   cells change. Density follows `GraphicsQuality`.
//! * Around the map: cheaper trees in the countryside out to the fog, kept
//!   clear of the highway corridor.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Cell, Grid, MAP_CELLS, MAP_SIZE_M, Occupant, World};

use super::mesh_util::MeshBuilder;
use super::noise::{self, Stream};
use super::palette::{self, mix, shade};
use super::props;
use super::scene::{WorldMaterials, quality_settings};
use crate::shared::{GameSet, GraphicsQuality, SimWorld};

const CHUNK: i32 = 32;
/// How far the countryside trees reach beyond the map edge, metres.
const OUTSIDE_REACH: f32 = 1100.0;
/// Countryside chunk size, metres.
const OUT_CHUNK: f32 = 512.0;

#[derive(Default)]
struct TreeChunks {
    key: Option<((u64, u64), GraphicsQuality)>,
    /// Chunk -> (cells hash, entity, trees placed).
    chunks: HashMap<IVec2, (u64, Entity, usize)>,
    outside: Vec<Entity>,
    outside_key: Option<(GraphicsQuality, Option<[u32; 2]>)>,
}

/// Number of trees currently placed (for stats).
#[derive(Resource, Default)]
pub struct TreeCount(pub usize);

pub struct TreesPlugin;

impl Plugin for TreesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TreeCount>()
            .add_systems(Update, (rebuild_trees, rebuild_outside).in_set(GameSet::Render));
    }
}

/// Expected trees per cell at a point (0 in meadows, up to ~4 in woods).
fn density(x: f32, z: f32) -> f32 {
    let n = noise::fbm(x / 260.0, z / 260.0, 4, 77);
    let woods = ((n - 0.5) / 0.2).clamp(0.0, 1.0);
    let sparse = 0.04 + 0.08 * noise::value(x / 60.0, z / 60.0, 91);
    woods * woods * 3.6 + sparse
}

#[derive(Clone, Copy)]
struct Tree {
    pos: Vec3,
    scale: f32,
    pine: bool,
    canopy: Color,
    yaw: f32,
    /// Bush or rock instead of a tree.
    small: u8,
}

/// The trees of one cell, before quality thinning.
fn cell_trees(cx: i32, cy: i32, out: &mut Vec<Tree>) {
    let h = noise::hash2(cx, cy, 0x7EE5);
    let (x0, z0) = (cx as f32 * CELL_SIZE, cy as f32 * CELL_SIZE);
    let d = density(x0 + 4.0, z0 + 4.0);
    let mut rng = Stream::new(h);
    let mut n = d.floor() as u32;
    if rng.f() < d.fract() {
        n += 1;
    }
    let pine_bias = noise::value(x0 / 400.0, z0 / 400.0, 5);
    for _ in 0..n.min(5) {
        let pos = Vec3::new(x0 + rng.range(1.0, 7.0), 0.0, z0 + rng.range(1.0, 7.0));
        let canopy = mix(rng.pick(&palette::CANOPIES), palette::GRASS_LIGHT, rng.range(0.0, 0.18));
        out.push(Tree {
            pos,
            scale: rng.range(0.7, 1.25),
            pine: rng.f() < 0.15 + 0.6 * pine_bias,
            canopy,
            yaw: rng.range(0.0, std::f32::consts::TAU),
            small: 0,
        });
    }
    // Meadow clutter: a few bushes and rocks.
    if d < 0.3 && rng.chance(0.05) {
        out.push(Tree {
            pos: Vec3::new(x0 + rng.range(1.5, 6.5), 0.0, z0 + rng.range(1.5, 6.5)),
            scale: rng.range(0.7, 1.4),
            pine: false,
            canopy: shade(rng.pick(&palette::CANOPIES), 1.08),
            yaw: rng.range(0.0, 6.0),
            small: if rng.chance(0.3) { 2 } else { 1 },
        });
    }
}

fn keep(t: &Tree, density: f32) -> bool {
    noise::unit(noise::hash2(t.pos.x as i32 * 13, t.pos.z as i32 * 7, 99)) < density
}

fn draw_tree(m: &mut MeshBuilder, t: &Tree, cheap: bool) {
    match (t.small, cheap) {
        (1, _) => props::bush(m, t.pos, t.scale, t.canopy, t.yaw),
        (2, _) => props::blob(
            m,
            t.pos + Vec3::Y * 0.15 * t.scale,
            Vec3::new(0.7, 0.45, 0.55) * t.scale,
            Color::srgb_u8(0x9C, 0x99, 0x90),
            t.yaw,
        ),
        (_, false) if t.pine => props::pine_tree(m, t.pos, t.scale, t.canopy, t.yaw),
        (_, false) => props::round_tree(m, t.pos, t.scale, t.canopy, t.yaw),
        (_, true) if t.pine => m.cone(t.pos, 1.9 * t.scale, 7.5 * t.scale, 5, t.canopy, t.yaw),
        (_, true) => {
            props::blob(
                m,
                t.pos + Vec3::Y * 3.6 * t.scale,
                Vec3::new(2.3, 2.6, 2.3) * t.scale,
                t.canopy,
                t.yaw,
            );
            m.cylinder(t.pos, 0.25 * t.scale, 1.5 * t.scale, 3, palette::TRUNK, palette::TRUNK);
        }
    }
}

/// Cells where trees may stand: open land away from roads.
fn open(grid: &Grid, c: Cell) -> bool {
    let Some(cd) = grid.get(c) else { return true };
    if cd.zone.is_some() || cd.occupant != Occupant::Empty {
        return false;
    }
    for dy in -1..=1 {
        for dx in -1..=1 {
            if grid.get(c.offset(dx, dy)).is_some_and(|n| n.occupant == Occupant::Road) {
                return false;
            }
        }
    }
    true
}

fn chunk_hash(grid: &Grid, cx: i32, cy: i32) -> u64 {
    let mut h = DefaultHasher::new();
    for y in cy * CHUNK..(cy + 1) * CHUNK {
        for x in cx * CHUNK..(cx + 1) * CHUNK {
            open(grid, Cell::new(x, y)).hash(&mut h);
        }
    }
    h.finish()
}

fn rebuild_trees(
    mut commands: Commands,
    sim: Res<SimWorld>,
    quality: Res<GraphicsQuality>,
    mats: Res<WorldMaterials>,
    mut state: Local<TreeChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut count: ResMut<TreeCount>,
) {
    let key = (sim.key(sim.world.versions.grid), *quality);
    if state.key == Some(key) {
        return;
    }
    let quality_changed = state.key.is_some_and(|(_, q)| q != *quality);
    state.key = Some(key);
    let grid = &sim.world.grid;
    let dens = quality_settings(*quality).tree_density;
    let n = MAP_CELLS / CHUNK;
    let mut trees = Vec::new();
    for cy in 0..n {
        for cx in 0..n {
            let c = IVec2::new(cx, cy);
            let hash = chunk_hash(grid, cx, cy);
            if !quality_changed && state.chunks.get(&c).is_some_and(|(h, _, _)| *h == hash) {
                continue;
            }
            let mut m = MeshBuilder::new();
            let mut placed = 0;
            for y in cy * CHUNK..(cy + 1) * CHUNK {
                for x in cx * CHUNK..(cx + 1) * CHUNK {
                    if !open(grid, Cell::new(x, y)) {
                        continue;
                    }
                    trees.clear();
                    cell_trees(x, y, &mut trees);
                    for t in trees.iter().filter(|t| keep(t, dens)) {
                        draw_tree(&mut m, t, false);
                        placed += usize::from(t.small == 0);
                    }
                }
            }
            let old = state.chunks.get(&c).map(|(_, e, _)| *e);
            let e = match old {
                Some(e) => e,
                None => commands
                    .spawn((
                        Name::new("Trees"),
                        MeshMaterial3d(mats.lit.clone()),
                        Transform::default(),
                        Visibility::default(),
                    ))
                    .id(),
            };
            if m.is_empty() {
                commands.entity(e).remove::<Mesh3d>();
            } else {
                commands.entity(e).insert(Mesh3d(meshes.add(m.build())));
            }
            state.chunks.insert(c, (hash, e, placed));
        }
    }
    count.0 = state.chunks.values().map(|(_, _, n)| n).sum();
}

/// Countryside trees around the map (static; rebuilt on quality change).
fn rebuild_outside(
    mut commands: Commands,
    sim: Res<SimWorld>,
    quality: Res<GraphicsQuality>,
    mats: Res<WorldMaterials>,
    mut state: Local<TreeChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let entry = highway_corridor(&sim.world);
    let key = (*quality, entry.map(|(p, _)| [p.x.to_bits(), p.y.to_bits()]));
    if state.outside_key == Some(key) {
        return;
    }
    state.outside_key = Some(key);
    for e in state.outside.drain(..) {
        commands.entity(e).despawn();
    }
    let dens = quality_settings(*quality).tree_density * 0.55;
    let lo = -OUTSIDE_REACH;
    let hi = MAP_SIZE_M + OUTSIDE_REACH;
    let n = ((hi - lo) / OUT_CHUNK).ceil() as i32;
    let mut trees = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let (x0, z0) = (lo + i as f32 * OUT_CHUNK, lo + j as f32 * OUT_CHUNK);
            let mut m = MeshBuilder::new();
            let (c0x, c0y) = ((x0 / CELL_SIZE).floor() as i32, (z0 / CELL_SIZE).floor() as i32);
            let cells = (OUT_CHUNK / CELL_SIZE) as i32;
            for cy in c0y..c0y + cells {
                for cx in c0x..c0x + cells {
                    if Cell::new(cx, cy).in_bounds() {
                        continue;
                    }
                    let centre = Vec2::new((cx as f32 + 0.5) * CELL_SIZE, (cy as f32 + 0.5) * CELL_SIZE);
                    // Outside the reach, or on the highway.
                    let out = (-centre.x)
                        .max(-centre.y)
                        .max(centre.x - MAP_SIZE_M)
                        .max(centre.y - MAP_SIZE_M);
                    if out > OUTSIDE_REACH {
                        continue;
                    }
                    if let Some((p, d)) = entry {
                        let rel = centre - p;
                        if rel.dot(d) > -20.0 && rel.perp_dot(d).abs() < 22.0 {
                            continue;
                        }
                    }
                    trees.clear();
                    cell_trees(cx, cy, &mut trees);
                    // Thin out with distance so the edge melts into the fog.
                    let fade = 1.0 - (out / OUTSIDE_REACH) * 0.5;
                    for t in trees.iter().filter(|t| keep(t, dens * fade) && t.small == 0) {
                        draw_tree(&mut m, t, true);
                    }
                }
            }
            if m.is_empty() {
                continue;
            }
            let e = commands
                .spawn((
                    Name::new("Countryside trees"),
                    Mesh3d(meshes.add(m.build())),
                    MeshMaterial3d(mats.lit.clone()),
                    Transform::default(),
                ))
                .id();
            state.outside.push(e);
        }
    }
}

/// The highway entry point and the direction pointing off the map.
fn highway_corridor(world: &World) -> Option<(Vec2, Vec2)> {
    let entry = world.roads.entry?;
    let node = world.roads.nodes.get(entry)?;
    let seg = world.roads.segments.get(*node.segments.first()?)?;
    let other = world.roads.nodes.get(seg.other(entry))?;
    let p = Vec2::from(node.pos);
    let d = (p - Vec2::from(other.pos)).normalize_or_zero();
    Some((p, d))
}
