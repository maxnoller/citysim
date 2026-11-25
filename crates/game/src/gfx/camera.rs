//! City-builder camera: an orbit around a focus point on the ground.
//!
//! * WASD / arrows pan (speed scales with zoom), Q / E rotate,
//!   mouse wheel zooms towards the point under the cursor,
//!   middle-drag rotates (horizontal) and tilts (vertical).
//! * Everything is damped: input moves a target pose and the shown pose
//!   eases towards it.
//! * The tilt follows the zoom: low and cinematic close up, more top-down
//!   (a flatter, map-like view) when zoomed out; middle-drag adds an offset.
//! * Focus is clamped to the map. `CameraOverride` shows an exact pose.
//! * While `CameraInputBlocked` (pause menu) all camera input is ignored.

use bevy::camera::PerspectiveProjection;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::{MAP_SIZE_M, World};

use super::scene::camera_look;
use crate::shared::{CameraInputBlocked, CameraOverride, CameraPose, GameSet, PointerOverUi, WorldCamera, cell_center};

pub const MIN_DISTANCE: f32 = 22.0;
pub const MAX_DISTANCE: f32 = 2000.0;
/// Extra tilt the player can add with middle-drag, radians.
const MAX_TILT_OFFSET: f32 = 0.45;
const MIN_PITCH: f32 = 0.22;
const MAX_PITCH: f32 = 1.48;
/// How far the focus may leave the map, metres.
const FOCUS_MARGIN: f32 = 60.0;

/// The player-controlled pose of the world camera.
#[derive(Component, Debug, Clone, Copy)]
pub struct CameraRig {
    /// Where input wants the camera (pitch is derived, see `tilt`).
    pub target: CameraPose,
    /// Pose actually shown this frame.
    pub current: CameraPose,
    /// Player tilt offset added to the zoom-derived pitch.
    pub tilt: f32,
}

/// Pitch for a zoom distance before the player's tilt offset.
pub fn auto_pitch(distance: f32) -> f32 {
    let t = ((distance / MIN_DISTANCE).ln() / (MAX_DISTANCE / MIN_DISTANCE).ln()).clamp(0.0, 1.0);
    0.50 + 0.70 * t.powf(1.1)
}

/// Starting pose: looking at the inner end of the highway stub.
pub fn start_pose() -> CameraPose {
    let stub_end = World::highway_entry_cell().offset(citysim_sim::balance::HIGHWAY_STUB_CELLS - 1, 0);
    let distance = 300.0;
    CameraPose {
        focus: cell_center(stub_end, 0.0) + Vec3::new(60.0, 0.0, 0.0),
        yaw: 0.6,
        pitch: auto_pitch(distance),
        distance,
    }
}

/// Dev aid: `CITYSIM_GFX_POSE="x,z,yaw,pitch,distance"` (metres, radians)
/// shows that pose, taking precedence over `CameraOverride`, so gfx
/// screenshots of any spot are reproducible.
#[derive(Resource, Default)]
struct GfxPose(Option<CameraPose>);

fn gfx_pose_from_env() -> Option<CameraPose> {
    let s = std::env::var("CITYSIM_GFX_POSE").ok()?;
    let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (v.len() == 5).then(|| CameraPose {
        focus: Vec3::new(v[0], 0.0, v[1]),
        yaw: v[2],
        pitch: v[3],
        distance: v[4],
    })
}

/// Dev aid: `CITYSIM_GFX_TOUR=1` flies a fixed camera path (for videos);
/// `CITYSIM_GFX_TOUR=4` flies it four times slower (record a slow machine,
/// then speed the video up).
#[derive(Resource, Default)]
struct GfxTour(bool, f32);

/// Tour keyframes: (seconds, focus x, focus z, yaw, pitch, distance).
const TOUR: [(f32, f32, f32, f32, f32, f32); 6] = [
    (0.0, 330.0, 1010.0, 0.25, 1.05, 760.0),
    (5.0, 400.0, 1000.0, 0.7, 0.85, 330.0),
    (10.0, 300.0, 1030.0, 1.3, 0.42, 120.0),
    (15.0, 285.0, 1032.0, 2.2, 0.3, 70.0),
    (19.0, 330.0, 1040.0, 2.9, 0.45, 160.0),
    (24.0, 420.0, 1010.0, 3.4, 0.9, 520.0),
];

fn tour_pose(t: f32) -> CameraPose {
    let k = TOUR.iter().rposition(|f| f.0 <= t).unwrap_or(0).min(TOUR.len() - 2);
    let (a, b) = (TOUR[k], TOUR[k + 1]);
    let u = ((t - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    let u = u * u * (3.0 - 2.0 * u);
    let l = |x: f32, y: f32| x + (y - x) * u;
    CameraPose {
        focus: Vec3::new(l(a.1, b.1), 0.0, l(a.2, b.2)),
        yaw: l(a.3, b.3),
        pitch: l(a.4, b.4),
        distance: (a.5.ln() + (b.5.ln() - a.5.ln()) * u).exp(),
    }
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        let tour_var = std::env::var("CITYSIM_GFX_TOUR").unwrap_or_default();
        let tour = !tour_var.is_empty() && tour_var != "0";
        let slow = tour_var.parse::<f32>().unwrap_or(1.0).max(1.0);
        app.insert_resource(GfxPose(gfx_pose_from_env()))
            .insert_resource(GfxTour(tour, slow))
            .add_systems(Startup, spawn_camera)
            .add_systems(Update, (camera_controls, apply_rig).chain().in_set(GameSet::Tools));
    }
}

fn spawn_camera(mut commands: Commands) {
    let pose = start_pose();
    commands.spawn((
        Name::new("World camera"),
        WorldCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 40f32.to_radians(),
            near: 0.5,
            far: 40_000.0,
            ..default()
        }),
        pose.transform(),
        CameraRig {
            target: pose,
            current: pose,
            tilt: 0.0,
        },
        camera_look(),
    ));
}

