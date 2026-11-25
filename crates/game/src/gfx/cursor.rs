//! In-world tool feedback, driven only by `HoveredCell`, `ActiveTool` and
//! `ToolPreview` (written by ui):
//! * a hovered-cell frame that glides to the cursor, tinted by the tool;
//! * a faint cell grid around the cursor while a tool is active;
//! * the tool ghost: a translucent road (red and hatched when invalid), or
//!   the zone / bulldoze rectangle with its affected cells.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Cell, RoadKind};

use super::heights;
use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix};
use super::road_geom;
use super::zones::zone_color;
use crate::shared::{
    ActiveTool, GameSet, HoveredCell, PathPreview, PreviewKind, RoadChoice, ToolPreview, cell_center, cell_corners,
};

/// Grid radius around the cursor, cells.
const GRID_R: i32 = 14;

#[derive(Component)]
struct HoverFrame {
    pos: Vec3,
}

#[derive(Component)]
struct CellGrid;

#[derive(Component)]
struct GhostMesh;

#[derive(Resource)]
struct CursorMaterials {
    /// Unlit, blended, vertex colours (ghosts).
    ghost: Handle<StandardMaterial>,
    /// Hover frame (colour set per tool).
    frame: Handle<StandardMaterial>,
    grid: Handle<StandardMaterial>,
}

pub struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, (move_frame, move_grid, draw_ghost).in_set(GameSet::Render));
    }
}

fn overlay_material(materials: &mut Assets<StandardMaterial>, color: Color, bias: f32) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: color,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        depth_bias: bias,
        ..default()
    })
}

fn frame_mesh() -> Mesh {
    // A cell outline with a soft fill, slightly larger than the cell.
    let mut m = MeshBuilder::new();
    let (a, b) = (-0.25, CELL_SIZE + 0.25);
    let w = 0.45;
    let line = [1.0, 1.0, 1.0, 1.0];
    let fill = [1.0, 1.0, 1.0, 0.22];
    let q = |x0: f32, z0: f32, x1: f32, z1: f32| {
        [
            Vec3::new(x0, 0.0, z0),
            Vec3::new(x1, 0.0, z0),
            Vec3::new(x1, 0.0, z1),
            Vec3::new(x0, 0.0, z1),
        ]
    };
    m.quad_up(q(a, a, b, a + w), line);
    m.quad_up(q(a, b - w, b, b), line);
    m.quad_up(q(a, a + w, a + w, b - w), line);
    m.quad_up(q(b - w, a + w, b, b - w), line);
    m.quad_up(q(a + w, a + w, b - w, b - w), fill);
    m.build()
}

fn grid_mesh() -> Mesh {
    let mut m = MeshBuilder::new();
    let r = GRID_R as f32 * CELL_SIZE;
    let w = 0.12;
    let alpha = |p: Vec2| {
        let d = (p.length() / r).min(1.0);
        0.4 * (1.0 - d * d)
    };
    // Lines split into cell-long pieces so the alpha fades smoothly.
    for i in -GRID_R..=GRID_R {
        let c = i as f32 * CELL_SIZE;
        for k in -GRID_R..GRID_R {
            let (t0, t1) = (k as f32 * CELL_SIZE, (k + 1) as f32 * CELL_SIZE);
            for horizontal in [true, false] {
                let (p0, p1) = if horizontal {
                    (Vec2::new(t0, c), Vec2::new(t1, c))
                } else {
                    (Vec2::new(c, t0), Vec2::new(c, t1))
                };
                let n = if horizontal { Vec2::Y * w } else { Vec2::X * w };
                let (a0, a1) = (alpha(p0), alpha(p1));
                if a0 <= 0.0 && a1 <= 0.0 {
                    continue;
                }
                let col = |a: f32| [1.0, 1.0, 1.0, a];
                let v = [p0 - n, p1 - n, p1 + n, p0 + n].map(|p| Vec3::new(p.x, 0.0, p.y));
                let cols = [col(a0), col(a1), col(a1), col(a0)];
                if (v[1] - v[0]).cross(v[2] - v[0]).y >= 0.0 {
                    m.quad_c(v, cols);
                } else {
                    m.quad_c([v[0], v[3], v[2], v[1]], [cols[0], cols[3], cols[2], cols[1]]);
                }
            }
        }
    }
    m.build()
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let mats = CursorMaterials {
        ghost: overlay_material(&mut materials, Color::WHITE, 0.0),
        frame: overlay_material(&mut materials, Color::WHITE, 0.0),
        grid: overlay_material(&mut materials, Color::WHITE, 0.0),
    };
    commands.spawn((
        Name::new("Hovered cell"),
        HoverFrame { pos: Vec3::ZERO },
        Mesh3d(meshes.add(frame_mesh())),
        MeshMaterial3d(mats.frame.clone()),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Cell grid"),
        CellGrid,
        Mesh3d(meshes.add(grid_mesh())),
        MeshMaterial3d(mats.grid.clone()),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Tool ghost"),
        GhostMesh,
        MeshMaterial3d(mats.ghost.clone()),
        Transform::default(),
        Visibility::default(),
        NotShadowCaster,
    ));
    commands.insert_resource(mats);
}

