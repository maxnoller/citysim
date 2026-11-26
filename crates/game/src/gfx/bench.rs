//! Windowless benchmark: `CITYSIM_BENCH=out.json`.
//!
//! The game runs without a window (`main.rs` drops winit and runs the app
//! loop itself); the world camera renders to an off-screen image
//! (`CITYSIM_BENCH_SIZE`, default 1920x1080) along a fixed camera path over
//! the city, frame by frame (`CITYSIM_BENCH_FRAMES`, default 1500, after
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

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use bevy::camera::RenderTarget;
use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::render::render_resource::{PollType, TextureFormat};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};

use super::city_material::CityMaterial;
use crate::shared::{
    CameraOverride, CameraPose, GameSet, GraphicsQuality, SceneReady, SimClock, SimStep, SimWorld, WorldCamera,
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
        app.insert_resource(Bench {
            path,
            size: (w, h),
            warmup: env("CITYSIM_BENCH_WARMUP").unwrap_or(240),
            frames: env("CITYSIM_BENCH_FRAMES").unwrap_or(1500),
            quality: match std::env::var("CITYSIM_GFX_QUALITY").as_deref() {
                Ok("low") => GraphicsQuality::Low,
                Ok("medium") => GraphicsQuality::Medium,
                _ => GraphicsQuality::High,
            },
            paused: env::<u32>("CITYSIM_BENCH_PAUSED") == Some(1),
            speed: env::<usize>("CITYSIM_BENCH_SPEED").unwrap_or(0),
            ui: env::<u32>("CITYSIM_BENCH_UI") == Some(1),
            ..default()
        })
        .init_resource::<SimTiming>()
        .add_systems(FixedUpdate, sim_start.before(SimStep))
        .add_systems(FixedUpdate, sim_end.after(SimStep))
        .add_systems(Update, drive.in_set(GameSet::Input));
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render.init_resource::<InFlight>().add_systems(
                Render,
                throttle_gpu.after(RenderSystems::Render).before(RenderSystems::Cleanup),
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

/// Submissions the GPU may still be working on (oldest first), as closures
/// that block until theirs has finished.
#[derive(Resource, Default)]
struct InFlight(VecDeque<Box<dyn FnOnce() + Send + Sync>>);

fn throttle_gpu(device: Res<RenderDevice>, queue: Res<RenderQueue>, mut in_flight: ResMut<InFlight>) {
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
    phase: Phase,
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
    main_draws: f64,
    shadow_draws: f64,
    shadow_views: f64,
}

impl Sums {
    fn max(&mut self, o: &Sums) {
        self.visible = self.visible.max(o.visible);
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
    sim: Res<SimWorld>,
    ready: Res<SceneReady>,
    mut clock: ResMut<SimClock>,
    mut quality: ResMut<GraphicsQuality>,
    mut camera: ResMut<CameraOverride>,
    mut images: ResMut<Assets<Image>>,
    cams: Query<(Entity, Option<&RenderTarget>), With<WorldCamera>>,
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
    camera.0 = Some(path_pose(u, centre, radius));

    match bench.phase {
        Phase::Loading => {
            if ready.0 {
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
            if let Some(dt) = dt
                && bench.frame > 0
            {
                bench.times.push(dt);
            }
            bench.frame += 1;
            let steps = std::mem::take(&mut sim_timing.1);
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
            let frame = Sums {
                visible: visible as f64,
                main_draws: c.main as f64,
                shadow_draws: c.shadow as f64,
                shadow_views: c.shadow_views as f64,
            };
            bench.sums.visible += frame.visible;
            bench.sums.main_draws += frame.main_draws;
            bench.sums.shadow_draws += frame.shadow_draws;
            bench.sums.shadow_views += frame.shadow_views;
            bench.max.max(&frame);
            if bench.frame >= total {
                bench.phase = Phase::Done;
                bench.sim_ticks.1 = sim.world.time.tick;
                let report = report(&bench, &sim, &queries, &assets);
                match std::fs::write(&bench.path, &report) {
                    Ok(()) => info!("bench: wrote {}", bench.path),
                    Err(e) => error!("bench: cannot write {}: {e}", bench.path),
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
    let frames = bench.frames.max(1) as f64;
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
    format!(
        "{{\n  \"quality\": \"{:?}\",\n  \"ui\": {},\n  \"sim_paused\": {},\n  \"sim_speed\": {},\n  \"size\": [{}, {}],\n  \"frames\": {},\n  \
         \"population\": {},\n  \"buildings\": {},\n  \"vehicles\": {},\n  \"sim_ticks_per_s\": {:.1},\n  \"sim_step_ms\": {{ \"avg\": {:.3}, \"p99\": {:.3}, \"max\": {:.3}, \"per_frame\": {:.3} }},\n  \
         \"frame_ms\": {{ \"avg\": {:.3}, \"median\": {:.3}, \"p99\": {:.3}, \"worst_1pct\": {:.3}, \"max\": {:.3} }},\n  \
         \"fps_avg\": {:.1},\n  \
         \"gpu_ms_per_frame\": {{\n{}\n  }},\n  \"triangles_per_frame\": {{\n{}\n  }},\n  \
         \"entities\": {},\n  \"mesh_entities\": {},\n  \"visible_mesh_entities\": {{ \"avg\": {:.0}, \"max\": {:.0} }},\n  \
         \"mesh_assets\": {},\n  \"materials\": {},\n  \"lights\": {},\n  \
         \"draw_calls\": {{ \"main_avg\": {:.0}, \"main_max\": {:.0}, \"shadow_avg\": {:.0}, \"shadow_max\": {:.0}, \
         \"shadow_views\": {:.0} }}\n}}\n",
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
        assets.0.len(),
        assets.1.len() + assets.2.len(),
        queries.2.iter().count(),
        bench.sums.main_draws / frames,
        bench.max.main_draws,
        bench.sums.shadow_draws / frames,
        bench.max.shadow_draws,
        bench.max.shadow_views,
    )
}
