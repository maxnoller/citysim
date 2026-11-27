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
//! * With `DisplaySettings::edge_scroll` the view pans while the cursor
//!   rests at a window edge (only while the window has focus and no mouse
//!   button is held).
//! * While `CameraInputBlocked` (pause menu) all camera input is ignored.
//! * `CameraFollow` keeps the focus on what is followed (panning ignored):
//!   a car's body or the off-map traveller's marker as drawn this frame
//!   (`follow_drawn`, after they are placed), else its point. It glides
//!   onto a new target (an offset from it that dies away) and is then
//!   locked to it exactly, so the followed thing holds still on screen
//!   whatever the frame times. `CameraGoTo` moves the focus (and zoom) once.

use bevy::camera::PerspectiveProjection;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::{MAP_SIZE_M, World};

use super::scene::camera_look;
use super::vehicles::DrawnSet;
use crate::shared::{
    CameraFollow, CameraGoTo, CameraInputBlocked, CameraOverride, CameraPose, DisplaySettings, DrawnPoses, FollowOn,
    GameSet, PointerOverUi, WorldCamera, cell_center,
};

pub const MIN_DISTANCE: f32 = 22.0;
pub const MAX_DISTANCE: f32 = 2000.0;
/// Extra tilt the player can add with middle-drag, radians.
const MAX_TILT_OFFSET: f32 = 0.45;
const MIN_PITCH: f32 = 0.22;
const MAX_PITCH: f32 = 1.48;
/// How far the focus may leave the map, metres.
const FOCUS_MARGIN: f32 = 60.0;
/// Edge scrolling: band along the window border, logical pixels.
const EDGE_BAND: f32 = 6.0;

/// Pan direction (x right, y forward) for a cursor at `pos` in a window of
/// `size`: non-zero only within `EDGE_BAND` of a border.
pub fn edge_pan(pos: Vec2, size: Vec2) -> Vec2 {
    let mut pan = Vec2::ZERO;
    if pos.x <= EDGE_BAND {
        pan.x -= 1.0;
    } else if pos.x >= size.x - EDGE_BAND {
        pan.x += 1.0;
    }
    if pos.y <= EDGE_BAND {
        pan.y += 1.0;
    } else if pos.y >= size.y - EDGE_BAND {
        pan.y -= 1.0;
    }
    pan
}

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
            .add_systems(Update, (camera_controls, apply_rig).chain().in_set(GameSet::Tools))
            .add_systems(Update, follow_drawn.in_set(GameSet::Render).after(DrawnSet::Traveller));
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
    display: Res<DisplaySettings>,
    overridden: Res<CameraOverride>,
    (follow, mut goto): (Res<CameraFollow>, MessageReader<CameraGoTo>),
    gfx_pose: Res<GfxPose>,
    tour: Res<GfxTour>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut rigs: Query<(&mut CameraRig, &Camera, &GlobalTransform)>,
) {
    let dt = time.delta_secs().min(0.1);
    let jump = goto.read().last().copied();
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
        if display.edge_scroll
            && pan == Vec2::ZERO
            && !mouse.any_pressed([MouseButton::Left, MouseButton::Right, MouseButton::Middle])
            && let Ok(w) = window.single()
            && w.focused
            && let Some(pos) = w.cursor_position()
        {
            pan = edge_pan(pos, Vec2::new(w.width(), w.height()));
        }
        let boost = if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
            2.5
        } else {
            1.0
        };
        if let Some(j) = jump {
            rig.target.focus = Vec3::new(j.focus[0], 0.0, j.focus[1]);
            if let Some(d) = j.distance {
                rig.target.distance = d;
            }
        }
        // Following: `follow_drawn` moves the focus later in the frame.
        let following = follow.focus.is_some();
        if following {
            pan = Vec2::ZERO;
        }
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
        if !following {
            c.focus = c.focus.lerp(t.focus, a);
        }
        // Zoom in log space so it feels even at all distances.
        c.distance = (c.distance.ln() + (t.distance.ln() - c.distance.ln()) * a).exp();
        c.pitch += (t.pitch - c.pitch) * a;
        let dyaw = (t.yaw - c.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        c.yaw += dyaw * a;
        // Keep both yaws near each other so the angle never unwinds.
        rig.target.yaw = rig.current.yaw + (dyaw - dyaw * a);
    }
}

/// How fast the camera glides onto a new followed target (rad/s of a
/// critically damped spring, about the camera's easing).
const FOLLOW_GLIDE: f32 = 9.0;
/// A followed point that moves further than this in a frame jumped (a new
/// building): glide there.
const FOLLOW_JUMP_M: f32 = 12.0;

