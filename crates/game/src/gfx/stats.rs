//! Dev aid: `CITYSIM_GFX_STATS=1` logs scene statistics every 5 seconds
//! (entities, meshes, materials, building models, trees, triangles drawn by
//! visible meshes) to check that the scene stays batched.
//! `CITYSIM_GFX_QUALITY=low|medium|high` sets the starting `GraphicsQuality`
//! (dev aid for measuring; at runtime ui owns that resource).

use bevy::mesh::Indices;
use bevy::prelude::*;

use super::buildings::BuildingModels;
use super::trees::TreeCount;
use super::vehicles::VehicleModels;
use crate::shared::GraphicsQuality;

pub struct StatsPlugin;

impl Plugin for StatsPlugin {
    fn build(&self, app: &mut App) {
        let q = match std::env::var("CITYSIM_GFX_QUALITY").as_deref() {
            Ok("low") => Some(GraphicsQuality::Low),
            Ok("medium") => Some(GraphicsQuality::Medium),
            Ok("high") => Some(GraphicsQuality::High),
            _ => None,
        };
        if let Some(q) = q {
            app.add_systems(Startup, move |mut r: ResMut<GraphicsQuality>| *r = q);
        }
        if std::env::var("CITYSIM_GFX_TOOLDEMO").is_ok_and(|v| !v.is_empty() && v != "0") {
            app.add_systems(Update, tool_demo.before(crate::shared::GameSet::Render));
        }
        if std::env::var("CITYSIM_GFX_STATS").is_ok_and(|v| !v.is_empty() && v != "0") {
            app.add_systems(Update, log_stats);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn log_stats(
    time: Res<Time<Real>>,
    mut last: Local<f32>,
    entities: Query<Entity>,
    mesh_entities: Query<(&Mesh3d, &ViewVisibility)>,
    meshes: Res<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    buildings: Res<BuildingModels>,
    vehicles: Res<VehicleModels>,
    trees: Res<TreeCount>,
) {
    let now = time.elapsed_secs();
    if now - *last < 5.0 {
        return;
    }
    *last = now;
    let mut tris = 0usize;
    let mut visible = 0usize;
    for (m, vis) in &mesh_entities {
        if !vis.get() {
            continue;
        }
        visible += 1;
        if let Some(mesh) = meshes.get(&m.0) {
            tris += match mesh.indices() {
                Some(Indices::U32(i)) => i.len() / 3,
                Some(Indices::U16(i)) => i.len() / 3,
                None => 0,
            };
        }
    }
    info!(
        "gfx stats: {} entities, {} mesh entities ({} visible, {:.2} M triangles), {} mesh assets, {} materials, \
         {} building models, {} car models, {} trees",
        entities.iter().count(),
        mesh_entities.iter().count(),
        visible,
        tris as f64 / 1e6,
        meshes.len(),
        materials.len(),
        buildings.count(),
        vehicles.count(),
        trees.0,
    );
}

/// Dev aid (`CITYSIM_GFX_TOOLDEMO=1`, best with the sample city and a
/// gfx-only build): fills the ui-owned construction feedback resources with
/// a fixed example so their drawing can be checked without input.
fn tool_demo(
    sim: Res<crate::shared::SimWorld>,
    mut tool: ResMut<crate::shared::ActiveTool>,
    mut preview: ResMut<crate::shared::ToolPreview>,
    mut path: ResMut<crate::shared::PathPreview>,
    mut snap: ResMut<crate::shared::SnapFeedback>,
    mut hl: ResMut<crate::shared::WorldHighlight>,
) {
    use citysim_sim::Cell;

    use crate::shared::*;
    let world = &sim.world;
    tool.set_if_neq(ActiveTool::Road);
    let mut p = ToolPreview {
        kind: PreviewKind::Road,
        line: Some((Cell::new(52, 140), Cell::new(58, 146))),
        ..default()
    };
    p.cells.clear();
    preview.set_if_neq(p);
    let legs = vec![
        PathLeg {
            from: Cell::new(46, 140),
            to: Cell::new(52, 140),
            error: None,
            upgrade_cells: vec![],
        },
        PathLeg {
            from: Cell::new(52, 140),
            to: Cell::new(58, 146),
            error: None,
            upgrade_cells: vec![],
        },
        PathLeg {
            from: Cell::new(58, 146),
            to: Cell::new(58, 152),
            error: Some(citysim_sim::CommandError::OutOfBounds),
            upgrade_cells: vec![],
        },
    ];
    path.set_if_neq(PathPreview {
        kind: RoadChoice::Avenue,
        points: vec![
            Cell::new(46, 140),
            Cell::new(52, 140),
            Cell::new(58, 146),
            Cell::new(58, 152),
        ],
        placed: 3,
        legs,
        length_m: 120.0,
        cost: 10_000,
        error: None,
    });
    let end = Cell::new(58, 152).center();
    snap.set_if_neq(SnapFeedback {
        point: Some(end),
        cell: Some(Cell::new(58, 152)),
        target: SnapTarget::Perpendicular,
        matched: vec![SnapTarget::Perpendicular, SnapTarget::Grid],
        guides: vec![
            Guide {
                from: Cell::new(58, 146).center(),
                to: Cell::new(58, 170).center(),
                kind: GuideKind::Angle,
            },
            Guide {
                from: Cell::new(45, 152).center(),
                to: Cell::new(66, 152).center(),
                kind: GuideKind::Perpendicular,
            },
        ],
        spacing_cells: None,
        suspended: false,
    });
    let at = Vec2::new(250.0, 1040.0);
    let building = world
        .buildings
        .iter()
        .min_by(|a, b| {
            let d = |c: Cell| Vec2::from(c.center()).distance(at);
            d(a.1.footprint.min).total_cmp(&d(b.1.footprint.min))
        })
        .map(|(id, _)| id);
    let junction = citysim_sim::junction::junction_nodes(world)
        .min_by(|a, b| {
            let d = |n| Vec2::from(world.roads.nodes[n].pos).distance(at);
            d(*a).total_cmp(&d(*b))
        })
        .map(|n| (n, world.roads.nodes[n].pos));
    hl.set_if_neq(WorldHighlight {
        hovered: junction.map(|(node, pos)| WorldTarget::Junction { node, pos }),
        selected: building.map(WorldTarget::Building),
        doomed_buildings: vec![],
        doomed_road_cells: vec![],
    });
}
