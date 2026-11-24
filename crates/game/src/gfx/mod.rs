//! GFX: everything drawn in 3D, and the camera. Talks to the rest of the
//! game only through `crate::shared` (never import from `crate::ui`).

mod building_gen;
mod buildings;
mod camera;
mod cursor;
mod mesh_util;
mod noise;
mod overlay;
mod palette;
mod props;
mod road_geom;
mod roads;
mod scene;
mod stats;
mod trees;
mod vehicle_gen;
mod vehicles;
mod zones;

use bevy::prelude::*;

/// Heights above the ground for flat overlays, to avoid z-fighting.
pub(crate) mod heights {
    pub const ZONE: f32 = 0.045;
    pub const GHOST: f32 = 0.24;
    pub const HIGHLIGHT: f32 = 0.27;
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
            trees::TreesPlugin,
            stats::StatsPlugin,
        ));
    }
}
