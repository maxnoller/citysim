//! GFX: everything drawn in 3D, and the camera. Talks to the rest of the
//! game only through `crate::shared` (never import from `crate::ui`).

mod buildings;
mod camera;
mod cursor;
mod mesh_util;
mod overlay;
mod roads;
mod scene;
mod vehicles;
mod zones;

use bevy::prelude::*;

/// Heights above the ground for flat overlays, to avoid z-fighting.
pub(crate) mod heights {
    pub const ZONE: f32 = 0.03;
    pub const ROAD: f32 = 0.06;
    pub const HIGHWAY: f32 = 0.07;
    pub const OVERLAY: f32 = 0.10;
    pub const GHOST: f32 = 0.12;
    pub const HIGHLIGHT: f32 = 0.15;
}

pub struct GfxPlugin;

impl Plugin for GfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            scene::ScenePlugin,
            camera::CameraPlugin,
            roads::RoadsPlugin,
            zones::ZonesPlugin,
            buildings::BuildingsPlugin,
            vehicles::VehiclesPlugin,
            cursor::CursorPlugin,
            overlay::OverlayPlugin,
        ));
    }
}
