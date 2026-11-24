//! CitySim game binary: Bevy front-end for the `citysim_sim` crate.
//!
//! The game owns a `citysim_sim::World` (in [`sim_driver::SimWorld`]), sends
//! it `Command`s, steps it on the fixed timestep and draws what it reads from
//! the World's public fields. No game rules live in this crate.

mod camera;
mod coords;
mod cursor;
mod hud;
mod mesh_util;
mod road_render;
mod scene;
mod script;
mod sim_driver;
mod tools;
mod zone_render;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};

/// Frame phases, run in this order every `Update`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameSet {
    /// Read devices / scripts into resources (cursor cell, pointer buttons).
    Input,
    /// Camera movement and tool logic; tools emit `SimCommand`s.
    Tools,
    /// Apply queued `SimCommand`s to the World.
    Sim,
    /// Rebuild meshes / HUD from World state.
    Render,
}

fn main() {
    let script = script::ScriptMode::from_env();
    App::new()
        .insert_resource(ClearColor(scene::SKY_COLOR))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "CitySim".into(),
                resolution: WindowResolution::new(1280, 720),
                present_mode: PresentMode::AutoVsync,
                ..default()
            }),
            ..default()
        }))
        .configure_sets(
            Update,
            (GameSet::Input, GameSet::Tools, GameSet::Sim, GameSet::Render).chain(),
        )
        .add_plugins((
            sim_driver::SimDriverPlugin,
            scene::ScenePlugin,
            camera::CameraPlugin,
            cursor::CursorPlugin,
            tools::ToolsPlugin,
            road_render::RoadRenderPlugin,
            zone_render::ZoneRenderPlugin,
            hud::HudPlugin,
            script::ScriptPlugin { mode: script },
        ))
        .run();
}
