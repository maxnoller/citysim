//! The trip of the selected car or person (`shared::RouteView`, written by
//! ui): a ribbon along the lanes still ahead, the destination building
//! glowing with a ring on the ground and a pin floating above it, and a
//! quieter ring where the trip started.
//!
//! The ribbon is the interface accent with a dark edge (it reads on asphalt,
//! on grass, at night and over the traffic map's colours), white chevrons
//! every few car lengths point the way, and its first metres fade in from
//! the car. A trip not under way yet (or off the map) is dashed and paler.
//! Everything is unlit, so it keeps its colours at night.
//!
//! A person under way off the map (`RouteView::traveller`: on foot, a lift,
//! a car beyond those drawn) gets the line of a car from where they should
//! be by now, and the selection's marker there: white and accent rings on
//! the ground and a floating diamond, gliding along as ui moves the
//! estimate.
//!
//! The mesh is rebuilt only when `RouteView` changes (ui republishes a
//! moving car's line every few metres); the pin and the marker only move.
//! Between rebuilds the line still starts at the car as drawn this frame
//! (or at the traveller's marker): a little route material
//! (`assets/gfx/shaders/route.wesl`) trims everything behind a cut held in
//! the entity's `MeshTag`, set every frame. The mesh measures distances
//! along the trip in one frame of reference for the whole selection (a
//! rebuilt line carries on the old one's), so the cut, the fade from it and
//! the chevrons stay put on the ground whatever the rebuilds; the line also
//! runs a few metres back behind its first point, so a car drawn a little
//! behind the published start is still reached.
//!
//! The off-map traveller moves along the line at the pace ui gives
//! (`RouteView::traveller_pace` from `traveller_tick`), at the moment the
//! cars are drawn (`vehicles::DrawClock`): it glides evenly however rarely
//! ui republishes, and the camera follows it there (`DrawnPoses`).

use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use bevy::mesh::MeshTag;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use citysim_sim::BuildingId;

use super::buildings::{BuildingEntities, BuildingModels};
use super::heights;
use super::mesh_util::MeshBuilder;
use super::palette::lin;
use super::vehicles::{DrawClock, DrawnSet};
use crate::shared::{DrawnPoses, GameSet, RouteState, RouteView, SimWorld, WorldCamera};

/// Unlit, vertex-coloured, trimmed at the cut in the entity's `MeshTag`
/// (`route.wesl`).
pub type RouteMaterial = ExtendedMaterial<StandardMaterial, RouteExt>;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct RouteExt {}

impl MaterialExtension for RouteExt {
    fn fragment_shader() -> ShaderRef {
        "gfx/shaders/route.wesl".into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

pub struct RoutePlugin;

impl Plugin for RoutePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<RouteMaterial>::default())
            .init_resource::<RouteShape>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (draw_route, move_pin)
                    .chain()
                    .in_set(GameSet::Render)
                    .before(DrawnSet::Traveller),
            )
            .add_systems(Update, move_traveller.in_set(DrawnSet::Traveller))
            .add_systems(Update, end_warm_up)
            .add_systems(Update, trim_route.in_set(GameSet::Render).after(DrawnSet::Traveller));
    }
}

/// The interface accent (ui theme `ACCENT_SET.base`), as gfx's guides use it.
const ACCENT: Color = Color::srgb(0.561, 0.482, 0.847);
/// The ribbon's dark edge: the accent sunk towards black.
const EDGE: Color = Color::srgb(0.09, 0.06, 0.16);

/// Ribbon widths (metres): the dark edge and the accent core.
const EDGE_W: f32 = 2.5;
const CORE_W: f32 = 1.3;
/// Chevrons every this many metres (they start a little ahead of the car).
const CHEVRON_EVERY: f32 = 16.0;
/// The ribbon fades in over its first metres from the car.
const FADE_M: f32 = 9.0;
/// Dashes of a planned / unseen trip: on, then off.
const DASH_ON: f32 = 5.0;
const DASH_OFF: f32 = 3.5;
/// The solid core starts this far ahead of the cut (under the car).
const CORE_LEAD: f32 = 2.5;
/// The line runs this far back behind its first point (trimmed at the cut).
const BACK_M: f32 = 12.0;
/// How far along the line (metres from its start) the drawn car is looked
/// for: ui republishes every few metres, or every 0.15 s at fast speeds.
const CUT_SEARCH_M: f32 = 150.0;

