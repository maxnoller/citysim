//! Day and night from the sim's time of day: the sun moves across the sky
//! and changes colour (warm at dawn and dusk), the moon lights the night in a
//! cool, readable blue, the sky / fog / ambient light follow, stars come out,
//! and the shared city material gets its night inputs (street lamps,
//! headlights, the share of lit windows).
//!
//! `CITYSIM_GFX_HOUR=h` pins the drawn time of day (dev aid for screenshots).

use std::f32::consts::PI;

use bevy::light::GlobalAmbientLight;
use bevy::pbr::DistanceFog;
use bevy::prelude::*;

use super::city_material::{CityMaterial, SkyMaterial};
use super::palette::{self, mix};
use super::roads::{LampPoolMesh, LampPools};
use super::scene::{Bounce, Sun, WorldMaterials, quality_settings};
use crate::shared::{GameSet, GraphicsQuality, SimWorld, WorldCamera};

/// Everything the time of day decides about the look of the scene.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct DayLight {
    pub hour: f32,
    /// Sun elevation, degrees (negative below the horizon).
    pub elevation: f32,
    /// Unit vector towards the sun.
    pub to_sun: Vec3,
    /// Unit vector towards the moon.
    pub to_moon: Vec3,
    /// Direction the main light travels (sun by day, moon by night).
    pub light_dir: Vec3,
    pub light_color: Color,
    pub light_lux: f32,
    pub ambient: Color,
    pub ambient_brightness: f32,
    pub horizon: Color,
    pub mid: Color,
    pub zenith: Color,
    pub glow: Color,
    pub glow_strength: f32,
    /// 0 by day .. 1 at night: lamps, headlights, window glow.
    pub night: f32,
    /// Share of windows lit.
    pub windows: f32,
    pub stars: f32,
}

impl Default for DayLight {
    fn default() -> Self {
        day_light(15.0)
    }
}

/// Pinned hour from `CITYSIM_GFX_HOUR`.
#[derive(Resource)]
struct PinnedHour(Option<f32>);

pub struct DayNightPlugin;