fn clamp_target(rig: &mut CameraRig) {
    let p = &mut rig.target;
    p.distance = p.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
    rig.tilt = rig.tilt.clamp(-MAX_TILT_OFFSET, MAX_TILT_OFFSET);
    p.pitch = (auto_pitch(p.distance) + rig.tilt).clamp(MIN_PITCH, MAX_PITCH);
    p.focus.x = p.focus.x.clamp(-FOCUS_MARGIN, MAP_SIZE_M + FOCUS_MARGIN);
    p.focus.z = p.focus.z.clamp(-FOCUS_MARGIN, MAP_SIZE_M + FOCUS_MARGIN);
    p.focus.y = 0.0;
}

/// Ground point (y = 0) under the cursor, from the shown camera.
fn cursor_ground(window: &Window, camera: &Camera, tf: &GlobalTransform) -> Option<Vec3> {
    let pos = window.cursor_position()?;
    let ray = camera.viewport_to_world(tf, pos).ok()?;
    let hit = ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
    // Ignore hits near the horizon (they would fling the camera away).
    ((hit - tf.translation()).length() < 6000.0).then_some(hit)
}

#[allow(clippy::too_many_arguments)]
fn camera_controls(
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    over_ui: Res<PointerOverUi>,
    blocked: Res<CameraInputBlocked>,
    overridden: Res<CameraOverride>,
    gfx_pose: Res<GfxPose>,
    tour: Res<GfxTour>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut rigs: Query<(&mut CameraRig, &Camera, &GlobalTransform)>,
) {
    let dt = time.delta_secs().min(0.1);
    for (mut rig, camera, cam_tf) in &mut rigs {
        let toured = tour.0.then(|| tour_pose(time.elapsed_secs() / tour.1 - 1.0));
        if let Some(pose) = toured.or(gfx_pose.0).or(overridden.0) {
            rig.target = pose;
            rig.current = pose;
            rig.tilt = pose.pitch - auto_pitch(pose.distance);
            continue;
        }
        // A modal ui (pause menu) owns the keyboard and mouse: no input,
        // but the shown pose still settles towards the target.
        let input = !blocked.0;
        let mut pan = Vec2::ZERO;
        if keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]) {
            pan.y += 1.0;
        }
        if keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]) {
            pan.y -= 1.0;
        }
        if keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]) {
            pan.x += 1.0;
        }
        if keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]) {
            pan.x -= 1.0;
        }
        let boost = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
            2.5
        } else {
            1.0
        };
        if input && pan != Vec2::ZERO {
            let yaw = rig.target.yaw;
            let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
            let right = Vec3::new(-forward.z, 0.0, forward.x);
            let speed = rig.target.distance.max(60.0) * 1.1 * boost;
            rig.target.focus += (forward * pan.y + right * pan.x).normalize() * speed * dt;
        }
        if input && keys.pressed(KeyCode::KeyQ) {
            rig.target.yaw -= 1.6 * dt;
        }
        if input && keys.pressed(KeyCode::KeyE) {
            rig.target.yaw += 1.6 * dt;
        }
        if input && mouse.pressed(MouseButton::Middle) {
            rig.target.yaw -= motion.delta.x * 0.006;
            rig.tilt += motion.delta.y * 0.004;
        }
        if input && !over_ui.0 {
            let lines = match scroll.unit {
                MouseScrollUnit::Line => scroll.delta.y,
                MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
            };
            if lines != 0.0 {
                let old = rig.target.distance;
                let new = (old * 0.85f32.powf(lines)).clamp(MIN_DISTANCE, MAX_DISTANCE);
                rig.target.distance = new;
                // Keep the ground point under the cursor roughly in place.
                if let Ok(w) = window.single()
                    && let Some(hit) = cursor_ground(w, camera, cam_tf)
                {
                    let k = 1.0 - new / old;
                    let f = rig.target.focus;
                    rig.target.focus = f + (hit - f).with_y(0.0) * k;
                }
            }
        }
        clamp_target(&mut rig);
        // Ease the shown pose towards the target.
        let a = 1.0 - (-dt * 9.0).exp();
        let t = rig.target;
        let c = &mut rig.current;
        c.focus = c.focus.lerp(t.focus, a);
        // Zoom in log space so it feels even at all distances.
        c.distance = (c.distance.ln() + (t.distance.ln() - c.distance.ln()) * a).exp();
        c.pitch += (t.pitch - c.pitch) * a;
        let dyaw = (t.yaw - c.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        c.yaw += dyaw * a;
        // Keep both yaws near each other so the angle never unwinds.
        rig.target.yaw = rig.current.yaw + (dyaw - dyaw * a);
    }
}

fn apply_rig(mut q: Query<(&CameraRig, &mut Transform)>) {
    for (rig, mut t) in &mut q {
        let new = rig.current.transform();
        if *t != new {
            *t = new;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_flattens_towards_top_down_when_zooming_out() {
        assert!(auto_pitch(MIN_DISTANCE) < auto_pitch(300.0));
        assert!(auto_pitch(300.0) < auto_pitch(MAX_DISTANCE));
        assert!(auto_pitch(MAX_DISTANCE) < MAX_PITCH);
        assert!(auto_pitch(MIN_DISTANCE) > MIN_PITCH);
    }
}
