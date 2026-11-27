//! Feedback for drawing free-form roads: the road that would be built at
//! its true width along its curve (the sim's planned pieces, split at their
//! nodes), stretches the sim refuses in red with the spot it names, the
//! control points and tangent handles, the tightest bend's radius as an
//! arc of that circle (red when too tight), the snapped point and its
//! guide lines, and the junctions the road would create (end, join, T,
//! crossing).
//!
//! Drawn from `shared::CurveToolView`, written by the interface's road
//! tool. `CITYSIM_GFX_TOOLDEMO=4|5|6` fills it from
//! real `World::preview` results on the road-shape demo town
//! (`CITYSIM_DEMO=shapes`) (`4`: a curved
//! street from the avenue across a street into another, `5`: an avenue
//! bending too tightly, `6`: a road the sim refuses for running along
//! another).

use bevy::light::NotShadowCaster;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use citysim_sim::command::{NodeChange, PieceChange};
use citysim_sim::{Command, Curve, RoadKind};

use super::heights;
use super::mesh_util::MeshBuilder;
use super::road_geom::{self, Frame, SegLayout, lift, strip};
use crate::shared::{CurveToolView, GameSet, Guide, GuideKind, SimWorld, SnapTarget};

#[derive(Component)]
struct ToolPart(usize);

/// Unlit vertex-coloured marks drawn over everything (no depth test): a
/// faint copy of the tool marks so they read through trees and buildings.
pub type XrayMaterial = ExtendedMaterial<StandardMaterial, XrayExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct XrayExt {}

impl MaterialExtension for XrayExt {
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_compare = Some(CompareFunction::Always);
            ds.depth_write_enabled = Some(false);
        }
        Ok(())
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

pub struct RoadToolPlugin;

impl Plugin for RoadToolPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<XrayMaterial>::default())
            .add_systems(Startup, setup)
            .add_systems(Update, draw.in_set(GameSet::Render));
        let demo: u32 = std::env::var("CITYSIM_GFX_TOOLDEMO")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if (4..=6).contains(&demo) {
            app.add_systems(Update, sample.before(GameSet::Render));
        }
    }
}

fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut xray: ResMut<Assets<XrayMaterial>>,
) {
    // 0: the road (lit, translucent); 1: marks over everything nearby.
    let road = materials.add(StandardMaterial {
        base_color: Color::srgba(0.86, 0.93, 1.0, 0.75),
        emissive: LinearRgba::rgb(0.2, 0.24, 0.3),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.9,
        depth_bias: 20.0,
        ..default()
    });
    let marks = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        depth_bias: 40.0,
        ..default()
    });
    for (i, m) in [road, marks].into_iter().enumerate() {
        commands.spawn((
            Name::new("Curve tool"),
            ToolPart(i),
            MeshMaterial3d(m),
            Transform::default(),
            Visibility::Hidden,
            NotShadowCaster,
        ));
    }
    // 2: the marks again, faint, through whatever hides them.
    commands.spawn((
        Name::new("Curve tool (x-ray)"),
        ToolPart(2),
        MeshMaterial3d(xray.add(ExtendedMaterial {
            base: StandardMaterial {
                base_color: Color::srgba(1.0, 1.0, 1.0, 0.32),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            },
            extension: XrayExt {},
        })),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
const SOFT: [f32; 4] = [1.0, 1.0, 1.0, 0.35];
const RED: [f32; 4] = [1.0, 0.25, 0.2, 0.95];
const RED_SOFT: [f32; 4] = [1.0, 0.3, 0.25, 0.3];
const AMBER: [f32; 4] = [1.0, 0.72, 0.2, 0.95];
const BLUE: [f32; 4] = [0.45, 0.8, 1.0, 0.95];

fn disc(m: &mut MeshBuilder, p: Vec2, r: f32, y: f32, c: [f32; 4]) {
    let ring: Vec<Vec3> = (0..20)
        .map(|k| lift(p + Vec2::from_angle(k as f32 / 20.0 * std::f32::consts::TAU) * r, y))
        .collect();
    m.fan_up(lift(p, y), &ring, c);
}

fn ring(m: &mut MeshBuilder, p: Vec2, r: f32, w: f32, y: f32, c: [f32; 4]) {
    let n = 28;
    for k in 0..n {
        let (a0, a1) = (
            k as f32 / n as f32 * std::f32::consts::TAU,
            (k + 1) as f32 / n as f32 * std::f32::consts::TAU,
        );
        let (d0, d1) = (Vec2::from_angle(a0), Vec2::from_angle(a1));
        m.quad_up(
            [p + d0 * r, p + d1 * r, p + d1 * (r + w), p + d0 * (r + w)].map(|q| lift(q, y)),
            c,
        );
    }
}

/// A flat line from `a` to `b`, `w` wide; dashed when `dash` > 0.
fn line(m: &mut MeshBuilder, a: Vec2, b: Vec2, w: f32, y: f32, dash: f32, c: [f32; 4]) {
    let f = Frame::line(a, b);
    if dash <= 0.0 {
        strip(m, &f, 0.0, f.len(), -w * 0.5, w * 0.5, y, c);
        return;
    }
    let mut t = 0.0;
    while t < f.len() {
        strip(m, &f, t, (t + dash).min(f.len()), -w * 0.5, w * 0.5, y, c);
        t += dash * 2.0;
    }
}

/// The marks for one view: (road mesh, marks mesh). `k` scales the marks
/// (points, handles, lines) so they stay readable when zoomed out.
pub fn meshes(view: &CurveToolView, k: f32) -> (MeshBuilder, MeshBuilder) {
    let mut road = MeshBuilder::new();
    let mut marks = MeshBuilder::new();
    let kind = view.kind.unwrap_or(RoadKind::Street);
    let y = road_geom::WALK_Y + 0.08;
    let outer = road_geom::style(kind).outer();
    // The road as it will be, edged in white (amber: an upgrade of an
    // existing piece).
    for (curve, change) in &view.pieces {
        let seg = SegLayout::bare(curve, kind);
        road_geom::mesh_segment_with(&mut road, &seg, false);
        let edge = if *change == PieceChange::New { WHITE } else { AMBER };
        // A soft fill of the road's width (what the x-ray copy shows of it
        // through trees and buildings).
        strip(
            &mut marks,
            &seg.frame,
            0.0,
            seg.len(),
            -outer,
            outer,
            y - 0.02,
            [0.85, 0.93, 1.0, 0.16],
        );
        for side in [1.0f32, -1.0] {
            let (a, b) = (side * (outer - 0.3), side * (outer - 0.02));
            strip(&mut marks, &seg.frame, 0.0, seg.len(), a.min(b), a.max(b), y, edge);
        }
    }
    road.translate(Vec3::Y * 0.04);
    // Refused stretches: a red band over their whole width.
    for curve in &view.refused {
        let f = Frame::new(curve);
        strip(&mut marks, &f, 0.0, f.len(), -outer, outer, y - 0.01, RED_SOFT);
        for side in [1.0f32, -1.0] {
            let (a, b) = (side * (outer - 0.3), side * outer);
            strip(&mut marks, &f, 0.0, f.len(), a.min(b), a.max(b), y, RED);
        }
    }
    if let Some(p) = view.error_at {
        let p = Vec2::from(p);
        ring(&mut marks, p, 2.2 * k, 0.5 * k, y + 0.02, RED);
        for d in [Vec2::new(1.0, 1.0), Vec2::new(1.0, -1.0)] {
            let d = d.normalize() * 1.6 * k;
            line(&mut marks, p - d, p + d, 0.45 * k, y + 0.03, 0.0, RED);
        }
    }
    // The tightest bend: an arc of its circle through the point.
    if let Some((p, c, r, bad)) = view.tightest {
        let (p, c) = (Vec2::from(p), Vec2::from(c));
        let col = if bad { RED } else { AMBER };
        let a0 = (p - c).to_angle();
        let span = (14.0 / r.max(1.0)).min(1.2);
        let n = 24;
        for i in 0..n {
            if i % 2 == 1 {
                continue;
            }
            let (b0, b1) = (
                a0 - span + 2.0 * span * i as f32 / n as f32,
                a0 - span + 2.0 * span * (i + 1) as f32 / n as f32,
            );
            let (q0, q1) = (c + Vec2::from_angle(b0) * r, c + Vec2::from_angle(b1) * r);
            line(&mut marks, q0, q1, 0.3 * k, y + 0.02, 0.0, col);
        }
        // Radius spoke from the centre to the road, and the centre.
        line(&mut marks, c, p, 0.18 * k, y + 0.02, 0.8 * k, col);
        disc(&mut marks, c, 0.45 * k, y + 0.02, col);
    }
    // Tangent handles and control points.
    for (a, h) in &view.handles {
        let (a, h) = (Vec2::from(*a), Vec2::from(*h));
        line(&mut marks, a, h, 0.16 * k, y + 0.03, 0.0, SOFT);
        let s = 0.55 * k;
        marks.quad_up(
            [
                h + Vec2::new(-s, -s),
                h + Vec2::new(s, -s),
                h + Vec2::new(s, s),
                h + Vec2::new(-s, s),
            ]
            .map(|q| lift(q, y + 0.04)),
            BLUE,
        );
    }
    for (i, p) in view.points.iter().enumerate() {
        let p = Vec2::from(*p);
        let last = i + 1 == view.points.len();
        let r = if last { 1.1 } else { 0.85 } * k;
        disc(&mut marks, p, r, y + 0.05, WHITE);
        ring(&mut marks, p, r, 0.25 * k, y + 0.05, [0.1, 0.12, 0.15, 0.9]);
    }
    // Junctions it makes: rings sized and coloured by kind.
    for (p, change) in &view.nodes {
        let p = Vec2::from(*p);
        let (r, c) = match change {
            NodeChange::End => (1.2, SOFT),
            NodeChange::Join => (outer + 1.0, BLUE),
            NodeChange::Split => (outer + 1.0, [0.4, 1.0, 0.85, 0.95]),
            NodeChange::Crossing => (outer + 1.5, AMBER),
        };
        ring(&mut marks, p, r.max(1.2 * k), 0.4 * k, y + 0.03, c);
    }
    // Snap marker: a diamond; guides dashed.
    if let Some((p, _)) = view.snap {
        let p = Vec2::from(p);
        let s = 1.4 * k;
        let q = [p + Vec2::X * s, p + Vec2::Y * s, p - Vec2::X * s, p - Vec2::Y * s];
        marks.quad_up(q.map(|v| lift(v, y + 0.06)), [0.3, 0.85, 1.0, 0.9]);
    }
    for g in &view.guides {
        let c = match g.kind {
            GuideKind::Angle => SOFT,
            _ => [0.45, 0.8, 1.0, 0.7],
        };
        line(
            &mut marks,
            Vec2::from(g.from),
            Vec2::from(g.to),
            0.22 * k,
            y + 0.01,
            1.6 * k,
            c,
        );
    }
    (road, marks)
}

fn draw(
    mut commands: Commands,
    view: Res<CurveToolView>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut parts: Query<(Entity, &ToolPart, &mut Visibility)>,
    rigs: Query<&super::camera::CameraRig>,
    mut last_k: Local<f32>,
    tool_time: Res<super::bench::ToolDrawTime>,
) {
    let _timed = tool_time.time();
    // Marks grow with the zoom (in steps, so they are not re-meshed every
    // frame while zooming).
    let dist = rigs.iter().next().map_or(150.0, |r| r.current.distance);
    let k = 2f32.powf((dist / 140.0).max(1.0).log2().round().min(3.0));
    if !view.is_changed() && k == *last_k {
        return;
    }
    *last_k = k;
    let (road, marks) = if view.active {
        meshes_of(&view, k)
    } else {
        (MeshBuilder::new(), MeshBuilder::new())
    };
    let xray = marks.clone();
    let mut built = [Some(road), Some(marks), Some(xray)];
    for (e, part, mut vis) in &mut parts {
        let Some(m) = built[part.0].take() else { continue };
        if m.is_empty() {
            vis.set_if_neq(Visibility::Hidden);
        } else {
            // Never remove Mesh3d (see cursor.rs): replace it.
            commands.entity(e).insert(Mesh3d(meshes.add(m.build())));
            vis.set_if_neq(Visibility::Inherited);
        }
    }
}

fn meshes_of(view: &CurveToolView, k: f32) -> (MeshBuilder, MeshBuilder) {
    let (road, mut marks) = meshes(view, k);
    // Marks sit just above the zone overlay's height range.
    marks.translate(Vec3::Y * (heights::GHOST - road_geom::WALK_Y - 0.08).max(0.0));
    (road, marks)
}

/// Fill the view from a real preview of `curve` (dev sample data).
pub fn view_of(world: &citysim_sim::World, curve: Curve, kind: RoadKind, handles: bool) -> CurveToolView {
    let mut v = CurveToolView {
        active: true,
        kind: Some(kind),
        points: vec![curve.start(), curve.end()],
        ..default()
    };
    if handles {
        v.handles = vec![(curve.p[0], curve.p[1]), (curve.p[3], curve.p[2])];
    }
    let (r, d) = curve.min_radius();
    if r.is_finite() && r < 400.0 {
        let pose = curve.pose_at(d, 0.0);
        let n = Vec2::from(pose.right());
        // The centre is on the inside of the bend.
        let ahead = Vec2::from(curve.pose_at((d + 1.0).min(curve.length()), 0.0).dir);
        let side = if Vec2::from(pose.dir).perp_dot(ahead) > 0.0 {
            1.0
        } else {
            -1.0
        };
        let c = Vec2::from(pose.pos) + n * side * r;
        v.tightest = Some((pose.pos, c.into(), r, r < kind.min_radius_m()));
    }
    match world.preview(&Command::BuildCurve { curve, kind }) {
        Ok(p) => {
            v.pieces = p.pieces.iter().map(|pc| (pc.curve, pc.change)).collect();
            v.nodes = p.nodes.iter().map(|n| (n.pos, n.change)).collect();
        }
        Err(e) => {
            v.refused = vec![curve];
            v.error_at = e.at();
        }
    }
    let end = curve.end();
    v.snap = Some((end, SnapTarget::Road));
    let dir = Vec2::from(curve.end_dir());
    v.guides = vec![Guide {
        from: end,
        to: (Vec2::from(end) + dir * 60.0).into(),
        kind: GuideKind::Extension,
    }];
    v
}

/// `CITYSIM_GFX_TOOLDEMO=4|5|6`: sample tool states on the shapes town.
fn sample(sim: Res<SimWorld>, mut view: ResMut<CurveToolView>, mut done: Local<Option<(u64, u64)>>) {
    let key = sim.key(sim.world.versions.roads);
    if *done == Some(key) {
        return;
    }
    *done = Some(key);
    let case: u32 = std::env::var("CITYSIM_GFX_TOOLDEMO")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let w = &sim.world;
    // Ends onto the nearest road, as the tool's snapping would put them.
    let snap = |p: [f32; 2]| {
        w.roads
            .nearest_segment(p, 12.0)
            .and_then(|(s, d, _)| w.roads.segments.get(s).map(|seg| seg.pose(d, 0.0).pos))
            .unwrap_or(p)
    };
    // From the avenue at a right angle, bending west onto a street of
    // the south grid; `end_y` 1150 crosses a street on the way (valid),
    // 1200 runs along one (refused).
    let from_avenue = |end_y: f32| {
        let start = w
            .roads
            .nearest_segment([520.0, 900.0], 40.0)
            .map(|(s, d, _)| w.roads.segments[s].pose(d, 0.0));
        let start = start.unwrap_or(citysim_sim::Pose {
            pos: [520.0, 900.0],
            dir: [1.0, 0.0],
        });
        let r = start.right();
        let south = if r[1] > 0.0 { r } else { [-r[0], -r[1]] };
        let e = snap([300.0, end_y]);
        Curve::new(
            start.pos,
            [start.pos[0] + south[0] * 120.0, start.pos[1] + south[1] * 120.0],
            [e[0] + 140.0, e[1]],
            e,
        )
    };
    let v = match case {
        4 => view_of(w, from_avenue(1150.0), RoadKind::Street, true),
        // An avenue bending too tightly.
        5 => view_of(
            w,
            Curve::new([560.0, 980.0], [640.0, 980.0], [600.0, 1020.0], [600.0, 1100.0]),
            RoadKind::Avenue,
            true,
        ),
        _ => view_of(w, from_avenue(1200.0), RoadKind::Street, false),
    };
    view.set_if_neq(v);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_meshes_without_nan() {
        let mut v = CurveToolView {
            active: true,
            kind: Some(RoadKind::Avenue),
            pieces: vec![(
                Curve::new([0.0, 0.0], [50.0, 0.0], [80.0, 30.0], [80.0, 80.0]),
                PieceChange::New,
            )],
            refused: vec![Curve::straight([0.0, 10.0], [40.0, 10.0])],
            error_at: Some([20.0, 10.0]),
            points: vec![[0.0, 0.0], [80.0, 80.0]],
            handles: vec![([0.0, 0.0], [50.0, 0.0])],
            tightest: Some(([60.0, 20.0], [40.0, 40.0], 28.0, false)),
            snap: Some(([80.0, 80.0], SnapTarget::Node)),
            guides: vec![Guide {
                from: [80.0, 80.0],
                to: [80.0, 140.0],
                kind: GuideKind::Extension,
            }],
            nodes: vec![([80.0, 80.0], NodeChange::Split), ([0.0, 0.0], NodeChange::End)],
        };
        let (road, marks) = meshes(&v, 1.0);
        assert!(road.triangle_count() > 10 && marks.triangle_count() > 50);
        assert!(road.pos.iter().chain(&marks.pos).flatten().all(|x| x.is_finite()));
        v.active = false;
        assert!(!meshes_of(&v, 2.0).0.is_empty());
    }
}
