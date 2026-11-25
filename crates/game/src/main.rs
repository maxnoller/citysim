//! CitySim game binary: Bevy front-end for the `citysim_sim` crate.
//!
//! Layout and ownership (see CLAUDE.md):
//! * `shared` — the only types `gfx` and `ui` both use (sim driver, tool /
//!   preview / overlay / quality resources, hover picking, coordinates).
//! * `gfx` — all 3D drawing and the camera (`GfxPlugin`, feature `gfx`).
//! * `ui` — 2D interface, tools, input, save/load (`UiPlugin`, feature `ui`).
//! * `dev` — development-only scenarios, scripted runs and screenshots.

mod dev;
#[cfg(feature = "gfx")]
mod gfx;
// Partial builds (one side disabled) leave parts of the shared contract unused.
#[cfg_attr(not(all(feature = "gfx", feature = "ui")), allow(dead_code))]
mod shared;
#[cfg(feature = "ui")]
mod ui;

use bevy::prelude::*;
use bevy::window::{PresentMode, WindowResolution};

/// `crates/game/assets` when it exists (so the binary also finds assets when
/// started directly instead of via `cargo run`), else `assets` next to it.
fn asset_root() -> String {
    let dev_path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
    if std::path::Path::new(dev_path).is_dir() {
        dev_path.to_string()
    } else {
        "assets".to_string()
    }
}

fn main() {
    let dev = dev::DevConfig::from_env();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "CitySim".into(),
                    resolution: WindowResolution::new(1280, 720),
                    present_mode: PresentMode::AutoVsync,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: asset_root(),
                ..default()
            }),
    )
    .add_plugins(shared::SharedPlugin);
    #[cfg(feature = "gfx")]
    app.add_plugins(gfx::GfxPlugin);
    #[cfg(feature = "ui")]
    app.add_plugins(ui::UiPlugin);
    app.add_plugins(dev::DevPlugin { config: dev });
    app.run();
}
