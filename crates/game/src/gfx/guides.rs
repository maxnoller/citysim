//! Construction feedback the ui publishes in `shared` (gfx only draws it):
//! * the snap marker (a different glyph for snapping to a node, onto a road,
//!   the grid, an angle, a perpendicular / parallel alignment, an extension
//!   or a block spacing) with a soft pulse;
//! * dashed guide lines, coloured by why they are shown;
//! * highlights (issue 6): the hovered and selected building glows (a tinted
//!   copy of its own model) with a ring on the ground, a road stretch gets a
//!   bright band, a junction a ring coloured by its control; buildings and
//!   road cells the bulldozer would remove glow red.
//! * a hovered or selected car gets a ring that drives with it.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::RoadKind;
use citysim_sim::junction::{self, JunctionControl};

use super::buildings::{BuildingEntities, BuildingModels};
use super::heights;
use super::mesh_util::MeshBuilder;
use super::palette::{self, lin, mix};
use super::road_geom;
use super::vehicles::DrawnSet;
use crate::shared::{
    ActiveTool, DrawnPoses, GameSet, GuideKind, RoadMode, SimWorld, SnapFeedback, SnapTarget, ToolModes, WorldCamera,
    WorldHighlight, WorldTarget,
};

#[derive(Component)]
struct SnapMarker;

#[derive(Component)]
struct GuideLines;

#[derive(Component)]
struct HighlightGround;

/// Ring under the hovered / selected vehicle (moved every frame).
#[derive(Component)]
struct VehicleRing;

/// Diamond floating above the selected / followed vehicle: it grows with
/// the camera distance so the car stays unmistakable far away and at night
/// (unlit).
#[derive(Component)]
struct VehicleMarker;

/// Tinted copies of highlighted buildings.
#[derive(Component)]
struct HighlightCopy;

#[derive(Resource)]
struct GuideMaterials {
    /// Additive tint for hovered / selected buildings.
    glow: Handle<StandardMaterial>,
    /// Additive red for buildings the bulldozer would remove.
    doom: Handle<StandardMaterial>,
}

pub struct GuidesPlugin;

impl Plugin for GuidesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(
                Update,
                (draw_snap, pulse_marker, draw_highlight).in_set(GameSet::Render),
            )
            // On the cars as they are drawn this frame.
            .add_systems(
                Update,
                (vehicle_ring, vehicle_marker)
                    .in_set(GameSet::Render)
                    .after(DrawnSet::Traveller),
            );
    }
}