/// Whose line it is: state, car, under way off the map, destination.
type ShapeKey = (RouteState, Option<citysim_sim::VehicleId>, bool, Option<[f32; 2]>);

/// The line as last meshed, for the cut: its points (with the stretch
/// behind the start) and their distances along the trip in the selection's
/// frame of reference.
#[derive(Resource, Default)]
struct RouteShape {
    /// Trimmed (a car's or a traveller's line); planned trips are whole.
    trimmed: bool,
    pts: Vec<Vec2>,
    /// Distance along the trip of each point.
    along: Vec<f32>,
    /// Distance along the trip of `RouteView::points[0]`.
    start: f32,
    /// The traveller's estimate: (tick, pace) as published.
    traveller: Option<(u64, f32)>,
    vehicle: Option<citysim_sim::VehicleId>,
    walker: Option<citysim_sim::CitizenId>,
    /// What the frame of reference belongs to (a new selection starts a
    /// new one).
    key: Option<ShapeKey>,
    /// Where the traveller's marker is drawn this frame (distance along the
    /// trip), for the cut.
    traveller_at: Option<f32>,
}

impl RouteShape {
    /// The nearest point of the line to `p` within its first stretch: its
    /// distance along the trip and how far `p` is from it.
    fn project(&self, p: Vec2, within: f32) -> Option<(f32, f32)> {
        let mut best: Option<(f32, f32)> = None;
        let end = self.along.first()? + within;
        for (i, w) in self.pts.windows(2).enumerate() {
            if self.along[i] > end {
                break;
            }
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = if ab.length_squared() > 1e-8 {
                ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let d = p.distance(a + ab * t);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((self.along[i] + (self.along[i + 1] - self.along[i]) * t, d));
            }
        }
        best
    }

    /// The point `d` along the trip (before the start: back along the first
    /// stretch; past the end: the end).
    fn point_at(&self, d: f32) -> Option<Vec2> {
        let n = self.pts.len();
        if n < 2 {
            return self.pts.first().copied();
        }
        let i = self.along.partition_point(|&c| c <= d).clamp(1, n - 1);
        let (a, b) = (self.pts[i - 1], self.pts[i]);
        let span = (self.along[i] - self.along[i - 1]).max(1e-4);
        let f = ((d - self.along[i - 1]) / span).min(1.0);
        Some(if i == 1 { a.lerp(b, f) } else { a.lerp(b, f.max(0.0)) })
    }
}

/// The route's two meshes: the soft dark edge (blended) and the solid
/// core, chevrons and rings (opaque, so they always draw over the traffic
/// map's translucent strips and other blended overlays).
#[derive(Component)]
struct RouteLine {
    solid: bool,
}

/// Draws the route materials during loading (`setup`).
#[derive(Component)]
struct RouteWarmUp;

/// Seconds after the scene is ready that the warm-up triangles stay.
const WARM_UP_S: f32 = 20.0;

/// Remove the warm-up triangles a while after the scene is ready.
fn end_warm_up(
    mut commands: Commands,
    ready: Res<crate::shared::SceneReady>,
    time: Res<Time<Real>>,
    q: Query<Entity, With<RouteWarmUp>>,
    mut since: Local<Option<f32>>,
) {
    if q.is_empty() || !ready.0 {
        return;
    }
    let t = *since.get_or_insert(time.elapsed_secs());
    if time.elapsed_secs() - t > WARM_UP_S {
        for e in &q {
            commands.entity(e).despawn();
        }
    }
}

/// The pin above the destination (moved every frame).
#[derive(Component)]
struct RoutePin;

/// The marker of a person under way off the map: the rings on the ground
/// (`false`) and the diamond above (`true`).
#[derive(Component)]
struct Traveller(bool);

