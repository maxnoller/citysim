//! In-world tool feedback, driven only by `HoveredCell` / `HoveredPoint`,
//! `ActiveTool` and `ToolPreview` (written by ui):
//! * a hovered-cell frame that glides to the cursor, tinted by the tool,
//!   turned to the zoning lattice under the cursor (cells along a rotated
//!   or curved road are rotated with it);
//! * a faint cell grid around the cursor while a tool is active, on the
//!   same lattice;
//! * the tool ghost: a translucent road (red and hatched when invalid), or
//!   the zone / bulldoze rectangle with its affected cells.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::{CELL_SIZE, Cell, RoadKind};

use super::heights;
use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix};
use super::road_geom;
use super::road_ghost::{GhostLeg, ghost_meshes};
use super::zones::zone_color;
use crate::shared::{
    ActiveTool, GameSet, HoveredCell, HoveredPoint, PathPreview, PreviewKind, RoadChoice, SimWorld, ToolPreview,
    cell_center, cell_corners, zone_cell_corners,
};

/// Grid radius around the cursor, cells.
const GRID_R: i32 = 14;

#[derive(Component)]
struct HoverFrame {
    pos: Vec3,
    /// Corners (around the centre) the mesh was built for.
    shape: [Vec2; 4],
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
    /// Translucent lit road preview, valid / refused.
    road_ok: Handle<StandardMaterial>,
    road_bad: Handle<StandardMaterial>,
}

/// Part of the road preview: 0 road, 1 outline, 2 refused legs.
#[derive(Component)]
struct GhostRoadPart(usize);

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

/// A cell outline with a soft fill, a little larger than the cell, around
/// its centre (`q`: corners relative to the centre, in order).
fn frame_mesh(q: [Vec2; 4]) -> Mesh {
    let mut m = MeshBuilder::new();
    let w = 0.45;
    let line = [1.0, 1.0, 1.0, 1.0];
    let fill = [1.0, 1.0, 1.0, 0.22];
    let grow = |p: Vec2, d: f32| p + p.normalize_or_zero() * d;
    let outer = q.map(|p| grow(p, 0.35));
    let inner = q.map(|p| grow(p, 0.35 - w * std::f32::consts::SQRT_2));
    let v = |p: Vec2| Vec3::new(p.x, 0.0, p.y);
    let ccw = (q[1] - q[0]).perp_dot(q[2] - q[0]) > 0.0;
    let mut quad = |a: [Vec2; 4], c: [f32; 4]| {
        let pts = a.map(v);
        if ccw {
            m.quad_up(pts, c);
        } else {
            m.quad_up([pts[0], pts[3], pts[2], pts[1]], c);
        }
    };
    for k in 0..4 {
        let n = (k + 1) % 4;
        quad([outer[k], outer[n], inner[n], inner[k]], line);
    }
    quad(inner, fill);
    m.build()
}