fn tool_color(tool: ActiveTool) -> Color {
    match tool {
        ActiveTool::Inspect => Color::WHITE,
        ActiveTool::Road => palette::TOOL_OK,
        ActiveTool::Zone(Some(z)) => mix(zone_color(z), Color::WHITE, 0.25),
        ActiveTool::Zone(None) => Color::WHITE,
        ActiveTool::Bulldoze => palette::TOOL_BULLDOZE,
    }
}

fn move_frame(
    time: Res<Time<Real>>,
    hovered: Res<HoveredCell>,
    tool: Res<ActiveTool>,
    mats: Res<CursorMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut q: Query<(&mut HoverFrame, &mut Transform, &mut Visibility)>,
) {
    let Ok((mut frame, mut t, mut vis)) = q.single_mut() else {
        return;
    };
    let Some(c) = hovered.0 else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    let target = Vec3::new(c.x as f32 * CELL_SIZE, heights::HIGHLIGHT, c.y as f32 * CELL_SIZE);
    if *vis == Visibility::Hidden || frame.pos.distance(target) > 80.0 {
        frame.pos = target;
    }
    let a = 1.0 - (-time.delta_secs() * 28.0).exp();
    frame.pos = frame.pos.lerp(target, a);
    t.translation = frame.pos;
    vis.set_if_neq(Visibility::Inherited);
    if tool.is_changed()
        && let Some(mut m) = materials.get_mut(&mats.frame)
    {
        let alpha = if *tool == ActiveTool::Inspect { 0.45 } else { 0.9 };
        m.base_color = tool_color(*tool).with_alpha(alpha);
    }
}

