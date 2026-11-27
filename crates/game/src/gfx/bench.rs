//! Windowless benchmark: `CITYSIM_BENCH=out.json`.
//!
//! The game runs without a window (`main.rs` drops winit and runs the app
//! loop itself); the world camera renders to an off-screen image
//! (`CITYSIM_BENCH_SIZE`, default 1920x1080) along a fixed camera path over
//! the city (or a fixed `CITYSIM_BENCH_POSE=x,z,yaw,pitch,distance`),
//! frame by frame (`CITYSIM_BENCH_FRAMES`, default 1500, after
//! `CITYSIM_BENCH_WARMUP` frames, default 240), with the simulation running
//! at 1x (`CITYSIM_BENCH_SPEED=i`: speed index, 3 = the top speed). Use it with a demo city (`CITYSIM_DEMO=town|city|hierarchy`) and
//! `CITYSIM_GFX_QUALITY=low|medium|high`. `CITYSIM_BENCH_UI=1` draws the
//! interface into the image as well (it is not drawn otherwise: there is no
//! window); with `CITYSIM_PERF=off|small|full` that measures the
//! performance panel's own cost. `CITYSIM_BENCH_GPU=0` leaves GPU pass
//! timing (and draw counts) to the panel (`perf.rs`: on only while it is
//! full), as in a game.
//!
//! Without a swapchain nothing would hold the CPU back from queueing frames
//! faster than the GPU draws them, so the render world waits (after each
//! submit) until the frame before the last one has finished on the GPU: two
//! frames in flight, as with a real swapchain.
//!
//! The report has the average, median, 99th percentile ("worst 1 %") and
//! worst frame times, the GPU time of each render pass (timestamp queries,
//! `RenderDiagnosticsPlugin`, added by `perf.rs`), triangles rasterised in the main and shadow
//! passes (pipeline statistics), and entity, mesh, material, light and
//! draw-call counts (draw calls counted from the render phases: one per
//! multi-draw batch set, batchable bin and unbatchable entity, per view).
//!
//! Hitches: every frame is recorded with the main world's CPU time (from
//! `First` to `Last`), the render thread's (`perf.rs`), the simulation's,
//! the mesh and material assets added, changed or removed, the render
//! pipelines created and still compiling, and tags for what happened in it
//! (a script action, the simulation changing roads, zones or buildings, a
//! quality switch applied). The report lists the 20 slowest frames with
//! their tags and how many frames went over 33 and 50 ms, and the frames go
//! to `<out>.frames.csv` (one line each). `CITYSIM_BENCH_SCRIPT=quality|
//! actions` runs a scripted test instead of the plain camera path (see
//! `bench_script.rs`).

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use std::sync::{Arc, Mutex};

use bevy::camera::RenderTarget;
use bevy::diagnostic::DiagnosticsStore;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::render::render_resource::{PipelineCache, PollType, TextureFormat};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};

use super::bench_script::{self as script, Act, ScriptKind, Street, Switch, ToolAct, ToolSite};
use super::city_material::CityMaterial;
use super::daynight::PinnedHour;
use super::scene::QualityApplied;
use crate::shared::{
    ActiveTool, BulldozeMode, CameraOverride, CameraPose, GameSet, GraphicsQuality, OverlayMode, RoadChoice, RoadMode,
    SceneReady, ScriptPointer, SimClock, SimCommand, SimStep, SimWorld, ToolModes, WorldCamera, WorldHighlight,
    ZoneMode,
};

/// `CITYSIM_BENCH` is set: the app runs windowless (see `main.rs`).
pub fn enabled() -> bool {
    std::env::var_os("CITYSIM_BENCH").is_some_and(|v| !v.is_empty())
}

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

pub struct BenchPlugin;

impl Plugin for BenchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolDrawTime>();
        if !enabled() {
            return;
        }
        let path = std::env::var("CITYSIM_BENCH").unwrap_or_default();
        let (w, h) = std::env::var("CITYSIM_BENCH_SIZE")
            .ok()
            .and_then(|s| {
                let (a, b) = s.split_once('x')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            })
            .unwrap_or((1920u32, 1080u32));
        // Render diagnostics and draw counts: `perf.rs` (always on here).
        let script = ScriptKind::from_env();
        let frames = match script {
            Some(k @ (ScriptKind::Actions | ScriptKind::Tools)) => script::actions_frames(k),
            // Ends when the last switch is done.
            Some(ScriptKind::Quality) => u32::MAX,
            None => env("CITYSIM_BENCH_FRAMES").unwrap_or(1500),
        };
        let pipelines = Pipelines::default();
        app.insert_resource(Bench {
            path,
            size: (w, h),
            warmup: env("CITYSIM_BENCH_WARMUP").unwrap_or(240),
            frames,
            script,
            quality: match std::env::var("CITYSIM_GFX_QUALITY").as_deref() {
                Ok("low") => GraphicsQuality::Low,
                Ok("medium") => GraphicsQuality::Medium,
                _ => GraphicsQuality::High,
            },
            paused: env::<u32>("CITYSIM_BENCH_PAUSED") == Some(1),
            speed: env::<usize>("CITYSIM_BENCH_SPEED").unwrap_or(0),
            fixed_pose: std::env::var("CITYSIM_BENCH_POSE").ok().and_then(|s| {
                let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (v.len() == 5).then(|| CameraPose {
                    focus: Vec3::new(v[0], 0.0, v[1]),
                    yaw: v[2],
                    pitch: v[3],
                    distance: v[4],
                })
            }),
            ui: env::<u32>("CITYSIM_BENCH_UI") == Some(1),
            ..default()
        })
        .init_resource::<SimTiming>()
        .init_resource::<MainTiming>()
        .init_resource::<MainMarks>()
        .insert_resource(pipelines.clone())
        .add_systems(First, main_start)
        .add_systems(Last, main_end)
        .add_systems(
            Update,
            (
                main_mark::<1>.before(GameSet::Input),
                main_mark::<2>.after(GameSet::Hover).before(GameSet::Tools),
                main_mark::<3>.after(GameSet::Tools).before(GameSet::Sim),
                main_mark::<4>.after(GameSet::Sim).before(GameSet::Render),
                main_mark::<5>.after(GameSet::Render),
            ),
        )
        .add_systems(PreUpdate, press_keys.after(InputSystems))
        .add_systems(FixedUpdate, sim_start.before(SimStep))
        .add_systems(FixedUpdate, sim_end.after(SimStep))
        .add_systems(Update, (drive, run_script, record).chain().in_set(GameSet::Input));
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .init_resource::<InFlight>()
                .insert_resource(pipelines)
                .add_systems(
                    Render,
                    (
                        render_mark::<0>.before(RenderSystems::ExtractCommands),
                        render_mark::<1>
                            .after(RenderSystems::PrepareAssets)
                            .before(RenderSystems::PrepareMeshes),
                        render_mark::<2>
                            .after(RenderSystems::Specialize)
                            .before(RenderSystems::PrepareViews),
                        render_mark::<3>
                            .after(RenderSystems::Queue)
                            .before(RenderSystems::PhaseSort),
                        render_mark::<4>
                            .after(RenderSystems::PhaseSort)
                            .before(RenderSystems::Prepare),
                        render_mark::<5>
                            .after(RenderSystems::Prepare)
                            .before(RenderSystems::Render),
                        render_mark::<6>
                            .after(RenderSystems::Render)
                            .before(RenderSystems::Cleanup)
                            .before(throttle_gpu),
                        throttle_gpu.after(RenderSystems::Render).before(RenderSystems::Cleanup),
                        count_pipelines.in_set(RenderSystems::Cleanup),
                    ),
                );
        }
    }
}

/// Time spent in `World::step` (the `SimStep` set): (start of the current
/// step, per-step durations in ms since the last frame).
#[derive(Resource, Default)]
struct SimTiming(Option<Instant>, Vec<f64>);