#[derive(Resource)]
struct RouteMaterials {
    glow: Handle<StandardMaterial>,
}

fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut route_mats: ResMut<Assets<RouteMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let route_mat = |base: StandardMaterial| ExtendedMaterial {
        base,
        extension: RouteExt {},
    };
    let overlay = route_mats.add(route_mat(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        depth_bias: 60.0,
        // Ribbons and the pin are built without minding their winding.
        cull_mode: None,
        double_sided: true,
        ..default()
    }));
    let glow = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.20, 0.55),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        ..default()
    });
    let solid = route_mats.add(route_mat(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Opaque,
        unlit: true,
        depth_bias: 60.0,
        cull_mode: None,
        double_sided: true,
        ..default()
    }));
    let (overlay_warm, solid_warm) = (overlay.clone(), solid.clone());
    commands.spawn((
        Name::new("Route edge"),
        RouteLine { solid: false },
        MeshMaterial3d(overlay),
        MeshTag::new(0),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Route"),
        RouteLine { solid: true },
        MeshMaterial3d(solid.clone()),
        MeshTag::new(0),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Route pin"),
        RoutePin,
        Mesh3d(meshes.add(pin_mesh().build())),
        MeshMaterial3d(solid),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    let (rings, diamond) = traveller_meshes();
    commands.spawn((
        Name::new("Route traveller rings"),
        Traveller(false),
        Mesh3d(meshes.add(rings.build())),
        MeshMaterial3d(overlay_for_marker(&mut materials)),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Route traveller"),
        Traveller(true),
        Mesh3d(meshes.add(diamond.build())),
        MeshMaterial3d(overlay_for_marker(&mut materials)),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    // Compile the route material's pipelines while the game loads (as the
    // other materials' are), not on the first selection: a triangle of zero
    // area per material, never culled, drawn until a while after the scene
    // is ready (the warm-up cameras of the other qualities run then too).
    let mut zero = MeshBuilder::new();
    zero.tri_c(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, [[1.0; 4]; 3]);
    let zero = meshes.add(zero.build());
    for (name, mat) in [("Route warm-up (edge)", overlay_warm), ("Route warm-up", solid_warm)] {
        commands.spawn((
            Name::new(name),
            RouteWarmUp,
            Mesh3d(zero.clone()),
            MeshMaterial3d(mat),
            MeshTag::new(0),
            Transform::default(),
            Visibility::Visible,
            NoFrustumCulling,
            NotShadowCaster,
        ));
    }
    commands.insert_resource(RouteMaterials { glow });
}

fn overlay_for_marker(materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        depth_bias: 80.0,
        cull_mode: None,
        double_sided: true,
        ..default()
    })
}

/// The selection's look (as a selected car's): a white inner and an accent
/// outer ring over a faint accent disc, and a diamond.
fn traveller_meshes() -> (MeshBuilder, MeshBuilder) {
    let mut rings = MeshBuilder::new();
    let white = lin(Color::srgb(1.0, 1.0, 1.0).with_alpha(0.95));
    let accent = lin(ACCENT.with_alpha(0.95));
    ring(&mut rings, Vec2::ZERO, 2.6, 0.4, 0.0, white);
    ring(&mut rings, Vec2::ZERO, 3.4, 0.45, 0.0, accent);
    disc(
        &mut rings,
        Vec2::ZERO,
        2.5,
        -0.01,
        [accent[0], accent[1], accent[2], 0.25],
    );
    disc(&mut rings, Vec2::ZERO, 0.8, 0.01, white);
    let mut diamond = MeshBuilder::new();
    let (top, bot) = (Vec3::new(0.0, 1.4, 0.0), Vec3::new(0.0, -1.6, 0.0));
    let rim: Vec<Vec3> = (0..4)
        .map(|i| {
            let a = i as f32 * std::f32::consts::FRAC_PI_2;
            Vec3::new(a.cos(), 0.0, a.sin())
        })
        .collect();
    let light = lin(Color::srgb(0.78, 0.72, 1.0));
    for i in 0..4 {
        let (p, q) = (rim[i], rim[(i + 1) % 4]);
        let shade = if i % 2 == 0 { accent } else { light };
        diamond.tri_c(top, q, p, [shade; 3]);
        diamond.tri_c(bot, p, q, [accent; 3]);
    }
    (rings, diamond)
}

/// The off-map traveller's marker: from ui's estimate on along the line at
/// its pace, at the moment the cars are drawn (smooth however rarely ui
/// republishes, and the same moment as the cars). Published in
/// `DrawnPoses::traveller` for the camera and the leader line.
#[allow(clippy::too_many_arguments)]
fn move_traveller(
    time: Res<Time<Real>>,
    route: Res<RouteView>,
    clock: Res<DrawClock>,
    mut shape: ResMut<RouteShape>,
    mut drawn: ResMut<DrawnPoses>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&Traveller, &mut Transform, &mut Visibility)>,
) {
    let at = shape
        .traveller
        .filter(|_| route.state != RouteState::None && route.traveller.is_some())
        .and_then(|(tick, pace)| {
            let end = shape.along.last().copied().unwrap_or(shape.start);
            let d = (shape.start + ((clock.tick() - tick as f64) * pace as f64) as f32).min(end);
            Some((d, shape.point_at(d)?))
        });
    shape.traveller_at = at.map(|a| a.0);
    let Some((_, p)) = at else {
        if drawn.traveller.is_some() {
            drawn.traveller = None;
        }
        for (_, _, mut vis) in &mut q {
            vis.set_if_neq(Visibility::Hidden);
        }
        return;
    };
    if drawn.traveller != Some(p.to_array()) {
        drawn.traveller = Some(p.to_array());
    }
    let base = Vec3::new(p.x, 0.0, p.y);
    let k = camera
        .single()
        .map(|c| (c.translation().distance(base) / 120.0).clamp(1.0, 8.0))
        .unwrap_or(1.0);
    let e = time.elapsed_secs();
    for (which, mut t, mut vis) in &mut q {
        if which.0 {
            t.translation = base + Vec3::Y * (5.5 + 2.2 * k + 0.35 * k * (e * 2.6).sin());
            t.rotation = Quat::from_rotation_y(e * 0.9);
            t.scale = Vec3::splat(k);
        } else {
            t.translation = base + Vec3::Y * (heights::HIGHLIGHT + 0.12);
            t.rotation = Quat::IDENTITY;
            t.scale = Vec3::new(k.min(3.0), 1.0, k.min(3.0));
        }
        vis.set_if_neq(Visibility::Inherited);
    }
}