/// The square map cell, corners around its centre.
fn square() -> [Vec2; 4] {
    let h = CELL_SIZE * 0.5;
    [Vec2::new(-h, -h), Vec2::new(h, -h), Vec2::new(h, h), Vec2::new(-h, h)]
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
    let ghost_road = |c: Color| StandardMaterial {
        base_color: c,
        emissive: LinearRgba::from(c) * 0.3,
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.9,
        depth_bias: 20.0,
        ..default()
    };
    let mats = CursorMaterials {
        ghost: overlay_material(&mut materials, Color::WHITE, 0.0),
        frame: overlay_material(&mut materials, Color::WHITE, 0.0),
        grid: overlay_material(&mut materials, Color::WHITE, 0.0),
        road_ok: materials.add(ghost_road(Color::srgba(0.86, 0.93, 1.0, 0.72))),
        road_bad: materials.add(ghost_road(Color::srgba(1.0, 0.55, 0.5, 0.72))),
    };
    let outline = overlay_material(&mut materials, Color::srgba(0.85, 0.93, 1.0, 0.9), 30.0);
    let bad = overlay_material(&mut materials, Color::srgb(1.0, 0.3, 0.25), 30.0);
    for (i, mat) in [(0, mats.road_ok.clone()), (1, outline), (2, bad)] {
        commands.spawn((
            Name::new("Road preview"),
            GhostRoadPart(i),
            MeshMaterial3d(mat),
            Transform::default(),
            Visibility::Hidden,
            NotShadowCaster,
        ));
    }
    commands.spawn((
        Name::new("Hovered cell"),
        HoverFrame {
            pos: Vec3::ZERO,
            shape: square(),
        },
        Mesh3d(meshes.add(frame_mesh(square()))),
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

/// The hovered cell as drawn: its centre and corners around it. With a
/// zone tool only zoneable land (the zoning cell under the cursor, whatever
/// lattice: a strip cell along a curve is a trapezoid); with other tools
/// the map cell when there is no zoning cell.
fn hovered_quad(
    sim: &SimWorld,
    hovered: &HoveredCell,
    point: &HoveredPoint,
    tool: ActiveTool,
) -> Option<(Vec2, [Vec2; 4])> {
    if let Some(c) = point.zone_cell
        && let Some(q) = sim.world.zoning.cell_corners(c)
    {
        let q = q.map(Vec2::from);
        let ctr = (q[0] + q[1] + q[2] + q[3]) * 0.25;
        return Some((ctr, q.map(|p| p - ctr)));
    }
    if matches!(tool, ActiveTool::Zone(_)) {
        return None;
    }
    let c = hovered.0?;
    let ctr = Vec2::new((c.x as f32 + 0.5) * CELL_SIZE, (c.y as f32 + 0.5) * CELL_SIZE);
    Some((ctr, square()))
}

/// Placement of the faint cell grid (corner `(i, j)` at the origin, `+x`
/// along the lattice's `+i`): the hovered cell's straight lattice, else the
/// map grid; none on a strip (it bends).
fn grid_frame(sim: &SimWorld, hovered: &HoveredCell, point: &HoveredPoint) -> Option<(Vec3, Quat)> {
    if let Some(c) = point.zone_cell
        && let Some(l) = sim.world.zoning.lattice(c.lattice)
    {
        if l.is_strip() {
            return None;
        }
        let p = l.world_at(c.i as f32, c.j as f32);
        return Some((Vec3::new(p[0], 0.0, p[1]), Quat::from_rotation_y(-l.angle())));
    }
    let c = hovered.0?;
    Some((
        Vec3::new(c.x as f32 * CELL_SIZE, 0.0, c.y as f32 * CELL_SIZE),
        Quat::IDENTITY,
    ))
}

#[allow(clippy::too_many_arguments)]
fn move_frame(
    time: Res<Time<Real>>,
    hovered: Res<HoveredCell>,
    point: Res<HoveredPoint>,
    sim: Res<SimWorld>,
    tool: Res<ActiveTool>,
    mats: Res<CursorMaterials>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q: Query<(&mut HoverFrame, &mut Transform, &mut Visibility, &Mesh3d)>,
) {
    let Ok((mut frame, mut t, mut vis, mesh)) = q.single_mut() else {
        return;
    };
    // Inspect highlights what it points at (buildings, roads, cars); a
    // cell frame on bare ground would only look like a stray tile.
    let at = hovered_quad(&sim, &hovered, &point, *tool).filter(|_| *tool != ActiveTool::Inspect);
    let Some((ctr, shape)) = at else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    if shape.iter().zip(frame.shape.iter()).any(|(a, b)| a.distance(*b) > 0.01)
        && let Some(mut m) = meshes.get_mut(&mesh.0)
    {
        *m = frame_mesh(shape);
        frame.shape = shape;
    }
    let target = Vec3::new(ctr.x, heights::HIGHLIGHT, ctr.y);
    if *vis == Visibility::Hidden || frame.pos.distance(target) > 80.0 {
        frame.pos = target;
    }
    let a = 1.0 - (-time.delta_secs() * 28.0).exp();
    frame.pos = frame.pos.lerp(target, a);
    t.translation = frame.pos;
    t.rotation = Quat::IDENTITY;
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
    point: Res<HoveredPoint>,
    sim: Res<SimWorld>,
    tool: Res<ActiveTool>,
    mut q: Query<(&mut Transform, &mut Visibility), With<CellGrid>>,
) {
    let Ok((mut t, mut vis)) = q.single_mut() else { return };
    let zone_land = !matches!(*tool, ActiveTool::Zone(_)) || point.zone_cell.is_some();
    match grid_frame(&sim, &hovered, &point).filter(|_| zone_land) {
        Some((corner, rot)) if *tool != ActiveTool::Inspect => {
            t.translation = corner + Vec3::Y * 0.05;
            t.rotation = rot;
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

/// Outline of a quad (any rotation), `w` wide inside its edges.
fn quad_ring(m: &mut MeshBuilder, q: &[[f32; 2]; 4], y: f32, w: f32, c: [f32; 4]) {
    let p = q.map(Vec2::from);
    let center = (p[0] + p[1] + p[2] + p[3]) * 0.25;
    for i in 0..4 {
        let (a, b) = (p[i], p[(i + 1) % 4]);
        let (ai, bi) = (
            a + (center - a).normalize_or_zero() * w * std::f32::consts::SQRT_2,
            b + (center - b).normalize_or_zero() * w * std::f32::consts::SQRT_2,
        );
        m.quad_up([a, b, bi, ai].map(|v| Vec3::new(v.x, y, v.y)), c);
    }
}

fn cell_quad(m: &mut MeshBuilder, c: Cell, inset: f32, y: f32, col: [f32; 4]) {
    m.quad_up(cell_corners(c, inset, y), col);
}

fn kind_of(c: RoadChoice) -> RoadKind {
    match c {
        RoadChoice::Street => RoadKind::Street,
        RoadChoice::Avenue => RoadKind::Avenue,
    }
}

/// Placed corners of the path: small accent dots.
fn corner_marks(m: &mut MeshBuilder, path: &PathPreview) {
    let y = road_geom::WALK_Y + 0.1;
    for (i, &c) in path.points.iter().enumerate().take(path.placed.max(1)) {
        let p = cell_center(c, y);
        let r = if i == 0 { 0.9 } else { 0.7 };
        let ring: Vec<Vec3> = (0..16)
            .map(|k| {
                let a = k as f32 / 16.0 * std::f32::consts::TAU;
                p + Vec3::new(a.cos() * r, 0.0, a.sin() * r)
            })
            .collect();
        m.fan_up(p, &ring, [1.0, 1.0, 1.0, 0.95]);
    }
}

/// Show `m` on entity `e` (hide it when empty; never remove `Mesh3d`).
fn show_mesh(commands: &mut Commands, meshes: &mut Assets<Mesh>, e: Entity, vis: &mut Mut<Visibility>, m: MeshBuilder) {
    if m.is_empty() {
        vis.set_if_neq(Visibility::Hidden);
    } else {
        commands.entity(e).insert(Mesh3d(meshes.add(m.build())));
        vis.set_if_neq(Visibility::Inherited);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_ghost(
    mut commands: Commands,
    preview: Res<ToolPreview>,
    path: Res<PathPreview>,
    sim: Res<SimWorld>,
    mats: Res<CursorMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q: Query<(Entity, &mut Visibility), With<GhostMesh>>,
    mut roads: Query<
        (
            Entity,
            &mut Visibility,
            &mut MeshMaterial3d<StandardMaterial>,
            &GhostRoadPart,
        ),
        Without<GhostMesh>,
    >,
    mut last_roads: Local<Option<(u64, u64)>>,
) {
    let roads_key = sim.key(sim.world.versions.roads);
    if !preview.is_changed() && !path.is_changed() && *last_roads == Some(roads_key) {
        return;
    }
    *last_roads = Some(roads_key);
    let Ok((entity, mut vis)) = q.single_mut() else { return };
    let mut m = MeshBuilder::new();
    let valid = preview.is_valid();
    // The road preview: real geometry on a copy of the road graph.
    let mut parts = [MeshBuilder::new(), MeshBuilder::new(), MeshBuilder::new()];
    let mut road_valid = true;
    if preview.kind == PreviewKind::Road {
        let (legs, kind): (Vec<GhostLeg>, RoadKind) = if !path.legs.is_empty() {
            (
                path.legs
                    .iter()
                    .map(|l| GhostLeg {
                        from: l.from,
                        to: l.to,
                        valid: l.error.is_none(),
                    })
                    .collect(),
                kind_of(path.kind),
            )
        } else if let Some((from, to)) = preview.line {
            (vec![GhostLeg { from, to, valid }], kind_of(path.kind))
        } else {
            (Vec::new(), RoadKind::Street)
        };
        road_valid = legs.iter().all(|l| l.valid) && path.error.is_none();
        if !legs.is_empty() {
            let g = ghost_meshes(&sim.world.roads, &legs, kind);
            parts[0] = g.road;
            parts[1] = g.outline;
            parts[2] = g.bad;
            corner_marks(&mut parts[1], &path);
        }
    }
    for (e, mut v, mut mat, part) in &mut roads {
        let i = part.0;
        if i == 0 {
            let want = if road_valid { &mats.road_ok } else { &mats.road_bad };
            if mat.0 != *want {
                mat.0 = want.clone();
            }
        }
        let mesh = std::mem::take(&mut parts[i]);
        show_mesh(&mut commands, &mut meshes, e, &mut v, mesh);
    }
    match preview.kind {
        PreviewKind::None | PreviewKind::Road => {}
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
                if preview.zone_cells.is_empty() {
                    for &c in &preview.cells {
                        cell_quad(&mut m, c, 0.3, y, fill);
                    }
                } else {
                    for &c in &preview.zone_cells {
                        if let Some(q) = zone_cell_corners(&sim.world, c, 0.3, y) {
                            m.quad_up(q, fill);
                        }
                    }
                }
            }
            if let Some(q) = preview.area {
                let edge = lin(mix(base, Color::WHITE, 0.3).with_alpha(0.95));
                quad_ring(&mut m, &q, y + 0.01, 0.5, edge);
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
    show_mesh(&mut commands, &mut meshes, entity, &mut vis, m);
}