impl Plugin for DayNightPlugin {
    fn build(&self, app: &mut App) {
        let pinned = std::env::var("CITYSIM_GFX_HOUR").ok().and_then(|s| s.parse().ok());
        app.insert_resource(PinnedHour(pinned))
            .init_resource::<DayLight>()
            .add_systems(
                Update,
                (update_day_light, apply_lights, apply_materials)
                    .chain()
                    .in_set(GameSet::Render),
            );
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Unit vector from elevation and azimuth (degrees) in Bevy space.
fn dir(elevation: f32, azimuth: f32) -> Vec3 {
    let (e, a) = (elevation.to_radians(), azimuth.to_radians());
    Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin())
}

const SUNRISE: f32 = 6.0;
const SUNSET: f32 = 19.5;

/// Sun elevation and azimuth (degrees) at `hour`. Mid-afternoon matches the
/// classic diorama light (about 45 degrees up, from the south-east of the
/// default view) so fronts are lit and shadows fall diagonally.
pub fn sun_angles(hour: f32) -> (f32, f32) {
    let s = (hour - SUNRISE) / (SUNSET - SUNRISE);
    // Below the horizon the sun keeps going round (s < 0 or > 1).
    let elevation = (s * PI).sin() * 56.0;
    let elevation = if (0.0..=1.0).contains(&s) {
        elevation
    } else {
        // Night: how far past sunset / before sunrise, as a smooth dip.
        let past = if s > 1.0 {
            (hour - SUNSET).max(0.0)
        } else {
            let h = if hour < SUNRISE { hour + 24.0 } else { hour };
            (SUNRISE + 24.0 - h).max(0.0)
        };
        -(past * 14.0).min(40.0)
    };
    let azimuth = 35.0 + (s - 0.667) * 160.0;
    (elevation, azimuth)
}

/// One look of the sky at a sun elevation.
#[derive(Clone, Copy)]
struct Key {
    elevation: f32,
    horizon: Color,
    mid: Color,
    zenith: Color,
    glow: Color,
    glow_strength: f32,
    ambient: Color,
    ambient_brightness: f32,
}

fn keys() -> [Key; 5] {
    let c = Color::srgb_u32;
    [
        Key {
            elevation: -14.0,
            horizon: c(0x243154),
            mid: c(0x17213F),
            zenith: c(0x0B1229),
            glow: c(0x34436E),
            glow_strength: 0.25,
            ambient: Color::srgb(0.42, 0.55, 1.0),
            ambient_brightness: 330.0,
        },
        Key {
            elevation: -6.0,
            horizon: c(0x4A4C72),
            mid: c(0x2C3A68),
            zenith: c(0x16214A),
            glow: c(0xB0707A),
            glow_strength: 0.5,
            ambient: Color::srgb(0.6, 0.6, 0.95),
            ambient_brightness: 520.0,
        },
        Key {
            elevation: -0.5,
            horizon: c(0xEDA176),
            mid: c(0x7A86B8),
            zenith: c(0x36508F),
            glow: c(0xFF8C4A),
            glow_strength: 0.85,
            ambient: Color::srgb(0.95, 0.78, 0.8),
            ambient_brightness: 720.0,
        },
        Key {
            elevation: 7.0,
            horizon: c(0xF1CDA6),
            mid: c(0x8DB2E2),
            zenith: c(0x4A7DCC),
            glow: c(0xFFC07A),
            glow_strength: 0.7,
            ambient: Color::srgb(0.95, 0.86, 0.84),
            ambient_brightness: 700.0,
        },
        Key {
            elevation: 22.0,
            horizon: palette::SKY_HORIZON,
            mid: palette::SKY_MID,
            zenith: palette::SKY_ZENITH,
            glow: palette::SKY_GLOW,
            glow_strength: 0.45,
            ambient: Color::srgb(0.80, 0.88, 1.0),
            ambient_brightness: 760.0,
        },
    ]
}

fn sample(e: f32) -> Key {
    let k = keys();
    if e <= k[0].elevation {
        return k[0];
    }
    for w in k.windows(2) {
        let (a, b) = (w[0], w[1]);
        if e <= b.elevation {
            let t = smoothstep(a.elevation, b.elevation, e);
            return Key {
                elevation: e,
                horizon: mix(a.horizon, b.horizon, t),
                mid: mix(a.mid, b.mid, t),
                zenith: mix(a.zenith, b.zenith, t),
                glow: mix(a.glow, b.glow, t),
                glow_strength: a.glow_strength + (b.glow_strength - a.glow_strength) * t,
                ambient: mix(a.ambient, b.ambient, t),
                ambient_brightness: a.ambient_brightness + (b.ambient_brightness - a.ambient_brightness) * t,
            };
        }
    }
    k[k.len() - 1]
}

/// Share of windows lit at `hour` (people at home and awake).
pub fn window_share(hour: f32) -> f32 {
    const P: [(f32, f32); 9] = [
        (0.0, 0.2),
        (4.5, 0.1),
        (6.5, 0.4),
        (8.5, 0.15),
        (16.0, 0.15),
        (18.5, 0.6),
        (21.5, 0.7),
        (23.0, 0.42),
        (24.0, 0.2),
    ];
    let h = hour.rem_euclid(24.0);
    for w in P.windows(2) {
        if h <= w[1].0 {
            let t = (h - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + (w[1].1 - w[0].1) * t;
        }
    }
    P[0].1
}

/// The look of the scene at `hour` (0..24).
pub fn day_light(hour: f32) -> DayLight {
    let (elevation, azimuth) = sun_angles(hour);
    let to_sun = dir(elevation, azimuth);
    // The moon rises in the evening and crosses the sky through the night,
    // high enough to give the town readable shading.
    let night_t = ((hour - SUNSET).rem_euclid(24.0) / (24.0 - SUNSET + SUNRISE)).clamp(0.0, 1.0);
    let to_moon = dir(22.0 + (night_t * PI).sin() * 30.0, 70.0 + night_t * 140.0);
    let key = sample(elevation);
    // Sun light fades out a little below the horizon; the moon takes over.
    let (light_dir, light_color, light_lux) = if elevation > -4.0 {
        let k = ((elevation + 4.0) / 18.0).clamp(0.0, 1.0);
        let warm = smoothstep(2.0, 26.0, elevation);
        let color = mix(Color::srgb_u32(0xFFA466), palette::SUN, warm);
        // Keep shadows from going infinitely long at the horizon.
        let d = dir(elevation.max(4.0), azimuth);
        (-d, color, 6_200.0 * k)
    } else {
        let k = ((-4.0 - elevation) / 8.0).clamp(0.0, 1.0);
        (-to_moon, Color::srgb(0.62, 0.74, 1.0), 1_500.0 * k)
    };
    DayLight {
        hour,
        elevation,
        to_sun,
        to_moon,
        light_dir,
        light_color,
        light_lux,
        ambient: key.ambient,
        ambient_brightness: key.ambient_brightness,
        horizon: key.horizon,
        mid: key.mid,
        zenith: key.zenith,
        glow: key.glow,
        glow_strength: key.glow_strength,
        night: smoothstep(7.0, -4.0, elevation),
        windows: window_share(hour),
        stars: smoothstep(-5.0, -12.0, elevation),
    }
}

fn update_day_light(sim: Res<SimWorld>, pinned: Res<PinnedHour>, mut light: ResMut<DayLight>) {
    let hour = pinned.0.unwrap_or_else(|| sim.world.time.hour());
    if (hour - light.hour).abs() > 1e-4 || light.is_added() {
        *light = day_light(hour);
    }
}

#[allow(clippy::type_complexity)]
fn apply_lights(
    light: Res<DayLight>,
    mut suns: Query<(&mut DirectionalLight, &mut Transform), (With<Sun>, Without<Bounce>)>,
    mut bounces: Query<(&mut DirectionalLight, &mut Transform), (With<Bounce>, Without<Sun>)>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut fogs: Query<&mut DistanceFog, With<WorldCamera>>,
    mut done: Local<bool>,
) {
    if !light.is_changed() && *done {
        return;
    }
    *done = true;
    for (mut l, mut t) in &mut suns {
        l.color = light.light_color;
        l.illuminance = light.light_lux;
        *t = Transform::from_translation(-light.light_dir).looking_at(Vec3::ZERO, Vec3::Y);
    }
    let sun_k = (light.light_lux / 6_200.0).clamp(0.0, 1.0);
    for (mut l, mut t) in &mut bounces {
        l.illuminance = 650.0 * sun_k;
        l.color = mix(light.light_color, Color::srgb(1.0, 0.93, 0.82), 0.5);
        *t = Transform::from_translation(light.light_dir.with_y(-0.35)).looking_at(Vec3::ZERO, Vec3::Y);
    }
    ambient.color = light.ambient;
    ambient.brightness = light.ambient_brightness;
    clear.0 = light.horizon;
    for mut fog in &mut fogs {
        fog.color = light.horizon;
        fog.directional_light_color =
            mix(light.glow, Color::NONE, 1.0 - light.glow_strength * 0.5).with_alpha(0.25 * sun_k);
    }
}

fn lin4(c: Color, a: f32) -> Vec4 {
    let l = c.to_linear();
    Vec4::new(l.red, l.green, l.blue, a)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn apply_materials(
    light: Res<DayLight>,
    quality: Res<GraphicsQuality>,
    mats: Option<Res<WorldMaterials>>,
    mut city: ResMut<Assets<CityMaterial>>,
    mut sky: ResMut<Assets<SkyMaterial>>,
    pools: Option<Res<LampPools>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut pool_meshes: Query<&mut Visibility, With<LampPoolMesh>>,
    mut last: Local<Option<(f32, f32, f32, f32, GraphicsQuality)>>,
) {
    // New chunks spawn hidden: show them while the lamps are on.
    let lamps_on = light.night > 0.02;
    for mut v in &mut pool_meshes {
        let want = if lamps_on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        v.set_if_neq(want);
    }
    let Some(mats) = mats else { return };
    let clouds = if quality_settings(*quality).cloud_shadows {
        0.28
    } else {
        0.0
    };
    let now = (light.night, light.windows, light.elevation, clouds, *quality);
    if let Some(prev) = *last
        && prev.4 == now.4
        && (prev.0 - now.0).abs() < 0.004
        && (prev.1 - now.1).abs() < 0.004
        && (prev.2 - now.2).abs() < 0.08
        && prev.3 == now.3
    {
        return;
    }
    *last = Some(now);
    if let Some(pools) = pools
        && let Some(mut m) = std_mats.get_mut(&pools.material)
    {
        let k = light.night * 0.42;
        m.base_color = Color::linear_rgb(1.0 * k, 0.72 * k, 0.4 * k);
    }
    for h in mats.city_handles() {
        if let Some(mut m) = city.get_mut(h) {
            m.extension.params.night = light.night;
            m.extension.params.windows = light.windows;
            m.extension.params.clouds = clouds;
        }
    }
    if let Some(mut m) = sky.get_mut(&mats.sky) {
        let p = &mut m.extension.params;
        p.horizon = lin4(light.horizon, 1.0);
        p.mid = lin4(light.mid, 1.0);
        p.zenith = lin4(light.zenith, 1.0);
        p.glow = lin4(light.glow, light.glow_strength);
        p.sun = light.to_sun.extend(if light.elevation > -1.5 { 18.0 } else { 0.0 });
        p.moon = light.to_moon.extend(smoothstep(-2.0, -8.0, light.elevation));
        p.stars = Vec4::new(light.stars * 1.4, 0.0, 0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_bright_and_midnight_is_dark_but_lit_by_the_moon() {
        let noon = day_light(13.0);
        let night = day_light(1.0);
        assert!(noon.elevation > 45.0, "{}", noon.elevation);
        assert!(night.elevation < -10.0);
        assert!(noon.light_lux > 5_000.0 && night.light_lux > 500.0 && night.light_lux < 2_500.0);
        assert!(noon.night < 0.01 && night.night > 0.99);
        assert!(night.windows > noon.windows);
        assert!(night.stars > 0.9 && noon.stars < 0.01);
        // The moon is above the horizon all night.
        for h in [20.5, 22.0, 0.0, 3.0, 5.5] {
            assert!(day_light(h).to_moon.y > 0.2, "{h}");
        }
    }

    #[test]
    fn the_sun_rises_and_sets_smoothly() {
        let mut prev = day_light(0.0);
        for i in 1..=24 * 20 {
            let d = day_light(i as f32 / 20.0);
            assert!((d.elevation - prev.elevation).abs() < 4.0, "{}", d.hour);
            assert!((d.light_lux - prev.light_lux).abs() < 400.0, "{}", d.hour);
            assert!(d.light_dir.y < 0.0, "light from below at {}", d.hour);
            prev = d;
        }
        assert!(day_light(SUNRISE + 0.01).elevation.abs() < 1.0);
        assert!(day_light(SUNSET - 0.01).elevation.abs() < 1.0);
    }
}