/// A map pin: a bead on a slim spike pointing down at the destination, with
/// a white collar (reads against sky, roofs and night).
fn pin_mesh() -> MeshBuilder {
    let mut m = MeshBuilder::new();
    let accent = lin(ACCENT.with_alpha(0.97));
    let light = lin(Color::srgb(0.80, 0.74, 1.0).with_alpha(0.97));
    let white = lin(Color::srgba(1.0, 1.0, 1.0, 0.97));
    let n = 6;
    let ring = |r: f32, y: f32| -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                Vec3::new(a.cos() * r, y, a.sin() * r)
            })
            .collect()
    };
    // Spike: from the tip at 0 up to the bead.
    let tip = Vec3::ZERO;
    let collar = ring(0.55, 2.6);
    let crown = ring(1.25, 3.6);
    let top = Vec3::new(0.0, 5.0, 0.0);
    for i in 0..n {
        let (a, b) = (collar[i], collar[(i + 1) % n]);
        let shade = if i % 2 == 0 { accent } else { light };
        m.tri_c(tip, b, a, [shade; 3]);
        let (c, d) = (crown[i], crown[(i + 1) % n]);
        m.quad_c([a, b, d, c], [white; 4]);
        m.tri_c(c, d, top, [if i % 2 == 0 { light } else { accent }; 3]);
    }
    m
}

fn v3(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

fn with_alpha(c: [f32; 4], a: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * a]
}

