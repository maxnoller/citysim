//! Zoning on the ground.
//!
//! Zones are drawn only while they matter: in `OverlayMode::Zones` or while
//! a zoning tool is active (a 40 % fill and a bright outline on every zoned
//! cell). The rest of the time the ground carries no zone colour at all.
//!
//! Drawn per zoning cell on whatever lattice it belongs to (rotated quads
//! along curved and rotated roads), each cell fitted clear of the drawn
//! roads. Chunked; a chunk is re-meshed only when its cells change while
//! the overlay is showing. Hiding it just hides the chunk entities.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::{CellRef, Zone};

use super::heights;
use super::land::{self, CellView, RoadEdges};
use super::mesh_util::MeshBuilder;
use super::palette::{self, lin};
use crate::shared::{ActiveTool, GameSet, OverlayMode, SimWorld};

/// Chunk size, metres.
const CHUNK_M: f32 = 256.0;

pub fn zone_color(zone: Zone) -> Color {
    match zone {
        Zone::Residential => palette::ZONE_RES,
        Zone::Commercial => palette::ZONE_COM,
        Zone::Industrial => palette::ZONE_IND,
    }
}

/// Whether the zone overlay is showing.
pub fn full_overlay(mode: OverlayMode, tool: ActiveTool) -> bool {
    mode == OverlayMode::Zones || matches!(tool, ActiveTool::Zone(_))
}

#[derive(Resource)]
struct ZoneMaterial(Handle<StandardMaterial>);

#[derive(Default)]
struct ZoneChunks {
    /// Grid and roads keys the chunks were last built for.
    key: Option<((u64, u64), (u64, u64))>,
    fits: land::FitCache,
    shown: bool,
    chunks: HashMap<IVec2, (u64, Entity, bool)>,
}

pub struct ZonesPlugin;

impl Plugin for ZonesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, rebuild_zones.in_set(GameSet::Render).after(land::LandUpdate));
    }
}

fn setup(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(ZoneMaterial(materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        fog_enabled: true,
        ..default()
    })));
}

/// Hash of a chunk's cells (their place and zone).
fn chunk_hash(cells: &[CellView], zones: &HashMap<CellRef, Zone>) -> u64 {
    let mut h = DefaultHasher::new();
    for c in cells {
        c.cell.hash(&mut h);
        c.zone.hash(&mut h);
        for p in c.quad {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
        }
        // Outlines depend on the neighbours.
        for n in neighbours(c.cell) {
            zones.get(&n).hash(&mut h);
        }
    }
    h.finish()
}

/// The four same-lattice neighbours, in the order of the quad's edges:
/// `-j` (corners 0-1), `+i` (1-2), `+j` (2-3), `-i` (3-0).
pub fn neighbours(c: CellRef) -> [CellRef; 4] {
    [c.offset(0, -1), c.offset(1, 0), c.offset(0, 1), c.offset(-1, 0)]
}

fn chunk_mesh(cells: &[CellView], zones: &HashMap<CellRef, Zone>) -> Option<Mesh> {
    let mut m = MeshBuilder::new();
    let y = heights::ZONE;
    let (fill_a, line_a) = (0.4, 1.0);
    let line_w = 0.6;
    for c in cells {
        let Some(z) = c.zone else { continue };
        let base = zone_color(z);
        let fill = lin(base.with_alpha(fill_a));
        let line = lin(palette::mix(base, Color::WHITE, 0.45).with_alpha(line_a));
        m.quad_up(c.quad.map(|p| Vec3::new(p.x, y, p.y)), fill);
        // Outline on edges where the neighbour shows a different zone.
        let centre = c.center();
        for (k, n) in neighbours(c.cell).into_iter().enumerate() {
            if zones.get(&n) == Some(&z) {
                continue;
            }
            let (a, b) = (c.quad[k], c.quad[(k + 1) % 4]);
            let inward = ((a + b) * 0.5 - centre).normalize_or_zero() * -line_w;
            m.quad_up(
                [a, b, b + inward, a + inward].map(|p| Vec3::new(p.x, y + 0.01, p.y)),
                line,
            );
        }
    }
    (!m.is_empty()).then(|| m.build())
}

#[allow(clippy::too_many_arguments)]
fn rebuild_zones(
    mut commands: Commands,
    sim: Res<SimWorld>,
    mode: Res<OverlayMode>,
    tool: Res<ActiveTool>,
    mat: Res<ZoneMaterial>,
    mut state: Local<ZoneChunks>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut vis: Query<&mut Visibility>,
    edges: Res<RoadEdges>,
) {
    let show = full_overlay(*mode, *tool);
    if show != state.shown {
        state.shown = show;
        let v = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        for (_, e, has_mesh) in state.chunks.values() {
            if let Ok(mut vis) = vis.get_mut(*e) {
                vis.set_if_neq(if *has_mesh { v } else { Visibility::Hidden });
            }
        }
    }
    // Re-mesh only while showing; a hidden overlay catches up when shown.
    let key = (sim.key(sim.world.versions.grid), sim.key(sim.world.versions.roads));
    if !show || state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    let world = &sim.world;
    let by_chunk = land::cells_by_chunk(world, &edges, &mut state.fits, CHUNK_M, |z| z.zone.is_some());
    let zones: HashMap<CellRef, Zone> = by_chunk
        .values()
        .flatten()
        .filter_map(|c| Some((c.cell, c.zone?)))
        .collect();
    // Chunks that lost all their zones too.
    let mut keys: Vec<IVec2> = by_chunk.keys().copied().collect();
    keys.extend(state.chunks.keys().copied());
    keys.sort_by_key(|c| (c.x, c.y));
    keys.dedup();
    for c in keys {
        let cells = by_chunk.get(&c).map(Vec::as_slice).unwrap_or(&[]);
        {
            let hash = chunk_hash(cells, &zones);
            if state.chunks.get(&c).is_some_and(|(h, _, _)| *h == hash) {
                continue;
            }
            let mesh = chunk_mesh(cells, &zones);
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
                            Name::new("Zones"),
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(mat.0.clone()),
                            Transform::default(),
                            Visibility::Inherited,
                            NotShadowCaster,
                        ))
                        .id();
                    state.chunks.insert(c, (hash, e, true));
                }
                (None, Some((h, e, has_mesh))) => {
                    *h = hash;
                    *has_mesh = false;
                    // Hide, never remove Mesh3d (see cursor.rs).
                    commands.entity(*e).insert(Visibility::Hidden);
                }
                (None, None) => {}
            }
        }
    }
}
