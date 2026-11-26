//! What only the renderer knows, for ui's performance panel
//! (`shared::PerfProbe`) and the windowless benchmark (`bench.rs`).
//!
//! Everything here is measured only while it is wanted: while the panel is
//! in its full size (`PerfDetail::Full`) or `CITYSIM_BENCH` runs. Otherwise
//! no system of this module does any work, and Bevy's
//! `RenderDiagnosticsPlugin` records nothing: its recorder is taken out of
//! the render world while the render graph runs, so no pass writes
//! timestamp or pipeline-statistics queries (its per-frame bookkeeping
//! stays, with nothing to read back).
//!
//! Measured: GPU time per top-level pass (timestamp queries; Vulkan and
//! DX12), triangles in the main passes (pipeline statistics), the render
//! thread's CPU time (from the start of its frame to the GPU submit: neither
//! the wait for a swapchain image nor the present counts), draw calls
//! (counted from the render phases, see `count_draws`), mesh and car
//! entities and how many are on screen, the game's GPU memory (the wgpu
//! allocator's report; Vulkan and DX12), and the graphics card in use.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d, Transparent3d};
use bevy::core_pipeline::prepass::Opaque3dPrepass;
use bevy::diagnostic::DiagnosticsStore;
use bevy::pbr::Shadow;
use bevy::prelude::*;
use bevy::render::diagnostic::{DiagnosticsRecorder, RenderDiagnosticsPlugin, resolve_encoder};
use bevy::render::render_phase::{BinnedPhaseItem, BinnedRenderPhase, ViewBinnedRenderPhases, ViewSortedRenderPhases};
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice, RenderGraph, RenderGraphSystems};
use bevy::render::view::prepare_windows;
use bevy::render::{Render, RenderApp, RenderSystems};

use super::vehicles::VehicleVisual;
use crate::shared::{GameSet, PerfDetail, PerfProbe, RenderStats};

/// Seconds between refreshes of `PerfProbe::render`.
const REFRESH_S: f32 = 0.25;
/// Seconds between GPU memory reports (each lists every allocation).
const MEMORY_S: f32 = 1.0;

/// Whether the render world measures this frame (shared by both worlds).
#[derive(Resource, Clone, Default)]
pub(super) struct Gate(Arc<AtomicBool>);

impl Gate {
    fn on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

fn gate_on(gate: Res<Gate>) -> bool {
    gate.on()
}

/// Draw calls of one frame (see `count_draws`).
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Counts {
    /// Draw calls in the main camera's passes (prepass, opaque, transparent).
    pub main: usize,
    /// Draw calls in all shadow views.
    pub shadow: usize,
    /// Number of views with a shadow phase (cascades).
    pub shadow_views: usize,
}

/// Render-thread figures, shared with the main world.
#[derive(Default, Debug)]
pub(super) struct RenderFrames {
    /// The last frame's draw calls (the benchmark reads these every frame).
    pub last: Counts,
    /// Sums since the panel last read them.
    draws: (usize, usize),
    cpu_ms: f64,
    frames: u32,
    /// Start of the current render frame and of its swapchain wait (the
    /// frame ends at the submit, before the present).
    start: Option<Instant>,
    wait_start: Option<Instant>,
    wait_ms: f64,
}

#[derive(Resource, Clone, Default)]
pub(super) struct RenderShared(pub Arc<Mutex<RenderFrames>>);

/// The benchmark runs (it measures always, unless `CITYSIM_BENCH_GPU=0`).
#[derive(Resource, Clone, Copy)]
struct AlwaysOn(bool);

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        let gate = Gate::default();
        let shared = RenderShared::default();
        app.add_plugins(RenderDiagnosticsPlugin)
            .insert_resource(gate.clone())
            .insert_resource(shared.clone())
            .insert_resource(AlwaysOn(
                super::bench::enabled() && std::env::var("CITYSIM_BENCH_GPU").as_deref() != Ok("0"),
            ))
            .init_resource::<Refresh>()
            .add_systems(First, open_gate)
            .add_systems(Update, refresh.in_set(GameSet::Render).run_if(gate_on));
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .insert_resource(gate)
                .insert_resource(shared)
                .init_resource::<Stash>()
                .add_systems(
                    RenderGraph,
                    (
                        stash_recorder.before(RenderGraphSystems::Begin),
                        restore_recorder
                            .after(RenderGraphSystems::Render)
                            .before(resolve_encoder),
                        frame_end.in_set(RenderGraphSystems::Finish).run_if(gate_on),
                    ),
                )
                .add_systems(
                    Render,
                    (
                        frame_start.before(RenderSystems::ExtractCommands),
                        wait_start.before(prepare_windows),
                        wait_end.after(prepare_windows),
                        count_draws.after(RenderSystems::Render).before(RenderSystems::Cleanup),
                    )
                        .run_if(gate_on),
                );
        }
    }
}

