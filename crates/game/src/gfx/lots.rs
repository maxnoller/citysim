//! Zoned-but-empty land, always visible (issue 9) but far lighter than the
//! zone overlay: low surveyor's pegs with a coloured string around each
//! empty zoned area, a faint painted border on the grass, and a small
//! zone-coloured "lot" sign every few cells along the road. Shown while
//! `DisplaySettings::lot_markers` is on and the full zone overlay is not.
//!
//! Drawn per zoning cell of any lattice, fitted clear of the drawn roads.
//! Chunked like the zone overlay; a chunk is re-meshed only when its empty
//! zoned cells change.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::prelude::*;
use citysim_sim::zoning::facing_bit;
use citysim_sim::{CellRef, Facing, Zone};

use super::land::{self, CellView, RoadEdges};
use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix, shade};
use super::scene::WorldMaterials;
use super::zones::{full_overlay, zone_color};
use crate::shared::{ActiveTool, DisplaySettings, GameSet, OverlayMode, SimWorld};

/// Chunk size, metres.
const CHUNK_M: f32 = 256.0;

#[derive(Default)]
struct LotChunks {
    key: Option<((u64, u64), (u64, u64))>,
    shown: bool,
    chunks: HashMap<IVec2, (u64, Entity, bool)>,
}

pub struct LotsPlugin;

impl Plugin for LotsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, rebuild_lots.in_set(GameSet::Render).after(land::LandUpdate));
    }
}

fn chunk_hash(cells: &[CellView], free: &HashMap<CellRef, Zone>) -> u64 {
    let mut h = DefaultHasher::new();
    for c in cells {
        (c.cell, c.zone, c.fronts).hash(&mut h);
        for p in &c.poly {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
        }
        for n in &c.edge_nbrs {
            n.and_then(|n| free.get(&n)).hash(&mut h);
        }
    }
    h.finish()
}

/// Wooden peg with a zone-coloured tip.
fn peg(m: &mut MeshBuilder, p: Vec2, tip: Color) {
    let wood = Color::srgb_u32(0xB08A5B);
    m.box_yaw(Vec3::new(p.x, 0.0, p.y), Vec3::new(0.09, 0.42, 0.09), 0.6, wood, wood);
    m.box_yaw(Vec3::new(p.x, 0.42, p.y), Vec3::new(0.13, 0.13, 0.13), 0.6, tip, tip);
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
        Vec3::new(mid.x, 0.34, mid.y),
        Vec3::new(len, 0.03, 0.03),
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

fn chunk_mesh(cells: &[CellView], free: &HashMap<CellRef, Zone>) -> Option<Mesh> {
    let mut m = MeshBuilder::new();
    let mut pegs: HashMap<(i32, i32), (Vec2, Color)> = HashMap::new();
    let q = |p: Vec2| ((p.x * 4.0).round() as i32, (p.y * 4.0).round() as i32);
    for c in cells {
        let Some(z) = c.zone else { continue };
        let base = zone_color(z);
        // Soft surveyor's string: zone colour washed towards white, so a
        // street of empty lots does not read as a row of fences.
        let tint = mix(base, Color::WHITE, 0.55);
        let paint = lin(mix(base, palette::GRASS_LIGHT, 0.62));
        let centre = c.poly_center();
        let np = c.poly.len();
        for k in 0..np {
            if c.edge_nbrs[k].and_then(|nb| free.get(&nb)) == Some(&z) {
                continue;
            }
            let (a, b) = (c.poly[k], c.poly[(k + 1) % np]);
            let inward = (centre - (a + b) * 0.5).normalize_or_zero();
            // Painted border on the grass, inside the edge.
            let w = 0.3;
            let (a2, b2) = (a + inward * 0.35, b + inward * 0.35);
            m.quad_up(
                [a2, b2, b2 + inward * w, a2 + inward * w].map(|p| Vec3::new(p.x, 0.04, p.y)),
                paint,
            );
            // String between pegs a little inside the edge.
            string(&mut m, a2, b2, tint);
            for p in [a2, b2] {
                pegs.entry(q(p)).or_insert((p, base));
            }
        }
        // A sign on every few frontage cells, facing the road.
        let (i, j) = (c.cell.i, c.cell.j);
        if c.fronts != 0 && (i + 2 * j).rem_euclid(5) == 0 {
            for f in Facing::ALL {
                if c.fronts & facing_bit(f) == 0 {
                    continue;
                }
                let (di, dj) = f.offset();
                let face = (c.u * di as f32 + c.v * dj as f32).normalize_or_zero();
                let side = Vec2::new(-face.y, face.x);
                // The front edge of the (fitted) cell.
                let reach = c.quad.iter().map(|p| (*p - centre).dot(face)).fold(f32::MIN, f32::max);
                let p = centre + face * (reach - 0.9) + side * 1.6;
                sign(&mut m, p, face, base);
                break;
            }
        }
    }
    for (p, color) in pegs.values() {
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
    (edges, mut fits): (Res<RoadEdges>, ResMut<land::FitCache>),
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
    let by_chunk = land::cells_by_chunk(world, &edges, &mut fits, CHUNK_M, |z| {
        z.zone.is_some() && z.building.is_none()
    });
    let free: HashMap<CellRef, Zone> = by_chunk
        .values()
        .flatten()
        .filter_map(|c| Some((c.cell, c.zone?)))
        .collect();
    let mut keys: Vec<IVec2> = by_chunk.keys().copied().collect();
    keys.extend(state.chunks.keys().copied());
    keys.sort_by_key(|c| (c.x, c.y));
    keys.dedup();
    for c in keys {
        let cells = by_chunk.get(&c).map(Vec::as_slice).unwrap_or(&[]);
        {
            let hash = chunk_hash(cells, &free);
            if state.chunks.get(&c).is_some_and(|(h, _, _)| *h == hash) {
                continue;
            }
            let mesh = chunk_mesh(cells, &free);
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
