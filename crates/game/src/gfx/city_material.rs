//! The shared materials: `CityMaterial` (StandardMaterial + night glow and
//! cloud shadows, `assets/gfx/shaders/city.wesl`) for everything
//! vertex-coloured, and `SkyMaterial` for the sky dome (`sky.wesl`). Their
//! uniforms are driven by `daynight`.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

pub type CityMaterial = ExtendedMaterial<StandardMaterial, CityExt>;
pub type SkyMaterial = ExtendedMaterial<StandardMaterial, SkyExt>;

/// Night-time and weather inputs of the city shader.
#[derive(ShaderType, Reflect, Debug, Clone, Copy, PartialEq)]
pub struct CityParams {
    /// 0 by day .. 1 at night (lamps, headlights, window glow strength).
    pub night: f32,
    /// Share of windows lit now.
    pub windows: f32,
    /// 1 for the braking-car material.
    pub brake: f32,
    /// Cloud shadow strength, 0 = off.
    pub clouds: f32,
    /// Cloud drift (m/s, x and z), cloud size (m), unused.
    pub wind: Vec4,
}

impl Default for CityParams {
    fn default() -> Self {
        CityParams {
            night: 0.0,
            windows: 0.0,
            brake: 0.0,
            clouds: 0.0,
            wind: Vec4::new(7.0, 3.0, 420.0, 0.0),
        }
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct CityExt {
    #[uniform(100)]
    pub params: CityParams,
}

impl MaterialExtension for CityExt {
    fn fragment_shader() -> ShaderRef {
        "gfx/shaders/city.wesl".into()
    }
}

/// Sky colours and the sun / moon (linear colours).
#[derive(ShaderType, Reflect, Debug, Clone, Copy, PartialEq, Default)]
pub struct SkyParams {
    pub horizon: Vec4,
    pub mid: Vec4,
    pub zenith: Vec4,
    /// Glow colour towards the sun (rgb) and strength (a).
    pub glow: Vec4,
    /// Towards the sun (xyz), disc brightness (w).
    pub sun: Vec4,
    /// Towards the moon (xyz), brightness (w).
    pub moon: Vec4,
    /// Stars brightness (x).
    pub stars: Vec4,
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct SkyExt {
    #[uniform(100)]
    pub params: SkyParams,
}

impl MaterialExtension for SkyExt {
    fn fragment_shader() -> ShaderRef {
        "gfx/shaders/sky.wesl".into()
    }

    fn enable_shadows() -> bool {
        false
    }

    fn enable_prepass() -> bool {
        false
    }
}

/// The lit, vertex-coloured base every city surface uses.
pub fn city_material(base: StandardMaterial) -> CityMaterial {
    ExtendedMaterial {
        base,
        extension: CityExt::default(),
    }
}

pub struct CityMaterialPlugin;

impl Plugin for CityMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<CityMaterial>::default(),
            MaterialPlugin::<SkyMaterial>::default(),
        ));
    }
}