/// Point and direction `d` metres along a polyline (with its cumulative
/// lengths).
fn at(pts: &[Vec2], cum: &[f32], d: f32) -> (Vec2, Vec2) {
    let i = cum.partition_point(|&c| c <= d).clamp(1, pts.len() - 1);
    let (a, b) = (pts[i - 1], pts[i]);
    let span = (cum[i] - cum[i - 1]).max(1e-4);
    let f = ((d - cum[i - 1]) / span).clamp(0.0, 1.0);
    (a.lerp(b, f), (b - a).normalize_or(Vec2::X))
}

/// A ribbon of width `w` over `pts[from..=to]` metres (mitred joins),
/// `alpha(d)` per vertex and `uv(d)` for the route shader.
#[allow(clippy::too_many_arguments)]
fn ribbon(
    m: &mut MeshBuilder,
    pts: &[Vec2],
    cum: &[f32],
    span: (f32, f32),
    w: f32,
    y: f32,
    col: [f32; 4],
    alpha: &dyn Fn(f32) -> f32,
    uv: &dyn Fn(f32) -> [f32; 2],
) {
    let (from, to) = span;
    if to - from < 0.05 {
        return;
    }
    // The polyline's own points inside the span, plus its two ends.
    let mut ds = vec![from];
    ds.extend(cum.iter().copied().filter(|&c| c > from + 0.01 && c < to - 0.01));
    ds.push(to);
    let side = |d: f32| {
        let (p, dir) = at(pts, cum, d);
        // Mitre: the mean of the directions before and after a corner.
        let (_, before) = at(pts, cum, (d - 0.02).max(0.0));
        let (_, after) = at(pts, cum, d + 0.02);
        let n = Vec2::new(-(before + after).y, (before + after).x).normalize_or(Vec2::new(-dir.y, dir.x));
        let k = n.dot(Vec2::new(-dir.y, dir.x)).max(0.5);
        (p, n * (w * 0.5 / k))
    };
    let mut prev = side(ds[0]);
    for win in ds.windows(2) {
        let next = side(win[1]);
        let (a0, a1) = (alpha(win[0]), alpha(win[1]));
        let first = m.pos.len();
        m.quad_c(
            [
                v3(prev.0 + prev.1, y),
                v3(prev.0 - prev.1, y),
                v3(next.0 - next.1, y),
                v3(next.0 + next.1, y),
            ],
            [
                with_alpha(col, a0),
                with_alpha(col, a0),
                with_alpha(col, a1),
                with_alpha(col, a1),
            ],
        );
        let (u0, u1) = (uv(win[0]), uv(win[1]));
        m.uv[first..first + 4].copy_from_slice(&[u0, u0, u1, u1]);
        prev = next;
    }
}

/// A chevron pointing along `dir` at `p`.
fn chevron(m: &mut MeshBuilder, p: Vec2, dir: Vec2, y: f32, col: [f32; 4], k: f32) {
    let n = Vec2::new(-dir.y, dir.x);
    let (len, half, w) = (0.75 * k, 0.62 * k, 0.26 * k);
    for s in [1.0, -1.0] {
        let tip = p + dir * len * 0.5;
        let tail = p - dir * len * 0.5 + n * half * s;
        let off = Vec2::new(-(tip - tail).y, (tip - tail).x).normalize_or_zero() * w * 0.5;
        m.quad_up(
            [v3(tail + off, y), v3(tip + off, y), v3(tip - off, y), v3(tail - off, y)],
            col,
        );
    }
}

/// A flat ring around a footprint's corners (inflated by `grow`).
fn footprint_ring(m: &mut MeshBuilder, corners: [[f32; 2]; 4], grow: f32, w: f32, y: f32, col: [f32; 4]) {
    let p = corners.map(Vec2::from);
    let c = (p[0] + p[1] + p[2] + p[3]) * 0.25;
    let p = p.map(|q| q + (q - c).normalize_or_zero() * grow * std::f32::consts::SQRT_2);
    for i in 0..4 {
        let (a, b) = (p[i], p[(i + 1) % 4]);
        let d = (b - a).normalize_or_zero();
        let n = Vec2::new(-d.y, d.x) * w * 0.5;
        let (a, b) = (a - d * w * 0.5, b + d * w * 0.5);
        m.quad_up([v3(a - n, y), v3(b - n, y), v3(b + n, y), v3(a + n, y)], col);
    }
}

