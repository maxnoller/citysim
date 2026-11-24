//! Zoning on the ground.
//!
//! * Normally only zoned cells that are still empty get a faint tint and a
//!   thin outline ("surveyed plots"), so the player sees where things will
//!   grow without the map turning into a colour chart.
//! * The full overlay (35 % fill and a bright outline on every zoned cell)
//!   shows in `OverlayMode::Zones` or while a zoning tool is active.
//!
//! Chunked; a chunk is re-meshed only when its cells or the mode change.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Cell, Grid, MAP_CELLS, Occupant, Zone};

use super::heights;
use super::mesh_util::MeshBuilder;
use super::palette::{self, lin};
use crate::shared::{ActiveTool, GameSet, OverlayMode, SimWorld};

const CHUNK: i32 = 32;

pub fn zone_color(zone: Zone) -> Color {
    match zone {
        Zone::Residential => palette::ZONE_RES,
        Zone::Commercial => palette::ZONE_COM,
        Zone::Industrial => palette::ZONE_IND,
    }
}

/// Whether the full overlay is showing.
pub fn full_overlay(mode: OverlayMode, tool: ActiveTool) -> bool {
    mode == OverlayMode::Zones || matches!(tool, ActiveTool::Zone(_))
}

#[derive(Resource)]
struct ZoneMaterial(Handle<StandardMaterial>);

#[derive(Default)]
struct ZoneChunks {
    key: Option<((u64, u64), bool, bool)>,
    chunks: HashMap<IVec2, (u64, Entity)>,
}

pub struct ZonesPlugin;

impl Plugin for ZonesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, rebuild_zones.in_set(GameSet::Render));
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

fn shown(grid: &Grid, c: Cell, full: bool) -> Option<Zone> {
    let cd = grid.get(c)?;
    let z = cd.zone?;
    (full || cd.occupant == Occupant::Empty).then_some(z)
}

fn chunk_hash(grid: &Grid, cx: i32, cy: i32, full: bool) -> u64 {
    let mut h = DefaultHasher::new();
    full.hash(&mut h);
    // Include a one-cell border: outlines depend on neighbours.
    for y in cy * CHUNK - 1..=(cy + 1) * CHUNK {
        for x in cx * CHUNK - 1..=(cx + 1) * CHUNK {
            shown(grid, Cell::new(x, y), full).hash(&mut h);
        }
    }
    h.finish()
}

fn chunk_mesh(grid: &Grid, cx: i32, cy: i32, full: bool) -> Option<Mesh> {
    let mut m = MeshBuilder::new();
    let y = heights::ZONE;
    let (fill_a, line_a) = if full { (0.4, 1.0) } else { (0.10, 0.38) };
    let inset = 0.0;
    let line_w = if full { 0.6 } else { 0.3 };
    for cyy in cy * CHUNK..(cy + 1) * CHUNK {
        for cxx in cx * CHUNK..(cx + 1) * CHUNK {
            let c = Cell::new(cxx, cyy);
            let Some(z) = shown(grid, c, full) else { continue };
            let base = zone_color(z);
            let fill = lin(base.with_alpha(fill_a));
            let line = lin(palette::mix(base, Color::WHITE, 0.45).with_alpha(line_a));
            let (x0, z0) = (cxx as f32 * CELL_SIZE + inset, cyy as f32 * CELL_SIZE + inset);
            let (x1, z1) = (
                (cxx + 1) as f32 * CELL_SIZE - inset,
                (cyy + 1) as f32 * CELL_SIZE - inset,
            );
            m.quad_up(
                [
                    Vec3::new(x0, y, z0),
                    Vec3::new(x1, y, z0),
                    Vec3::new(x1, y, z1),
                    Vec3::new(x0, y, z1),
                ],
                fill,
            );
            // Outline on edges where the neighbour shows a different zone.
            let yl = y + 0.01;
            let w = line_w;
            for (dx, dy, r) in [
                (1, 0, [x1 - w, z0, x1, z1]),
                (-1, 0, [x0, z0, x0 + w, z1]),
                (0, 1, [x0, z1 - w, x1, z1]),
                (0, -1, [x0, z0, x1, z0 + w]),
            ] {
                if shown(grid, c.offset(dx, dy), full) != Some(z) {
                    m.quad_up(
                        [
                            Vec3::new(r[0], yl, r[1]),
                            Vec3::new(r[2], yl, r[1]),
                            Vec3::new(r[2], yl, r[3]),
                            Vec3::new(r[0], yl, r[3]),
                        ],
                        line,
                    );
                }
            }
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
) {
    let full = full_overlay(*mode, *tool);
    let hide = *mode == OverlayMode::Traffic;
    let key = (sim.key(sim.world.versions.grid), full, hide);
    if state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    let grid = &sim.world.grid;
    let n = MAP_CELLS / CHUNK;
    for cy in 0..n {
        for cx in 0..n {
            let c = IVec2::new(cx, cy);
            let hash = if hide { 0 } else { chunk_hash(grid, cx, cy, full) };
            if state.chunks.get(&c).is_some_and(|(h, _)| *h == hash) {
                continue;
            }
            let mesh = if hide { None } else { chunk_mesh(grid, cx, cy, full) };
            match (mesh, state.chunks.get_mut(&c)) {
                (Some(mesh), Some((h, e))) => {
                    *h = hash;
                    commands.entity(*e).insert(Mesh3d(meshes.add(mesh)));
                }
                (Some(mesh), None) => {
                    let e = commands
                        .spawn((
                            Name::new("Zones"),
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(mat.0.clone()),
                            Transform::default(),
                            NotShadowCaster,
                        ))
                        .id();
                    state.chunks.insert(c, (hash, e));
                }
                (None, Some((h, e))) => {
                    *h = hash;
                    commands.entity(*e).remove::<Mesh3d>();
                }
                (None, None) => {}
            }
        }
    }
}
