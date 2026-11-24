//! Zoning on the ground.
//!
//! Zones are drawn only while they matter: in `OverlayMode::Zones` or while
//! a zoning tool is active (a 40 % fill and a bright outline on every zoned
//! cell). The rest of the time the ground carries no zone colour at all.
//!
//! Chunked; a chunk is re-meshed only when its cells change while the
//! overlay is showing. Hiding it just hides the chunk entities.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Cell, Grid, MAP_CELLS, Zone};

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

/// Whether the zone overlay is showing.
pub fn full_overlay(mode: OverlayMode, tool: ActiveTool) -> bool {
    mode == OverlayMode::Zones || matches!(tool, ActiveTool::Zone(_))
}

#[derive(Resource)]
struct ZoneMaterial(Handle<StandardMaterial>);

#[derive(Default)]
struct ZoneChunks {
    /// Grid key the chunks were last built for.
    key: Option<(u64, u64)>,
    shown: bool,
    chunks: HashMap<IVec2, (u64, Entity, bool)>,
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

fn shown(grid: &Grid, c: Cell) -> Option<Zone> {
    grid.get(c)?.zone
}

fn chunk_hash(grid: &Grid, cx: i32, cy: i32) -> u64 {
    let mut h = DefaultHasher::new();
    // Include a one-cell border: outlines depend on neighbours.
    for y in cy * CHUNK - 1..=(cy + 1) * CHUNK {
        for x in cx * CHUNK - 1..=(cx + 1) * CHUNK {
            shown(grid, Cell::new(x, y)).hash(&mut h);
        }
    }
    h.finish()
}

fn chunk_mesh(grid: &Grid, cx: i32, cy: i32) -> Option<Mesh> {
    let mut m = MeshBuilder::new();
    let y = heights::ZONE;
    let (fill_a, line_a) = (0.4, 1.0);
    let inset = 0.0;
    let line_w = 0.6;
    for cyy in cy * CHUNK..(cy + 1) * CHUNK {
        for cxx in cx * CHUNK..(cx + 1) * CHUNK {
            let c = Cell::new(cxx, cyy);
            let Some(z) = shown(grid, c) else { continue };
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
                if shown(grid, c.offset(dx, dy)) != Some(z) {
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
    mut vis: Query<&mut Visibility>,
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
    let key = sim.key(sim.world.versions.grid);
    if !show || state.key == Some(key) {
        return;
    }
    state.key = Some(key);
    let grid = &sim.world.grid;
    let n = MAP_CELLS / CHUNK;
    for cy in 0..n {
        for cx in 0..n {
            let c = IVec2::new(cx, cy);
            let hash = chunk_hash(grid, cx, cy);
            if state.chunks.get(&c).is_some_and(|(h, _, _)| *h == hash) {
                continue;
            }
            let mesh = chunk_mesh(grid, cx, cy);
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
