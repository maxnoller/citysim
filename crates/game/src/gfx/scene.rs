//! Static scene: gradient sky dome, countryside ground that runs far past the
//! map into the fog, a subtle map boundary, the sun and sky fill light, and
//! the per-camera look (tone mapping, fog, shadows, AO, anti-aliasing) scaled
//! by `GraphicsQuality`.

use bevy::anti_alias::fxaa::Fxaa;
use bevy::anti_alias::smaa::{Smaa, SmaaPreset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap, GlobalAmbientLight, NotShadowCaster};
use bevy::pbr::{DistanceFog, FogFalloff, ScreenSpaceAmbientOcclusion, ScreenSpaceAmbientOcclusionQualityLevel};
use bevy::prelude::*;
use citysim_sim::MAP_SIZE_M;

use super::camera::CameraRig;
use super::mesh_util::MeshBuilder;
use super::noise;
use super::palette::{self, lin, mix};
use crate::shared::{GameSet, GraphicsQuality, WorldCamera};

/// How far the countryside extends beyond each map edge, metres.
pub const OUTSIDE: f32 = 9000.0;
const SKY_RADIUS: f32 = 24_000.0;

/// Direction the sunlight travels (from the sun towards the ground).
pub fn sun_direction() -> Vec3 {
    // Mid-afternoon: about 40 degrees above the horizon, from the south-east
    // of the default view so fronts are lit and shadows fall diagonally.
    let elevation = 40f32.to_radians();
    let azimuth = 35f32.to_radians();
    -Vec3::new(
        elevation.cos() * azimuth.cos(),
        elevation.sin(),
        elevation.cos() * azimuth.sin(),
    )
}

/// One shared material for everything vertex-coloured and lit.
#[derive(Resource, Clone)]
pub struct WorldMaterials {
    /// Opaque, lit, vertex colours.
    pub lit: Handle<StandardMaterial>,
}

#[derive(Component)]
struct SkyDome;

#[derive(Component)]
pub struct Sun;

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(palette::SKY_HORIZON))
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.80, 0.88, 1.0),
                brightness: 900.0,
                ..default()
            })
            .insert_resource(DirectionalLightShadowMap { size: 4096 })
            .add_systems(PreStartup, init_materials)
            .add_systems(Startup, setup_scene)
            .add_systems(
                Update,
                (apply_quality, follow_camera, tune_fog_and_shadows)
                    .chain()
                    .in_set(GameSet::Render),
            );
    }
}

fn init_materials(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(WorldMaterials {
        lit: materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.92,
            reflectance: 0.25,
            ..default()
        }),
    });
}

fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world_mats: Res<WorldMaterials>,
) {
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(ground_mesh())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 1.0,
            reflectance: 0.1,
            ..default()
        })),
        Transform::default(),
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Map boundary"),
        Mesh3d(meshes.add(boundary_mesh())),
        MeshMaterial3d(world_mats.lit.clone()),
        Transform::default(),
        NotShadowCaster,
    ));
    commands.spawn((
        Name::new("Sky"),
        SkyDome,
        Mesh3d(meshes.add(sky_mesh())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            fog_enabled: false,
            cull_mode: None,
            ..default()
        })),
        Transform::default(),
        NotShadowCaster,
        NoFrustumCulling,
    ));
    commands.spawn((
        Name::new("Sun"),
        Sun,
        DirectionalLight {
            color: palette::SUN,
            illuminance: 5_200.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_translation(-sun_direction()).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder::default().build(),
    ));
    // Soft bounce light from the ground side, no shadows: keeps shadowed
    // faces from going flat.
    commands.spawn((
        Name::new("Bounce"),
        DirectionalLight {
            color: Color::srgb(1.0, 0.93, 0.82),
            illuminance: 650.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_translation(sun_direction().with_y(-0.35)).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Grass colour at a point: large soft patches of lighter and darker green,
/// duller outside the playable map.
pub fn grass_color(x: f32, z: f32) -> Color {
    let n1 = noise::fbm(x / 420.0, z / 420.0, 3, 11);
    let n2 = noise::value(x / 95.0, z / 95.0, 23);
    let t = (n1 * 0.75 + n2 * 0.25).clamp(0.0, 1.0);
    let base = if t < 0.5 {
        mix(palette::GRASS_DARK, palette::GRASS, t * 2.0)
    } else {
        mix(palette::GRASS, palette::GRASS_LIGHT, (t - 0.5) * 2.0)
    };
    // Outside the map: a little drier and duller.
    let out = (-x).max(z - MAP_SIZE_M).max(x - MAP_SIZE_M).max(-z);
    let k = (out / 60.0).clamp(0.0, 1.0);
    mix(base, mix(palette::GRASS_OUTSIDE, base, 0.35), k * 0.8)
}

/// Grid coordinates along one axis: 32 m inside the map, widening outside.
fn ground_axis() -> Vec<f32> {
    let mut v = Vec::new();
    let mut d = 32.0;
    let mut x = 0.0f32;
    let mut outside = Vec::new();
    while x < OUTSIDE {
        x += d;
        outside.push(x.min(OUTSIDE));
        d *= 1.18;
    }
    for &o in outside.iter().rev() {
        v.push(-o);
    }
    let mut x = 0.0;
    while x <= MAP_SIZE_M {
        v.push(x);
        x += 32.0;
    }
    for &o in &outside {
        v.push(MAP_SIZE_M + o);
    }
    v
}

fn ground_mesh() -> Mesh {
    let axis = ground_axis();
    let mut b = MeshBuilder::new();
    let col: Vec<Vec<[f32; 4]>> = axis
        .iter()
        .map(|&z| axis.iter().map(|&x| lin(grass_color(x, z))).collect())
        .collect();
    for j in 0..axis.len() - 1 {
        for i in 0..axis.len() - 1 {
            let (x0, x1, z0, z1) = (axis[i], axis[i + 1], axis[j], axis[j + 1]);
            b.quad_c(
                [
                    Vec3::new(x0, 0.0, z0),
                    Vec3::new(x0, 0.0, z1),
                    Vec3::new(x1, 0.0, z1),
                    Vec3::new(x1, 0.0, z0),
                ],
                [col[j][i], col[j + 1][i], col[j + 1][i + 1], col[j][i + 1]],
            );
        }
    }
    b.build()
}

/// A faint pale dashed line marking the edge of the playable map.
fn boundary_mesh() -> Mesh {
    let mut b = MeshBuilder::new();
    let s = MAP_SIZE_M;
    let y = 0.02;
    let w = 0.7;
    let dash = 6.0;
    let gap = 4.0;
    let color = |x: f32, z: f32| lin(mix(grass_color(x, z), Color::srgb(0.98, 0.97, 0.9), 0.45));
    let mut t = 0.0;
    while t < s {
        let t1 = (t + dash).min(s);
        for (p0, p1, n) in [
            (Vec2::new(t, 0.0), Vec2::new(t1, 0.0), Vec2::Y),
            (Vec2::new(t, s), Vec2::new(t1, s), -Vec2::Y),
            (Vec2::new(0.0, t), Vec2::new(0.0, t1), Vec2::X),
            (Vec2::new(s, t), Vec2::new(s, t1), -Vec2::X),
        ] {
            let o = n * w * 0.5;
            let q = [p0 - o, p1 - o, p1 + o, p0 + o].map(|p| Vec3::new(p.x, y, p.y));
            b.quad_up(q, color(p0.x, p0.y));
        }
        t += dash + gap;
    }
    b.build()
}

fn sky_mesh() -> Mesh {
    let mut b = MeshBuilder::new();
    let rings = 14;
    let sectors = 36;
    let sun = -sun_direction();
    let sun_h = Vec2::new(sun.x, sun.z).normalize();
    // Elevation from -20 to 90 degrees.
    let elev = |r: usize| (-20.0 + 110.0 * (r as f32 / rings as f32).powf(1.4)).to_radians();
    let point = |r: usize, s: usize| {
        let (e, a) = (elev(r), s as f32 / sectors as f32 * std::f32::consts::TAU);
        Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin())
    };
    let color = |p: Vec3| {
        let h = p.y.max(0.0);
        let t = h.powf(0.55);
        let base = mix(palette::SKY_HORIZON, palette::SKY_ZENITH, t);
        // Warm glow near the horizon towards the sun.
        let toward = Vec2::new(p.x, p.z).normalize_or_zero().dot(sun_h).max(0.0);
        let glow = toward.powf(6.0) * (1.0 - t) * 0.35;
        lin(mix(base, Color::srgb(1.0, 0.95, 0.85), glow))
    };
    for r in 0..rings {
        for s in 0..sectors {
            let q = [point(r, s), point(r, s + 1), point(r + 1, s + 1), point(r + 1, s)];
            let c = q.map(color);
            b.quad_c(q.map(|p| p * SKY_RADIUS), c);
        }
    }
    b.build()
}

/// Keep the sky centred on the camera.
fn follow_camera(cams: Query<&GlobalTransform, With<WorldCamera>>, mut sky: Query<&mut Transform, With<SkyDome>>) {
    let Ok(cam) = cams.single() else { return };
    for mut t in &mut sky {
        t.translation = cam.translation().with_y(0.0);
    }
}

/// Settings per quality level.
pub struct QualitySettings {
    pub shadow_map: usize,
    pub cascades: usize,
    pub ssao: bool,
    pub msaa: Msaa,
    pub smaa: bool,
    pub fxaa: bool,
    /// Fraction of the full tree count to place.
    pub tree_density: f32,
}

pub fn quality_settings(q: GraphicsQuality) -> QualitySettings {
    match q {
        GraphicsQuality::Low => QualitySettings {
            shadow_map: 1024,
            cascades: 2,
            ssao: false,
            msaa: Msaa::Off,
            smaa: false,
            fxaa: true,
            tree_density: 0.35,
        },
        GraphicsQuality::Medium => QualitySettings {
            shadow_map: 2048,
            cascades: 3,
            ssao: false,
            msaa: Msaa::Sample4,
            smaa: false,
            fxaa: false,
            tree_density: 0.65,
        },
        GraphicsQuality::High => QualitySettings {
            shadow_map: 4096,
            cascades: 4,
            ssao: true,
            // SSAO needs MSAA off; SMAA does the edges instead.
            msaa: Msaa::Off,
            smaa: true,
            fxaa: false,
            tree_density: 1.0,
        },
    }
}

/// Camera look that does not depend on quality, added once.
pub fn camera_look() -> impl Bundle {
    let tm = match std::env::var("CITYSIM_GFX_TM").as_deref() {
        Ok("agx") => Tonemapping::AgX,
        Ok("filmic") => Tonemapping::BlenderFilmic,
        Ok("aces") => Tonemapping::AcesFitted,
        Ok("sbdt") => Tonemapping::SomewhatBoringDisplayTransform,
        Ok("reinhard") => Tonemapping::ReinhardLuminance,
        _ => Tonemapping::TonyMcMapface,
    };
    (
        tm,
        DistanceFog {
            color: palette::SKY_HORIZON,
            directional_light_color: Color::srgba(1.0, 0.95, 0.85, 0.25),
            directional_light_exponent: 12.0,
            falloff: FogFalloff::Linear {
                start: 800.0,
                end: 5000.0,
            },
        },
    )
}

fn apply_quality(
    quality: Res<GraphicsQuality>,
    mut commands: Commands,
    cams: Query<Entity, Added<WorldCamera>>,
    all_cams: Query<Entity, With<WorldCamera>>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
) {
    let targets: Vec<Entity> = if quality.is_changed() {
        all_cams.iter().collect()
    } else {
        cams.iter().collect()
    };
    if targets.is_empty() {
        return;
    }
    let s = quality_settings(*quality);
    if shadow_map.size != s.shadow_map {
        shadow_map.size = s.shadow_map;
    }
    for e in targets {
        let mut ec = commands.entity(e);
        ec.insert(s.msaa);
        if s.ssao {
            ec.insert(ScreenSpaceAmbientOcclusion {
                quality_level: ScreenSpaceAmbientOcclusionQualityLevel::Medium,
                radius: 1.6,
                ..default()
            });
        } else {
            ec.remove::<ScreenSpaceAmbientOcclusion>();
        }
        if s.smaa {
            ec.insert(Smaa {
                preset: SmaaPreset::High,
            });
        } else {
            ec.remove::<Smaa>();
        }
        if s.fxaa {
            ec.insert(Fxaa::default());
        } else {
            ec.remove::<Fxaa>();
        }
    }
}

/// Fog and shadow cascades follow the zoom level: close up, crisp shadows
/// over a small area and fog far away; zoomed out, shadows cover the map.
fn tune_fog_and_shadows(
    quality: Res<GraphicsQuality>,
    rigs: Query<&CameraRig>,
    mut fogs: Query<&mut DistanceFog, With<WorldCamera>>,
    mut suns: Query<&mut CascadeShadowConfig, With<Sun>>,
    mut last: Local<Option<(f32, GraphicsQuality)>>,
) {
    let Ok(rig) = rigs.single() else { return };
    let d = rig.current.distance;
    if let Some((prev, q)) = *last
        && q == *quality
        && (prev - d).abs() / d < 0.04
    {
        return;
    }
    *last = Some((d, *quality));
    for mut fog in &mut fogs {
        fog.falloff = FogFalloff::Linear {
            start: d * 1.2 + 500.0,
            end: d * 2.6 + 3200.0,
        };
    }
    let s = quality_settings(*quality);
    for mut cfg in &mut suns {
        *cfg = CascadeShadowConfigBuilder {
            num_cascades: s.cascades,
            minimum_distance: 1.0,
            first_cascade_far_bound: (d * 1.1).max(60.0),
            maximum_distance: (d * 3.2).clamp(300.0, 5000.0),
            overlap_proportion: 0.25,
        }
        .build();
    }
}

use bevy::light::CascadeShadowConfig;
