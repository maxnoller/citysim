//! City-builder camera: orbit around a focus point on the ground.
//! WASD/arrows pan, wheel zooms, Q/E or middle-drag rotates (middle-drag
//! vertically tilts). Honours `CameraOverride`; applies `GraphicsQuality`.

use bevy::camera::PerspectiveProjection;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use citysim_sim::{MAP_SIZE_M, World};

use crate::shared::{CameraOverride, CameraPose, GameSet, GraphicsQuality, PointerOverUi, WorldCamera, cell_center};

/// The player-controlled pose of the world camera.
#[derive(Component, Debug, Clone, Copy)]
pub struct CameraRig(pub CameraPose);

pub const MIN_DISTANCE: f32 = 25.0;
pub const MAX_DISTANCE: f32 = 1800.0;
pub const MIN_PITCH: f32 = 0.35;
pub const MAX_PITCH: f32 = 1.50;

fn clamp(p: &mut CameraPose) {
    p.distance = p.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
    p.pitch = p.pitch.clamp(MIN_PITCH, MAX_PITCH);
    p.focus.x = p.focus.x.clamp(0.0, MAX_SIZE);
    p.focus.z = p.focus.z.clamp(0.0, MAX_SIZE);
    p.focus.y = 0.0;
}
const MAX_SIZE: f32 = MAP_SIZE_M;

/// Starting pose: looking at the inner end of the highway stub.
pub fn start_pose() -> CameraPose {
    let stub_end = World::highway_entry_cell().offset(citysim_sim::balance::HIGHWAY_STUB_CELLS - 1, 0);
    CameraPose {
        focus: cell_center(stub_end, 0.0) + Vec3::new(40.0, 0.0, 0.0),
        yaw: 0.6,
        pitch: 0.95,
        distance: 260.0,
    }
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera).add_systems(
            Update,
            (camera_controls, apply_rig, apply_quality)
                .chain()
                .in_set(GameSet::Tools),
        );
    }
}

fn spawn_camera(mut commands: Commands) {
    let rig = CameraRig(start_pose());
    commands.spawn((
        Name::new("World camera"),
        WorldCamera,
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            near: 1.0,
            far: 8000.0,
            ..default()
        }),
        rig.0.transform(),
        rig,
    ));
}

#[allow(clippy::too_many_arguments)]
fn camera_controls(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    over_ui: Res<PointerOverUi>,
    overridden: Res<CameraOverride>,
    mut rigs: Query<&mut CameraRig>,
) {
    if let Some(pose) = overridden.0 {
        for mut rig in &mut rigs {
            rig.0 = pose;
        }
        return;
    }
    let dt = time.delta_secs();
    for mut rig in &mut rigs {
        let p = &mut rig.0;
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
        if pan != Vec2::ZERO {
            let forward = Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos());
            let right = Vec3::new(-forward.z, 0.0, forward.x);
            let speed = p.distance * 1.2;
            p.focus += (forward * pan.y + right * pan.x).normalize() * speed * dt;
        }
        if keys.pressed(KeyCode::KeyQ) {
            p.yaw -= 1.8 * dt;
        }
        if keys.pressed(KeyCode::KeyE) {
            p.yaw += 1.8 * dt;
        }
        if mouse.pressed(MouseButton::Middle) {
            p.yaw -= motion.delta.x * 0.006;
            p.pitch += motion.delta.y * 0.004;
        }
        if !over_ui.0 {
            let lines = match scroll.unit {
                MouseScrollUnit::Line => scroll.delta.y,
                MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
            };
            if lines != 0.0 {
                p.distance *= 0.88f32.powf(lines);
            }
        }
        clamp(p);
    }
}

fn apply_rig(mut q: Query<(&CameraRig, &mut Transform), Changed<CameraRig>>) {
    for (rig, mut t) in &mut q {
        *t = rig.0.transform();
    }
}

fn apply_quality(quality: Res<GraphicsQuality>, mut commands: Commands, q: Query<Entity, With<WorldCamera>>) {
    if !quality.is_changed() {
        return;
    }
    let msaa = match *quality {
        GraphicsQuality::Low => Msaa::Off,
        GraphicsQuality::Medium | GraphicsQuality::High => Msaa::Sample4,
    };
    for e in &q {
        commands.entity(e).insert(msaa);
    }
}