/// Measure from this frame on while the full panel shows (or the
/// benchmark runs).
fn open_gate(probe: Res<PerfProbe>, always: Res<AlwaysOn>, gate: Res<Gate>, shared: Res<RenderShared>) {
    let on = always.0 || probe.detail == PerfDetail::Full;
    if gate.on() != on {
        gate.0.store(on, Ordering::Relaxed);
        // Start the sums afresh.
        if on && let Ok(mut f) = shared.0.lock() {
            *f = RenderFrames::default();
        }
    }
}

/// The render diagnostics recorder while the graph runs without it.
#[derive(Resource, Default)]
struct Stash(Option<DiagnosticsRecorder>);

/// Gate closed: no pass finds the recorder, so none records anything.
fn stash_recorder(world: &mut World) {
    if world.resource::<Gate>().on() {
        return;
    }
    if let Some(r) = world.remove_resource::<DiagnosticsRecorder>() {
        world.resource_mut::<Stash>().0 = Some(r);
    }
}

/// Put it back for Bevy's own resolve / finish systems (they need it).
fn restore_recorder(world: &mut World) {
    if let Some(r) = world.resource_mut::<Stash>().0.take() {
        world.insert_resource(r);
    }
}

fn frame_start(shared: Res<RenderShared>) {
    if let Ok(mut f) = shared.0.lock() {
        f.start = Some(Instant::now());
        f.wait_ms = 0.0;
    }
}

fn wait_start(shared: Res<RenderShared>) {
    if let Ok(mut f) = shared.0.lock() {
        f.wait_start = Some(Instant::now());
    }
}

fn wait_end(shared: Res<RenderShared>) {
    if let Ok(mut f) = shared.0.lock()
        && let Some(s) = f.wait_start.take()
    {
        f.wait_ms += s.elapsed().as_secs_f64() * 1000.0;
    }
}

fn frame_end(shared: Res<RenderShared>) {
    if let Ok(mut f) = shared.0.lock()
        && let Some(s) = f.start.take()
    {
        let ms = s.elapsed().as_secs_f64() * 1000.0 - f.wait_ms;
        f.cpu_ms += ms.max(0.0);
        f.frames += 1;
    }
}

fn binned_draws<P: BinnedPhaseItem>(phase: &BinnedRenderPhase<P>) -> usize {
    phase.multidrawable_meshes.len()
        + phase.batchable_meshes.len()
        + phase
            .unbatchable_meshes
            .values()
            .map(|u| u.entities.len())
            .sum::<usize>()
        + phase.non_mesh_items.values().map(|u| u.entities.len()).sum::<usize>()
}

/// Draw calls from the render phases: one per multi-draw batch set,
/// batchable bin and unbatchable entity, per view.
fn count_draws(
    opaque: Option<Res<ViewBinnedRenderPhases<Opaque3d>>>,
    mask: Option<Res<ViewBinnedRenderPhases<AlphaMask3d>>>,
    prepass: Option<Res<ViewBinnedRenderPhases<Opaque3dPrepass>>>,
    transparent: Option<Res<ViewSortedRenderPhases<Transparent3d>>>,
    shadow: Option<Res<ViewBinnedRenderPhases<Shadow>>>,
    out: Res<RenderShared>,
) {
    let mut c = Counts::default();
    if let Some(p) = opaque {
        c.main += p.0.values().map(binned_draws).sum::<usize>();
    }
    if let Some(p) = mask {
        c.main += p.0.values().map(binned_draws).sum::<usize>();
    }
    if let Some(p) = prepass {
        c.main += p.0.values().map(binned_draws).sum::<usize>();
    }
    if let Some(p) = transparent {
        c.main += p.0.values().map(|s| s.items.len()).sum::<usize>();
    }
    if let Some(p) = shadow {
        c.shadow = p.0.values().map(binned_draws).sum();
        c.shadow_views = p.0.len();
    }
    if let Ok(mut o) = out.0.lock() {
        o.last = c;
        o.draws.0 += c.main;
        o.draws.1 += c.shadow;
    }
}

