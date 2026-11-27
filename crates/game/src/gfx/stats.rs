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
        if let Ok(at) = std::env::var("CITYSIM_GFX_SHOWCASE_SITES") {
            let v: Vec<f32> = at.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 2 {
                let at = Vec2::new(v[0], v[1]);
                app.add_systems(
                    PostStartup,
                    move |c: Commands,
                          m: ResMut<BuildingModels>,
                          a: ResMut<Assets<Mesh>>,
                          w: Res<super::scene::WorldMaterials>| { showcase_sites(c, m, a, w, at) },
                );
            }
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

/// Dev aid (`CITYSIM_GFX_TOOLDEMO=1|2|3|7`, a gfx-only build): fills the
/// ui-owned tool feedback with a fixed example so its drawing can be checked
/// without input, around `CITYSIM_GFX_TOOLDEMO_AT=x,z` (sim metres; by
/// default a spot of the `shapes` demo town). 1: Inspect, a junction hovered
/// and the nearest building selected; 2: the bulldozer's area (marquee) with
/// the roads and buildings it would remove; 3: zoning a marquee
/// commercial; 7: de-zoning the zoned cells round the spot. The free-form
/// road tool's samples 4-6 are in `road_tool`.
fn tool_demo(
    sim: Res<crate::shared::SimWorld>,
    mut tool: ResMut<crate::shared::ActiveTool>,
    mut preview: ResMut<crate::shared::ToolPreview>,
    mut hl: ResMut<crate::shared::WorldHighlight>,
) {
    use citysim_sim::{Command, Zone};

    use crate::shared::*;
    let world = &sim.world;
    let case: u32 = std::env::var("CITYSIM_GFX_TOOLDEMO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    if (4..=6).contains(&case) {
        return;
    }
    let at = std::env::var("CITYSIM_GFX_TOOLDEMO_AT")
        .ok()
        .and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() == 2).then(|| Vec2::new(v[0], v[1]))
        })
        .unwrap_or(Vec2::new(900.0, 1150.0));
    // A marquee 90 x 60 m round the spot.
    let quad = [[-45.0, -30.0], [45.0, -30.0], [45.0, 30.0], [-45.0, 30.0]].map(|[x, y]| [at.x + x, at.y + y]);
    match case {
        2 => {
            tool.set_if_neq(ActiveTool::Bulldoze);
            let p = world.preview(&Command::BulldozeArea { quad });
            let (roads, buildings) = match &p {
                Ok(p) => (
                    p.removed
                        .iter()
                        .map(|c| {
                            let kind = world
                                .roads
                                .nearest_segment(c.point_t(0.5), 4.0)
                                .and_then(|(s, _, _)| world.roads.segments.get(s))
                                .map_or(citysim_sim::RoadKind::Street, |s| s.kind);
                            (*c, kind)
                        })
                        .collect(),
                    p.buildings.clone(),
                ),
                Err(_) => (Vec::new(), Vec::new()),
            };
            preview.set_if_neq(ToolPreview {
                kind: PreviewKind::Bulldoze,
                area: Some(quad),
                error: p.as_ref().err().cloned(),
                cost: p.as_ref().map_or(0, |p| p.cost),
                ..default()
            });
            hl.set_if_neq(WorldHighlight {
                doomed_buildings: buildings,
                doomed_roads: roads,
                ..default()
            });
        }
        3 | 7 => {
            let zone = (case == 3).then_some(Zone::Commercial);
            tool.set_if_neq(ActiveTool::Zone(zone));
            let p = world.preview(&Command::ZoneArea { quad, zone });
            preview.set_if_neq(ToolPreview {
                kind: PreviewKind::Zone(zone),
                area: (case == 3).then_some(quad),
                zone_cells: p.as_ref().map(|p| p.zone_cells.clone()).unwrap_or_default(),
                error: p.as_ref().err().cloned(),
                cost: p.as_ref().map_or(0, |p| p.cost),
            });
        }
        _ => {
            tool.set_if_neq(ActiveTool::Inspect);
            let building = world
                .buildings
                .iter()
                .min_by(|a, b| {
                    let d = |c: [f32; 2]| Vec2::from(c).distance(at);
                    d(a.1.center()).total_cmp(&d(b.1.center()))
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
                ..default()
            });
        }
    }
}

/// Dev aid (`CITYSIM_GFX_SHOWCASE_SITES=x,z`): a row of building sites at
/// sim point `(x, z)` (both stages; tall ones with their crane) and the
/// finished buildings beside them, to look at sites without waiting for
/// the sim to start one.
fn showcase_sites(
    mut commands: Commands,
    mut models: ResMut<BuildingModels>,
    mut meshes: ResMut<Assets<Mesh>>,
    mats: Res<super::scene::WorldMaterials>,
    at: Vec2,
) {
    use citysim_sim::Zone;

    use super::building_gen::ModelKey;
    let keys = [
        (Zone::Residential, (2, 2), 3),
        (Zone::Commercial, (2, 2), 3),
        (Zone::Residential, (1, 2), 3),
        (Zone::Commercial, (1, 1), 2),
        (Zone::Industrial, (2, 2), 2),
    ];
    for (k, &(kind, cells, level)) in keys.iter().enumerate() {
        let key = ModelKey {
            kind,
            cells,
            level,
            variant: 3,
        };
        for (row, stage) in [(0, Some(0u8)), (1, Some(1)), (2, None)] {
            let mesh = match stage {
                Some(s) => models.site(key, s, &mut meshes),
                None => models.get(key, &mut meshes)[1].clone(),
            };
            let p = at + Vec2::new(k as f32 * 22.0, row as f32 * 22.0);
            commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(mats.lit.clone()),
                Transform::from_xyz(p.x, 0.0, p.y),
            ));
        }
    }
}
