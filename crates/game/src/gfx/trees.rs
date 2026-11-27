//! Trees and small props scattered deterministically over open land.
//!
//! * Inside the map: woods and copses from a noise field, cleared from
//!   zoned or built land (zoning cells of any lattice) and from the drawn
//!   roads and their verges. Merged into one mesh per 256 m chunk; a chunk
//!   is re-meshed only when the zoning or the roads in it change. Density
//!   follows `GraphicsQuality`.
//! * Around the map: cheaper trees in the countryside out to the fog, kept
//!   clear of the highway corridor.
//! * While a road is being drawn (`CurveToolView`), the trees its ghost
//!   would clear are left out of the chunks under it (re-meshed a few times
//!   a second at most), so the preview shows the land it takes.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Cell, MAP_CELLS, MAP_SIZE_M, World};

use super::greens::{Greens, GreensUpdate};
use super::land::RoadEdges;
use super::mesh_util::MeshBuilder;
use super::noise::{self, Stream};
use super::palette::{self, mix, shade};
use super::props;
use super::road_geom::chunk_of;
use super::scene::{QUALITY_REMESH_MS, QualityApplied, WorldMaterials, drawn_quality, quality_settings};
use crate::shared::{CurveToolView, GameSet, GraphicsQuality, SimWorld};

const CHUNK: i32 = 32;
/// How far the countryside trees reach beyond the map edge, metres.
const OUTSIDE_REACH: f32 = 1100.0;
/// Camera distance ranges for detailed and distant tree chunks (measured
/// to the chunk centre, so generous).
const TREE_LOD_NEAR: (std::ops::Range<f32>, std::ops::Range<f32>) = (0.0..0.0, 600.0..650.0);
const TREE_LOD_FAR: (std::ops::Range<f32>, std::ops::Range<f32>) = (600.0..650.0, 1e6..1e6);
/// Countryside chunk size, metres.
const OUT_CHUNK: f32 = 512.0;

/// Grid and roads keys.
type Keys = ((u64, u64), (u64, u64));

/// (keys, ghost key and `Greens::version`, chunk hashes, taken land of the
/// whole map once a quality change needs it).
type Land = ((Keys, u64, u64), HashMap<IVec2, u64>, Option<Taken>);

#[derive(Default)]
struct TreeChunks {
    key: Option<(Keys, GraphicsQuality, u64)>,
    /// `Greens::version` the chunks were made for.
    greens: u64,
    /// Some chunks still show another quality's density (a quality change
    /// re-meshes a few chunks per frame, `QUALITY_REMESH_MS`).
    stale: bool,
    /// Seconds since the last re-mesh for a changed road ghost.
    since_ghost: f32,
    /// Chunk -> (cells hash, [near, far] entities, trees placed, density
    /// quality it was meshed for).
    chunks: HashMap<IVec2, (u64, [Entity; 2], usize, GraphicsQuality)>,
    /// Countryside chunks (by index) and the quality each was meshed for.
    outside: Vec<(Option<Entity>, GraphicsQuality)>,
    outside_key: Option<(GraphicsQuality, Option<[u32; 2]>)>,
    outside_entry: Option<[u32; 2]>,
    /// Chunk hashes and the taken land for a (keys, ghost) key, kept while
    /// a quality change is re-meshed over several frames.
    land: Option<Land>,
}

/// Number of trees currently placed (for stats).
#[derive(Resource, Default)]
pub struct TreeCount(pub usize);

pub struct TreesPlugin;

impl Plugin for TreesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TreeCount>().add_systems(
            Update,
            (rebuild_trees, rebuild_outside)
                .in_set(GameSet::Render)
                .after(super::land::LandUpdate)
                .after(GreensUpdate),
        );
    }
}

