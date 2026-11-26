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
//! The mesh is rebuilt only when `RouteView` changes (ui republishes a
//! moving car's line every few metres); the pin only moves.

use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use citysim_sim::BuildingId;

use super::buildings::{BuildingEntities, BuildingModels};
use super::heights;
use super::mesh_util::MeshBuilder;
use super::palette::lin;
use crate::shared::{GameSet, RouteState, RouteView, SimWorld, WorldCamera};

pub struct RoutePlugin;

impl Plugin for RoutePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, (draw_route, move_pin).chain().in_set(GameSet::Render));
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

#[derive(Component)]
struct RouteLine;

/// The pin above the destination (moved every frame).
#[derive(Component)]
struct RoutePin;

#[derive(Resource)]
struct RouteMaterials {
    glow: Handle<StandardMaterial>,
}

fn setup(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>, mut meshes: ResMut<Assets<Mesh>>) {
    let overlay = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        depth_bias: 60.0,
        // Ribbons and the pin are built without minding their winding.
        cull_mode: None,
        double_sided: true,
        ..default()
    });
    let glow = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.20, 0.55),
        alpha_mode: AlphaMode::Add,
        unlit: true,
        ..default()
    });
    commands.spawn((
        Name::new("Route"),
        RouteLine,
        MeshMaterial3d(overlay.clone()),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Route pin"),
        RoutePin,
        Mesh3d(meshes.add(pin_mesh().build())),
        MeshMaterial3d(overlay),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
    ));
    commands.insert_resource(RouteMaterials { glow });
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
/// `alpha(d)` per vertex.
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
        m.quad_c(
            [
                v3(prev.0 + prev.1, y),
                v3(prev.0 - prev.1, y),
                v3(next.0 - next.1, y),
                v3(next.0 + next.1, y),
            ],
            [with_alpha(col, a0), with_alpha(col, a0), with_alpha(col, a1), with_alpha(col, a1)],
        );
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
        m.quad_up([v3(tail + off, y), v3(tip + off, y), v3(tip - off, y), v3(tail - off, y)], col);
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

/// The route's mesh: ribbon, chevrons, the two ends on the ground.
fn route_mesh(world: &citysim_sim::World, route: &RouteView, k: f32) -> MeshBuilder {
    let mut m = MeshBuilder::new();
    let y = heights::HIGHLIGHT + 0.06;
    let planned = route.state != RouteState::Driving;
    let pts: Vec<Vec2> = route.points.iter().map(|&p| Vec2::from(p)).collect();
    if pts.len() >= 2 {
        let mut cum = Vec::with_capacity(pts.len());
        let mut acc = 0.0;
        cum.push(0.0);
        for w in pts.windows(2) {
            acc += w[0].distance(w[1]);
            cum.push(acc);
        }
        let total = acc;
        let edge = lin(EDGE.with_alpha(if planned { 0.45 } else { 0.62 }));
        let core = lin(if planned {
            ACCENT.with_alpha(0.75)
        } else {
            ACCENT.with_alpha(0.97)
        });
        // A car's line fades in from under the car; a planned one is whole.
        let fade = move |d: f32| if planned { 1.0 } else { (d / (FADE_M * k)).clamp(0.0, 1.0).powf(0.8) };
        let spans: Vec<(f32, f32)> = if planned {
            let mut v = Vec::new();
            let mut d = 0.0;
            while d < total {
                v.push((d, (d + DASH_ON * k).min(total)));
                d += (DASH_ON + DASH_OFF) * k;
            }
            v
        } else {
            vec![(0.0, total)]
        };
        for &s in &spans {
            ribbon(&mut m, &pts, &cum, s, EDGE_W * k, y, edge, &fade);
        }
        for &s in &spans {
            ribbon(&mut m, &pts, &cum, s, CORE_W * k, y + 0.01, core, &fade);
        }
        if !planned {
            let white = lin(Color::srgba(1.0, 1.0, 1.0, 0.92));
            let mut d = (FADE_M + 4.0) * k;
            while d < total - 4.0 * k {
                let (p, dir) = at(&pts, &cum, d);
                chevron(&mut m, p, dir, y + 0.02, white, k);
                d += CHEVRON_EVERY * k;
            }
        }
    }
    let ground = |m: &mut MeshBuilder, b: Option<BuildingId>, pos: [f32; 2], strong: bool| {
        let col = if strong {
            lin(ACCENT.with_alpha(0.95))
        } else {
            lin(Color::srgba(1.0, 1.0, 1.0, 0.7))
        };
        match b.and_then(|b| world.buildings.get(b)) {
            Some(bb) => {
                if strong {
                    footprint_ring(m, bb.corners(), 1.2, 0.9, y, lin(EDGE.with_alpha(0.6)));
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
    m
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
    mut line: Query<(Entity, &mut Visibility), With<RouteLine>>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
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
    let Ok((e, mut vis)) = line.single_mut() else { return };
    // The destination building glows (re-made when buildings were rebuilt).
    let dest = route.destination.and_then(|d| d.building).filter(|_| route.state != RouteState::None);
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
    if route.state == RouteState::None {
        vis.set_if_neq(Visibility::Hidden);
        return;
    }
    let m = route_mesh(&sim.world, &route, k);
    if m.is_empty() {
        vis.set_if_neq(Visibility::Hidden);
    } else {
        commands.entity(e).insert(Mesh3d(meshes.add(m.build())));
        vis.set_if_neq(Visibility::Inherited);
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
