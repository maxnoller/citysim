//! UI: all 2D interface, tools / input that produce sim `Command`s,
//! pause/speed, overlays & quality switches, save/load. Talks to the rest of
//! the game only through `crate::shared` (never import from `crate::gfx`).

mod controls;
mod hud;
mod pointer;
mod save_load;
mod tools;

use bevy::camera::ClearColorConfig;
use bevy::prelude::*;

use crate::shared::WorldCamera;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            pointer::PointerPlugin,
            tools::ToolsPlugin,
            controls::ControlsPlugin,
            save_load::SaveLoadPlugin,
            hud::HudPlugin,
        ))
        .add_systems(PostStartup, fallback_ui_camera);
    }
}

/// UI renders on the world camera when gfx provides one; otherwise (gfx
/// plugin absent) spawn a plain 2D camera so the UI is still visible.
fn fallback_ui_camera(mut commands: Commands, world_cams: Query<(), With<WorldCamera>>) {
    if world_cams.is_empty() {
        commands.spawn((
            Name::new("UI fallback camera"),
            Camera2d,
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.1, 0.1, 0.12)),
                ..default()
            },
        ));
    }
}
