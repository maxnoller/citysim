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

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::camera::{Hdr, RenderTarget};
use bevy::post_process::bloom::{Bloom, BloomCompositeMode, BloomPrefilter};
use bevy::render::render_resource::{PipelineCache, TextureFormat};
use bevy::render::{Render, RenderApp, RenderSystems};

use super::camera::CameraRig;
use super::city_material::{CityMaterial, DayParams, SkyExt, SkyMaterial, city_material, day_buffer};
use super::mesh_util::MeshBuilder;
use super::noise;
use super::palette::{self, lin, mix};
use crate::shared::{GameSet, GraphicsQuality, WorldCamera};
use bevy::render::storage::ShaderBuffer;

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
    /// Opaque, lit, vertex colours (with night glow, see `city_material`).
    pub lit: Handle<CityMaterial>,
    /// The ground: like `lit`, fully rough.
    pub ground: Handle<CityMaterial>,
    pub sky: Handle<SkyMaterial>,
    /// Time-of-day inputs shared by every city material (`DayParams`).
    pub day: Handle<ShaderBuffer>,
}

#[derive(Component)]
struct SkyDome;

#[derive(Component)]
pub struct Sun;

/// Soft fill light from the ground side.
#[derive(Component)]
pub struct Bounce;

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn finish(&self, app: &mut App) {
        let pending = app.world().get_resource::<PendingPipelines>().cloned();
        if let (Some(pending), Some(render)) = (pending, app.get_sub_app_mut(RenderApp)) {
            render
                .insert_resource(pending)
                .add_systems(Render, count_pending_pipelines.in_set(RenderSystems::Cleanup));
        } else {
            // No renderer (tests): quality applies at once.
            app.world_mut().remove_resource::<PendingPipelines>();
        }
    }

    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(palette::SKY_HORIZON))
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.80, 0.88, 1.0),
                brightness: 760.0,
                ..default()
            })
            .insert_resource(DirectionalLightShadowMap { size: 4096 })
            .init_resource::<PendingPipelines>()
            .init_resource::<QualityApplied>()
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

fn init_materials(
    mut commands: Commands,
    mut materials: ResMut<Assets<CityMaterial>>,
    mut skies: ResMut<Assets<SkyMaterial>>,
    mut buffers: ResMut<Assets<ShaderBuffer>>,
) {
    let day = buffers.add(day_buffer(DayParams::default()));
    let lit = StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.92,
        reflectance: 0.25,
        ..default()
    };
    commands.insert_resource(WorldMaterials {
        lit: materials.add(city_material(lit, day.clone())),
        ground: materials.add(city_material(
            StandardMaterial {
                base_color: Color::WHITE,
                perceptual_roughness: 1.0,
                reflectance: 0.1,
                ..default()
            },
            day.clone(),
        )),
        sky: skies.add(SkyMaterial {
            base: StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                fog_enabled: false,
                cull_mode: None,
                ..default()
            },
            extension: SkyExt::default(),
        }),
        day,
    });
}

