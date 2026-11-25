//! The shared materials: `CityMaterial` (StandardMaterial + night glow and
//! cloud shadows, `assets/gfx/shaders/city.wesl`) for everything
//! vertex-coloured, and `SkyMaterial` for the sky dome (`sky.wesl`). Their
//! inputs are driven by `daynight`.
//!
//! The city material's time-of-day inputs live in a shared storage buffer
//! (`DayParams`), not in the material: changing a material asset makes Bevy
//! re-extract and re-specialise every mesh that uses it (thousands), while
//! rewriting the buffer's contents costs nothing. Brake lights are a
//! per-instance `MeshTag` (bit 0) for the same reason.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

pub type CityMaterial = ExtendedMaterial<StandardMaterial, CityExt>;
pub type SkyMaterial = ExtendedMaterial<StandardMaterial, SkyExt>;

/// Night-time and weather inputs of the city shader (`CityParams` in
/// `city.wesl`), stored in the shared `WorldMaterials::day` buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayParams {
    /// 0 by day .. 1 at night (lamps, headlights, window glow strength).
    pub night: f32,
    /// Share of windows lit now.
    pub windows: f32,
    /// Cloud shadow strength, 0 = off.
    pub clouds: f32,
    /// Cloud drift (m/s, x and z), cloud size (m).
    pub wind: Vec3,
}

impl Default for DayParams {
    fn default() -> Self {
        DayParams {
            night: 0.0,
            windows: 0.0,
            clouds: 0.0,
            wind: Vec3::new(7.0, 3.0, 420.0),
        }
    }
}

impl DayParams {
    /// The buffer contents (two vec4s).
    pub fn to_buffer(self) -> Vec<[f32; 4]> {
        vec![
            [self.night, self.windows, self.clouds, 0.0],
            [self.wind.x, self.wind.y, self.wind.z, 0.0],
        ]
    }
}

/// A new buffer for the day parameters.
pub fn day_buffer(params: DayParams) -> ShaderBuffer {
    ShaderBuffer::from(params.to_buffer())
}

/// `MeshTag` bit: the vehicle's brake lights are on.
pub const TAG_BRAKE: u32 = 1;

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct CityExt {
    #[storage(100, read_only)]
    pub day: Handle<ShaderBuffer>,
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

/// The lit, vertex-coloured base every city surface uses, reading the
/// shared day parameters from `day`.
pub fn city_material(base: StandardMaterial, day: Handle<ShaderBuffer>) -> CityMaterial {
    ExtendedMaterial {
        base,
        extension: CityExt { day },
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
