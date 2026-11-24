//! City-builder camera: orbit around a focus point on the ground.
//! WASD/arrows pan, wheel zooms, Q/E or middle-drag rotates (middle-drag
//! vertically tilts).

use bevy::camera::PerspectiveProjection;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use citysim_sim::{MAP_SIZE_M, World};

use crate::{GameSet, coords};

/// Where the camera looks and from how far. Edit this, not the Transform.
#[derive(Component, Debug, Clone, Copy)]
pub struct CameraRig {
    pub focus: Vec3,
    /// Radians around +Y.
    pub yaw: f32,
    /// Radians above the horizon.
    pub pitch: f32,
    pub distance: f32,
}

pub const MIN_DISTANCE: f32 = 25.0;
pub const MAX_DISTANCE: f32 = 1800.0;
pub const MIN_PITCH: f32 = 0.35;
pub const MAX_PITCH: f32 = 1.50;

impl CameraRig {
    pub fn transform(&self) -> Transform {
        let offset = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        ) * self.distance;
        Transform::from_translation(self.focus + offset).looking_at(self.focus, Vec3::Y)
    }

    pub fn clamp(&mut self) {
        self.distance = self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.pitch = self.pitch.clamp(MIN_PITCH, MAX_PITCH);
        self.focus.x = self.focus.x.clamp(0.0, MAP_SIZE_M);
        self.focus.z = self.focus.z.clamp(0.0, MAP_SIZE_M);
        self.focus.y = 0.0;
    }
}

impl Default for CameraRig {
    /// Looking at the inner end of the highway stub.
    fn default() -> Self {
        let stub_end = World::highway_entry_cell().offset(citysim_sim::balance::HIGHWAY_STUB_CELLS - 1, 0);
        CameraRig {
            focus: coords::cell_center(stub_end, 0.0) + Vec3::new(40.0, 0.0, 0.0),
            yaw: 0.6,
            pitch: 0.95,
            distance: 260.0,
        }
    }
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_camera)
            .add_systems(Update, (camera_controls, apply_rig).chain().in_set(GameSet::Tools));
    }
}

fn spawn_camera(mut commands: Commands) {
    let rig = CameraRig::default();
    commands.spawn((
        Name::new("Main camera"),
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            near: 1.0,
            far: 8000.0,
            ..default()
        }),
        rig.transform(),
        rig,
    ));
}

/// Disables manual camera control (used by scripted runs).
#[derive(Resource)]
pub struct CameraScripted;

fn camera_controls(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    scripted: Option<Res<CameraScripted>>,
    mut rigs: Query<&mut CameraRig>,
) {
    if scripted.is_some() {
        return;
    }
    let dt = time.delta_secs();
    for mut rig in &mut rigs {
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
            // Forward = from camera towards focus, flattened.
            let forward = Vec3::new(-rig.yaw.sin(), 0.0, -rig.yaw.cos());
            let right = Vec3::new(-forward.z, 0.0, forward.x);
            let speed = rig.distance * 1.2;
            rig.focus += (forward * pan.y + right * pan.x).normalize() * speed * dt;
        }
        if keys.pressed(KeyCode::KeyQ) {
            rig.yaw -= 1.8 * dt;
        }
        if keys.pressed(KeyCode::KeyE) {
            rig.yaw += 1.8 * dt;
        }
        if mouse.pressed(MouseButton::Middle) {
            rig.yaw -= motion.delta.x * 0.006;
            rig.pitch += motion.delta.y * 0.004;
        }
        let lines = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        if lines != 0.0 {
            rig.distance *= 0.88f32.powf(lines);
        }
        rig.clamp();
    }
}

fn apply_rig(mut q: Query<(&CameraRig, &mut Transform), Changed<CameraRig>>) {
    for (rig, mut t) in &mut q {
        *t = rig.transform();
    }
}
