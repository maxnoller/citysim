//! In-world tool feedback, driven only by `HoveredCell` / `HoveredPoint`,
//! `ActiveTool` and `ToolPreview` (written by ui):
//! * a hovered-cell frame that glides to the cursor, tinted by the tool,
//!   turned to the zoning lattice under the cursor (cells along a rotated
//!   or curved road are rotated with it);
//! * a faint cell grid around the cursor while the road or zone tool is
//!   active, on the same lattice (not for the bulldozer, which marks what
//!   it would remove);
//! * the tool ghost: a translucent road (red and hatched when invalid), or
//!   the zone / bulldoze rectangle with its affected cells.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::zoning::{CellRef, LandShapes};
use citysim_sim::{CELL_SIZE, World};

use super::heights;
use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix};
use super::zones::zone_color;
use crate::shared::{ActiveTool, GameSet, HoveredCell, HoveredPoint, PreviewKind, SimWorld, ToolPreview};

/// Grid radius around the cursor, cells.
const GRID_R: i32 = 14;

#[derive(Component)]
struct HoverFrame {
    pos: Vec3,
    /// Corners (around the centre) the mesh was built for.
    shape: Vec<Vec2>,
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

/// A cell outline with a soft fill, a little larger than the cell, around
/// its centre (`q`: corners of the convex land relative to the centre, in
/// order: a cell's quad, or its cut shape where two roads' rows meet).
fn frame_mesh(q: &[Vec2]) -> Mesh {
    let mut m = MeshBuilder::new();
    let w = 0.45;
    let line = [1.0, 1.0, 1.0, 1.0];
    let fill = [1.0, 1.0, 1.0, 0.22];
    let grow = |p: Vec2, d: f32| p + p.normalize_or_zero() * d;
    let outer: Vec<Vec2> = q.iter().map(|&p| grow(p, 0.35)).collect();
    let inner: Vec<Vec2> = q
        .iter()
        .map(|&p| grow(p, 0.35 - w * std::f32::consts::SQRT_2))
        .collect();
    let v = |p: Vec2| Vec3::new(p.x, 0.0, p.y);
    let n = q.len();
    for k in 0..n {
        let j = (k + 1) % n;
        m.quad_up([v(outer[k]), v(outer[j]), v(inner[j]), v(inner[k])], line);
    }
    let ring: Vec<Vec3> = inner.iter().map(|&p| v(p)).collect();
    m.fan_up(Vec3::ZERO, &ring, fill);
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
    let mats = CursorMaterials {
        ghost: overlay_material(&mut materials, Color::WHITE, 0.0),
        frame: overlay_material(&mut materials, Color::WHITE, 0.0),
        grid: overlay_material(&mut materials, Color::WHITE, 0.0),
    };
    commands.spawn((
        Name::new("Hovered cell"),
        HoverFrame {
            pos: Vec3::ZERO,
            shape: square().to_vec(),
        },
        Mesh3d(meshes.add(frame_mesh(&square()))),
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

/// `zoning::cell_shape` for the cursor and the zone tool's preview: the
/// blocks indexed once per roads and zoning version.
#[derive(Default)]
pub struct CellShapes {
    key: Option<((u64, u64), (u64, u64))>,
    shapes: Option<LandShapes>,
}

impl CellShapes {
    fn refresh(&mut self, sim: &SimWorld) {
        let key = (sim.key(sim.world.versions.roads), sim.key(sim.world.versions.grid));
        if self.key != Some(key) {
            self.key = Some(key);
            // One lattice: nothing is cut.
            self.shapes = sim
                .world
                .zoning
                .lattices()
                .nth(1)
                .is_some()
                .then(|| LandShapes::new(&sim.world));
        }
    }

    /// The land a zoning cell holds: its quad, or where the rows of two
    /// roads meet at an angle its cut shape (none when nothing is left).
    fn land(&self, world: &World, c: CellRef) -> Option<Vec<Vec2>> {
        let quad = world.zoning.cell_corners(c)?.map(Vec2::from).to_vec();
        match self.shapes.as_ref().and_then(|s| s.shape(world, c)) {
            Some(p) if p.len() >= 3 => Some(p.into_iter().map(Vec2::from).collect()),
            Some(_) => None,
            None => Some(quad),
        }
    }
}

/// The hovered cell as drawn: its centre and corners around it. With a
/// zone tool only zoneable land (the zoning cell under the cursor, whatever
/// lattice: a strip cell along a curve is a trapezoid, a cell where two
/// roads' rows meet is cut along the line between them); with other tools
/// the map cell when there is no zoning cell.
fn hovered_quad(
    sim: &SimWorld,
    shapes: &CellShapes,
    hovered: &HoveredCell,
    point: &HoveredPoint,
    tool: ActiveTool,
) -> Option<(Vec2, Vec<Vec2>)> {
    if let Some(c) = point.zone_cell
        && let Some(q) = shapes.land(&sim.world, c)
    {
        let ctr = q.iter().copied().sum::<Vec2>() / q.len() as f32;
        return Some((ctr, q.into_iter().map(|p| p - ctr).collect()));
    }
    if matches!(tool, ActiveTool::Zone(_)) {
        return None;
    }
    let c = hovered.0?;
    let ctr = Vec2::new((c.x as f32 + 0.5) * CELL_SIZE, (c.y as f32 + 0.5) * CELL_SIZE);
    Some((ctr, square().to_vec()))
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
    mut shapes: Local<CellShapes>,
) {
    let Ok((mut frame, mut t, mut vis, mesh)) = q.single_mut() else {
        return;
    };
    shapes.refresh(&sim);
    // Inspect highlights what it points at (buildings, roads, cars); a
    // cell frame on bare ground would only look like a stray tile.
    // The bulldozer marks what it would remove (red); a cell frame and the
    // grid mean nothing to it with free-form roads.
    let at = hovered_quad(&sim, &shapes, &hovered, &point, *tool).filter(|_| shows_cells(*tool));
    let Some((ctr, shape)) = at else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    if (shape.len() != frame.shape.len() || shape.iter().zip(frame.shape.iter()).any(|(a, b)| a.distance(*b) > 0.01))
        && let Some(mut m) = meshes.get_mut(&mesh.0)
    {
        *m = frame_mesh(&shape);
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

/// Tools that work on cells: the hovered-cell frame and the faint grid
/// around the cursor show while they are active.
fn shows_cells(tool: ActiveTool) -> bool {
    !matches!(tool, ActiveTool::Inspect | ActiveTool::Bulldoze)
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
        Some((corner, rot)) if shows_cells(*tool) => {
            t.translation = corner + Vec3::Y * 0.05;
            t.rotation = rot;
            vis.set_if_neq(Visibility::Inherited);
        }
        _ => {
            vis.set_if_neq(Visibility::Hidden);
        }
    }
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

/// Show `m` on entity `e` (hide it when empty; never remove `Mesh3d`).
fn show_mesh(commands: &mut Commands, meshes: &mut Assets<Mesh>, e: Entity, vis: &mut Mut<Visibility>, m: MeshBuilder) {
    if m.is_empty() {
        vis.set_if_neq(Visibility::Hidden);
    } else {
        commands.entity(e).insert(Mesh3d(meshes.add(m.build())));
        vis.set_if_neq(Visibility::Inherited);
    }
}

fn draw_ghost(
    mut commands: Commands,
    preview: Res<ToolPreview>,
    sim: Res<SimWorld>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut q: Query<(Entity, &mut Visibility), With<GhostMesh>>,
    mut shapes: Local<CellShapes>,
) {
    if !preview.is_changed() {
        return;
    }
    shapes.refresh(&sim);
    let Ok((entity, mut vis)) = q.single_mut() else { return };
    let mut m = MeshBuilder::new();
    let valid = preview.is_valid();
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
                // Each cell's land as the overlay draws it (cut where two
                // roads' rows meet), pulled in a little.
                for &c in &preview.zone_cells {
                    let Some(p) = shapes.land(&sim.world, c) else { continue };
                    let ctr = p.iter().copied().sum::<Vec2>() / p.len() as f32;
                    let ring: Vec<Vec3> = p
                        .iter()
                        .map(|&q| {
                            let q = q + (ctr - q).normalize_or_zero() * 0.3 * std::f32::consts::SQRT_2;
                            Vec3::new(q.x, y, q.y)
                        })
                        .collect();
                    m.fan_up(Vec3::new(ctr.x, y, ctr.y), &ring, fill);
                }
            }
            if let Some(q) = preview.area {
                let edge = lin(mix(base, Color::WHITE, 0.3).with_alpha(0.95));
                quad_ring(&mut m, &q, y + 0.01, 0.5, edge);
            }
        }
    }
    if let Some((node, size)) = preview.roundabout {
        ring_ghost(
            &mut m,
            &sim.world,
            node,
            size,
            valid,
            preview.error.as_ref().and_then(|e| e.at()),
        );
    }
    // Hide rather than remove `Mesh3d`: an entity whose Mesh3d was removed
    // and later re-inserted was never drawn again (seen with Bevy 0.20.1).
    show_mesh(&mut commands, &mut meshes, entity, &mut vis, m);
}

/// The roundabout the junction tool would build: its carriageway and
/// island, the edge of the land it takes, the arms' give-way lines; red
/// (with the trouble spot marked) when that size does not fit.
fn ring_ghost(
    m: &mut MeshBuilder,
    world: &World,
    node: citysim_sim::NodeId,
    size: citysim_sim::RoundaboutSize,
    valid: bool,
    trouble: Option<[f32; 2]>,
) {
    use std::f32::consts::TAU;
    let Some(pos) = world.roads.nodes.get(node).map(|n| Vec2::from(n.pos)) else {
        return;
    };
    let dims = size.dims();
    let plan = citysim_sim::roundabout::plan_ring(world, node, size).ok();
    let y = heights::GHOST;
    let base = if valid { palette::TOOL_OK } else { palette::TOOL_BAD };
    let fill = lin(base.with_alpha(if valid { 0.42 } else { 0.3 }));
    let edge = lin(mix(base, Color::WHITE, 0.35).with_alpha(0.95));
    let island = lin(mix(palette::MEDIAN_GRASS, base, 0.35).with_alpha(0.55));
    let ring = |m: &mut MeshBuilder, r0: f32, r1: f32, y: f32, c: [f32; 4]| {
        let n = ((r1 * TAU) / 1.2).ceil().max(24.0) as usize;
        for i in 0..n {
            let (a, b) = (i as f32 / n as f32 * TAU, (i + 1) as f32 / n as f32 * TAU);
            let d = |t: f32| Vec2::new(t.cos(), t.sin());
            let q = [pos + d(a) * r0, pos + d(b) * r0, pos + d(b) * r1, pos + d(a) * r1];
            m.quad_up(q.map(|v| Vec3::new(v.x, y, v.y)), c);
        }
    };
    ring(m, dims.inner_r, dims.outer_r, y, fill);
    ring(m, 0.0, dims.island_r, y, island);
    ring(m, dims.outer_r - 0.25, dims.outer_r + 0.25, y + 0.01, edge);
    // The edge of the land it takes: dashes.
    let r = dims.reserve_r();
    let n = ((r * TAU) / 3.0).ceil() as usize;
    for i in (0..n).step_by(2) {
        let (a, b) = (i as f32 / n as f32 * TAU, (i + 1) as f32 / n as f32 * TAU);
        let d = |t: f32| Vec2::new(t.cos(), t.sin());
        let q = [
            pos + d(a) * (r - 0.2),
            pos + d(b) * (r - 0.2),
            pos + d(b) * (r + 0.2),
            pos + d(a) * (r + 0.2),
        ];
        m.quad_up(q.map(|v| Vec3::new(v.x, y + 0.01, v.y)), edge);
    }
    // Give-way lines where the arms will end.
    if let Some(p) = &plan {
        for a in &p.arms {
            let Some(seg) = world.roads.segments.get(a.segment) else {
                continue;
            };
            let pose = seg.pose_dir(seg.a == node, a.setback, 0.0);
            let (c, d) = (Vec2::from(pose.pos), Vec2::from(pose.dir));
            let n = Vec2::new(-d.y, d.x) * a.kind.half_width_m();
            let q = [c - n - d * 0.3, c + n - d * 0.3, c + n + d * 0.3, c - n + d * 0.3];
            m.quad_up(q.map(|v| Vec3::new(v.x, y + 0.02, v.y)), edge);
        }
    }
    if let Some(t) = trouble {
        let t = Vec2::from(t);
        let bad = lin(palette::TOOL_BAD.with_alpha(0.9));
        let n = 20;
        for i in 0..n {
            let (a, b) = (i as f32 / n as f32 * TAU, (i + 1) as f32 / n as f32 * TAU);
            let d = |s: f32| Vec2::new(s.cos(), s.sin());
            let q = [t + d(a) * 1.2, t + d(b) * 1.2, t + d(b) * 2.2, t + d(a) * 2.2];
            m.quad_up(q.map(|v| Vec3::new(v.x, y + 0.03, v.y)), bad);
        }
    }
}