fn sim_start(mut t: ResMut<SimTiming>) {
    t.0 = Some(Instant::now());
}

fn sim_end(mut t: ResMut<SimTiming>) {
    if let Some(s) = t.0.take() {
        t.1.push(s.elapsed().as_secs_f64() * 1000.0);
    }
}

/// The main world's update: (start of this one, length of the last, ms).
#[derive(Resource, Default)]
struct MainTiming(Option<Instant>, f64);

fn main_start(mut t: ResMut<MainTiming>, mut m: ResMut<MainMarks>) {
    let now = Instant::now();
    t.0 = Some(now);
    m.0[0] = Some(now);
}

fn main_end(mut t: ResMut<MainTiming>, mut m: ResMut<MainMarks>) {
    if let Some(s) = t.0.take() {
        t.1 = s.elapsed().as_secs_f64() * 1000.0;
    }
    let last = MAIN_PARTS.len() - 1;
    if let Some(prev) = m.0[last] {
        m.1[last] = prev.elapsed().as_secs_f64() * 1000.0;
    }
    m.2 = m.1;
}

/// CPU time gfx spent this frame drawing the tools' feedback (the road
/// tool's preview, the cursor, guides and highlights, trees cleared under a
/// road being drawn), nanoseconds: each of those systems adds its own time
/// (shared access, so they still run side by side).
#[derive(Resource, Default)]
pub struct ToolDrawTime(std::sync::atomic::AtomicU64);

/// Adds the time until it is dropped to a [`ToolDrawTime`].
pub struct Timed<'a>(&'a ToolDrawTime, Instant);

impl ToolDrawTime {
    pub fn time(&self) -> Timed<'_> {
        Timed(self, Instant::now())
    }

    /// The time added since the last call, ms.
    fn take_ms(&self) -> f64 {
        self.0.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 1e6
    }
}