fn move_grid(
    hovered: Res<HoveredCell>,
    tool: Res<ActiveTool>,
    mut q: Query<(&mut Transform, &mut Visibility), With<CellGrid>>,
) {
    let Ok((mut t, mut vis)) = q.single_mut() else { return };
    match hovered.0 {
        Some(c) if *tool != ActiveTool::Inspect => {
            t.translation = Vec3::new(c.x as f32 * CELL_SIZE, 0.05, c.y as f32 * CELL_SIZE);
            vis.set_if_neq(Visibility::Inherited);
        }
        _ => {
            vis.set_if_neq(Visibility::Hidden);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rect_ring(m: &mut MeshBuilder, x0: f32, z0: f32, x1: f32, z1: f32, y: f32, w: f32, c: [f32; 4]) {
    let q = |a: f32, b: f32, cc: f32, d: f32| {
        [
            Vec3::new(a, y, b),
            Vec3::new(cc, y, b),
            Vec3::new(cc, y, d),
            Vec3::new(a, y, d),
        ]
    };
    m.quad_up(q(x0, z0, x1, z0 + w), c);
    m.quad_up(q(x0, z1 - w, x1, z1), c);
    m.quad_up(q(x0, z0 + w, x0 + w, z1 - w), c);
    m.quad_up(q(x1 - w, z0 + w, x1, z1 - w), c);
}

fn cell_quad(m: &mut MeshBuilder, c: Cell, inset: f32, y: f32, col: [f32; 4]) {
    m.quad_up(cell_corners(c, inset, y), col);
}

/// Translucent road from `a` to `b` (Bevy positions on the ground) drawn
/// as a street.
fn road_ghost(m: &mut MeshBuilder, a: Vec3, b: Vec3, valid: bool) {
    road_ghost_kind(m, a, b, valid, RoadKind::Street);
}

/// Translucent road of `kind` (its real width) from `a` to `b`.
fn road_ghost_kind(m: &mut MeshBuilder, a: Vec3, b: Vec3, valid: bool, kind: RoadKind) {
    let y = heights::GHOST;
    let d = (b - a).with_y(0.0);
    let len = d.length();
    if len < 1e-3 {
        return;
    }
    let dir = d / len;
    let n = Vec3::new(-dir.z, 0.0, dir.x);
    let base = if valid { palette::TOOL_OK } else { palette::TOOL_BAD };
    let body = lin(base.with_alpha(0.55));
    let edge = lin(mix(base, Color::WHITE, 0.4).with_alpha(0.95));
    let at = |t: f32, o: f32| a.with_y(y) + dir * t + n * o;
    let st = road_geom::style(kind);
    let (h, walk) = (st.h, st.walk);
    m.quad_up(
        [
            at(0.0, -h - walk),
            at(len, -h - walk),
            at(len, h + walk),
            at(0.0, h + walk),
        ],
        body,
    );
    for s in [1.0f32, -1.0] {
        let (o0, o1) = (s * (h + walk - 0.35), s * (h + walk));
        m.quad_up([at(0.0, o0), at(len, o0), at(len, o1), at(0.0, o1)], edge);
    }
    if valid {
        // Centre dashes (a solid median for avenues, with lane dashes).
        if st.median > 0.0 {
            m.quad_up([at(0.0, -0.3), at(len, -0.3), at(len, 0.3), at(0.0, 0.3)], edge);
        }
        let lanes: &[f32] = if st.median > 0.0 { &[-3.25, 3.25] } else { &[0.0] };
        for &o in lanes {
            let mut t = 1.0;
            while t + 2.0 < len {
                m.quad_up(
                    [
                        at(t, o - 0.12),
                        at(t + 2.0, o - 0.12),
                        at(t + 2.0, o + 0.12),
                        at(t, o + 0.12),
                    ],
                    edge,
                );
                t += 4.0;
            }
        }
    } else {
        // Diagonal hatching.
        let mut t = 0.0;
        while t + 1.2 < len {
            let w = h + walk - 0.4;
            m.quad_up(
                [at(t, -w), at(t + 0.9, -w), at(t + 3.0, w), at(t + 2.1, w)].map(|p| p + Vec3::Y * 0.01),
                edge,
            );
            t += 3.0;
        }
    }
    // Round end caps.
    for (p, s) in [(a, -1.0f32), (b, 1.0)] {
        let r = h + walk;
        let ring: Vec<Vec3> = (0..=12)
            .map(|k| {
                let ang = std::f32::consts::PI * k as f32 / 12.0;
                p.with_y(y) + (dir * s * ang.sin() + n * ang.cos()) * r
            })
            .collect();
        m.fan_up(p.with_y(y), &ring, body);
    }
}

fn kind_of(c: RoadChoice) -> RoadKind {
    match c {
        RoadChoice::Street => RoadKind::Street,
        RoadChoice::Avenue => RoadKind::Avenue,
    }
}

/// The whole multi-leg road being drawn: every leg at its real width (red
/// where the sim would refuse it), cells that would be upgraded marked
/// amber, and the corners placed so far.
fn path_ghost(m: &mut MeshBuilder, path: &PathPreview) {
    let kind = kind_of(path.kind);
    for leg in &path.legs {
        road_ghost_kind(
            m,
            cell_center(leg.from, 0.0),
            cell_center(leg.to, 0.0),
            leg.error.is_none(),
            kind,
        );
        let amber = lin(palette::TRAFFIC_BUSY.with_alpha(0.6));
        for &c in &leg.upgrade_cells {
            cell_quad(m, c, 1.2, heights::GHOST + 0.02, amber);
        }
    }
    let y = heights::GHOST + 0.04;
    for (i, &c) in path.points.iter().enumerate() {
        let p = cell_center(c, y);
        let placed = i < path.placed;
        let col = if placed {
            lin(Color::WHITE.with_alpha(0.95))
        } else {
            lin(palette::TOOL_OK.with_alpha(0.9))
        };
        let r = if placed { 1.1 } else { 1.5 };
        let ring: Vec<Vec3> = (0..16)
            .map(|k| {
                p + Vec3::new(
                    (k as f32 / 16.0 * std::f32::consts::TAU).cos() * r,
                    0.0,
                    (k as f32 / 16.0 * std::f32::consts::TAU).sin() * r,
                )
            })
            .collect();
        m.fan_up(p, &ring, col);
    }
}

fn draw_ghost(
    mut commands: Commands,
    preview: Res<ToolPreview>,
    path: Res<PathPreview>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q: Query<(Entity, &mut Visibility), With<GhostMesh>>,
) {
    if !preview.is_changed() && !path.is_changed() {
        return;
    }
    let Ok((entity, mut vis)) = q.single_mut() else { return };
    let mut m = MeshBuilder::new();
    let valid = preview.is_valid();
    match preview.kind {
        PreviewKind::None => {}
        PreviewKind::Road if !path.legs.is_empty() || !path.points.is_empty() => path_ghost(&mut m, &path),
        PreviewKind::Road => {
            if let Some((from, to)) = preview.line {
                road_ghost(&mut m, cell_center(from, 0.0), cell_center(to, 0.0), valid);
            }
        }
        kind => {
            let base = match kind {
                _ if !valid => palette::TOOL_BAD,
                PreviewKind::Zone(Some(z)) => zone_color(z),
                PreviewKind::Zone(None) => Color::WHITE,
                _ => palette::TOOL_BULLDOZE,
            };
            let y = heights::GHOST;
            let fill = lin(base.with_alpha(if valid { 0.5 } else { 0.25 }));
            if valid {
                for &c in &preview.cells {
                    cell_quad(&mut m, c, 0.3, y, fill);
                }
            }
            if let Some(r) = preview.rect {
                let (x0, z0) = (r.min.x as f32 * CELL_SIZE, r.min.y as f32 * CELL_SIZE);
                let (x1, z1) = ((r.max.x + 1) as f32 * CELL_SIZE, (r.max.y + 1) as f32 * CELL_SIZE);
                if !valid {
                    m.quad_up(
                        [
                            Vec3::new(x0, y, z0),
                            Vec3::new(x1, y, z0),
                            Vec3::new(x1, y, z1),
                            Vec3::new(x0, y, z1),
                        ],
                        fill,
                    );
                }
                let edge = lin(mix(base, Color::WHITE, 0.3).with_alpha(0.95));
                rect_ring(&mut m, x0, z0, x1, z1, y + 0.01, 0.5, edge);
            }
        }
    }
    // Hide rather than remove `Mesh3d`: an entity whose Mesh3d was removed
    // and later re-inserted was never drawn again (seen with Bevy 0.20.1).
    if m.is_empty() {
        vis.set_if_neq(Visibility::Hidden);
    } else {
        commands.entity(entity).insert(Mesh3d(meshes.add(m.build())));
        vis.set_if_neq(Visibility::Inherited);
    }
}