fn disc(m: &mut MeshBuilder, c: Vec2, r: f32, y: f32, col: [f32; 4]) {
    let pts: Vec<Vec3> = (0..20)
        .map(|i| v3(c + Vec2::from_angle(i as f32 / 20.0 * std::f32::consts::TAU) * r, y))
        .collect();
    m.fan_up(v3(c, y), &pts, col);
}

fn ring(m: &mut MeshBuilder, c: Vec2, r: f32, w: f32, y: f32, col: [f32; 4]) {
    let n = 28;
    for i in 0..n {
        let (u0, u1) = (
            Vec2::from_angle(i as f32 / n as f32 * std::f32::consts::TAU),
            Vec2::from_angle((i + 1) as f32 / n as f32 * std::f32::consts::TAU),
        );
        m.quad_up(
            [c + u0 * (r - w), c + u1 * (r - w), c + u1 * r, c + u0 * r].map(|p| v3(p, y)),
            col,
        );
    }
}

/// The route's mesh: ribbon, chevrons, the two ends on the ground. Along
/// `shape` (distances along the trip); a car's or traveller's line carries
/// its trim in the vertices' uv (`route.wesl`).
fn route_mesh(world: &citysim_sim::World, route: &RouteView, shape: &RouteShape, k: f32) -> (MeshBuilder, MeshBuilder) {
    let mut soft = MeshBuilder::new();
    let mut m = MeshBuilder::new();
    let y = heights::HIGHLIGHT + 0.06;
    // On foot: a narrower line along the pavements and zebras.
    let wk = if route.state == RouteState::Walking { 0.42 } else { 1.0 };
    let planned = !shape.trimmed;
    let (pts, cum) = (&shape.pts, &shape.along);
    if pts.len() >= 2 {
        let (first, total) = (cum[0], cum[cum.len() - 1]);
        let edge = lin(EDGE.with_alpha(if planned { 0.45 } else { 0.62 }));
        // Planned: paler (opaque, so mixed towards the edge colour).
        let core = lin(if planned {
            super::palette::mix(ACCENT, Color::WHITE, 0.25)
        } else {
            ACCENT
        });
        let whole = |_: f32| 1.0;
        let plain = |_: f32| [0.0, 0.0];
        // A car's line: the dark edge fades in from the cut (under the car),
        // the core starts a little ahead of it.
        let edge_uv = |d: f32| [d, FADE_M * k * wk];
        let core_uv = |d: f32| [d - CORE_LEAD * wk, 0.001];
        if planned {
            let mut d = first;
            while d < total {
                let span = (d, (d + DASH_ON * k).min(total));
                ribbon(&mut soft, pts, cum, span, EDGE_W * k, y, edge, &whole, &plain);
                ribbon(&mut m, pts, cum, span, CORE_W * k, y + 0.01, core, &whole, &plain);
                d += (DASH_ON + DASH_OFF) * k;
            }
        } else {
            let span = (first, total);
            ribbon(&mut soft, pts, cum, span, EDGE_W * k * wk, y, edge, &whole, &edge_uv);
            ribbon(
                &mut m,
                pts,
                cum,
                span,
                CORE_W * k * wk,
                y + 0.01,
                core,
                &whole,
                &core_uv,
            );
            // Chevrons at whole steps along the trip (they stay put when
            // the line is rebuilt), shown from a little ahead of the car on
            // (each eaten from behind as the cut reaches it).
            let white = lin(Color::WHITE);
            let every = CHEVRON_EVERY * k * wk;
            let lead = (FADE_M + 4.0) * k * wk;
            let mut d = (first / every).ceil() * every;
            while d < total - 4.0 * k * wk {
                let (p, dir) = at(pts, cum, d);
                let from = m.pos.len();
                chevron(&mut m, p, dir, y + 0.02, white, k * wk);
                for i in from..m.pos.len() {
                    let q = Vec2::new(m.pos[i][0], m.pos[i][2]);
                    m.uv[i] = [d + (q - p).dot(dir) - lead, 0.001];
                }
                d += every;
            }
        }
    }
    let ground = |m: &mut MeshBuilder, b: Option<BuildingId>, pos: [f32; 2], strong: bool| {
        let col = if strong {
            lin(ACCENT)
        } else {
            lin(Color::srgb(0.85, 0.85, 0.9))
        };
        match b.and_then(|b| world.buildings.get(b)) {
            Some(bb) => {
                if strong {
                    footprint_ring(m, bb.corners(), 1.2, 0.9, y, lin(EDGE));
                    footprint_ring(m, bb.corners(), 1.2, 0.5, y + 0.01, col);
                } else {
                    footprint_ring(m, bb.corners(), 0.8, 0.35, y, col);
                }
            }
            None => {
                let p = Vec2::from(pos);
                ring(m, p, if strong { 4.0 } else { 3.0 }, 0.5, y, col);
                disc(m, p, 1.0, y, col);
            }
        }
    };
    if let Some(o) = route.origin {
        ground(&mut m, o.building, o.pos, false);
    }
    if let Some(d) = route.destination {
        ground(&mut m, d.building, d.pos, true);
    }
    (soft, m)
}