/// What the camera follows and how far it still is from it.
#[derive(Default)]
struct FollowState {
    /// What was followed last frame, and whether it was drawn.
    on: Option<(FollowOn, bool)>,
    /// The followed point last frame.
    last: Vec3,
    /// The camera's focus minus the followed point, and how fast that
    /// changes: a critically damped spring brings both to zero.
    offset: Vec3,
    offset_vel: Vec3,
    /// The target's own speed is not known yet (its first frame).
    target_unknown: bool,
    /// The shown focus last frame and its velocity (following or not).
    focus: Option<Vec3>,
    focus_vel: Vec3,
}

/// One step of `dt` s of a critically damped spring of rate `w`:
/// (offset, its velocity) -> the same `dt` later (exact for any `dt`).
fn spring(e: Vec3, v: Vec3, w: f32, dt: f32) -> (Vec3, Vec3) {
    let k = (-w * dt).exp();
    let c = v + e * w;
    ((e + c * dt) * k, (v - c * (w * dt)) * k)
}

/// Keep the camera on the followed thing as it is drawn this frame: the
/// focus is the drawn point plus an offset that dies away (a glide onto a
/// new target that starts at the camera's own speed: never a snap or a
/// lurch), so once settled the followed body stays put on screen.
#[allow(clippy::too_many_arguments)]
fn follow_drawn(
    time: Res<Time<Real>>,
    follow: Res<CameraFollow>,
    drawn: Res<DrawnPoses>,
    overridden: Res<CameraOverride>,
    gfx_pose: Res<GfxPose>,
    tour: Res<GfxTour>,
    mut rigs: Query<(&mut CameraRig, &mut Transform)>,
    mut state: Local<FollowState>,
) {
    let dt = time.delta_secs().min(0.1);
    let scripted = tour.0 || gfx_pose.0.is_some() || overridden.0.is_some();
    // The point, and whether it is a drawn one (moving continuously).
    let point = follow.focus.filter(|_| !scripted).map(|f| {
        let (p, drawn) = match follow.on {
            FollowOn::Vehicle(v) => drawn.vehicle(v).map_or((f, false), |d| (d.pos, true)),
            FollowOn::Traveller => drawn.traveller.map_or((f, false), |t| (t, true)),
            FollowOn::Point => (f, false),
        };
        let p = Vec3::new(
            p[0].clamp(-FOCUS_MARGIN, MAP_SIZE_M + FOCUS_MARGIN),
            0.0,
            p[1].clamp(-FOCUS_MARGIN, MAP_SIZE_M + FOCUS_MARGIN),
        );
        (p, (follow.on, drawn))
    });
    let Ok((mut rig, mut tf)) = rigs.single_mut() else {
        return;
    };
    if let Some((p, source)) = point {
        // A new target (or the same one drawn / not drawn any more), or a
        // point that jumped: glide from where the camera is, as fast as it
        // moves.
        let fresh = state.on != Some(source) || (!source.1 && p.distance(state.last) > FOLLOW_JUMP_M);
        if fresh {
            state.offset = rig.current.focus - p;
            state.offset_vel = state.focus_vel;
            state.target_unknown = true;
        } else {
            if state.target_unknown && dt > 1e-5 {
                // Now the target's speed is known: the camera keeps its own.
                let target_vel = (p - state.last) / dt;
                state.offset_vel -= target_vel;
                state.target_unknown = false;
            }
            let (e, v) = spring(state.offset, state.offset_vel, FOLLOW_GLIDE, dt);
            state.offset = e;
            state.offset_vel = v;
        }
        if state.offset.length() < 1e-4 && state.offset_vel.length() < 1e-3 {
            state.offset = Vec3::ZERO;
            state.offset_vel = Vec3::ZERO;
        }
        rig.target.focus = p;
        rig.current.focus = p + state.offset;
        let new = rig.current.transform();
        if *tf != new {
            *tf = new;
        }
        state.on = Some(source);
        state.last = p;
    } else {
        state.on = None;
    }
    let focus = rig.current.focus;
    if let Some(last) = state.focus
        && dt > 1e-5
    {
        state.focus_vel = (focus - last) / dt;
    }
    state.focus = Some(focus);
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
    fn edge_scroll_only_at_the_border() {
        let size = Vec2::new(1280.0, 720.0);
        assert_eq!(edge_pan(Vec2::new(640.0, 360.0), size), Vec2::ZERO);
        assert_eq!(edge_pan(Vec2::new(2.0, 360.0), size), Vec2::new(-1.0, 0.0));
        assert_eq!(edge_pan(Vec2::new(1279.0, 1.0), size), Vec2::new(1.0, 1.0));
        assert_eq!(edge_pan(Vec2::new(640.0, 719.0), size), Vec2::new(0.0, -1.0));
    }

    #[test]
    fn pitch_flattens_towards_top_down_when_zooming_out() {
        assert!(auto_pitch(MIN_DISTANCE) < auto_pitch(300.0));
        assert!(auto_pitch(300.0) < auto_pitch(MAX_DISTANCE));
        assert!(auto_pitch(MAX_DISTANCE) < MAX_PITCH);
        assert!(auto_pitch(MIN_DISTANCE) > MIN_PITCH);
    }
}
