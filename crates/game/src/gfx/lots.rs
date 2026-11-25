//! Zoned-but-empty land, always visible (issue 9) but far lighter than the
//! zone overlay: low surveyor's pegs with a coloured string around each
//! empty zoned area, a faint painted border on the grass, and a small
//! zone-coloured "lot" sign every few cells along the road. Shown while
//! `DisplaySettings::lot_markers` is on and the full zone overlay is not.
//!
//! Chunked like the zone overlay; a chunk is re-meshed only when its empty
//! zoned cells change.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Cell, Grid, MAP_CELLS, Occupant, RoadKind, World, Zone};

use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix, shade};
use super::scene::WorldMaterials;
use super::zones::{full_overlay, zone_color};
use crate::shared::{ActiveTool, DisplaySettings, GameSet, OverlayMode, SimWorld};

const CHUNK: i32 = 32;
/// How far a lot's edge steps back from an avenue beside it, m (the avenue
/// is drawn 16 m wide on its 8 m cell line).
const AVENUE_STEP: f32 = 4.3;

#[derive(Default)]
struct LotChunks {
    key: Option<((u64, u64), (u64, u64))>,
    shown: bool,
    chunks: HashMap<IVec2, (u64, Entity, bool)>,
}

pub struct LotsPlugin;

impl Plugin for LotsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, rebuild_lots.in_set(GameSet::Render));
    }
}

/// Zone of a cell that is zoned and still empty.
fn empty_zone(grid: &Grid, c: Cell) -> Option<Zone> {
    let d = grid.get(c)?;
    (d.occupant == Occupant::Empty).then_some(d.zone).flatten()
}

fn is_road(grid: &Grid, c: Cell) -> bool {
    grid.get(c).is_some_and(|d| d.occupant == Occupant::Road)
}

/// Cells under axis-aligned avenues.
fn avenue_cells(world: &World) -> HashSet<Cell> {
    let mut out = HashSet::new();
    let roads = &world.roads;
    for (_, seg) in roads.segments.iter() {
        if seg.kind != RoadKind::Avenue {
            continue;
        }
        let (Some(a), Some(b)) = (roads.nodes.get(seg.a), roads.nodes.get(seg.b)) else {
            continue;
        };
        let (ca, cb) = (Cell::from_pos(a.pos), Cell::from_pos(b.pos));
        let (dx, dy) = ((cb.x - ca.x).signum(), (cb.y - ca.y).signum());
        let n = (cb.x - ca.x).abs().max((cb.y - ca.y).abs());
        for k in 0..=n {
            out.insert(Cell::new(ca.x + dx * k, ca.y + dy * k));
        }
    }
    out
}

fn chunk_hash(grid: &Grid, avenues: &HashSet<Cell>, cx: i32, cy: i32) -> u64 {
    let mut h = DefaultHasher::new();
    for y in cy * CHUNK - 1..=(cy + 1) * CHUNK {
        for x in cx * CHUNK - 1..=(cx + 1) * CHUNK {
            let c = Cell::new(x, y);
            empty_zone(grid, c).hash(&mut h);
            is_road(grid, c).hash(&mut h);
            avenues.contains(&c).hash(&mut h);
        }
    }
    h.finish()
}

/// Wooden peg with a zone-coloured tip.
fn peg(m: &mut MeshBuilder, p: Vec2, tip: Color) {
    let wood = Color::srgb_u32(0xB08A5B);
    m.box_yaw(Vec3::new(p.x, 0.0, p.y), Vec3::new(0.12, 0.5, 0.12), 0.6, wood, wood);
    m.box_yaw(Vec3::new(p.x, 0.5, p.y), Vec3::new(0.16, 0.16, 0.16), 0.6, tip, tip);
}

/// Taut string between two pegs.
fn string(m: &mut MeshBuilder, a: Vec2, b: Vec2, color: Color) {
    let d = b - a;
    let len = d.length();
    if len < 1e-3 {
        return;
    }
    let mid = (a + b) * 0.5;
    let yaw = -d.y.atan2(d.x);
    m.box_yaw(
        Vec3::new(mid.x, 0.4, mid.y),
        Vec3::new(len, 0.05, 0.05),
        yaw,
        color,
        color,
    );
}

/// "Lot" sign on a post at `p`, facing `face`.
fn sign(m: &mut MeshBuilder, p: Vec2, face: Vec2, z: Color) {
    let post = Color::srgb_u32(0xE9E4D8);
    m.box_yaw(Vec3::new(p.x, 0.0, p.y), Vec3::new(0.08, 1.15, 0.08), 0.0, post, post);
    let yaw = -face.y.atan2(face.x) + std::f32::consts::FRAC_PI_2;
    let board = Vec3::new(p.x, 0.75, p.y) + Vec3::new(face.x, 0.0, face.y) * 0.06;
    m.box_yaw(board, Vec3::new(0.85, 0.5, 0.05), yaw, z, shade(z, 1.1));
    // A white band across the board, both faces.
    let n = Vec3::new(face.x, 0.0, face.y);
    let side = Vec3::new(-face.y, 0.0, face.x);
    for s in [1.0f32, -1.0] {
        let c = board + Vec3::Y * 0.27 + n * s * 0.03;
        let q = [
            c - side * 0.32 - Vec3::Y * 0.06,
            c + side * 0.32 - Vec3::Y * 0.06,
            c + side * 0.32 + Vec3::Y * 0.06,
            c - side * 0.32 + Vec3::Y * 0.06,
        ];
        m.quad_facing(q, n * s, palette::FENCE);
    }
}