/// Main-world bookkeeping between refreshes.
#[derive(Resource, Default)]
struct Refresh {
    last: f32,
    memory_at: Option<f32>,
    memory: Option<(u64, u64)>,
    /// Newest measurement time already counted, per diagnostic path.
    seen: BTreeMap<String, Instant>,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh(
    time: Res<Time<Real>>,
    mut state: ResMut<Refresh>,
    mut probe: ResMut<PerfProbe>,
    store: Res<DiagnosticsStore>,
    shared: Res<RenderShared>,
    device: Option<Res<RenderDevice>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    meshes: Query<&ViewVisibility, With<Mesh3d>>,
    vehicles: Query<&ViewVisibility, With<VehicleVisual>>,
) {
    let now = time.elapsed_secs();
    if probe.detail != PerfDetail::Full || now - state.last < REFRESH_S {
        return;
    }
    state.last = now;
    // Render diagnostics: the mean of every measurement since the last
    // refresh, per top-level pass.
    let mut gpu: Vec<(String, f32)> = Vec::new();
    let mut triangles: Option<u64> = None;
    for d in store.iter() {
        let path = d.path().as_str();
        let Some(rest) = path.strip_prefix("render/") else {
            continue;
        };
        let since = state.seen.get(path).copied();
        let (mut sum, mut n, mut newest) = (0.0, 0u32, since);
        for m in d.measurements() {
            if since.is_none_or(|s| m.time > s) {
                sum += m.value;
                n += 1;
                newest = Some(newest.map_or(m.time, |t: Instant| t.max(m.time)));
            }
        }
        if let Some(t) = newest {
            state.seen.insert(path.to_string(), t);
        }
        if n == 0 {
            continue;
        }
        let mean = sum / n as f64;
        if let Some(pass) = rest.strip_suffix("/elapsed_gpu")
            && !pass.contains('/')
        {
            gpu.push((pass.to_string(), mean as f32));
        } else if let Some(pass) = rest.strip_suffix("/clipper_primitives_out")
            && pass.starts_with("main_")
        {
            *triangles.get_or_insert(0) += mean.round() as u64;
        }
    }
    gpu.sort_by(|a, b| b.1.total_cmp(&a.1));
    let gpu_ms = (!gpu.is_empty()).then(|| gpu.iter().map(|p| p.1).sum::<f32>());
    // Render thread and draw calls since the last refresh.
    let (render_cpu_ms, draw_calls) = match shared.0.lock() {
        Ok(mut f) => {
            let frames = f.frames.max(1) as f64;
            let cpu = (f.frames > 0).then(|| (f.cpu_ms / frames) as f32);
            let draws = (
                (f.draws.0 as f64 / frames).round() as u32,
                (f.draws.1 as f64 / frames).round() as u32,
            );
            f.cpu_ms = 0.0;
            f.frames = 0;
            f.draws = (0, 0);
            (cpu, draws)
        }
        Err(_) => (None, (0, 0)),
    };
    // GPU memory: once a second (the report lists every allocation).
    if state.memory_at.is_none_or(|t| now - t >= MEMORY_S) {
        state.memory_at = Some(now);
        state.memory = device
            .as_ref()
            .and_then(|d| d.wgpu_device().generate_allocator_report())
            .map(|r| (r.total_allocated_bytes, r.total_reserved_bytes));
    }
    let count = |q: &mut dyn Iterator<Item = &ViewVisibility>| {
        let (mut all, mut seen) = (0u32, 0u32);
        for v in q {
            all += 1;
            seen += v.get() as u32;
        }
        (all, seen)
    };
    let (name, backend, driver) = match adapter.as_deref() {
        Some(a) => (
            a.name.clone(),
            format!("{:?}", a.backend),
            [a.driver.as_str(), a.driver_info.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        None => Default::default(),
    };
    probe.render = Some(RenderStats {
        at: now,
        gpu_ms,
        passes: gpu,
        render_cpu_ms,
        draw_calls,
        triangles,
        meshes: count(&mut meshes.iter()),
        vehicles: count(&mut vehicles.iter()),
        gpu_memory: state.memory,
        adapter: name,
        backend,
        driver,
    });
}