#[derive(Default)]
struct Drawn {
    /// The glowing copy of the destination building and which one it is.
    copy: Option<(Entity, BuildingId)>,
    buildings: Option<(u64, u64)>,
    /// Width factor the mesh was built for.
    k: f32,
}

/// Rebuild the route's mesh when the route changes.
#[allow(clippy::too_many_arguments)]
fn draw_route(
    mut commands: Commands,
    route: Res<RouteView>,
    sim: Res<SimWorld>,
    index: Res<BuildingEntities>,
    mut models: ResMut<BuildingModels>,
    mats: Res<RouteMaterials>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut lines: Query<(Entity, &RouteLine, &mut Visibility)>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut shape: ResMut<RouteShape>,
    mut drawn: Local<Drawn>,
) {
    let key = sim.key(sim.world.versions.buildings);
    // Wider from afar (in steps, so zooming rebuilds it only now and then).
    let k = camera
        .single()
        .map(|c| (c.translation().y / 110.0).clamp(1.0, 5.0))
        .unwrap_or(1.0);
    let k = (k * 4.0).round() / 4.0;
    if !route.is_changed() && drawn.buildings == Some(key) && drawn.k == k {
        return;
    }
    drawn.k = k;
    // The destination building glows (re-made when buildings were rebuilt).
    let dest = route
        .destination
        .and_then(|d| d.building)
        .filter(|_| route.state != RouteState::None);
    if drawn.copy.map(|c| c.1) != dest || drawn.buildings != Some(key) {
        if let Some((c, _)) = drawn.copy.take() {
            commands.entity(c).try_despawn();
        }
        if let Some(b) = dest
            && let Some(&(parent, model, model_tf)) = index.0.get(&b)
        {
            let (detail, _) = models.get(model, &mut meshes);
            let mut tf = model_tf;
            tf.scale *= Vec3::new(1.02, 1.012, 1.02);
            let copy = commands
                .spawn((Mesh3d(detail), MeshMaterial3d(mats.glow.clone()), tf, NotShadowCaster))
                .id();
            commands.queue(move |world: &mut World| {
                if world.get_entity(parent).is_ok() {
                    if let Ok(mut c) = world.get_entity_mut(copy) {
                        c.insert(ChildOf(parent));
                    }
                } else if let Ok(c) = world.get_entity_mut(copy) {
                    c.despawn();
                }
            });
            drawn.copy = Some((copy, b));
        }
    }
    drawn.buildings = Some(key);
    if route.is_changed() {
        update_shape(&mut shape, &route);
    }
    let built = (route.state != RouteState::None).then(|| route_mesh(&sim.world, &route, &shape, k));
    for (e, line, mut vis) in &mut lines {
        let m = built
            .as_ref()
            .map(|(soft, solid)| if line.solid { solid } else { soft });
        match m.filter(|m| !m.is_empty()) {
            Some(m) => {
                commands.entity(e).insert(Mesh3d(meshes.add(m.clone().build())));
                vis.set_if_neq(Visibility::Inherited);
            }
            None => {
                vis.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// The line of a new `RouteView` in the selection's frame of reference:
/// a rebuilt line of the same selection starts where its first point lies
/// on the old one.
fn update_shape(shape: &mut RouteShape, route: &RouteView) {
    let trimmed = matches!(route.state, RouteState::Driving | RouteState::Walking) || route.traveller.is_some();
    let key = (
        route.state,
        route.vehicle,
        route.traveller.is_some(),
        route.destination.map(|d| d.pos),
    );
    let pts: Vec<Vec2> = route.points.iter().map(|&p| Vec2::from(p)).collect();
    let start = match pts.first() {
        Some(&p0) if trimmed && shape.key == Some(key) => shape
            .project(p0, CUT_SEARCH_M + BACK_M)
            .filter(|(_, off)| *off < 3.0)
            .map_or(shape.start, |(d, _)| d),
        _ => 0.0,
    };
    let mut out = Vec::with_capacity(pts.len() + 1);
    let mut along = Vec::with_capacity(pts.len() + 1);
    if trimmed && pts.len() >= 2 {
        // Back behind the first point, straight on.
        let dir = (pts[1] - pts[0]).normalize_or(Vec2::X);
        out.push(pts[0] - dir * BACK_M);
        along.push(start - BACK_M);
    }
    let mut acc = start;
    for (i, &p) in pts.iter().enumerate() {
        if i > 0 {
            acc += p.distance(pts[i - 1]);
        }
        out.push(p);
        along.push(acc);
    }
    *shape = RouteShape {
        trimmed,
        pts: out,
        along,
        start,
        traveller: route.traveller.map(|_| (route.traveller_tick, route.traveller_pace)),
        vehicle: route.vehicle,
        walker: route.walker,
        key: Some(key),
        traveller_at: shape.traveller_at,
    };
}

/// Every frame: the cut of a car's or traveller's line at the front of the
/// car as drawn now (or the traveller's marker), in the entities' `MeshTag`.
fn trim_route(shape: Res<RouteShape>, drawn: Res<DrawnPoses>, mut lines: Query<&mut MeshTag, With<RouteLine>>) {
    if !shape.trimmed {
        return;
    }
    let cut = shape
        .traveller_at
        .filter(|_| shape.traveller.is_some())
        .or_else(|| {
            let car = drawn.vehicle(shape.vehicle?)?;
            let (d, _) = shape.project(Vec2::from(car.pos), CUT_SEARCH_M + BACK_M)?;
            Some(d + car.half_length)
        })
        .or_else(|| {
            // On foot: from the figure.
            let p = drawn.walker(shape.walker?)?;
            Some(shape.project(Vec2::from(p), CUT_SEARCH_M + BACK_M)?.0)
        })
        .unwrap_or(shape.start);
    let bits = cut.to_bits();
    for mut t in &mut lines {
        if t.value != bits {
            t.value = bits;
        }
    }
}

/// The pin floats above the destination, bobbing a little; it grows with
/// the camera distance so it stays visible across the city.
fn move_pin(
    time: Res<Time<Real>>,
    route: Res<RouteView>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&mut Transform, &mut Visibility), With<RoutePin>>,
) {
    let Ok((mut t, mut vis)) = q.single_mut() else { return };
    let Some(d) = route.destination.filter(|_| route.state != RouteState::None) else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    let base = Vec3::new(d.pos[0], 0.0, d.pos[1]);
    let k = camera
        .single()
        .map(|c| (c.translation().distance(base) / 140.0).clamp(1.0, 7.0))
        .unwrap_or(1.0);
    let e = time.elapsed_secs();
    t.translation = base + Vec3::Y * (16.0 + 3.0 * k + 0.4 * k * (e * 2.2).sin());
    t.rotation = Quat::from_rotation_y(e * 0.6);
    t.scale = Vec3::splat(k);
    vis.set_if_neq(Visibility::Inherited);
}