/// Expected trees per cell at a point (0 in meadows, up to ~4 in woods).
fn density(x: f32, z: f32) -> f32 {
    let n = noise::fbm(x / 260.0, z / 260.0, 4, 77);
    let woods = ((n - 0.5) / 0.2).clamp(0.0, 1.0);
    let sparse = 0.02 + 0.04 * noise::value(x / 60.0, z / 60.0, 91);
    woods * woods * 2.6 + sparse
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
        let mut canopy = mix(rng.pick(&palette::CANOPIES), palette::GRASS_LIGHT, rng.range(0.0, 0.18));
        // A few lighter / yellower crowns break up the woods.
        if rng.chance(0.1) {
            canopy = mix(canopy, Color::srgb_u32(0xA3AE4E), rng.range(0.3, 0.6));
        }
        out.push(Tree {
            pos,
            scale: rng.range(0.7, 1.25),
            pine: rng.f() < 0.15 + 0.6 * pine_bias,
            canopy,
            yaw: rng.range(0.0, std::f32::consts::TAU),
            small: 0,
        });
    }
    // Undergrowth in and at the edge of woods.
    if d > 0.4 && rng.chance(0.4) {
        out.push(Tree {
            pos: Vec3::new(x0 + rng.range(1.0, 7.0), 0.0, z0 + rng.range(1.0, 7.0)),
            scale: rng.range(0.9, 1.6),
            pine: false,
            canopy: shade(rng.pick(&palette::CANOPIES), rng.range(0.85, 1.1)),
            yaw: rng.range(0.0, 6.0),
            small: if rng.chance(0.12) { 2 } else { 1 },
        });
    }
    // Meadow clutter: a few bushes and rocks.
    if d < 0.3 && rng.chance(0.07) {
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
        (_, true) if t.small > 0 => {}
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
            props::octa_blob(
                m,
                t.pos + Vec3::Y * 4.0 * t.scale,
                Vec3::new(2.4, 2.6, 2.4) * t.scale,
                t.canopy,
                t.yaw,
            );
            m.cylinder(t.pos, 0.22 * t.scale, 1.6 * t.scale, 3, palette::TRUNK, palette::TRUNK);
        }
    }
}

/// Zoned or built cells (their quads on whatever lattice), bucketed by
/// 16 m squares, for clearing trees from them.
struct Taken(HashMap<IVec2, Vec<[Vec2; 4]>>);

impl Taken {
    fn new(world: &World) -> Taken {
        Taken::new_in(world, &[(Vec2::splat(f32::MIN), Vec2::splat(f32::MAX))])
    }

    /// Only cells whose centre lies within 16 m of one of the rectangles
    /// (enough for every tree inside them).
    fn new_in(world: &World, rects: &[(Vec2, Vec2)]) -> Taken {
        let mut out: HashMap<IVec2, Vec<[Vec2; 4]>> = HashMap::new();
        for (c, zc) in world.zoning.cells() {
            if zc.zone.is_none() && zc.building.is_none() {
                continue;
            }
            let Some(p) = world.zoning.cell_center(c) else { continue };
            let p = Vec2::from(p);
            if !rects
                .iter()
                .any(|(lo, hi)| p.x >= lo.x - 16.0 && p.y >= lo.y - 16.0 && p.x <= hi.x + 16.0 && p.y <= hi.y + 16.0)
            {
                continue;
            }
            let Some(q) = world.zoning.cell_corners(c) else {
                continue;
            };
            let q = q.map(Vec2::from);
            let lo = q.iter().fold(Vec2::splat(f32::MAX), |a, p| a.min(*p));
            let hi = q.iter().fold(Vec2::splat(f32::MIN), |a, p| a.max(*p));
            let (b0, b1) = (chunk_of(lo, 16.0), chunk_of(hi, 16.0));
            for y in b0.y..=b1.y {
                for x in b0.x..=b1.x {
                    out.entry(IVec2::new(x, y)).or_default().push(q);
                }
            }
        }
        Taken(out)
    }

    fn contains(&self, p: Vec2) -> bool {
        self.0.get(&chunk_of(p, 16.0)).is_some_and(|qs| {
            qs.iter()
                .any(|q| citysim_sim::zoning::in_quad(&q.map(|v| v.into()), p.into()))
        })
    }
}

/// The road being drawn, as line pieces with the half width it clears.
#[derive(Default)]
struct Ghost {
    segs: Vec<(Vec2, Vec2, f32)>,
}