fn chunk_mesh(grid: &Grid, avenues: &HashSet<Cell>, cx: i32, cy: i32) -> Option<Mesh> {
    let mut m = MeshBuilder::new();
    let mut pegs: HashMap<(i32, i32), (Vec2, Color, u8)> = HashMap::new();
    let q = |p: Vec2| ((p.x * 4.0).round() as i32, (p.y * 4.0).round() as i32);
    for y in cy * CHUNK..(cy + 1) * CHUNK {
        for x in cx * CHUNK..(cx + 1) * CHUNK {
            let c = Cell::new(x, y);
            let Some(z) = empty_zone(grid, c) else { continue };
            let base = zone_color(z);
            let tint = mix(base, Color::WHITE, 0.15);
            // The cell's drawn rectangle: stepped back from avenues.
            let mut lo = Vec2::new(x as f32, y as f32) * CELL_SIZE;
            let mut hi = lo + Vec2::splat(CELL_SIZE);
            if avenues.contains(&c.offset(-1, 0)) {
                lo.x += AVENUE_STEP;
            }
            if avenues.contains(&c.offset(1, 0)) {
                hi.x -= AVENUE_STEP;
            }
            if avenues.contains(&c.offset(0, -1)) {
                lo.y += AVENUE_STEP;
            }
            if avenues.contains(&c.offset(0, 1)) {
                hi.y -= AVENUE_STEP;
            }
            let paint = lin(mix(base, palette::GRASS_LIGHT, 0.3));
            for (dx, dy, a, b) in [
                (1, 0, Vec2::new(hi.x, lo.y), hi),
                (-1, 0, lo, Vec2::new(lo.x, hi.y)),
                (0, 1, Vec2::new(lo.x, hi.y), hi),
                (0, -1, lo, Vec2::new(hi.x, lo.y)),
            ] {
                if empty_zone(grid, c.offset(dx, dy)) == Some(z) {
                    continue;
                }
                // Painted border on the grass, inside the edge.
                let inward = -Vec2::new(dx as f32, dy as f32);
                let w = 0.45;
                let (a2, b2) = (a + inward * 0.35, b + inward * 0.35);
                m.quad_up(
                    [a2, b2, b2 + inward * w, a2 + inward * w].map(|p| Vec3::new(p.x, 0.04, p.y)),
                    paint,
                );
                // String between pegs a little inside the edge.
                let (pa, pb) = (a + inward * 0.35, b + inward * 0.35);
                string(&mut m, pa, pb, tint);
                for p in [pa, pb] {
                    let e = pegs.entry(q(p)).or_insert((p, base, 0));
                    e.2 += 1;
                }
            }
            // A sign on every few frontage cells, facing the road.
            if (x + 2 * y).rem_euclid(5) == 0 {
                for (dx, dy) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
                    if is_road(grid, c.offset(dx, dy)) {
                        let face = Vec2::new(dx as f32, dy as f32);
                        let side = Vec2::new(-face.y, face.x);
                        let centre = (lo + hi) * 0.5;
                        let half = (hi - lo) * 0.5;
                        let p = centre + face * (half.dot(face.abs()) - 0.9) + side * 1.6;
                        sign(&mut m, p, face, base);
                        break;
                    }
                }
            }
        }
    }
    for (p, color, _) in pegs.values() {
        peg(&mut m, *p, *color);
    }
    (!m.is_empty()).then(|| m.build())
}

#[allow(clippy::too_many_arguments)]
fn rebuild_lots(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mode: Res<OverlayMode>,
    tool: Res<ActiveTool>,
    settings: Res<DisplaySettings>,
    mats: Res<WorldMaterials>,
    mut state: Local<LotChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut vis: Query<&mut Visibility>,
) {
    let show = settings.lot_markers && !full_overlay(*mode, *tool);
    if show != state.shown {
        state.shown = show;
        for (_, e, has_mesh) in state.chunks.values() {
            if let Ok(mut v) = vis.get_mut(*e) {
                v.set_if_neq(if show && *has_mesh {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                });
            }
        }
    }
    let world = &sim.world;
    let key = (sim.key(world.versions.grid), sim.key(world.versions.roads));
    if !show || state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    let avenues = avenue_cells(world);
    let grid = &world.grid;
    let n = MAP_CELLS / CHUNK;
    for cy in 0..n {
        for cx in 0..n {
            let c = IVec2::new(cx, cy);
            let hash = chunk_hash(grid, &avenues, cx, cy);
            if state.chunks.get(&c).is_some_and(|(h, _, _)| *h == hash) {
                continue;
            }
            let mesh = chunk_mesh(grid, &avenues, cx, cy);
            match (mesh, state.chunks.get_mut(&c)) {
                (Some(mesh), Some((h, e, has_mesh))) => {
                    *h = hash;
                    *has_mesh = true;
                    commands
                        .entity(*e)
                        .insert((Mesh3d(meshes.add(mesh)), Visibility::Inherited));
                }
                (Some(mesh), None) => {
                    let e = commands
                        .spawn((
                            Name::new("Lot markers"),
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(mats.lit.clone()),
                            Transform::default(),
                            Visibility::Inherited,
                        ))
                        .id();
                    state.chunks.insert(c, (hash, e, true));
                }
                (None, Some((h, e, has_mesh))) => {
                    *h = hash;
                    *has_mesh = false;
                    commands.entity(*e).insert(Visibility::Hidden);
                }
                (None, None) => {}
            }
        }
    }
}
