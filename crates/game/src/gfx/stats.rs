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

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
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
    lights: Query<(), Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>>,
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
         {} building models, {} car models, {} trees, {} real lights",
        entities.iter().count(),
        mesh_entities.iter().count(),
        visible,
        tris as f64 / 1e6,
        meshes.len(),
        materials.len(),
        buildings.count(),
        vehicles.count(),
        trees.0,
        lights.iter().count(),
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
    let case: u32 = std::env::var("CITYSIM_GFX_TOOLDEMO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let leg = |a: (i32, i32), b: (i32, i32), ok: bool| PathLeg {
        from: Cell::new(a.0, a.1),
        to: Cell::new(b.0, b.1),
        error: (!ok).then_some(citysim_sim::CommandError::OutOfBounds),
        upgrade_cells: vec![],
    };
    // 1: a street from a dead end; 2: an avenue from a street to the main
    // avenue (two T junctions); 3: a path whose last leg is refused.
    let (kind, pts, legs): (RoadChoice, Vec<(i32, i32)>, Vec<PathLeg>) = match case {
        2 => (
            RoadChoice::Avenue,
            vec![(45, 140), (58, 140), (58, 128)],
            vec![leg((45, 140), (58, 140), true), leg((58, 140), (58, 128), true)],
        ),
        3 => (
            RoadChoice::Street,
            vec![(60, 150), (70, 150), (76, 144)],
            vec![leg((60, 150), (70, 150), true), leg((70, 150), (76, 144), false)],
        ),
        _ => (
            RoadChoice::Street,
            vec![(30, 156), (30, 166), (40, 176)],
            vec![leg((30, 156), (30, 166), true), leg((30, 166), (40, 176), true)],
        ),
    };
    let last = *pts.last().expect("points");
    let error = legs.iter().find_map(|l| l.error.clone());
    preview.set_if_neq(ToolPreview {
        kind: PreviewKind::Road,
        line: Some((
            Cell::new(pts[pts.len() - 2].0, pts[pts.len() - 2].1),
            Cell::new(last.0, last.1),
        )),
        error: error.clone(),
        ..default()
    });
    path.set_if_neq(PathPreview {
        kind,
        points: pts.iter().map(|p| Cell::new(p.0, p.1)).collect(),
        placed: pts.len() - 1,
        legs,
        length_m: 120.0,
        cost: 10_000,
        error: error.clone(),
    });
    let end = Cell::new(last.0, last.1);
    let prev = pts[pts.len() - 2];
    snap.set_if_neq(SnapFeedback {
        point: Some(end.center()),
        cell: Some(end),
        target: if case == 2 { SnapTarget::Road } else { SnapTarget::Angle },
        matched: vec![SnapTarget::Angle, SnapTarget::Grid],
        guides: vec![Guide {
            from: Cell::new(prev.0, prev.1).center(),
            to: Cell::new(last.0 + (last.0 - prev.0), last.1 + (last.1 - prev.1)).center(),
            kind: GuideKind::Angle,
        }],
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