impl Ghost {
    fn of(view: &CurveToolView) -> Ghost {
        let mut segs = Vec::new();
        if view.active {
            let hw = view.kind.map_or(4.0, |k| k.half_width_m()) + 2.5;
            let curves = view.pieces.iter().map(|(c, _)| c).chain(view.refused.iter());
            for c in curves {
                let pts = c.flatten(0.5);
                for w in pts.windows(2) {
                    segs.push((Vec2::from(w[0]), Vec2::from(w[1]), hw));
                }
            }
        }
        Ghost { segs }
    }

    /// Hash of the geometry (quantised to half a metre).
    fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        for (a, b, w) in &self.segs {
            for v in [a.x, a.y, b.x, b.y, *w] {
                ((v * 2.0).round() as i32).hash(&mut h);
            }
        }
        h.finish()
    }

    fn covers(&self, p: Vec2) -> bool {
        self.segs.iter().any(|(a, b, w)| {
            let ab = *b - *a;
            let t = ((p - *a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
            (*a + ab * t).distance(p) < *w
        })
    }
}

/// Whether a tree may stand at `p`: away from the drawn roads and the road
/// being drawn, and not on zoned or built land.
fn open(taken: &Taken, edges: &RoadEdges, ghost: &Ghost, p: Vec2) -> bool {
    !taken.contains(p) && edges.clearance(p) > 2.5 && !ghost.covers(p)
}

/// What a tree chunk depends on: zoning cells with a zone or building whose
/// centre is in it (or near), the roads near it (by their geometry) and the
/// greens over it.
fn chunk_hashes(world: &World, ghost: &Ghost, greens: &Greens) -> HashMap<IVec2, u64> {
    let size = CHUNK as f32 * CELL_SIZE;
    let mut hashers: HashMap<IVec2, DefaultHasher> = HashMap::new();
    let mut add = |lo: Vec2, hi: Vec2, f: &dyn Fn(&mut DefaultHasher)| {
        let (c0, c1) = (chunk_of(lo, size), chunk_of(hi, size));
        for y in c0.y..=c1.y {
            for x in c0.x..=c1.x {
                f(hashers.entry(IVec2::new(x, y)).or_default());
            }
        }
    };
    for (c, zc) in world.zoning.cells() {
        if zc.zone.is_none() && zc.building.is_none() {
            continue;
        }
        let Some(p) = world.zoning.cell_center(c) else { continue };
        let p = Vec2::from(p);
        // Only whether the cell is taken matters (a building on an
        // already zoned cell changes nothing for the trees).
        add(p - 6.0, p + 6.0, &|h| c.hash(h));
    }
    for (a, b, w) in &ghost.segs {
        let (lo, hi) = (a.min(*b) - *w, a.max(*b) + *w);
        add(lo, hi, &|h| {
            for v in [a.x, a.y, b.x, b.y] {
                ((v * 2.0).round() as i32).hash(h);
            }
        });
    }
    // Greens keep trees out: a chunk a green covers changes with it.
    for g in &greens.areas {
        let lo = Vec2::from(g.origin);
        let hi = lo + Vec2::new(g.size[0] as f32, g.size[1] as f32) * g.step;
        add(lo, hi, &|h| (g.face, g.signature).hash(h));
    }
    for (_, seg) in world.roads.segments.iter() {
        let (lo, hi) = seg.curve.bounds();
        let pad = 16.0;
        add(Vec2::from(lo) - pad, Vec2::from(hi) + pad, &|h| {
            for p in seg.curve.p {
                p[0].to_bits().hash(h);
                p[1].to_bits().hash(h);
            }
            (seg.kind as u8).hash(h);
        });
    }
    hashers.into_iter().map(|(c, h)| (c, h.finish())).collect()
}

#[allow(clippy::too_many_arguments)]
fn rebuild_trees(
    mut commands: Commands,
    sim: Res<SimWorld>,
    (requested, applied): (Res<GraphicsQuality>, Option<Res<QualityApplied>>),
    mats: Res<WorldMaterials>,
    mut state: Local<TreeChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut count: ResMut<TreeCount>,
    edges: Res<RoadEdges>,
    (view, time, greens): (Option<Res<CurveToolView>>, Res<Time<Real>>, Res<Greens>),
    tool_time: Res<super::bench::ToolDrawTime>,
) {
    let _timed = tool_time.time();
    state.since_ghost += time.delta_secs();
    let quality = drawn_quality(applied.as_deref(), *requested);
    let ghost = view.map(|v| Ghost::of(&v)).unwrap_or_default();
    let keys = (sim.key(sim.world.versions.grid), sim.key(sim.world.versions.roads));
    let key = (keys, quality, ghost.key());
    if state.key == Some(key) && state.greens == greens.version && !state.stale {
        return;
    }
    // Only the ghost moved: re-mesh at most a few times a second.
    let ghost_only = state
        .key
        .is_some_and(|(k, q, g)| k == keys && q == quality && g != key.2)
        && state.greens == greens.version;
    if ghost_only && state.since_ghost < 0.2 {
        return;
    }
    if ghost_only {
        state.since_ghost = 0.0;
    }
    state.key = Some(key);
    let world = &sim.world;
    state.greens = greens.version;
    let land_key = (keys, key.2, greens.version);
    if state.land.as_ref().is_none_or(|(k, _, _)| *k != land_key) {
        state.land = Some((land_key, chunk_hashes(world, &ghost, &greens), None));
    }
    let (_, hashes, mut full) = state.land.take().expect("land");
    let dens = quality_settings(quality).tree_density;
    let n = MAP_CELLS / CHUNK;
    // Chunks whose land changed are re-meshed now; those that only show
    // another quality's density a few per frame.
    let mut changed = Vec::new();
    let mut other_quality = Vec::new();
    for cy in 0..n {
        for cx in 0..n {
            let c = IVec2::new(cx, cy);
            let hash = hashes.get(&c).copied().unwrap_or(0);
            match state.chunks.get(&c) {
                Some((h, _, _, q)) if *h == hash => {
                    if *q != quality {
                        other_quality.push((c, hash));
                    }
                }
                _ => changed.push((c, hash)),
            }
        }
    }
    if changed.is_empty() && other_quality.is_empty() {
        state.stale = false;
        state.land = Some((land_key, hashes, full));
        return;
    }
    // Only the land of the chunks re-meshed (an edit touches a few).
    let size = CHUNK as f32 * CELL_SIZE;
    let local;
    let taken = if other_quality.is_empty() {
        let rects: Vec<(Vec2, Vec2)> = changed
            .iter()
            .map(|(c, _)| (c.as_vec2() * size, (c.as_vec2() + 1.0) * size))
            .collect();
        local = Taken::new_in(world, &rects);
        &local
    } else {
        &*full.get_or_insert_with(|| Taken::new(world))
    };
    let start = std::time::Instant::now();
    let mut trees = Vec::new();
    let budget = other_quality.len();
    let mut todo = changed;
    todo.extend(other_quality);
    let urgent = todo.len() - budget;
    state.stale = false;
    for (i, (c, hash)) in todo.into_iter().enumerate() {
        if i >= urgent.max(1) && start.elapsed().as_secs_f64() * 1000.0 > QUALITY_REMESH_MS {
            state.stale = true;
            break;
        }
        let (cx, cy) = (c.x, c.y);
        {
            let mut m = MeshBuilder::new();
            let mut far = MeshBuilder::new();
            let mut placed = 0;
            for y in cy * CHUNK..(cy + 1) * CHUNK {
                for x in cx * CHUNK..(cx + 1) * CHUNK {
                    trees.clear();
                    cell_trees(x, y, &mut trees);
                    for t in trees.iter().filter(|t| {
                        let p = Vec2::new(t.pos.x, t.pos.z);
                        keep(t, dens) && open(taken, &edges, &ghost, p) && !greens.contains(p)
                    }) {
                        draw_tree(&mut m, t, false);
                        draw_tree(&mut far, t, true);
                        placed += usize::from(t.small == 0);
                    }
                }
            }
            // Meshes are local to the chunk centre: the LOD cross-fade is
            // measured from the entity origin.
            let size = CHUNK as f32 * CELL_SIZE;
            let centre = Vec3::new((cx as f32 + 0.5) * size, 0.0, (cy as f32 + 0.5) * size);
            let ents = match state.chunks.get(&c) {
                Some((_, e, _, _)) => *e,
                None => [TREE_LOD_NEAR, TREE_LOD_FAR].map(|range| {
                    commands
                        .spawn((
                            Name::new("Trees"),
                            MeshMaterial3d(mats.lit.clone()),
                            Transform::from_translation(centre),
                            Visibility::default(),
                            VisibilityRange {
                                start_margin: range.0,
                                end_margin: range.1,
                                use_aabb: false,
                            },
                        ))
                        .id()
                }),
            };
            for (e, mut mb) in ents.into_iter().zip([m, far]) {
                mb.translate(-centre);
                if mb.is_empty() {
                    commands.entity(e).insert(Visibility::Hidden); // never remove Mesh3d: see cursor.rs
                } else {
                    commands
                        .entity(e)
                        .insert((Mesh3d(meshes.add(mb.build())), Visibility::Inherited));
                }
            }
            state.chunks.insert(c, (hash, ents, placed, quality));
        }
    }
    count.0 = state.chunks.values().map(|(_, _, n, _)| n).sum();
    // Kept only while chunks wait for the new quality.
    if state.stale {
        state.land = Some((land_key, hashes, full));
    }
}

/// Countryside trees around the map (static; rebuilt on quality change).
fn rebuild_outside(
    mut commands: Commands,
    sim: Res<SimWorld>,
    (requested, applied): (Res<GraphicsQuality>, Option<Res<QualityApplied>>),
    mats: Res<WorldMaterials>,
    mut state: Local<TreeChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let quality = drawn_quality(applied.as_deref(), *requested);
    let entry = highway_corridor(&sim.world);
    let entry_key = entry.map(|(p, _)| [p.x.to_bits(), p.y.to_bits()]);
    let key = (quality, entry_key);
    if state.outside_key == Some(key) {
        return;
    }
    let lo = -OUTSIDE_REACH;
    let hi = MAP_SIZE_M + OUTSIDE_REACH;
    let n = ((hi - lo) / OUT_CHUNK).ceil() as i32;
    // A new highway corridor (a new game, a load) re-meshes everything at
    // once; a quality change a few chunks per frame, in place.
    let urgent = state.outside_entry != entry_key || state.outside.is_empty();
    if urgent {
        for (e, _) in state.outside.drain(..) {
            if let Some(e) = e {
                commands.entity(e).despawn();
            }
        }
        state.outside_entry = entry_key;
    }
    let dens = quality_settings(quality).tree_density * 0.55;
    let start = std::time::Instant::now();
    let mut trees = Vec::new();
    let mut done = true;
    for j in 0..n {
        for i in 0..n {
            let index = (j * n + i) as usize;
            if state.outside.get(index).is_some_and(|(_, q)| *q == quality) {
                continue;
            }
            if !urgent && start.elapsed().as_secs_f64() * 1000.0 > QUALITY_REMESH_MS {
                done = false;
                continue;
            }
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
                    // Full trees near the map, where the camera can get close.
                    let cheap = out > 300.0;
                    for t in trees.iter().filter(|t| keep(t, dens * fade) && t.small == 0) {
                        draw_tree(&mut m, t, cheap);
                    }
                }
            }
            if state.outside.len() <= index {
                state.outside.resize(index + 1, (None, quality));
            }
            let slot = &mut state.outside[index];
            slot.1 = quality;
            match (slot.0, m.is_empty()) {
                (Some(e), true) => {
                    commands.entity(e).insert(Visibility::Hidden); // never remove Mesh3d: see cursor.rs
                }
                (Some(e), false) => {
                    commands
                        .entity(e)
                        .insert((Mesh3d(meshes.add(m.build())), Visibility::Inherited));
                }
                (None, true) => {}
                (None, false) => {
                    slot.0 = Some(
                        commands
                            .spawn((
                                Name::new("Countryside trees"),
                                Mesh3d(meshes.add(m.build())),
                                MeshMaterial3d(mats.lit.clone()),
                                Transform::default(),
                                Visibility::Inherited,
                            ))
                            .id(),
                    );
                }
            }
        }
    }
    if done {
        state.outside_key = Some(key);
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