fn setup(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>, mut meshes: ResMut<Assets<Mesh>>) {
    let overlay = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        depth_bias: 50.0,
        ..default()
    });
    let add = |c: Color| StandardMaterial {
        base_color: c,
        alpha_mode: AlphaMode::Add,
        unlit: true,
        ..default()
    };
    let mats = GuideMaterials {
        glow: materials.add(add(Color::srgb(0.22, 0.4, 0.62))),
        // Blended, not added: light roofs and walls read as red too (an
        // added dark red barely showed on a grey roof).
        doom: materials.add(StandardMaterial {
            base_color: Color::srgba(0.86, 0.1, 0.06, 0.6),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        }),
    };
    for (name, marker) in [("Snap marker", true), ("Guide lines", false)] {
        let mut ec = commands.spawn((
            Name::new(name),
            MeshMaterial3d(overlay.clone()),
            Transform::default(),
            Visibility::Hidden,
            NotShadowCaster,
        ));
        if marker {
            ec.insert(SnapMarker);
        } else {
            ec.insert(GuideLines);
        }
    }
    // White inner ring and an accent outer ring, as the interface draws a
    // selection.
    let mut ring_mesh = MeshBuilder::new();
    let white = lin(Color::srgb(1.0, 1.0, 1.0).with_alpha(0.95));
    let accent = lin(ACCENT.with_alpha(0.95));
    ring(&mut ring_mesh, Vec2::ZERO, 3.6, 0.45, 0.0, white, 32);
    ring(&mut ring_mesh, Vec2::ZERO, 4.5, 0.5, 0.0, accent, 32);
    disc(
        &mut ring_mesh,
        Vec2::ZERO,
        3.5,
        -0.01,
        [accent[0], accent[1], accent[2], 0.22],
    );
    let mut diamond = MeshBuilder::new();
    let (top, mid, bot) = (Vec3::new(0.0, 1.4, 0.0), 0.0, Vec3::new(0.0, -1.6, 0.0));
    let rim: Vec<Vec3> = (0..4)
        .map(|i| {
            let a = i as f32 * std::f32::consts::FRAC_PI_2;
            Vec3::new(a.cos() * 1.0, mid, a.sin() * 1.0)
        })
        .collect();
    let light = lin(Color::srgb(0.78, 0.72, 1.0));
    for i in 0..4 {
        let (p, q) = (rim[i], rim[(i + 1) % 4]);
        let shade = if i % 2 == 0 { accent } else { light };
        diamond.tri_c(top, q, p, [shade; 3]);
        diamond.tri_c(bot, p, q, [accent; 3]);
    }
    commands.spawn((
        Name::new("Vehicle marker"),
        VehicleMarker,
        Mesh3d(meshes.add(diamond.build())),
        MeshMaterial3d(overlay.clone()),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Vehicle ring"),
        VehicleRing,
        Mesh3d(meshes.add(ring_mesh.build())),
        MeshMaterial3d(overlay.clone()),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Highlight"),
        HighlightGround,
        MeshMaterial3d(overlay),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.insert_resource(mats);
}

/// Guides are a warm amber whatever the rule (the marker glyph says
/// which): it stands apart from the white construction grid and the
/// blue-white road ghost.
fn guide_color(_: GuideKind) -> Color {
    Color::srgba(1.0, 0.78, 0.3, 0.95)
}

/// The interface accent (ui theme `ACCENT_SET.base`, amethyst 8f7bd8).
const ACCENT: Color = Color::srgb(0.561, 0.482, 0.847);

fn snap_color(_: SnapTarget) -> Color {
    ACCENT
}

fn v3(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

/// Flat band from `a` to `b` of width `w`.
fn band(m: &mut MeshBuilder, a: Vec2, b: Vec2, w: f32, y: f32, c: [f32; 4]) {
    let d = (b - a).normalize_or_zero();
    if d == Vec2::ZERO {
        return;
    }
    let n = Vec2::new(-d.y, d.x) * w * 0.5;
    m.quad_up([a - n, b - n, b + n, a + n].map(|p| v3(p, y)), c);
}

/// Flat ring (annulus) around `c`.
fn ring(m: &mut MeshBuilder, c: Vec2, r: f32, w: f32, y: f32, col: [f32; 4], seg: usize) {
    for i in 0..seg {
        let (a0, a1) = (
            i as f32 / seg as f32 * std::f32::consts::TAU,
            (i + 1) as f32 / seg as f32 * std::f32::consts::TAU,
        );
        let (u0, u1) = (Vec2::from_angle(a0), Vec2::from_angle(a1));
        m.quad_up(
            [c + u0 * (r - w), c + u1 * (r - w), c + u1 * r, c + u0 * r].map(|p| v3(p, y)),
            col,
        );
    }
}

/// Filled disc.
fn disc(m: &mut MeshBuilder, c: Vec2, r: f32, y: f32, col: [f32; 4]) {
    let pts: Vec<Vec3> = (0..16)
        .map(|i| v3(c + Vec2::from_angle(i as f32 / 16.0 * std::f32::consts::TAU) * r, y))
        .collect();
    m.fan_up(v3(c, y), &pts, col);
}

/// Closed polygon outline.
fn outline(m: &mut MeshBuilder, pts: &[Vec2], w: f32, y: f32, col: [f32; 4]) {
    for i in 0..pts.len() {
        band(m, pts[i], pts[(i + 1) % pts.len()], w, y, col);
    }
}

/// Snap glyph centred on the origin (the entity is moved to the point).
fn snap_glyph(m: &mut MeshBuilder, t: SnapTarget, suspended: bool) {
    let y = 0.0;
    let col = if suspended {
        lin(Color::srgba(0.8, 0.8, 0.8, 0.7))
    } else {
        lin(snap_color(t).with_alpha(0.95))
    };
    let soft = [col[0], col[1], col[2], 0.25];
    let o = Vec2::ZERO;
    match t {
        SnapTarget::Node => {
            ring(m, o, 2.6, 0.45, y, col, 24);
            disc(m, o, 0.9, y, col);
            disc(m, o, 2.15, y - 0.005, soft);
        }
        SnapTarget::Road => {
            let d = [
                Vec2::new(2.4, 0.0),
                Vec2::new(0.0, 2.4),
                Vec2::new(-2.4, 0.0),
                Vec2::new(0.0, -2.4),
            ];
            outline(m, &d, 0.4, y, col);
            disc(m, o, 0.7, y, col);
        }
        SnapTarget::Perpendicular => {
            // A right-angle symbol.
            band(m, Vec2::new(-2.0, -2.0), Vec2::new(-2.0, 2.0), 0.4, y, col);
            band(m, Vec2::new(-2.2, 2.0), Vec2::new(2.0, 2.0), 0.4, y, col);
            outline(
                m,
                &[
                    Vec2::new(-2.0, 0.6),
                    Vec2::new(-0.6, 0.6),
                    Vec2::new(-0.6, 2.0),
                    Vec2::new(-2.0, 2.0),
                ],
                0.25,
                y,
                col,
            );
            disc(m, o, 0.5, y, col);
        }
        SnapTarget::Parallel => {
            band(m, Vec2::new(-2.2, -1.6), Vec2::new(1.0, 2.0), 0.4, y, col);
            band(m, Vec2::new(-1.0, -2.0), Vec2::new(2.2, 1.6), 0.4, y, col);
            disc(m, o, 0.5, y, col);
        }
        SnapTarget::Extension => {
            band(m, Vec2::new(-2.4, 0.0), Vec2::new(1.2, 0.0), 0.35, y, col);
            m.tri_up(
                v3(Vec2::new(0.8, -1.1), y),
                v3(Vec2::new(0.8, 1.1), y),
                v3(Vec2::new(2.6, 0.0), y),
                col,
            );
        }
        SnapTarget::Spacing => {
            band(m, Vec2::new(-2.4, -1.4), Vec2::new(-2.4, 1.4), 0.35, y, col);
            band(m, Vec2::new(2.4, -1.4), Vec2::new(2.4, 1.4), 0.35, y, col);
            band(m, Vec2::new(-2.2, 0.0), Vec2::new(2.2, 0.0), 0.3, y, col);
        }
        SnapTarget::Angle => {
            ring(m, o, 2.0, 0.35, y, col, 4);
            disc(m, o, 0.6, y, col);
        }
        SnapTarget::Length => {
            // A ruler: a bar with ticks.
            band(m, Vec2::new(-2.4, 0.0), Vec2::new(2.4, 0.0), 0.35, y, col);
            for x in [-2.4f32, -0.8, 0.8, 2.4] {
                let h = if x.abs() > 2.0 { 1.3 } else { 0.8 };
                band(m, Vec2::new(x, -h), Vec2::new(x, h), 0.3, y, col);
            }
        }
        SnapTarget::Grid | SnapTarget::None => {
            let s = 1.3;
            outline(
                m,
                &[Vec2::new(-s, -s), Vec2::new(s, -s), Vec2::new(s, s), Vec2::new(-s, s)],
                0.3,
                y,
                col,
            );
            disc(m, o, 0.45, y, col);
        }
    }
}

/// Dashed line between two points.
fn dashed(m: &mut MeshBuilder, a: Vec2, b: Vec2, w: f32, y: f32, col: [f32; 4]) {
    let len = a.distance(b);
    if len < 0.1 {
        return;
    }
    let d = (b - a) / len;
    let (dash, gap) = ((w * 5.0).max(2.2), (w * 3.5).max(1.6));
    let mut t = 0.0;
    while t < len {
        let e = (t + dash).min(len);
        band(m, a + d * t, a + d * e, w, y, col);
        t += dash + gap;
    }
}

#[allow(clippy::type_complexity)]
fn draw_snap(
    mut commands: Commands,
    snap: Res<SnapFeedback>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut markers: Query<(Entity, &mut Visibility, &mut Transform), (With<SnapMarker>, Without<GuideLines>)>,
    mut lines: Query<(Entity, &mut Visibility), (With<GuideLines>, Without<SnapMarker>)>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
) {
    if !snap.is_changed() {
        return;
    }
    let eye = camera.single().map(|t| t.translation()).ok();
    let y = heights::GHOST + 0.05;
    if let Ok((e, mut vis, mut t)) = markers.single_mut() {
        match snap.point {
            Some(p) => {
                let mut m = MeshBuilder::new();
                snap_glyph(&mut m, snap.target, snap.suspended);
                // Weaker rules that agreed: small dots around the glyph.
                for (k, extra) in snap.matched.iter().filter(|t| **t != snap.target).enumerate() {
                    let a = std::f32::consts::FRAC_PI_4 + k as f32 * 0.6;
                    disc(&mut m, Vec2::from_angle(a) * 3.4, 0.4, 0.0, lin(snap_color(*extra)));
                }
                commands.entity(e).insert(Mesh3d(meshes.add(m.build())));
                t.translation = Vec3::new(p[0], y + 0.02, p[1]);
                vis.set_if_neq(Visibility::Inherited);
            }
            None => {
                vis.set_if_neq(Visibility::Hidden);
            }
        }
    }
    if let Ok((e, mut vis)) = lines.single_mut() {
        if snap.guides.is_empty() || snap.suspended {
            vis.set_if_neq(Visibility::Hidden);
        } else {
            let mut m = MeshBuilder::new();
            for g in &snap.guides {
                let (a, b) = (Vec2::from(g.from), Vec2::from(g.to));
                // A few pixels wide at any zoom.
                let mid = v3((a + b) * 0.5, y);
                let w = eye.map_or(0.6, |e| (e.distance(mid) * 0.0045).clamp(0.4, 3.0));
                let col = lin(guide_color(g.kind));
                let halo = lin(Color::srgba(0.1, 0.08, 0.02, 0.35));
                band(&mut m, a, b, w * 2.2, y - 0.01, halo);
                dashed(&mut m, a, b, w, y, col);
            }
            commands.entity(e).insert(Mesh3d(meshes.add(m.build())));
            vis.set_if_neq(Visibility::Inherited);
        }
    }
}

/// Gentle pulse of the snap marker so it reads as live; it grows with the
/// distance to the camera so it stays readable when zoomed out.
fn pulse_marker(
    time: Res<Time<Real>>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<&mut Transform, With<SnapMarker>>,
) {
    let eye = camera.single().map(|t| t.translation()).ok();
    for mut t in &mut q {
        let k = eye.map_or(1.0, |e| (e.distance(t.translation) / 140.0).clamp(1.0, 6.0));
        let s = k * (1.0 + 0.08 * (time.elapsed_secs() * 5.0).sin());
        t.scale = Vec3::new(s, 1.0, s);
    }
}

/// Where the figure of a person walking on the map is drawn this frame.
fn walker_drawn(
    world: &citysim_sim::World,
    people: &Query<&super::people::PersonVisual>,
    c: citysim_sim::CitizenId,
) -> Option<Vec3> {
    let (leader, w) = citysim_sim::walk::walker_of(world, c)?;
    let slot = if c == leader {
        0
    } else {
        w.party.iter().position(|&p| p == c)? as u8 + 1
    };
    people
        .iter()
        .find(|v| v.leader == leader && v.slot == slot)
        .map(|v| v.drawn)
}

/// The ring under a hovered or selected vehicle, centred on its body as
/// drawn this frame, or under a person on foot (smaller, at the drawn
/// figure).
fn vehicle_ring(
    hl: Res<WorldHighlight>,
    sim: Res<SimWorld>,
    drawn: Res<DrawnPoses>,
    people: Query<&super::people::PersonVisual>,
    mut q: Query<(&mut Transform, &mut Visibility), With<VehicleRing>>,
) {
    let Ok((mut t, mut vis)) = q.single_mut() else { return };
    let walker = [&hl.selected, &hl.hovered].into_iter().find_map(|x| match x {
        Some(WorldTarget::Walker(c)) => walker_drawn(&sim.world, &people, *c),
        _ => None,
    });
    if let Some(p) = walker {
        t.translation = Vec3::new(p.x, p.y + 0.06, p.z);
        t.rotation = Quat::IDENTITY;
        t.scale = Vec3::new(0.32, 1.0, 0.32);
        vis.set_if_neq(Visibility::Inherited);
        return;
    }
    t.scale = Vec3::ONE;
    let pose = [&hl.selected, &hl.hovered]
        .into_iter()
        .find_map(|x| match x {
            Some(WorldTarget::Vehicle(v)) => Some(*v),
            _ => None,
        })
        .and_then(|v| drawn.vehicle(v));
    match pose {
        Some(d) => {
            t.translation = Vec3::new(d.pos[0], heights::HIGHLIGHT + 0.05, d.pos[1]);
            t.rotation = Quat::from_rotation_y(-d.heading);
            vis.set_if_neq(Visibility::Inherited);
        }
        None => {
            vis.set_if_neq(Visibility::Hidden);
        }
    }
}

/// Hang a highlight copy under its building, unless the building's entity
/// was despawned meanwhile (rebuilt this frame): then drop the copy.
fn attach_copy(commands: &mut Commands, copy: Entity, parent: Entity) {
    commands.queue(move |world: &mut World| {
        if world.get_entity(parent).is_ok() {
            if let Ok(mut e) = world.get_entity_mut(copy) {
                e.insert(ChildOf(parent));
            }
        } else if let Ok(e) = world.get_entity_mut(copy) {
            e.despawn();
        }
    });
}

/// The diamond above the selected (not merely hovered) vehicle, over its
/// body as drawn this frame: bobbing a little, spinning slowly, its height
/// and size growing with the camera distance.
fn vehicle_marker(
    time: Res<Time<Real>>,
    hl: Res<WorldHighlight>,
    sim: Res<SimWorld>,
    drawn: Res<DrawnPoses>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    people: Query<&super::people::PersonVisual>,
    mut q: Query<(&mut Transform, &mut Visibility), With<VehicleMarker>>,
) {
    let Ok((mut t, mut vis)) = q.single_mut() else { return };
    let pose = match &hl.selected {
        Some(WorldTarget::Vehicle(v)) => drawn.vehicle(*v).map(|d| d.pos),
        // Above a person on foot: lower, over their head.
        Some(WorldTarget::Walker(c)) => walker_drawn(&sim.world, &people, *c).map(|p| [p.x, p.z]),
        _ => None,
    };
    let Some(pos) = pose else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    let person = matches!(hl.selected, Some(WorldTarget::Walker(_)));
    let base = Vec3::new(pos[0], if person { -3.0 } else { 0.0 }, pos[1]);
    let k = camera
        .single()
        .map(|c| (c.translation().distance(base) / 120.0).clamp(1.0, 8.0))
        .unwrap_or(1.0);
    let e = time.elapsed_secs();
    t.translation = base + Vec3::Y * (5.5 + 2.2 * k + 0.35 * k * (e * 2.6).sin());
    t.rotation = Quat::from_rotation_y(e * 0.9);
    t.scale = Vec3::splat(k);
    vis.set_if_neq(Visibility::Inherited);
}

#[allow(clippy::type_complexity)]
#[derive(Default)]
struct HighlightState {
    key: Option<(
        WorldHighlight,
        ActiveTool,
        ToolModes,
        (u64, u64),
        (u64, u64),
        Option<JunctionControl>,
    )>,
    copies: Vec<Entity>,
}

/// Ground outline + glow copies for what the ui marks.
#[allow(clippy::too_many_arguments)]
fn draw_highlight(
    mut commands: Commands,
    hl: Res<WorldHighlight>,
    tool: Res<ActiveTool>,
    modes: Res<ToolModes>,
    sim: Res<SimWorld>,
    index: Res<BuildingEntities>,
    mut models: ResMut<BuildingModels>,
    mats: Res<GuideMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ground: Query<(Entity, &mut Visibility), With<HighlightGround>>,
    mut state: Local<HighlightState>,
) {
    let world = &sim.world;
    // A junction's control can change under the cursor (junction tool).
    let control = match &hl.hovered {
        Some(WorldTarget::Junction { node, .. }) => junction::junction_control(world, *node),
        _ => None,
    };
    let key = (
        hl.clone(),
        *tool,
        *modes,
        sim.key(world.versions.buildings),
        sim.key(world.versions.roads),
        control,
    );
    if state.key.as_ref() == Some(&key) {
        return;
    }
    state.key = Some(key);
    for e in state.copies.drain(..) {
        // The building (and with it the copy) may already be gone.
        commands.entity(e).try_despawn();
    }
    let Ok((ground_e, mut vis)) = ground.single_mut() else {
        return;
    };
    let mut m = MeshBuilder::new();
    let y = heights::HIGHLIGHT + 0.02;
    let junction_tool = *tool == ActiveTool::Road && modes.road == RoadMode::Junction;
    let targets = [(&hl.selected, true), (&hl.hovered, false)];
    for (target, selected) in targets {
        let Some(t) = target else { continue };
        let base = if selected {
            Color::srgb(1.0, 0.86, 0.35)
        } else {
            Color::srgb(0.55, 0.85, 1.0)
        };
        let col = lin(base.with_alpha(0.95));
        let soft = lin(base.with_alpha(0.22));
        match t {
            WorldTarget::Building(id) => {
                let Some(b) = world.buildings.get(*id) else { continue };
                let pts = b.corners().map(Vec2::from);
                outline(&mut m, &pts, if selected { 0.55 } else { 0.4 }, y, col);
                if let Some(&(parent, key, model_tf)) = index.0.get(id) {
                    let (detail, _) = models.get(key, &mut meshes);
                    let mut tf = model_tf;
                    tf.scale *= Vec3::new(1.015, 1.01, 1.015);
                    let copy = commands
                        .spawn((
                            HighlightCopy,
                            Mesh3d(detail),
                            MeshMaterial3d(mats.glow.clone()),
                            tf,
                            NotShadowCaster,
                        ))
                        .id();
                    attach_copy(&mut commands, copy, parent);
                    state.copies.push(copy);
                }
            }
            WorldTarget::Road(stretch) => {
                let kind = stretch
                    .segments
                    .iter()
                    .find_map(|s| world.roads.segments.get(*s))
                    .map_or(RoadKind::Street, |s| s.kind);
                let w = road_geom::style(kind).outer() * 2.0 + 1.0;
                for p in stretch.points.windows(2) {
                    band(&mut m, Vec2::from(p[0]), Vec2::from(p[1]), w, y, soft);
                    let (a, b) = (Vec2::from(p[0]), Vec2::from(p[1]));
                    let d = (b - a).normalize_or_zero();
                    let n = Vec2::new(-d.y, d.x) * (w * 0.5);
                    band(&mut m, a + n, b + n, 0.35, y + 0.01, col);
                    band(&mut m, a - n, b - n, 0.35, y + 0.01, col);
                }
                for p in &stretch.points {
                    disc(&mut m, Vec2::from(*p), w * 0.5, y, soft);
                }
            }
            WorldTarget::Vehicle(_) | WorldTarget::Walker(_) => {}
            WorldTarget::Junction { node, pos } => {
                let r = junction::junction(world, *node)
                    .map_or(8.0, |j| j.arms.iter().map(|a| a.setback).fold(6.0, f32::max) + 3.0);
                let col = if junction_tool && !selected {
                    match junction::junction_control(world, *node) {
                        Some(JunctionControl::Signal) => lin(palette::TRAFFIC_FREE.with_alpha(0.95)),
                        _ => lin(palette::TRAFFIC_BUSY.with_alpha(0.95)),
                    }
                } else {
                    col
                };
                let p = Vec2::from(*pos);
                ring(&mut m, p, r, 0.7, y, col, 40);
                disc(&mut m, p, r - 0.7, y - 0.01, [col[0], col[1], col[2], 0.15]);
            }
        }
    }
    let red = lin(mix(palette::TOOL_BAD, Color::WHITE, 0.1).with_alpha(0.45));
    for (curve, kind) in &hl.doomed_roads {
        let w = road_geom::style(*kind).outer() * 2.0 + 0.6;
        let pts = curve.flatten(0.25);
        for p in pts.windows(2) {
            band(&mut m, Vec2::from(p[0]), Vec2::from(p[1]), w, y, red);
        }
        for p in &pts[1..pts.len().saturating_sub(1)] {
            disc(&mut m, Vec2::from(*p), w * 0.5, y, red);
        }
    }
    for id in &hl.doomed_buildings {
        // The lot outlined in red on the ground: a small house deep in its
        // lot, or one under the road being drawn, still shows.
        if let Some(b) = world.buildings.get(*id) {
            outline(
                &mut m,
                &b.corners().map(Vec2::from),
                0.45,
                y,
                lin(palette::TOOL_BAD.with_alpha(0.9)),
            );
        }
        if let Some(&(parent, key, model_tf)) = index.0.get(id) {
            let (detail, _) = models.get(key, &mut meshes);
            let mut tf = model_tf;
            tf.scale *= Vec3::new(1.015, 1.01, 1.015);
            let copy = commands
                .spawn((
                    HighlightCopy,
                    Mesh3d(detail),
                    MeshMaterial3d(mats.doom.clone()),
                    tf,
                    NotShadowCaster,
                ))
                .id();
            attach_copy(&mut commands, copy, parent);
            state.copies.push(copy);
        }
    }
    if m.is_empty() {
        vis.set_if_neq(Visibility::Hidden);
    } else {
        commands.entity(ground_e).insert(Mesh3d(meshes.add(m.build())));
        vis.set_if_neq(Visibility::Inherited);
    }
}