impl Drop for Timed<'_> {
    fn drop(&mut self) {
        let ns = self.1.elapsed().as_nanos() as u64;
        self.0.0.fetch_add(ns, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Parts of the render world's frame, ms: extract commands and asset
/// preparation, views and specialisation, queueing, sorting, preparing
/// (buffers, batches, bind groups), the render graph with the submit.
const RENDER_PARTS: [&str; 6] = ["assets", "specialize", "queue", "sort", "prepare", "graph"];
/// Parts of the main world's update, ms: from `First` up to the game's
/// `Update` sets (input, fixed-step simulation), `GameSet::{Input, Hover}`
/// (camera, the bench itself, hover), `GameSet::Tools` (the tools: their
/// plans and previews), `GameSet::Sim` (commands applied),
/// `GameSet::Render` (gfx rebuilds, HUD), and the rest (`PostUpdate`:
/// transforms, visibility, UI layout; `Last`).
const MAIN_PARTS: [&str; 6] = ["pre", "input", "tools", "sim_cmds", "render_set", "post"];

/// What the render world measured in its last frame.
#[derive(Default, Clone, Copy)]
struct RenderSide {
    /// Render pipelines created so far, still compiling.
    created: usize,
    compiling: usize,
    /// Time stamps of the frame's parts (see `RENDER_PARTS`).
    marks: [Option<Instant>; 7],
    parts: [f64; 6],
    /// Waiting for the GPU to finish the frame before the last, ms.
    gpu_wait_ms: f64,
}

#[derive(Resource, Clone, Default)]
struct Pipelines(Arc<Mutex<RenderSide>>);

fn count_pipelines(cache: Res<PipelineCache>, out: Res<Pipelines>) {
    if let Ok(mut o) = out.0.lock() {
        o.created = cache.pipelines().count();
        o.compiling = cache.waiting_pipelines().count();
    }
}

fn render_mark<const I: usize>(out: Res<Pipelines>) {
    if let Ok(mut o) = out.0.lock() {
        let now = Instant::now();
        o.marks[I] = Some(now);
        if I > 0
            && let Some(prev) = o.marks[I - 1]
        {
            o.parts[I - 1] = (now - prev).as_secs_f64() * 1000.0;
        }
    }
}

/// Time stamps of the main world's parts (see `MAIN_PARTS`), the parts of
/// this update and of the last whole one.
#[derive(Resource, Default)]
struct MainMarks([Option<Instant>; 6], [f64; 6], [f64; 6]);

fn main_mark<const I: usize>(mut m: ResMut<MainMarks>) {
    let now = Instant::now();
    if I > 0
        && let Some(prev) = m.0[I - 1]
    {
        m.1[I - 1] = (now - prev).as_secs_f64() * 1000.0;
    }
    m.0[I] = Some(now);
}

/// Keys the script presses this frame (released the frame after).
#[derive(Default)]
struct Keys {
    press: Vec<KeyCode>,
    held: Vec<KeyCode>,
    /// Mouse buttons to press / let go of this frame (held until let go).
    mouse_press: Vec<MouseButton>,
    mouse_release: Vec<MouseButton>,
}

fn press_keys(
    mut bench: ResMut<Bench>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
) {
    for b in std::mem::take(&mut bench.keys.mouse_release) {
        mouse.release(b);
    }
    for b in std::mem::take(&mut bench.keys.mouse_press) {
        mouse.press(b);
    }
    for k in std::mem::take(&mut bench.keys.held) {
        keys.release(k);
    }
    let press = std::mem::take(&mut bench.keys.press);
    for &k in &press {
        keys.press(k);
    }
    bench.keys.held = press;
}

/// One measured frame.
#[derive(Default, Clone, Debug)]
struct FrameRec {
    /// Frame to frame, ms.
    ms: f64,
    /// Main world update, render thread CPU, simulation steps, ms.
    main_ms: f64,
    render_ms: f64,
    sim_ms: f64,
    /// Mesh and material assets added, changed or removed.
    meshes: u32,
    materials: u32,
    /// Render pipelines created since the frame before, still compiling.
    new_pipelines: usize,
    compiling: usize,
    /// See `MAIN_PARTS` / `RENDER_PARTS`.
    main_parts: [f64; 6],
    /// gfx drawing the tools' feedback ([`ToolDrawTime`]), ms.
    tool_draw_ms: f64,
    /// Commands applied (`shared::CommandTime`), ms.
    cmd_ms: f64,
    render_parts: [f64; 6],
    gpu_wait_ms: f64,
    tags: Vec<String>,
}

/// Submissions the GPU may still be working on (oldest first), as closures
/// that block until theirs has finished.
#[derive(Resource, Default)]
struct InFlight(VecDeque<Box<dyn FnOnce() + Send + Sync>>);

fn throttle_gpu(
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut in_flight: ResMut<InFlight>,
    side: Res<Pipelines>,
) {
    let start = Instant::now();
    // An empty submit returns the index of everything submitted so far.
    let index = queue.submit([]);
    let device = device.clone();
    in_flight.0.push_back(Box::new(move || {
        let _ = device.poll(PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        });
    }));
    while in_flight.0.len() > 2 {
        if let Some(wait) = in_flight.0.pop_front() {
            wait();
        }
    }
    if let Ok(mut o) = side.0.lock() {
        o.gpu_wait_ms = start.elapsed().as_secs_f64() * 1000.0;
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Loading,
    Warmup,
    Measure,
    Done,
}

#[derive(Resource, Default)]
struct Bench {
    path: String,
    size: (u32, u32),
    warmup: u32,
    frames: u32,
    quality: GraphicsQuality,
    /// `CITYSIM_BENCH_PAUSED=1`: the simulation stands still (to separate
    /// its cost from drawing).
    paused: bool,
    /// `CITYSIM_BENCH_SPEED=i`: index into `SPEEDS` (default 0, 1x).
    speed: usize,
    /// `CITYSIM_BENCH_UI=1`: the interface is drawn into the image too
    /// (without a window it is not drawn at all otherwise).
    ui: bool,
    script: Option<ScriptKind>,
    phase: Phase,
    /// Every measured frame.
    recs: Vec<FrameRec>,
    /// Tags for the frame being recorded next.
    tags: Vec<String>,
    keys: Keys,
    /// The script's camera pose for this frame (else the path's).
    pose: Option<CameraPose>,
    /// `CITYSIM_BENCH_POSE=x,z,yaw,pitch,distance`: a fixed camera instead
    /// of the path (to measure one view).
    fixed_pose: Option<CameraPose>,
    /// `quality` script: the switches so far.
    switches: Vec<Switch>,
    /// `actions` script: next action (`quality`: next level in the order),
    /// the street it built, the hour pin
    /// animation (from, by, start frame), the zoom's start frame and pose.
    next_act: usize,
    street: Option<Street>,
    hours: Option<(f32, f32, u32)>,
    zoom: Option<(u32, CameraPose)>,
    /// Where each action's frames start (record index, name).
    acts: Vec<(usize, String)>,
    /// The tool acts: where they work, the act in progress (with the frame
    /// it started), its camera pose and where the pointer is (sim metres).
    site: Option<ToolSite>,
    tool_act: Option<(ToolAct, u32)>,
    tool_pose: Option<CameraPose>,
    pointer_at: Option<Vec2>,
    /// The image the world camera renders to.
    image: Option<Handle<Image>>,
    /// `Pan`: the pointer's window position, held still.
    pointer_px: Option<Vec2>,
    tool_log_at: u32,
    versions: Option<(u64, u64, u64, usize)>,
    pipelines_seen: usize,
    frame: u32,
    started: Option<Instant>,
    last: Option<Instant>,
    /// Frame times, milliseconds.
    times: Vec<f64>,
    /// Per diagnostic path: (sum, count of frames it was seen in).
    diags: BTreeMap<String, (f64, u32)>,
    /// Newest measurement time seen per path.
    seen: BTreeMap<String, Instant>,
    /// Sums of per-frame counters: visible meshes, draw calls.
    sums: Sums,
    max: Sums,
    city: Option<(Vec2, f32)>,
    /// Duration of each sim step while measuring, ms.
    sim_ms: Vec<f64>,
    sim_ticks: (u64, u64),
}

#[derive(Default, Clone, Copy, Debug)]
struct Sums {
    visible: f64,
    /// Triangles of the visible mesh entities (in any view: the camera or
    /// a shadow cascade), thousands.
    visible_tris: f64,
    /// Triangles of the meshes the world camera itself draws (after
    /// frustum culling and level-of-detail ranges), thousands.
    camera_tris: f64,
    main_draws: f64,
    shadow_draws: f64,
    shadow_views: f64,
}

impl Sums {
    fn max(&mut self, o: &Sums) {
        self.visible = self.visible.max(o.visible);
        self.visible_tris = self.visible_tris.max(o.visible_tris);
        self.camera_tris = self.camera_tris.max(o.camera_tris);
        self.main_draws = self.main_draws.max(o.main_draws);
        self.shadow_draws = self.shadow_draws.max(o.shadow_draws);
        self.shadow_views = self.shadow_views.max(o.shadow_views);
    }
}

/// Centre and radius of the built-up area.
fn city_extent(sim: &SimWorld) -> (Vec2, f32) {
    let w = &sim.world;
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for (_, b) in w.buildings.iter() {
        lo = lo.min(Vec2::from(b.center()));
        hi = hi.max(Vec2::from(b.center()));
    }
    if lo.x > hi.x {
        let c = Vec2::splat(citysim_sim::MAP_SIZE_M * 0.5);
        return (c, 300.0);
    }
    ((lo + hi) * 0.5, ((hi - lo) * 0.5).length().max(150.0))
}

/// The fixed camera path at `u` in 0..=1: from the whole city high up,
/// down to street level through the centre, along a street, and back out to
/// a low wide view towards the horizon (the most expensive kind of view).
fn path_pose(u: f32, centre: Vec2, radius: f32) -> CameraPose {
    // (u, focus offset x, z as fractions of radius, yaw, pitch, distance or
    // negative = fraction of radius)
    const KEYS: [(f32, f32, f32, f32, f32, f32); 7] = [
        (0.00, 0.0, 0.0, 0.3, 1.05, -1.25),
        (0.18, 0.1, 0.05, 0.9, 0.85, 420.0),
        (0.36, 0.05, 0.0, 1.6, 0.34, 70.0),
        (0.52, -0.35, 0.1, 2.3, 0.36, 90.0),
        (0.66, -0.2, -0.2, 3.0, 0.55, 220.0),
        (0.84, 0.0, 0.0, 3.6, 0.5, -0.9),
        (1.00, 0.2, 0.1, 4.4, 0.75, -1.1),
    ];
    let k = KEYS.iter().rposition(|f| f.0 <= u).unwrap_or(0).min(KEYS.len() - 2);
    let (a, b) = (KEYS[k], KEYS[k + 1]);
    let t = ((u - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    let l = |x: f32, y: f32| x + (y - x) * t;
    let dist = |d: f32| if d < 0.0 { -d * radius } else { d };
    let focus = centre + Vec2::new(l(a.1, b.1), l(a.2, b.2)) * radius;
    CameraPose {
        focus: Vec3::new(focus.x, 0.0, focus.y),
        yaw: l(a.3, b.3),
        pitch: l(a.4, b.4),
        distance: (dist(a.5).ln() + (dist(b.5).ln() - dist(a.5).ln()) * t).exp(),
    }
}

/// Where `CITYSIM_BENCH_SHOTS` saves measured frame `frame`, if it does.
fn shot_at(frame: u32) -> Option<String> {
    let list = std::env::var("CITYSIM_BENCH_SHOTS").ok()?;
    list.split(';').find_map(|e| {
        let (f, p) = e.split_once('=')?;
        (f.trim().parse::<u32>().ok()? == frame).then(|| p.trim().to_string())
    })
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn drive(
    mut commands: Commands,
    mut bench: ResMut<Bench>,
    mut sim: ResMut<SimWorld>,
    ready: Res<SceneReady>,
    mut clock: ResMut<SimClock>,
    mut quality: ResMut<GraphicsQuality>,
    mut camera: ResMut<CameraOverride>,
    mut images: ResMut<Assets<Image>>,
    cams: Query<(Entity, Option<&RenderTarget>), With<WorldCamera>>,
    cam_vis: Query<&bevy::camera::visibility::VisibleEntities, With<WorldCamera>>,
    store: Res<DiagnosticsStore>,
    counts: Res<super::perf::RenderShared>,
    mut sim_timing: ResMut<SimTiming>,
    queries: (
        Query<Entity>,
        Query<(&Mesh3d, &ViewVisibility)>,
        Query<(), Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>>,
    ),
    assets: (
        Res<Assets<Mesh>>,
        Res<Assets<StandardMaterial>>,
        Res<Assets<CityMaterial>>,
    ),
    mut exit: MessageWriter<AppExit>,
) {
    let now = Instant::now();
    let dt = bench.last.map(|l| (now - l).as_secs_f64() * 1000.0);
    bench.last = Some(now);
    quality.set_if_neq(bench.quality);
    clock.paused = bench.paused;
    clock.speed = bench.speed;
    // Point the world camera at an off-screen image of the benchmark size.
    for (cam, target) in &cams {
        if !matches!(target, Some(RenderTarget::Image(_))) {
            let (w, h) = bench.size;
            let image = images.add(Image::new_target_texture(w, h, TextureFormat::Bgra8UnormSrgb, None));
            bench.image = Some(image.clone());
            commands.entity(cam).insert(RenderTarget::Image(image.into()));
            if bench.ui {
                commands.entity(cam).insert(bevy::ui::IsDefaultUiCamera);
            }
        }
    }
    let (centre, radius) = *bench.city.get_or_insert_with(|| city_extent(&sim));
    let total = bench.frames.max(1);
    let u = match bench.phase {
        Phase::Measure => bench.frame as f32 / total as f32,
        _ => 0.0,
    };
    camera.0 = Some(
        bench
            .pose
            .take()
            .or(bench.fixed_pose)
            .unwrap_or_else(|| path_pose(u, centre, radius)),
    );

    match bench.phase {
        Phase::Loading => {
            if ready.0 {
                if matches!(bench.script, Some(ScriptKind::Actions | ScriptKind::Tools)) {
                    let (nu, nv) = script::grid_blocks();
                    let t = Instant::now();
                    bench.site = script::find_site(&sim.world, centre, nu, nv);
                    // Money for everything the tool acts build (a player
                    // with a large grid in mind has it).
                    if bench.site.is_some() && sim.world.economy.money < script::TOOL_MONEY {
                        sim.world.economy.money = script::TOOL_MONEY;
                    }
                    match &bench.site {
                        Some(s) => info!(
                            "bench: tool site: grid {nu} x {nv} at {:?} (found in {:.0} ms)",
                            s.a,
                            t.elapsed().as_secs_f64() * 1e3
                        ),
                        None => warn!("bench: no free land for a {nu} x {nv} grid: the tool acts are skipped"),
                    }
                }
                info!("bench: scene ready, warming up {} frames", bench.warmup);
                bench.phase = Phase::Warmup;
                bench.frame = 0;
            }
        }
        Phase::Warmup => {
            sim_timing.1.clear();
            bench.frame += 1;
            if bench.frame >= bench.warmup {
                info!("bench: measuring {} frames", bench.frames);
                bench.phase = Phase::Measure;
                bench.frame = 0;
                bench.started = Some(now);
                bench.sim_ticks.0 = sim.world.time.tick;
                // Only diagnostics newer than this count.
                let seen: Vec<(String, Instant)> = store
                    .iter()
                    .filter_map(|d| Some((d.path().to_string(), d.measurement()?.time)))
                    .collect();
                bench.seen = seen.into_iter().collect();
            }
        }
        Phase::Measure => {
            // `CITYSIM_BENCH_SHOTS="frame=path;..."`: save the image at
            // those measured frames.
            if let Some(path) = shot_at(bench.frame)
                && let Some(image) = bench.image.clone()
            {
                info!("bench: frame {} saved to {path}", bench.frame);
                commands
                    .spawn(bevy::render::view::screenshot::Screenshot::image(image))
                    .observe(bevy::render::view::screenshot::save_to_disk(path));
            }
            if let Some(dt) = dt
                && bench.frame > 0
            {
                bench.times.push(dt);
                bench.recs.push(FrameRec { ms: dt, ..default() });
            }
            bench.frame += 1;
            let steps = std::mem::take(&mut sim_timing.1);
            if let Some(r) = bench.recs.last_mut() {
                r.sim_ms = steps.iter().sum();
            }
            bench.sim_ms.extend(steps);
            // Render diagnostics: sum every new measurement per path.
            let mut new: Vec<(String, f64, Instant)> = Vec::new();
            for d in store.iter() {
                let path = d.path().to_string();
                if !path.starts_with("render/") {
                    continue;
                }
                let since = bench.seen.get(&path).copied();
                let mut sum = 0.0;
                let mut newest = since;
                for m in d.measurements() {
                    if since.is_none_or(|s| m.time > s) {
                        sum += m.value;
                        newest = Some(newest.map_or(m.time, |n: Instant| n.max(m.time)));
                    }
                }
                if let Some(n) = newest
                    && Some(n) != since
                {
                    new.push((path, sum, n));
                }
            }
            for (path, sum, n) in new {
                let e = bench.diags.entry(path.clone()).or_default();
                e.0 += sum;
                e.1 += 1;
                bench.seen.insert(path, n);
            }
            let c = counts.0.lock().map(|c| c.last).unwrap_or_default();
            let visible = queries.1.iter().filter(|(_, v)| v.get()).count();
            let visible_tris: usize = queries
                .1
                .iter()
                .filter(|(_, v)| v.get())
                .filter_map(|(m, _)| {
                    assets
                        .0
                        .get(&m.0)
                        .map(|mesh| super::mesh_util::mesh_triangles(m.0.id(), mesh))
                })
                .sum();
            let camera_tris: usize = cam_vis
                .iter()
                .flat_map(|v| v.get(std::any::TypeId::of::<Mesh3d>()).iter().copied())
                .filter_map(|e| queries.1.get(e).ok())
                .filter_map(|(m, _)| {
                    assets
                        .0
                        .get(&m.0)
                        .map(|mesh| super::mesh_util::mesh_triangles(m.0.id(), mesh))
                })
                .sum();
            let frame = Sums {
                visible: visible as f64,
                visible_tris: visible_tris as f64 / 1e3,
                camera_tris: camera_tris as f64 / 1e3,
                main_draws: c.main as f64,
                shadow_draws: c.shadow as f64,
                shadow_views: c.shadow_views as f64,
            };
            bench.sums.visible += frame.visible;
            bench.sums.visible_tris += frame.visible_tris;
            bench.sums.camera_tris += frame.camera_tris;
            bench.sums.main_draws += frame.main_draws;
            bench.sums.shadow_draws += frame.shadow_draws;
            bench.sums.shadow_views += frame.shadow_views;
            bench.max.max(&frame);
            if bench.frame >= total || bench.script_done() {
                bench.phase = Phase::Done;
                bench.sim_ticks.1 = sim.world.time.tick;
                let report = report(&bench, &sim, &queries, &assets);
                match std::fs::write(&bench.path, &report) {
                    Ok(()) => info!("bench: wrote {}", bench.path),
                    Err(e) => error!("bench: cannot write {}: {e}", bench.path),
                }
                let csv = format!("{}.frames.csv", bench.path.trim_end_matches(".json"));
                if let Err(e) = std::fs::write(&csv, frames_csv(&bench)) {
                    error!("bench: cannot write {csv}: {e}");
                }
                println!("{report}");
                exit.write(AppExit::Success);
            }
        }
        Phase::Done => {}
    }
}

#[allow(clippy::type_complexity)]
fn report(
    bench: &Bench,
    sim: &SimWorld,
    queries: &(
        Query<Entity>,
        Query<(&Mesh3d, &ViewVisibility)>,
        Query<(), Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>>,
    ),
    assets: &(
        Res<Assets<Mesh>>,
        Res<Assets<StandardMaterial>>,
        Res<Assets<CityMaterial>>,
    ),
) -> String {
    let mut sorted = bench.times.clone();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len().max(1) as f64;
    let avg = sorted.iter().sum::<f64>() / n;
    // Worst 1 %: the average of the slowest 1 % of frames.
    let worst_n = (sorted.len() / 100).max(1);
    let worst_1 = sorted.iter().rev().take(worst_n).sum::<f64>() / worst_n as f64;
    let frames = bench.frame.max(1) as f64;
    let mut gpu = String::new();
    let mut tris = String::new();
    for (path, (sum, _)) in &bench.diags {
        let per_frame = sum / frames;
        let name = path.trim_start_matches("render/");
        if let Some(pass) = name.strip_suffix("/elapsed_gpu") {
            gpu.push_str(&format!("    \"{pass}\": {per_frame:.3},\n"));
        } else if let Some(pass) = name.strip_suffix("/clipper_primitives_out") {
            tris.push_str(&format!("    \"{pass}\": {:.0},\n", per_frame));
        }
    }
    let trim = |s: String| s.trim_end_matches(",\n").to_string();
    let mut steps = bench.sim_ms.clone();
    steps.sort_by(f64::total_cmp);
    let step_avg = steps.iter().sum::<f64>() / steps.len().max(1) as f64;
    let sim_per_frame = steps.iter().sum::<f64>() / frames;
    let w = &sim.world;
    let secs = bench.times.iter().sum::<f64>() / 1000.0;
    let sim_rate = (bench.sim_ticks.1 - bench.sim_ticks.0) as f64 / secs.max(1e-6);
    // Geometry held by every mesh asset (vertex and index buffers).
    let (mut mesh_bytes, mut mesh_tris) = (0usize, 0usize);
    for (_, m) in assets.0.iter() {
        // Meshes kept only on the GPU are counted from `gpu_mesh_stats`.
        if let Ok(i) = m.try_indices_option() {
            mesh_bytes += m.get_vertex_buffer_size() + m.get_index_buffer_bytes().map_or(0, <[u8]>::len);
            mesh_tris += i.map_or(0, |i| i.len() / 3);
        }
    }
    let (gpu_tris, gpu_bytes) = super::mesh_util::gpu_mesh_stats();
    mesh_bytes += gpu_bytes;
    mesh_tris += gpu_tris;
    format!(
        "{{\n  \"quality\": \"{:?}\",\n  \"ui\": {},\n  \"sim_paused\": {},\n  \"sim_speed\": {},\n  \"size\": [{}, {}],\n  \"frames\": {},\n  \
         \"population\": {},\n  \"buildings\": {},\n  \"vehicles\": {},\n  \"sim_ticks_per_s\": {:.1},\n  \"sim_step_ms\": {{ \"avg\": {:.3}, \"p99\": {:.3}, \"max\": {:.3}, \"per_frame\": {:.3} }},\n  \
         \"frame_ms\": {{ \"avg\": {:.3}, \"median\": {:.3}, \"p99\": {:.3}, \"worst_1pct\": {:.3}, \"max\": {:.3} }},\n  \
         \"fps_avg\": {:.1},\n  \
         \"gpu_ms_per_frame\": {{\n{}\n  }},\n  \"triangles_per_frame\": {{\n{}\n  }},\n  \
         \"entities\": {},\n  \"mesh_entities\": {},\n  \"visible_mesh_entities\": {{ \"avg\": {:.0}, \"max\": {:.0} }},\n  \"visible_triangles_k\": {:.0},\n  \"camera_triangles_k\": {:.0},\n  \
         \"mesh_assets\": {},\n  \"mesh_mb\": {:.1},\n  \"mesh_triangles_k\": {:.0},\n  \"materials\": {},\n  \"lights\": {},\n  \
         \"draw_calls\": {{ \"main_avg\": {:.0}, \"main_max\": {:.0}, \"shadow_avg\": {:.0}, \"shadow_max\": {:.0}, \
         \"shadow_views\": {:.0} }},\n{}}}\n",
        bench.quality,
        bench.ui,
        bench.paused,
        crate::shared::SPEEDS[bench.speed.min(crate::shared::SPEEDS.len() - 1)],
        bench.size.0,
        bench.size.1,
        bench.times.len(),
        w.stats.population,
        w.buildings.len(),
        w.vehicles.len(),
        sim_rate,
        step_avg,
        percentile(&steps, 0.99),
        steps.last().copied().unwrap_or(0.0),
        sim_per_frame,
        avg,
        percentile(&sorted, 0.5),
        percentile(&sorted, 0.99),
        worst_1,
        sorted.last().copied().unwrap_or(0.0),
        1000.0 / avg.max(1e-6),
        trim(gpu),
        trim(tris),
        queries.0.iter().count(),
        queries.1.iter().count(),
        bench.sums.visible / frames,
        bench.max.visible,
        bench.sums.visible_tris / frames,
        bench.sums.camera_tris / frames,
        assets.0.len(),
        mesh_bytes as f64 / 1e6,
        mesh_tris as f64 / 1e3,
        assets.1.len() + assets.2.len(),
        queries.2.iter().count(),
        bench.sums.main_draws / frames,
        bench.max.main_draws,
        bench.sums.shadow_draws / frames,
        bench.max.shadow_draws,
        bench.max.shadow_views,
        hitches(bench),
    )
}

impl Bench {
    /// The script has run to its end.
    fn script_done(&self) -> bool {
        match self.script {
            Some(ScriptKind::Quality) => {
                self.switches.len() == script::switches() && self.switches.last().is_some_and(|s| s.end > 0)
            }
            _ => false,
        }
    }

    fn tag(&mut self, t: impl Into<String>) {
        let t = t.into();
        info!("bench: {t}");
        self.tags.push(t);
    }
}

/// Plays the script (`bench_script.rs`) while measuring.
#[allow(clippy::too_many_arguments)]
fn run_script(
    mut bench: ResMut<Bench>,
    sim: Res<SimWorld>,
    applied: Res<QualityApplied>,
    mut overlay: ResMut<OverlayMode>,
    mut hour: ResMut<PinnedHour>,
    mut highlight: ResMut<WorldHighlight>,
    camera: Res<CameraOverride>,
    mut cmds: MessageWriter<SimCommand>,
    mut tools: (
        ResMut<ActiveTool>,
        ResMut<ToolModes>,
        ResMut<RoadChoice>,
        ResMut<ScriptPointer>,
    ),
    cams: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    seen: (
        Res<crate::shared::HoveredPoint>,
        Res<crate::shared::CurveToolView>,
        Res<crate::shared::ToolPreview>,
    ),
) {
    let Some(kind) = bench.script else { return };
    if bench.phase != Phase::Measure {
        return;
    }
    let (centre, radius) = bench.city.unwrap_or_default();
    let at = bench.recs.len();
    match kind {
        ScriptKind::Quality => {
            // One view over the city, a little below the middle of the path.
            bench.pose = Some(path_pose(0.25, centre, radius));
            let bench = bench.as_mut();
            let start_at = 30;
            if let Some(s) = bench.switches.last_mut() {
                if s.applied.is_none() && applied.0 == Some(s.to) {
                    let ms: f64 = bench.recs[s.start..].iter().map(|r| r.ms).sum();
                    s.applied = Some((at - s.start, ms));
                    let (to, frames) = (s.to, at - s.start);
                    bench.tag(format!("quality {to:?} applied after {frames} frames"));
                    return;
                }
                let s = bench.switches.last_mut().expect("switch");
                let Some((frames, _)) = s.applied else { return };
                if at < s.start + frames + script::AFTER_SWITCH as usize {
                    return;
                }
                s.end = at;
            } else if at < start_at {
                return;
            }
            let n = bench.switches.len();
            if n >= script::switches() {
                return;
            }
            let from = bench.quality;
            // The order starts from High; from another level, levels equal
            // to the current one are passed over.
            let order = &script::QUALITY_ORDER;
            while order[bench.next_act % order.len()] == from {
                bench.next_act += 1;
            }
            let to = order[bench.next_act % order.len()];
            bench.next_act += 1;
            bench.quality = to;
            bench.switches.push(Switch {
                from,
                to,
                start: at,
                applied: None,
                end: 0,
            });
            bench.tag(format!("quality {from:?} -> {to:?}"));
        }
        ScriptKind::Actions | ScriptKind::Tools => {
            let frame = bench.frame;
            // The tool act in progress: pointer, camera, buttons.
            let cam = cams.single().ok();
            tool_frame(&mut bench, frame, cam, &mut tools.3);
            // What the tool shows: as each tool act begins, and every 30
            // frames during one.
            if bench.tool_act.is_some() && frame >= bench.tool_log_at {
                bench.tool_log_at = frame + 30;
                let (hp, view, preview) = (&seen.0, &seen.1, &seen.2);
                let text = format!(
                    "tool state: pointer {:?} on ground {:?}, preview: {} pieces, {} nodes, {} refused, cost {}, \
                     {} zone cells, error {:?}; roads {}",
                    tools.3.0.map(|p| p.round()),
                    hp.pos.map(|p| [p[0].round(), p[1].round()]),
                    view.pieces.len(),
                    view.nodes.len(),
                    view.refused.len(),
                    preview.cost,
                    preview.zone_cells.len(),
                    preview.error,
                    sim.world.roads.segments.len()
                );
                bench.tag(text);
            }
            // Animations in progress.
            if let Some((from, by, start)) = bench.hours {
                let t = ((frame - start) as f32 / script::scaled(script::HOUR_FRAMES) as f32).min(1.0);
                hour.0 = Some((from + by * t).rem_euclid(24.0));
                if t >= 1.0 {
                    bench.hours = None;
                }
            }
            if let Some((start, pose)) = bench.zoom {
                let t = (frame - start) as f32 / script::scaled(script::ZOOM_FRAMES) as f32;
                if t >= 1.0 {
                    bench.zoom = None;
                } else {
                    let focus = Vec2::new(pose.focus.x, pose.focus.z);
                    bench.pose = Some(script::zoom_pose(t, focus, &pose));
                }
            }
            let Some(&(when, name, act)) = script::acts(kind).get(bench.next_act) else {
                return;
            };
            if frame < script::scaled(when) {
                return;
            }
            bench.next_act += 1;
            bench.acts.push((at, name.to_string()));
            bench.tag(format!("action: {name}"));
            if matches!(act, Act::Tool(_) | Act::End) {
                bench.tool_log_at = frame;
            }
            if false {
                let (hp, view, preview) = (&seen.0, &seen.1, &seen.2);
                bench.tag(format!(
                    "tool state: pointer {:?} on ground {:?}, preview: {} pieces, {} nodes, {} refused, cost {}, \
                     {} zone cells, error {:?}; roads {}",
                    tools.3.0.map(|p| p.round()),
                    hp.pos.map(|p| [p[0].round(), p[1].round()]),
                    view.pieces.len(),
                    view.nodes.len(),
                    view.refused.len(),
                    preview.cost,
                    preview.zone_cells.len(),
                    preview.error,
                    sim.world.roads.segments.len()
                ));
            }
            let w = &sim.world;
            match act {
                Act::BuildRoad => {
                    bench.street = script::find_street(w, centre);
                    match bench.street {
                        Some(st) => {
                            cmds.write(SimCommand(st.build()));
                        }
                        None => bench.tag("no buildable street found"),
                    }
                }
                Act::ZoneRoad => {
                    if let Some(st) = bench.street {
                        cmds.write(SimCommand(st.zone()));
                    }
                }
                Act::RemoveRoad => {
                    if let Some(st) = bench.street {
                        cmds.write(SimCommand(st.remove()));
                    }
                }
                Act::Signals => match script::find_junction(w, centre) {
                    Some(node) => {
                        cmds.write(SimCommand(citysim_sim::Command::SetNodeControl {
                            node,
                            control: citysim_sim::junction::JunctionSetting::Signal,
                        }));
                    }
                    None => bench.tag("no junction without signals"),
                },
                Act::Overlay(m) => *overlay = m,
                Act::Hours(by) => {
                    let from = hour.0.unwrap_or_else(|| w.time.hour());
                    bench.hours = Some((from, by, frame));
                }
                Act::Unpin => hour.0 = None,
                Act::Key(k) => bench.keys.press.push(k),
                Act::SelectCar => highlight.selected = script::find_car(w, centre),
                Act::Deselect => highlight.selected = None,
                Act::Speed(i) => bench.speed = i,
                Act::Zoom => {
                    if let Some(mut start) = camera.0 {
                        start.focus = Vec3::new(centre.x, 0.0, centre.y);
                        bench.zoom = Some((frame, start));
                    }
                }
                Act::Save | Act::Load => {
                    if std::env::var_os("CITYSIM_SAVE_DIR").is_some_and(|d| !d.is_empty()) {
                        let k = if act == Act::Save { KeyCode::F5 } else { KeyCode::F9 };
                        bench.keys.press.push(k);
                    } else {
                        bench.tag("skipped: CITYSIM_SAVE_DIR is not set");
                    }
                }
                Act::Tool(t) => {
                    if bench.site.is_none() {
                        bench.tag("skipped: no site for the tool acts");
                        return;
                    }
                    start_tool_act(&mut bench, t, frame, &mut tools.0, &mut tools.1, &mut tools.2);
                    let cam = cams.single().ok();
                    tool_frame(&mut bench, frame, cam, &mut tools.3);
                }
                Act::End => {
                    bench.tool_act = None;
                    tools.3.0 = None;
                }
            }
        }
    }
}

/// A tool act begins: the tool and mode it needs, its camera.
fn start_tool_act(
    bench: &mut Bench,
    act: ToolAct,
    frame: u32,
    tool: &mut ResMut<ActiveTool>,
    modes: &mut ResMut<ToolModes>,
    kind: &mut ResMut<RoadChoice>,
) {
    let Some(site) = bench.site.clone() else { return };
    let city = bench.city.map_or(site.zone_at, |c| c.0);
    let city_pose = CameraPose {
        focus: Vec3::new(city.x, 0.0, city.y),
        yaw: 0.35,
        pitch: 0.95,
        distance: 520.0,
    };
    let now = **modes;
    let road = |r: RoadMode| ToolModes { road: r, ..now };
    let zone = |z: ZoneMode| ToolModes { zone: z, ..now };
    let wanted: Option<(ActiveTool, ToolModes, Option<CameraPose>)> = match act {
        ToolAct::Setup => {
            kind.set_if_neq(RoadChoice::Street);
            Some((ActiveTool::Road, road(RoadMode::Grid), Some(site.pose())))
        }
        ToolAct::ChainClicks => Some((ActiveTool::Road, road(RoadMode::Continuous), Some(site.pose()))),
        ToolAct::Fill => Some((
            ActiveTool::Zone(Some(citysim_sim::Zone::Residential)),
            zone(ZoneMode::Fill),
            Some(city_pose),
        )),
        ToolAct::Marquee => Some((
            ActiveTool::Zone(Some(citysim_sim::Zone::Residential)),
            zone(ZoneMode::Marquee),
            Some(city_pose),
        )),
        ToolAct::Bulldoze => Some((
            ActiveTool::Bulldoze,
            ToolModes {
                bulldoze: BulldozeMode::Area,
                ..now
            },
            Some(city_pose),
        )),
        _ => None,
    };
    if let Some((t, m, pose)) = wanted {
        tool.set_if_neq(t);
        modes.set_if_neq(m);
        bench.tool_pose = pose;
    }
    match act {
        ToolAct::Click => {
            bench.keys.mouse_press.push(MouseButton::Left);
        }
        ToolAct::Release => {
            bench.keys.mouse_release.push(MouseButton::Left);
        }
        ToolAct::Undo => {
            bench.keys.press.extend([KeyCode::ControlLeft, KeyCode::KeyZ]);
        }
        ToolAct::Enter => bench.keys.press.push(KeyCode::Enter),
        _ => {}
    }
    bench.tool_act = Some((act, frame));
    bench.pointer_px = None;
}

/// One frame of the tool act in progress: where the camera looks, where the
/// pointer is (as a window position, from where it should be on the
/// ground), the clicks and drags.
fn tool_frame(bench: &mut Bench, frame: u32, cam: Option<(&Camera, &GlobalTransform)>, pointer: &mut ScriptPointer) {
    let (Some((act, start)), Some(site)) = (bench.tool_act, bench.site.clone()) else {
        return;
    };
    let t = frame - start;
    let mut pose = bench.tool_pose;
    let px_of = |p: Vec2| cam.and_then(|(c, tf)| c.world_to_viewport(tf, Vec3::new(p.x, 0.0, p.y)).ok());
    // Clicks `at` frames after the act's start: press, let go two later.
    let click_at = |bench: &mut Bench, at: u32| {
        if t == at {
            bench.keys.mouse_press.push(MouseButton::Left);
        }
        if t == at + 2 {
            bench.keys.mouse_release.push(MouseButton::Left);
        }
    };
    let (nu, nv) = (site.nu as f32, site.nv as f32);
    match act {
        ToolAct::Setup => bench.pointer_at = Some(site.a - site.v * 30.0),
        ToolAct::GridClicks => {
            bench.pointer_at = Some(match t {
                0..12 => site.a,
                12..24 => site.at(nu, 0.0),
                _ => site.at(nu, nv),
            });
            click_at(bench, 4);
            click_at(bench, 16);
        }
        ToolAct::ChainClicks => {
            let k = (t / 12) as usize;
            let last = site.chain.len() - 1;
            bench.pointer_at = Some(site.chain[k.min(last)]);
            if k < last {
                click_at(bench, k as u32 * 12 + 4);
            }
        }
        ToolAct::Pan => {
            // The pointer stays where it is on the screen; the ground under
            // it moves with the camera.
            if bench.pointer_px.is_none() {
                bench.pointer_px = bench.pointer_at.and_then(px_of);
            }
            if let Some(base) = bench.tool_pose {
                let a = t as f32 / 60.0 * std::f32::consts::TAU * 0.5;
                let r = 30.0;
                let mut p = base;
                p.focus += Vec3::new(a.sin() * r, 0.0, (1.0 - a.cos()) * r);
                p.distance *= 1.0 + 0.15 * (a * 0.7).sin();
                p.yaw += 0.08 * (a * 0.5).sin();
                pose = Some(p);
            }
        }
        ToolAct::Wiggle | ToolAct::Fill => {
            let c = if act == ToolAct::Fill {
                site.zone_at
            } else {
                site.at(nu, nv)
            };
            let a = t as f32 / 40.0 * std::f32::consts::TAU;
            let r = if act == ToolAct::Fill { 12.0 } else { GRID_WIGGLE_M };
            bench.pointer_at = Some(c + Vec2::from_angle(a) * r);
        }
        ToolAct::Marquee | ToolAct::Bulldoze => {
            let (a, b) = if act == ToolAct::Marquee {
                (site.zone_at, site.zone_to)
            } else {
                site.district
            };
            if t == 4 {
                bench.keys.mouse_press.push(MouseButton::Left);
            }
            let k = (t.saturating_sub(5) as f32 / 60.0).min(1.0);
            let wobble = Vec2::from_angle(t as f32 * 0.2) * 3.0 * k;
            bench.pointer_at = Some(a + (b - a) * k + wobble);
        }
        ToolAct::Click | ToolAct::Release | ToolAct::Undo | ToolAct::Enter => {
            if act == ToolAct::Click && t == 2 {
                bench.keys.mouse_release.push(MouseButton::Left);
            }
        }
    }
    bench.pose = pose;
    pointer.0 = match bench.pointer_px {
        Some(px) => Some(px),
        None => bench.pointer_at.and_then(px_of),
    };
}

/// How far the pointer circles round the grid's far corner (metres): far
/// enough to change the grid by a block now and then.
const GRID_WIGGLE_M: f32 = 26.0;

/// Fill in the frame record of the frame `drive` just closed.
#[allow(clippy::too_many_arguments)]
fn record(
    mut bench: ResMut<Bench>,
    sim: Res<SimWorld>,
    main: Res<MainTiming>,
    main_marks: Res<MainMarks>,
    counts: Res<super::perf::RenderShared>,
    pipelines: Res<Pipelines>,
    mut meshes: MessageReader<AssetEvent<Mesh>>,
    tool_draw: Res<ToolDrawTime>,
    cmd_time: Res<crate::shared::CommandTime>,
    mut mats: (
        MessageReader<AssetEvent<CityMaterial>>,
        MessageReader<AssetEvent<StandardMaterial>>,
    ),
) {
    let mesh_n = meshes
        .read()
        .filter(|e| !matches!(e, AssetEvent::Unused { .. }))
        .count() as u32;
    let mat_n = mats
        .0
        .read()
        .filter(|e| !matches!(e, AssetEvent::Unused { .. }))
        .count()
        + mats
            .1
            .read()
            .filter(|e| !matches!(e, AssetEvent::Unused { .. }))
            .count();
    let side = pipelines.0.lock().map(|p| *p).unwrap_or_default();
    let (created, compiling) = (side.created, side.compiling);
    let tool_draw_ms = tool_draw.take_ms();
    let render_ms = counts.0.lock().map(|c| c.last_cpu_ms).unwrap_or_default();
    // What the simulation changed since the last frame.
    let w = &sim.world;
    let signals = junction_signals(w);
    let now = (w.versions.roads, w.versions.grid, w.versions.buildings, signals);
    let mut sim_tags = Vec::new();
    if let Some(prev) = bench.versions {
        if prev.0 != now.0 {
            sim_tags.push("sim: roads changed".to_string());
        }
        if prev.1 != now.1 {
            sim_tags.push("sim: zones changed".to_string());
        }
        if prev.2 != now.2 {
            sim_tags.push("sim: buildings changed".to_string());
        }
        if prev.3 != now.3 {
            sim_tags.push(format!("sim: signalled junctions {} -> {}", prev.3, now.3));
        }
    }
    bench.versions = Some(now);
    let new_pipelines = created.saturating_sub(bench.pipelines_seen);
    bench.pipelines_seen = created;
    if bench.phase != Phase::Measure {
        bench.tags.clear();
        return;
    }
    let mut tags = std::mem::take(&mut bench.tags);
    tags.extend(sim_tags);
    if let Some(r) = bench.recs.last_mut() {
        r.main_ms = main.1;
        r.render_ms = render_ms;
        r.meshes = mesh_n;
        r.materials = mat_n as u32;
        r.new_pipelines = new_pipelines;
        r.compiling = compiling;
        r.main_parts = main_marks.2;
        r.tool_draw_ms = tool_draw_ms;
        r.cmd_ms = cmd_time.0 as f64;
        r.render_parts = side.parts;
        r.gpu_wait_ms = side.gpu_wait_ms;
        r.tags.extend(tags);
    } else {
        bench.tags = tags;
    }
}

/// Junctions on signals now (a cheap count: the sim may switch them itself).
fn junction_signals(w: &World) -> usize {
    junction_nodes(w)
        .filter(|&n| junction_view(w, n).is_some_and(|v| v.control == JunctionControl::Signal))
        .count()
}

use citysim_sim::World;
use citysim_sim::junction::{JunctionControl, junction_nodes, junction_view};

fn frames_csv(bench: &Bench) -> String {
    let mut out = String::from(
        "frame,ms,main_ms,render_ms,sim_ms,meshes,materials,new_pipelines,compiling,gpu_wait_ms,tool_draw_ms,cmd_ms",
    );
    for p in MAIN_PARTS {
        out.push_str(&format!(",main_{p}"));
    }
    for p in RENDER_PARTS {
        out.push_str(&format!(",render_{p}"));
    }
    out.push_str(",tags\n");
    for (i, r) in bench.recs.iter().enumerate() {
        let parts: String = r
            .main_parts
            .iter()
            .chain(r.render_parts.iter())
            .map(|v| format!("{v:.3},"))
            .collect();
        out.push_str(&format!(
            "{i},{:.3},{:.3},{:.3},{:.3},{},{},{},{},{:.3},{:.3},{:.3},{parts}\"{}\"\n",
            r.ms,
            r.main_ms,
            r.render_ms,
            r.sim_ms,
            r.meshes,
            r.materials,
            r.new_pipelines,
            r.compiling,
            r.gpu_wait_ms,
            r.tool_draw_ms,
            r.cmd_ms,
            r.tags.join("; ").replace('"', "'")
        ));
    }
    out
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Worst frames, frames over 33 / 50 ms, and the script's per-action or
/// per-switch figures, as JSON members (each line ends with ",\n" but the last).
fn hitches(bench: &Bench) -> String {
    let recs = &bench.recs;
    let mut order: Vec<usize> = (0..recs.len()).collect();
    order.sort_by(|&a, &b| recs[b].ms.total_cmp(&recs[a].ms));
    let rec_json = |i: usize| {
        let r = &recs[i];
        // Tags from the frames just before too: work shows a frame or two
        // after it was asked for (the render world runs a frame behind).
        let mut tags: Vec<String> = Vec::new();
        for (j, rec) in recs.iter().enumerate().take(i + 1).skip(i.saturating_sub(2)) {
            for t in &rec.tags {
                let t = if j < i {
                    format!("{t} ({} before)", i - j)
                } else {
                    t.clone()
                };
                tags.push(json_str(&t));
            }
        }
        let parts = |names: &[&str], v: &[f64]| {
            names
                .iter()
                .zip(v)
                .map(|(n, v)| format!("\"{n}\": {v:.2}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "{{ \"frame\": {i}, \"ms\": {:.2}, \"main_ms\": {:.2}, \"render_ms\": {:.2}, \"gpu_wait_ms\": {:.2}, \
             \"tool_draw_ms\": {:.2}, \
             \"meshes\": {}, \"materials\": {}, \"new_pipelines\": {}, \"compiling\": {}, \"main_parts\": {{ {} }}, \
             \"render_parts\": {{ {} }}, \"tags\": [{}] }}",
            r.ms,
            r.main_ms,
            r.render_ms,
            r.gpu_wait_ms,
            r.tool_draw_ms,
            r.meshes,
            r.materials,
            r.new_pipelines,
            r.compiling,
            parts(&MAIN_PARTS, &r.main_parts),
            parts(&RENDER_PARTS, &r.render_parts),
            tags.join(", ")
        )
    };
    let worst: Vec<String> = order.iter().take(20).map(|&i| format!("    {}", rec_json(i))).collect();
    let over = |ms: f64| recs.iter().filter(|r| r.ms > ms).count();
    let worst_in = |a: usize, b: usize| -> Option<usize> {
        (a..b.min(recs.len())).max_by(|&x, &y| recs[x].ms.total_cmp(&recs[y].ms))
    };
    let mut out = format!(
        "  \"frames_over_33ms\": {},\n  \"frames_over_50ms\": {},\n  \"main_ms_max\": {:.2},\n  \"render_ms_max\": {:.2},\n  \
         \"pipelines_created\": {},\n  \"worst_frames\": [\n{}\n  ]",
        over(33.0),
        over(50.0),
        recs.iter().map(|r| r.main_ms).fold(0.0, f64::max),
        recs.iter().map(|r| r.render_ms).fold(0.0, f64::max),
        recs.iter().map(|r| r.new_pipelines).sum::<usize>(),
        worst.join(",\n")
    );
    match bench.script {
        Some(ScriptKind::Actions | ScriptKind::Tools) => {
            let mut acts = Vec::new();
            for (k, (start, name)) in bench.acts.iter().enumerate() {
                let end = bench.acts.get(k + 1).map_or(recs.len(), |a| a.0);
                if end <= *start {
                    continue;
                }
                let n = (end - start) as f64;
                let avg = recs[*start..end].iter().map(|r| r.ms).sum::<f64>() / n;
                let w = worst_in(*start, end).unwrap_or(*start);
                let over33 = recs[*start..end].iter().filter(|r| r.ms > 33.0).count();
                // The CPU's worst on its own (on a software renderer the
                // GPU's time hides it in the frame times).
                let main_max = recs[*start..end].iter().map(|r| r.main_ms).fold(0.0, f64::max);
                let render_max = recs[*start..end].iter().map(|r| r.render_ms).fold(0.0, f64::max);
                // The tools' own work: the tool systems (`GameSet::Tools`),
                // their commands applied (`shared::CommandTime`) and gfx drawing
                // their feedback, per frame: average and worst.
                let part = |name: &str| MAIN_PARTS.iter().position(|p| *p == name).expect("part");
                let ti = part("tools");
                let stat = |f: &dyn Fn(&FrameRec) -> f64| {
                    let v: Vec<f64> = recs[*start..end].iter().map(f).collect();
                    (v.iter().sum::<f64>() / n, v.iter().copied().fold(0.0, f64::max))
                };
                let tools = stat(&|r| r.main_parts[ti]);
                let cmds = stat(&|r| r.cmd_ms);
                let draw = stat(&|r| r.tool_draw_ms);
                let all = stat(&|r| r.main_parts[ti] + r.cmd_ms + r.tool_draw_ms);
                acts.push(format!(
                    "    {{ \"action\": {}, \"frames\": {}, \"avg_ms\": {avg:.2}, \"worst_ms\": {:.2}, \
                     \"worst_at\": {}, \"over_33ms\": {over33}, \"main_ms_max\": {main_max:.2}, \
                     \"render_ms_max\": {render_max:.2}, \"tools_ms\": {{ \"avg\": {:.3}, \"max\": {:.3} }}, \
                     \"sim_cmds_ms\": {{ \"avg\": {:.3}, \"max\": {:.3} }}, \
                     \"tool_draw_ms\": {{ \"avg\": {:.3}, \"max\": {:.3} }}, \
                     \"tool_total_ms\": {{ \"avg\": {:.3}, \"max\": {:.3} }}, \"worst\": {} }}",
                    json_str(name),
                    end - start,
                    recs[w].ms,
                    w - start,
                    tools.0,
                    tools.1,
                    cmds.0,
                    cmds.1,
                    draw.0,
                    draw.1,
                    all.0,
                    all.1,
                    rec_json(w)
                ));
            }
            out.push_str(&format!(",\n  \"actions\": [\n{}\n  ]", acts.join(",\n")));
        }
        Some(ScriptKind::Quality) => {
            let mut rows = Vec::new();
            let mut worsts = Vec::new();
            for s in &bench.switches {
                let end = if s.end > 0 { s.end } else { recs.len() };
                let w = worst_in(s.start, end).unwrap_or(s.start);
                let slice = &recs[s.start..end.max(s.start)];
                worsts.push(recs.get(w).map_or(0.0, |r| r.ms));
                let (frames, ms) = s.applied.map_or((-1, -1.0), |(f, m)| (f as i64, m));
                let main_max = slice.iter().map(|r| r.main_ms).fold(0.0, f64::max);
                let render_max = slice.iter().map(|r| r.render_ms).fold(0.0, f64::max);
                rows.push(format!(
                    "    {{ \"from\": \"{:?}\", \"to\": \"{:?}\", \"frames_to_apply\": {frames}, \"ms_to_apply\": {ms:.1}, \
                     \"worst_ms\": {:.2}, \"over_33ms\": {}, \"over_50ms\": {}, \"new_pipelines\": {}, \
                     \"main_ms_max\": {main_max:.2}, \"render_ms_max\": {render_max:.2}, \"worst\": {} }}",
                    s.from,
                    s.to,
                    recs.get(w).map_or(0.0, |r| r.ms),
                    slice.iter().filter(|r| r.ms > 33.0).count(),
                    slice.iter().filter(|r| r.ms > 50.0).count(),
                    slice.iter().map(|r| r.new_pipelines).sum::<usize>(),
                    recs.get(w).map_or_else(|| "null".into(), |_| rec_json(w)),
                ));
            }
            let mut sorted = worsts.clone();
            sorted.sort_by(f64::total_cmp);
            out.push_str(&format!(
                ",\n  \"quality_switch_worst_ms\": {{ \"median\": {:.2}, \"max\": {:.2} }},\n  \"quality_switches\": [\n{}\n  ]",
                percentile(&sorted, 0.5),
                sorted.last().copied().unwrap_or(0.0),
                rows.join(",\n")
            ));
        }
        None => {}
    }
    out.push('\n');
    out
}