fn setup_scene(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, world_mats: Res<WorldMaterials>) {
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(ground_mesh())),
        MeshMaterial3d(world_mats.ground.clone()),
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
        MeshMaterial3d(world_mats.sky.clone()),
        Transform::default(),
        NotShadowCaster,
        NoFrustumCulling,
    ));
    commands.spawn((
        Name::new("Sun"),
        Sun,
        DirectionalLight {
            color: palette::SUN,
            illuminance: 6_200.0,
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
        Bounce,
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
    b.tag = [super::mesh_util::Glow::Ground as u32 as f32, 0.0];
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

/// Sky colour for a unit direction: pale warm haze at the horizon (the fog
/// colour, so the ground melts into it), clear blue a little higher up and a
/// deeper blue overhead, with a soft warm glow towards the sun.
pub fn sky_color(p: Vec3) -> Color {
    let sun = -sun_direction();
    let sun_h = Vec2::new(sun.x, sun.z).normalize();
    let h = p.y.max(0.0);
    let low = (h / 0.22).clamp(0.0, 1.0).powf(0.55);
    let high = ((h - 0.22) / 0.78).clamp(0.0, 1.0);
    let base = if h < 0.22 {
        mix(palette::SKY_HORIZON, palette::SKY_MID, low)
    } else {
        mix(palette::SKY_MID, palette::SKY_ZENITH, high)
    };
    let toward = Vec2::new(p.x, p.z).normalize_or_zero().dot(sun_h).max(0.0);
    let glow = toward.powf(4.0) * (1.0 - low) * 0.45;
    mix(base, palette::SKY_GLOW, glow)
}

fn sky_mesh() -> Mesh {
    let mut b = MeshBuilder::new();
    let sectors = 40;
    // Elevations (degrees): fine steps near the horizon where the gradient is.
    let elevs = [
        -30.0f32, -4.0, 0.0, 1.5, 3.0, 5.0, 7.5, 10.0, 13.0, 17.0, 22.0, 28.0, 36.0, 46.0, 58.0, 72.0, 90.0,
    ];
    let point = |r: usize, s: usize| {
        let (e, a) = (elevs[r].to_radians(), s as f32 / sectors as f32 * std::f32::consts::TAU);
        Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin())
    };
    for r in 0..elevs.len() - 1 {
        for s in 0..sectors {
            let q = [point(r, s), point(r, s + 1), point(r + 1, s + 1), point(r + 1, s)];
            let c = q.map(|p| lin(sky_color(p)));
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
    pub bloom: bool,
    /// Soft drifting cloud shadows on the sunlight.
    pub cloud_shadows: bool,
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
            bloom: false,
            cloud_shadows: false,
        },
        GraphicsQuality::Medium => QualitySettings {
            shadow_map: 2048,
            cascades: 3,
            ssao: false,
            msaa: Msaa::Sample4,
            smaa: false,
            fxaa: false,
            tree_density: 0.65,
            bloom: true,
            cloud_shadows: true,
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
            bloom: true,
            cloud_shadows: true,
        },
    }
}

/// Camera look that does not depend on quality, added once.
pub fn camera_look() -> impl Bundle {
    (
        Tonemapping::TonyMcMapface,
        // A little more punch than the neutral tonemapper gives: slightly
        // richer colour and midtone contrast (the diorama look).
        bevy::render::view::ColorGrading {
            global: bevy::render::view::ColorGradingGlobal {
                post_saturation: 1.1,
                ..default()
            },
            midtones: bevy::render::view::ColorGradingSection {
                contrast: 1.06,
                ..default()
            },
            ..default()
        },
        // Always HDR (bloom only on some qualities): switching quality must
        // not change the main pass's render target format.
        Hdr,
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

/// Pipelines the renderer is still compiling (written by the render world).
#[derive(Resource, Clone, Default)]
struct PendingPipelines(Arc<AtomicUsize>);

fn count_pending_pipelines(cache: Res<PipelineCache>, pending: Res<PendingPipelines>) {
    pending.0.store(cache.waiting_pipelines().count(), Ordering::Relaxed);
}

/// The quality the world camera shows now (the benchmark waits for it).
/// What depends on the quality besides the camera (tree density, road
/// furniture, shadow cascades, cloud shadows) follows this, not the
/// requested `GraphicsQuality`, so everything changes together once the
/// new pipelines are ready.
#[derive(Resource, Default)]
pub(super) struct QualityApplied(pub Option<GraphicsQuality>);

/// The quality drawn now: the applied one, or the requested one where
/// nothing applies it (tests without a renderer).
pub(super) fn drawn_quality(applied: Option<&QualityApplied>, requested: GraphicsQuality) -> GraphicsQuality {
    applied.and_then(|a| a.0).unwrap_or(requested)
}

/// Milliseconds a frame may spend re-meshing for a quality change (trees,
/// road furniture): the rest waits for the next frames.
pub(super) const QUALITY_REMESH_MS: f64 = 2.0;

/// Hidden camera that renders the scene with the requested quality into a
/// small image so its pipelines compile before the main camera switches.
#[derive(Component)]
struct WarmUpCamera;

struct WarmUp {
    entity: Entity,
    quality: GraphicsQuality,
    started: f32,
    frames: u32,
    calm: u32,
}

#[derive(Default)]
struct QualityState {
    camera: Option<Entity>,
    applied: Option<GraphicsQuality>,
    warm: Option<WarmUp>,
    /// Warm-up cameras for the levels not in use, started with the first
    /// frame so their pipelines compile while the loading card shows
    /// (`SceneReady` waits for every pending pipeline).
    prewarm: Vec<WarmUp>,
    /// Levels whose pipelines have been compiled: switching to them needs
    /// no warm-up.
    warmed: Vec<GraphicsQuality>,
    prewarm_started: bool,
}

/// A hidden camera rendering the scene at quality `q` into a small image.
fn spawn_warm_up(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    tf: &Transform,
    proj: &Projection,
    q: GraphicsQuality,
    now: f32,
) -> WarmUp {
    let image = images.add(Image::new_target_texture(320, 180, TextureFormat::Bgra8UnormSrgb, None));
    let mut ec = commands.spawn((
        Name::new("Quality warm-up camera"),
        WarmUpCamera,
        Camera3d::default(),
        Camera {
            order: -10,
            ..default()
        },
        RenderTarget::Image(image.into()),
        proj.clone(),
        *tf,
        camera_look(),
    ));
    insert_quality(&mut ec, &quality_settings(q));
    WarmUp {
        entity: ec.id(),
        quality: q,
        started: now,
        frames: 0,
        calm: 0,
    }
}

/// Put the per-quality camera components on `ec`.
fn insert_quality(ec: &mut EntityCommands, s: &QualitySettings) {
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
    if s.bloom {
        ec.insert(Bloom {
            intensity: 0.1,
            low_frequency_boost: 0.4,
            prefilter: BloomPrefilter {
                threshold: 1.6,
                threshold_softness: 0.4,
            },
            composite_mode: BloomCompositeMode::Additive,
            ..Bloom::NATURAL
        });
    } else {
        ec.remove::<Bloom>();
    }
}

/// Apply `GraphicsQuality` to the world camera. Changing MSAA / SSAO makes
/// the renderer re-specialise every pipeline, and while they compile nothing
/// is drawn (round 1 showed a white frame for seconds). So a switch first
/// renders the new settings on a hidden warm-up camera and moves the main
/// camera over only once no pipeline is pending any more.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn apply_quality(
    quality: Res<GraphicsQuality>,
    mut commands: Commands,
    cams: Query<(Entity, &Transform, &Projection), (With<WorldCamera>, Without<WarmUpCamera>)>,
    mut warm_cams: Query<&mut Transform, (With<WarmUpCamera>, Without<WorldCamera>)>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    pending: Option<Res<PendingPipelines>>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time<Real>>,
    mut state: Local<QualityState>,
    mut applied: ResMut<QualityApplied>,
) {
    let Ok((cam, cam_tf, proj)) = cams.single() else {
        return;
    };
    let want = *quality;
    let now = time.elapsed_secs();
    let apply = |commands: &mut Commands, shadow_map: &mut DirectionalLightShadowMap, q: GraphicsQuality| {
        let s = quality_settings(q);
        if shadow_map.size != s.shadow_map {
            shadow_map.size = s.shadow_map;
        }
        insert_quality(&mut commands.entity(cam), &s);
    };
    // A new camera (or the very first frame): apply at once.
    if state.camera != Some(cam) || pending.is_none() {
        state.camera = Some(cam);
        if state.applied != Some(want) || pending.is_none() {
            apply(&mut commands, &mut shadow_map, want);
        }
        state.applied = Some(want);
        applied.0 = Some(want);
        if !state.warmed.contains(&want) {
            state.warmed.push(want);
        }
        return;
    }
    let waiting = pending.as_ref().map_or(0, |p| p.0.load(Ordering::Relaxed));
    // Compile the other levels' pipelines ahead, once, from the start.
    if !state.prewarm_started {
        state.prewarm_started = true;
        for q in [GraphicsQuality::Low, GraphicsQuality::Medium, GraphicsQuality::High] {
            if !state.warmed.contains(&q) {
                let w = spawn_warm_up(&mut commands, &mut images, cam_tf, proj, q, now);
                state.prewarm.push(w);
            }
        }
    }
    if !state.prewarm.is_empty() {
        for w in &mut state.prewarm {
            if let Ok(mut t) = warm_cams.get_mut(w.entity) {
                *t = *cam_tf;
            }
            w.frames += 1;
            w.calm = if waiting == 0 { w.calm + 1 } else { 0 };
        }
        let w = &state.prewarm[0];
        if (w.frames >= 4 && w.calm >= 3) || now - w.started > 30.0 {
            for w in std::mem::take(&mut state.prewarm) {
                info!("graphics quality {:?} prepared after {} frames", w.quality, w.frames);
                commands.entity(w.entity).despawn();
                if !state.warmed.contains(&w.quality) {
                    state.warmed.push(w.quality);
                }
            }
        }
    }
    if state.applied == Some(want) {
        if let Some(w) = state.warm.take() {
            commands.entity(w.entity).despawn();
        }
        return;
    }
    // Prepared ahead: switch at once.
    if state.warmed.contains(&want) {
        if let Some(w) = state.warm.take() {
            commands.entity(w.entity).despawn();
        }
        info!("graphics quality {want:?} applied (prepared)");
        apply(&mut commands, &mut shadow_map, want);
        state.applied = Some(want);
        applied.0 = Some(want);
        return;
    }
    // Start (or restart) warming up the requested quality.
    if state.warm.as_ref().is_none_or(|w| w.quality != want) {
        if let Some(w) = state.warm.take() {
            commands.entity(w.entity).despawn();
        }
        state.warm = Some(spawn_warm_up(&mut commands, &mut images, cam_tf, proj, want, now));
        return;
    }
    let Some(w) = state.warm.as_mut() else { return };
    if let Ok(mut t) = warm_cams.get_mut(w.entity) {
        *t = *cam_tf;
    }
    w.frames += 1;
    w.calm = if waiting == 0 { w.calm + 1 } else { 0 };
    if (w.frames >= 4 && w.calm >= 3) || now - w.started > 20.0 {
        let entity = w.entity;
        info!("graphics quality {want:?} applied after {} warm-up frames", w.frames);
        state.warm = None;
        commands.entity(entity).despawn();
        apply(&mut commands, &mut shadow_map, want);
        state.applied = Some(want);
        applied.0 = Some(want);
        state.warmed.push(want);
    }
}

/// Fog and shadow cascades follow the zoom level: close up, crisp shadows
/// over a small area and fog far away; zoomed out, shadows cover the map.
fn tune_fog_and_shadows(
    requested: Res<GraphicsQuality>,
    applied: Res<QualityApplied>,
    rigs: Query<&CameraRig>,
    mut fogs: Query<&mut DistanceFog, With<WorldCamera>>,
    mut suns: Query<&mut CascadeShadowConfig, With<Sun>>,
    mut last: Local<Option<(f32, GraphicsQuality)>>,
) {
    let Ok(rig) = rigs.single() else { return };
    let quality = drawn_quality(Some(&applied), *requested);
    let d = rig.current.distance;
    if let Some((prev, q)) = *last
        && q == quality
        && (prev - d).abs() / d < 0.04
    {
        return;
    }
    *last = Some((d, quality));
    for mut fog in &mut fogs {
        fog.falloff = FogFalloff::Linear {
            start: d * 1.6 + 1200.0,
            end: d * 2.2 + 6500.0,
        };
    }
    let s = quality_settings(quality);
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
