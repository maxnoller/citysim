//! The performance panel: F3 (or Settings) switches it off, small or full;
//! the choice is remembered in the settings file.
//!
//! * Small: a plate under the city plate with frames per second, frame
//!   time and a three-second graph.
//! * Full: a docked two-column panel at the left edge: frames (now, ten
//!   second average, slowest 1 %, frame time and a five-second graph in
//!   which a stutter shows as a spike), where the frame goes (CPU of the
//!   game update and of the render thread against GPU time, the slowest GPU
//!   passes), the simulation (time per step, steps a second against the
//!   target, keeping up or not), resources (the game's CPU and memory,
//!   system memory, the game's GPU memory), the scene (entities, draw calls,
//!   triangles, cars against the vehicle budget, citizens, buildings) and
//!   the setup (graphics card, API and driver, resolution, quality, vsync).
//!
//! Costs: closed, none of its systems runs and gfx measures nothing
//! (`shared::PerfProbe::detail`). Small records one frame time a frame.
//! Full also times the game update and every simulation step, samples the
//! process off the main thread once a second (`sysinfo`), and has gfx fill
//! `PerfProbe::render`. Texts change four times a second, the graphs ten
//! times; nothing is rebuilt.
//!
//! `CITYSIM_PERF=off|small|full` sets the starting size (development).

use std::collections::VecDeque;
use std::time::Instant;

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use bevy::ui::ComputedNode;
use bevy::ui_render::prelude::{MaterialNode, UiMaterial, UiMaterialPlugin};
use bevy::window::{PresentMode, PrimaryWindow};
use citysim_sim::TICKS_PER_SECOND;

use super::UiState;
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, caps, info_tip, label, layout, plate_node,
    plate_shadow, slant_rule, spawn_button,
};
use crate::shared::{GameSet, GraphicsQuality, PerfDetail, PerfProbe, SimClock, SimStep, SimWorld};

/// Frames kept for the averages (seconds).
const KEEP_S: f64 = 10.0;
/// "Now": the frames of the last half second.
const NOW_S: f64 = 0.5;
/// Seconds between text updates and between graph updates.
const TEXT_S: f32 = 0.25;
const GRAPH_S: f32 = 0.1;
/// The full graph: five seconds in 100 columns; the small one three in 36.
const GRAPH_SPAN_S: f64 = 5.0;
const GRAPH_COLS: usize = 100;
const SPARK_SPAN_S: f64 = 3.0;
const SPARK_COLS: usize = 36;
/// A frame over this is over budget (60 frames a second).
pub const BUDGET_MS: f32 = 1000.0 / 60.0;
/// Graph scales (ms at the top).
const SCALES: [f32; 4] = [25.0, 50.0, 100.0, 200.0];
/// Seconds between process samples.
const PROC_S: f32 = 1.0;
/// Simulation rate window (seconds).
const RATE_S: f64 = 2.0;

/// Where the panels hang (logical px): under the city plate.
const TOP: f32 = 68.0;
const SMALL_H: f32 = 40.0;
const COL_A: f32 = 292.0;
const COL_B: f32 = 262.0;
const PAD: f32 = 20.0;
const GAP: f32 = 16.0;
/// Columns, the rule between them, padding and the edge line.
const FULL_W: f32 = PAD + COL_A + GAP + 1.0 + GAP + COL_B + PAD + t::LW;
const GRAPH_H: f32 = 76.0;
const PASSES: usize = 4;

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(UiMaterialPlugin::<GraphMaterial>::default())
            .init_resource::<Frames>()
            .init_resource::<Proc>()
            .add_systems(Startup, spawn_panels)
            .add_systems(PostStartup, detail_from_env)
            .add_systems(First, main_begin.before(bevy::time::TimeSystems).run_if(full))
            .add_systems(Last, main_end.run_if(full))
            .add_systems(
                FixedUpdate,
                (sim_begin.before(SimStep), sim_end.after(SimStep)).run_if(full),
            )
            .add_systems(Update, show_panels.in_set(GameSet::Render))
            .add_systems(
                Update,
                (
                    record_frame,
                    sample_process.run_if(full),
                    update_texts,
                    update_graphs,
                    fit_full,
                    place_toasts,
                )
                    .chain()
                    .after(show_panels)
                    .in_set(GameSet::Render)
                    .run_if(shown),
            );
    }
}

/// `CITYSIM_PERF=off|small|full` (development, benchmarks): the starting
/// size, over the saved one.
fn detail_from_env(mut ui: ResMut<UiState>) {
    let d = match std::env::var("CITYSIM_PERF").as_deref() {
        Ok("off") => PerfDetail::Off,
        Ok("small") => PerfDetail::Small,
        Ok("full") => PerfDetail::Full,
        _ => return,
    };
    ui.perf = d;
}

fn shown(ui: Res<UiState>) -> bool {
    ui.perf != PerfDetail::Off
}

fn full(ui: Res<UiState>) -> bool {
    ui.perf == PerfDetail::Full
}

/// F3: off, small, full, off.
pub fn next_detail(d: PerfDetail) -> PerfDetail {
    match d {
        PerfDetail::Off => PerfDetail::Small,
        PerfDetail::Small => PerfDetail::Full,
        PerfDetail::Full => PerfDetail::Off,
    }
}

// ---------------------------------------------------------------- measuring

/// Frame and simulation timings while the panel shows.
#[derive(Resource, Default)]
struct Frames {
    /// (real time at the end of the frame (s), frame time (ms)), oldest
    /// first.
    times: VecDeque<(f64, f32)>,
    main_start: Option<Instant>,
    /// Game-update time since the last text update: (sum ms, frames).
    main: (f64, u32),
    sim_start: Option<Instant>,
    /// Simulation steps: (real time (s), ms), oldest first.
    steps: VecDeque<(f64, f32)>,
    /// (real time, tick), oldest first, for steps a second.
    ticks: VecDeque<(f64, u64)>,
    /// The clock the tick samples were taken at.
    clock: Option<SimClock>,
    text_t: f32,
    graph_t: f32,
    /// The full graph's scale (ms).
    scale: f32,
}

fn main_begin(mut f: ResMut<Frames>) {
    f.main_start = Some(Instant::now());
}

fn main_end(mut f: ResMut<Frames>) {
    if let Some(s) = f.main_start.take() {
        f.main.0 += s.elapsed().as_secs_f64() * 1000.0;
        f.main.1 += 1;
    }
}

fn sim_begin(mut f: ResMut<Frames>) {
    f.sim_start = Some(Instant::now());
}

fn sim_end(mut f: ResMut<Frames>, time: Res<Time<Real>>, clock: Res<SimClock>) {
    // Per step (a fixed update runs `steps_per_tick` of them; none while
    // paused).
    let steps = clock.steps_per_tick();
    if let Some(s) = f.sim_start.take()
        && steps > 0
    {
        let ms = s.elapsed().as_secs_f64() as f32 * 1000.0 / steps as f32;
        let now = time.elapsed_secs_f64();
        f.steps.push_back((now, ms));
        while f.steps.front().is_some_and(|s| now - s.0 > RATE_S) {
            f.steps.pop_front();
        }
    }
}

fn record_frame(time: Res<Time<Real>>, mut f: ResMut<Frames>) {
    let dt = time.delta_secs_f64();
    if dt <= 0.0 {
        return;
    }
    let now = time.elapsed_secs_f64();
    f.times.push_back((now, (dt * 1000.0) as f32));
    while f.times.front().is_some_and(|s| now - s.0 > KEEP_S) {
        f.times.pop_front();
    }
}

/// Frame figures over the kept frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// Frames a second and mean frame time over the last half second.
    pub fps: f32,
    pub ms: f32,
    /// Frames a second over everything kept (up to ten seconds).
    pub fps_avg: f32,
    /// Frames a second of the slowest 1 % of frames (their mean time).
    pub low_1: f32,
    /// The slowest frame kept (ms).
    pub max_ms: f32,
}

pub fn frame_stats(times: &VecDeque<(f64, f32)>, now: f64) -> Option<FrameStats> {
    if times.is_empty() {
        return None;
    }
    let (mut n, mut sum) = (0u32, 0f64);
    for &(at, ms) in times.iter().rev() {
        if now - at > NOW_S && n > 0 {
            break;
        }
        n += 1;
        sum += ms as f64;
    }
    let ms = (sum / n as f64) as f32;
    let total: f64 = times.iter().map(|s| s.1 as f64).sum();
    let mut sorted: Vec<f32> = times.iter().map(|s| s.1).collect();
    sorted.sort_by(|a, b| b.total_cmp(a));
    let worst = (sorted.len() / 100).max(1);
    let low = sorted.iter().take(worst).map(|&v| v as f64).sum::<f64>() / worst as f64;
    Some(FrameStats {
        fps: 1000.0 / ms.max(1e-3),
        ms,
        fps_avg: (times.len() as f64 * 1000.0 / total.max(1e-6)) as f32,
        low_1: (1000.0 / low.max(1e-6)) as f32,
        max_ms: sorted[0],
    })
}

/// The slowest frame time (ms) in each of `cols` equal slices of the last
/// `span` seconds, oldest first; a frame counts in every slice it lasted
/// into (a long stall is as wide as it was long); -1 where nothing is
/// known yet. Slices are fixed in time so columns do not reshuffle.
pub fn columns(times: &VecDeque<(f64, f32)>, now: f64, span: f64, cols: usize) -> Vec<f32> {
    let w = span / cols as f64;
    let last = (now / w).floor() as i64;
    let first = last - cols as i64 + 1;
    let mut out = vec![-1.0f32; cols];
    for &(end, ms) in times.iter().rev() {
        let start = end - ms as f64 / 1000.0;
        let (a, b) = ((start / w).floor() as i64, (end / w).floor() as i64);
        if b < first {
            break;
        }
        for k in a.max(first)..=b.min(last) {
            let c = &mut out[(k - first) as usize];
            *c = c.max(ms);
        }
    }
    out
}

/// The smallest scale that holds `max` with a little room.
pub fn graph_scale(max: f32) -> f32 {
    SCALES
        .iter()
        .copied()
        .find(|s| max * 1.1 <= *s)
        .unwrap_or(SCALES[SCALES.len() - 1])
}

/// The game process: sampled once a second on a worker thread while the
/// full panel shows (`sysinfo`; a CPU figure needs two samples).
#[derive(Resource, Default)]
struct Proc {
    system: Option<sysinfo::System>,
    task: Option<Task<(sysinfo::System, ProcSample)>>,
    last: Option<ProcSample>,
    at: Option<f32>,
}

#[derive(Debug, Clone, Default)]
struct ProcSample {
    /// CPU cores' worth the game used since the sample before (None for
    /// the first sample).
    cores: Option<f32>,
    /// Logical cores of the machine.
    threads: usize,
    /// Resident memory of the game (bytes).
    memory: u64,
    /// Used and total system memory (bytes).
    system: (u64, u64),
    cpu_name: String,
}

fn take_sample(mut sys: sysinfo::System, first: bool) -> (sysinfo::System, ProcSample) {
    use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate};
    let pid = sysinfo::get_current_pid().ok();
    if let Some(pid) = pid {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
    }
    sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    if first {
        sys.refresh_cpu_list(CpuRefreshKind::nothing());
    }
    let process = pid.and_then(|p| sys.process(p));
    let sample = ProcSample {
        cores: process.filter(|_| !first).map(|p| p.cpu_usage() / 100.0),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        memory: process.map_or(0, |p| p.memory()),
        system: (sys.used_memory(), sys.total_memory()),
        cpu_name: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default(),
    };
    (sys, sample)
}

fn sample_process(time: Res<Time<Real>>, mut p: ResMut<Proc>) {
    let now = time.elapsed_secs();
    if let Some(task) = &mut p.task {
        if !task.is_finished() {
            return;
        }
        if let Some(task) = p.task.take() {
            let (sys, mut s) = block_on(task);
            if s.cpu_name.is_empty()
                && let Some(prev) = &p.last
            {
                s.cpu_name = prev.cpu_name.clone();
            }
            p.system = Some(sys);
            p.last = Some(s);
        }
    }
    if p.at.is_some_and(|a| now - a < PROC_S) {
        return;
    }
    p.at = Some(now);
    let first = p.system.is_none();
    let sys = p.system.take().unwrap_or_default();
    p.task = Some(AsyncComputeTaskPool::get().spawn(async move { take_sample(sys, first) }));
}

// ---------------------------------------------------------------- the graph material

/// The frame-time graph (`assets/ui/shaders/perf_graph.wesl`).
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct GraphMaterial {
    #[uniform(0)]
    values: [Vec4; 32],
    #[uniform(0)]
    bar: LinearRgba,
    #[uniform(0)]
    warn: LinearRgba,
    #[uniform(0)]
    back: LinearRgba,
    #[uniform(0)]
    line: LinearRgba,
    /// Columns, budget line, warning threshold (fractions of the scale),
    /// grid lines.
    #[uniform(0)]
    params: Vec4,
}

impl UiMaterial for GraphMaterial {
    fn fragment_shader() -> ShaderRef {
        "ui/shaders/perf_graph.wesl".into()
    }
}

impl GraphMaterial {
    fn new(cols: usize, grid: f32) -> GraphMaterial {
        GraphMaterial {
            values: [Vec4::splat(-1.0); 32],
            bar: t::ACCENT.to_linear(),
            warn: t::WARNING.to_linear(),
            back: t::WELL.to_linear(),
            line: t::alpha(t::TEXT_MUTED, 0.75).to_linear(),
            params: Vec4::new(cols as f32, 0.0, 2.0, grid),
        }
    }

    fn set(&mut self, cols: &[f32], scale: f32) {
        let mut v = [Vec4::splat(-1.0); 32];
        for (i, &c) in cols.iter().take(128).enumerate() {
            v[i / 4][i % 4] = if c < 0.0 { -1.0 } else { c / scale };
        }
        self.values = v;
        self.params.y = BUDGET_MS / scale;
        self.params.z = BUDGET_MS / scale;
    }
}

// ---------------------------------------------------------------- layout

#[derive(Component)]
struct SmallRoot;

#[derive(Component)]
struct FullRoot {
    /// Eased fit scale.
    fit: f32,
}

/// A graph and its material.
#[derive(Component)]
struct Graph {
    detail: PerfDetail,
    span: f64,
    cols: usize,
    material: Handle<GraphMaterial>,
}

/// Live texts.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum Txt {
    SmallFps,
    SmallMs,
    Fps,
    Avg,
    Low,
    Ms,
    Max,
    Scale,
    CpuMain,
    CpuRender,
    Gpu,
    Limit,
    PassName(usize),
    PassMs(usize),
    SimStep,
    SimRate,
    SimState,
    ProcCpu,
    ProcMem,
    SysMem,
    GpuMem,
    Entities,
    Draws,
    Tris,
    Cars,
    Citizens,
    Buildings,
    Card,
    Api,
    Cpu,
    Screen,
}

/// The fill of a time meter.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Meter {
    CpuMain,
    CpuRender,
    Gpu,
}

/// A GPU pass row (hidden when there is no such pass).
#[derive(Component)]
struct PassRow(usize);

/// The "60 fps" tag on the full graph's budget line.
#[derive(Component)]
struct BudgetTag;

fn value_text(fonts: &UiFonts, tag: Txt, size: f32) -> impl Bundle {
    (
        Text::new("\u{2014}"),
        fonts.figures(size, t::W_SEMIBOLD),
        TextColor(t::TEXT),
        bevy::text::LineHeight::RelativeToFont(1.1),
        TextLayout::no_wrap(),
        Pickable::IGNORE,
        tag,
    )
}

/// A caption with the accent tick.
fn section(p: &mut ChildSpawnerCommands, fonts: &UiFonts, title: &str, top: f32) {
    p.spawn(layout(Node {
        align_items: AlignItems::Center,
        column_gap: px(8),
        margin: UiRect::new(px(0), px(0), px(top), px(5)),
        ..default()
    }))
    .with_children(|h| {
        h.spawn((
            layout(Node {
                width: px(3),
                height: px(11),
                flex_shrink: 0.0,
                ..default()
            }),
            BackgroundColor(t::ACCENT),
        ));
        h.spawn(caps(fonts, title, t::FONT_CAP, t::TEXT));
    });
}

/// Label left, live value right.
fn row(p: &mut ChildSpawnerCommands, fonts: &UiFonts, name: &str, tag: Txt) -> Entity {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        column_gap: px(12),
        min_height: px(21),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(label(fonts, name, t::FONT_BODY, t::W_MEDIUM, t::TEXT_MUTED));
        r.spawn((
            value_text(fonts, tag, 16.0),
            Node {
                flex_shrink: 1.0,
                min_width: px(0),
                overflow: Overflow::clip(),
                ..default()
            },
        ));
    })
    .id()
}

/// A time row with a meter under it.
fn meter_row(p: &mut ChildSpawnerCommands, fonts: &UiFonts, name: &str, tag: Txt, meter: Meter) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(3),
        margin: UiRect::bottom(px(5)),
        ..default()
    }))
    .with_children(|c| {
        row(c, fonts, name, tag);
        c.spawn((
            layout(Node {
                width: percent(100),
                height: px(5),
                ..default()
            }),
            BackgroundColor(t::WELL),
        ))
        .with_child((
            layout(Node {
                width: percent(0),
                height: percent(100),
                ..default()
            }),
            BackgroundColor(t::ACCENT),
            meter,
        ));
    });
}

/// A small caption and figure pair of the frames block.
fn stat(p: &mut ChildSpawnerCommands, fonts: &UiFonts, name: &str, tag: Txt) {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        width: px(150),
        min_height: px(19),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(caps(fonts, name, t::FONT_KEY + 0.5, t::TEXT_MUTED));
        r.spawn(value_text(fonts, tag, 16.0));
    });
}

fn graph_node(
    p: &mut ChildSpawnerCommands,
    mats: &mut Assets<GraphMaterial>,
    detail: PerfDetail,
    span: f64,
    cols: usize,
    node: Node,
    grid: f32,
) -> Entity {
    let material = mats.add(GraphMaterial::new(cols, grid));
    p.spawn((
        layout(node),
        MaterialNode(material.clone()),
        Graph {
            detail,
            span,
            cols,
            material,
        },
    ))
    .id()
}

fn spawn_panels(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut plates: ResMut<Assets<PlateMaterial>>,
    mut graphs: ResMut<Assets<GraphMaterial>>,
) {
    let fonts = fonts.into_inner();
    // Small: a plate under the city plate, slanted on the right like it.
    let small_mat = cache.get(&mut plates, PlateStyle::PANEL.slanted([0.0, 0.0, 0.0, 0.5]));
    commands
        .spawn((
            Name::new("Performance (small)"),
            SmallRoot,
            plate_node(
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: px(TOP),
                    height: px(SMALL_H),
                    padding: UiRect::new(px(22), px(30), px(0), px(0)),
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    display: Display::None,
                    ..default()
                },
                UiRect::new(px(0), px(t::LW), px(t::LW), px(t::LW)),
                [0.0; 4],
            ),
            small_mat,
            plate_shadow(),
            GlobalZIndex(25),
            info_tip(
                "Performance",
                "Frames per second, frame time and the last three seconds of frames. F3: the full panel.",
            ),
        ))
        .with_children(|s| {
            s.spawn(layout(Node {
                align_items: AlignItems::Baseline,
                column_gap: px(5),
                ..default()
            }))
            .with_children(|f| {
                f.spawn((
                    value_text(fonts, Txt::SmallFps, 24.0),
                    Node {
                        min_width: px(34),
                        ..default()
                    },
                ));
                f.spawn(caps(fonts, "fps", t::FONT_CAP, t::TEXT_MUTED));
            });
            s.spawn(slant_rule(20.0));
            s.spawn(layout(Node {
                align_items: AlignItems::Baseline,
                column_gap: px(5),
                ..default()
            }))
            .with_children(|f| {
                f.spawn((
                    value_text(fonts, Txt::SmallMs, 18.0),
                    Node {
                        min_width: px(30),
                        ..default()
                    },
                ));
                f.spawn(caps(fonts, "ms", t::FONT_CAP, t::TEXT_MUTED));
            });
            s.spawn((
                layout(Node {
                    width: px(66),
                    height: px(22),
                    margin: UiRect::left(px(4)),
                    border: UiRect::all(px(1)),
                    ..default()
                }),
                BorderColor::all(t::RULE),
            ))
            .with_children(|w| {
                graph_node(
                    w,
                    &mut graphs,
                    PerfDetail::Small,
                    SPARK_SPAN_S,
                    SPARK_COLS,
                    Node {
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                    0.0,
                );
            });
            icon_button(
                s,
                fonts,
                UiAction::Perf(PerfDetail::Full),
                icon::CHEVRON_RIGHT,
                "Full panel",
            );
        });

    // Full: docked at the left edge, two columns.
    let full_mat = cache.get(&mut plates, PlateStyle::PANEL);
    commands
        .spawn((
            Name::new("Performance (full)"),
            FullRoot { fit: 1.0 },
            plate_node(
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    top: px(TOP),
                    width: px(FULL_W),
                    flex_direction: FlexDirection::Column,
                    display: Display::None,
                    ..default()
                },
                UiRect::new(px(0), px(t::LW), px(t::LW), px(t::LW)),
                [0.0, 0.0, t::CUT, 0.0],
            ),
            full_mat,
            plate_shadow(),
            UiTransform::default(),
            GlobalZIndex(25),
        ))
        .with_children(|p| {
            // Header strip.
            p.spawn((
                layout(Node {
                    height: px(40),
                    padding: UiRect::new(px(22), px(8), px(0), px(0)),
                    column_gap: px(10),
                    align_items: AlignItems::Center,
                    border: UiRect::bottom(px(t::LW)),
                    flex_shrink: 0.0,
                    ..default()
                }),
                BackgroundColor(t::HEAD),
                BorderColor::all(t::LINE),
            ))
            .with_children(|h| {
                h.spawn((
                    layout(Node {
                        width: px(5),
                        height: px(20),
                        flex_shrink: 0.0,
                        ..default()
                    }),
                    BackgroundColor(t::ACCENT),
                ));
                h.spawn((
                    Text::new("PERFORMANCE"),
                    fonts.figures(t::FONT_HEAD, t::W_BOLD),
                    t::caps_spacing(t::FONT_HEAD, 0.1),
                    TextColor(t::TEXT),
                    TextLayout::no_wrap(),
                    Node {
                        flex_grow: 1.0,
                        ..default()
                    },
                    Pickable::IGNORE,
                ));
                super::widgets::spawn_key(h, fonts, "F3", t::TEXT_MUTED);
                icon_button(h, fonts, UiAction::Perf(PerfDetail::Small), icon::MINUS, "Small panel");
                icon_button(h, fonts, UiAction::Perf(PerfDetail::Off), icon::CLOSE, "Close");
            });
            p.spawn(layout(Node {
                padding: UiRect::new(px(PAD), px(PAD), px(12), px(16)),
                column_gap: px(GAP),
                ..default()
            }))
            .with_children(|body| {
                body.spawn(layout(Node {
                    width: px(COL_A),
                    flex_direction: FlexDirection::Column,
                    flex_shrink: 0.0,
                    ..default()
                }))
                .with_children(|a| column_a(a, fonts, &mut graphs));
                body.spawn((
                    layout(Node {
                        width: px(1),
                        flex_shrink: 0.0,
                        ..default()
                    }),
                    BackgroundColor(t::RULE),
                ));
                body.spawn(layout(Node {
                    width: px(COL_B),
                    flex_direction: FlexDirection::Column,
                    flex_shrink: 0.0,
                    ..default()
                }))
                .with_children(|b| column_b(b, fonts));
            });
        });
}

fn icon_button(p: &mut ChildSpawnerCommands, fonts: &UiFonts, action: UiAction, glyph: char, name: &'static str) {
    let tip = match action {
        UiAction::Perf(PerfDetail::Off) => Tooltip::new(name, "Close the performance panel.").key("F3"),
        UiAction::Perf(PerfDetail::Small) => Tooltip::new(name, "Frames per second and frame time only."),
        _ => Tooltip::new(name, "Frames, CPU against GPU, simulation, resources, scene and setup.").key("F3"),
    };
    let id = spawn_button(
        p,
        UiButton::new(action, ButtonKind::Ghost),
        Node {
            width: px(28),
            height: px(28),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            ..default()
        },
        Some(tip),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, 16.0, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
        },
    );
    p.commands().entity(id).insert(Name::new(name));
}

fn column_a(a: &mut ChildSpawnerCommands, fonts: &UiFonts, graphs: &mut Assets<GraphMaterial>) {
    // Frames: the big figure and three small ones.
    a.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        ..default()
    }))
    .with_children(|r| {
        r.spawn(layout(Node {
            align_items: AlignItems::Baseline,
            column_gap: px(7),
            ..default()
        }))
        .with_children(|f| {
            f.spawn((
                Text::new("\u{2014}"),
                fonts.figures(46.0, t::W_BOLD),
                TextColor(t::TEXT),
                bevy::text::LineHeight::RelativeToFont(1.0),
                TextLayout::no_wrap(),
                Pickable::IGNORE,
                Txt::Fps,
                Node {
                    min_width: px(66),
                    ..default()
                },
            ));
            f.spawn(caps(fonts, "fps", t::FONT_CAP + 1.0, t::TEXT_MUTED));
        });
        r.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            ..default()
        }))
        .with_children(|c| {
            stat(c, fonts, "10 s average", Txt::Avg);
            stat(c, fonts, "Slowest 1 %", Txt::Low);
            stat(c, fonts, "Frame", Txt::Ms);
        });
    });
    // The graph.
    a.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        margin: UiRect::top(px(10)),
        ..default()
    }))
    .with_children(|r| {
        section(r, fonts, "Frame time · 5 s", 0.0);
        r.spawn((
            label(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
            Txt::Max,
            Node {
                margin: UiRect::bottom(px(5)),
                ..default()
            },
        ));
    });
    a.spawn((
        layout(Node {
            width: percent(100),
            height: px(GRAPH_H),
            border: UiRect::all(px(1)),
            ..default()
        }),
        BorderColor::all(t::RULE),
    ))
    .with_children(|w| {
        graph_node(
            w,
            graphs,
            PerfDetail::Full,
            GRAPH_SPAN_S,
            GRAPH_COLS,
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            3.0,
        );
        w.spawn((
            layout(Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                padding: UiRect::new(px(4), px(5), px(1), px(2)),
                ..default()
            }),
            BackgroundColor(t::alpha(t::WELL, 0.85)),
        ))
        .with_child((label(fonts, "", t::FONT_KEY, t::W_SEMIBOLD, t::TEXT_MUTED), Txt::Scale));
        w.spawn((
            label(fonts, "60 fps", t::FONT_KEY, t::W_SEMIBOLD, t::TEXT_MUTED),
            BudgetTag,
            Node {
                position_type: PositionType::Absolute,
                right: px(4),
                top: px(0),
                ..default()
            },
            Visibility::Hidden,
        ));
    });
    // Where the frame goes.
    section(a, fonts, "Where the frame goes", 12.0);
    meter_row(a, fonts, "CPU · game update", Txt::CpuMain, Meter::CpuMain);
    meter_row(a, fonts, "CPU · render thread", Txt::CpuRender, Meter::CpuRender);
    meter_row(a, fonts, "GPU", Txt::Gpu, Meter::Gpu);
    a.spawn((label(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED), Txt::Limit));
    section(a, fonts, "Slowest GPU passes", 12.0);
    for i in 0..PASSES {
        a.spawn((
            layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                min_height: px(19),
                ..default()
            }),
            PassRow(i),
        ))
        .with_children(|r| {
            r.spawn((
                label(fonts, "", t::FONT_BODY, t::W_MEDIUM, t::TEXT_MUTED),
                Txt::PassName(i),
            ));
            r.spawn(value_text(fonts, Txt::PassMs(i), 15.0));
        });
    }
}

fn column_b(b: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    section(b, fonts, "Simulation", 2.0);
    row(b, fonts, "Time per step", Txt::SimStep);
    row(b, fonts, "Steps a second", Txt::SimRate);
    row(b, fonts, "Keeping up", Txt::SimState);
    section(b, fonts, "Resources", 12.0);
    row(b, fonts, "Game CPU", Txt::ProcCpu);
    row(b, fonts, "Game memory", Txt::ProcMem);
    row(b, fonts, "System memory", Txt::SysMem);
    row(b, fonts, "GPU memory", Txt::GpuMem);
    section(b, fonts, "Scene", 12.0);
    row(b, fonts, "Entities", Txt::Entities);
    row(b, fonts, "Draw calls", Txt::Draws);
    row(b, fonts, "Triangles", Txt::Tris);
    row(b, fonts, "Cars drawn", Txt::Cars);
    row(b, fonts, "Citizens", Txt::Citizens);
    row(b, fonts, "Buildings", Txt::Buildings);
    section(b, fonts, "Setup", 12.0);
    row(b, fonts, "Card", Txt::Card);
    row(b, fonts, "API", Txt::Api);
    row(b, fonts, "CPU", Txt::Cpu);
    row(b, fonts, "Screen", Txt::Screen);
}

// ---------------------------------------------------------------- updating

/// Show the chosen size and tell gfx how much to measure.
#[allow(clippy::type_complexity)]
fn show_panels(
    ui: Res<UiState>,
    mut probe: ResMut<PerfProbe>,
    mut frames: ResMut<Frames>,
    mut proc_: ResMut<Proc>,
    mut small: Query<&mut Node, (With<SmallRoot>, Without<FullRoot>)>,
    mut full: Query<&mut Node, (With<FullRoot>, Without<SmallRoot>)>,
    mut toasts: Query<&mut Node, (With<super::toasts::ToastStack>, Without<SmallRoot>, Without<FullRoot>)>,
) {
    if probe.detail != ui.perf {
        probe.detail = ui.perf;
        // Fresh figures for the new size (and nothing kept while closed).
        let keep_frames = ui.perf != PerfDetail::Off;
        *frames = Frames {
            times: if keep_frames {
                std::mem::take(&mut frames.times)
            } else {
                VecDeque::new()
            },
            scale: frames.scale,
            ..default()
        };
        if ui.perf != PerfDetail::Full {
            probe.render = None;
            // A CPU figure spans the time between two samples: start over.
            *proc_ = Proc::default();
        }
    }
    let show = |n: &mut Node, on: bool| {
        let d = if on { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    };
    for mut n in &mut small {
        show(&mut n, ui.perf == PerfDetail::Small);
    }
    for mut n in &mut full {
        show(&mut n, ui.perf == PerfDetail::Full);
    }
    if ui.perf == PerfDetail::Off {
        // Toasts back in their place.
        for mut n in &mut toasts {
            if n.top != px(super::toasts::STACK_TOP) || n.left != px(0) {
                n.top = px(super::toasts::STACK_TOP);
                n.left = px(0);
            }
        }
    }
}

fn ms_text(ms: f32) -> String {
    if ms < 1.0 {
        format!("{ms:.2} ms")
    } else if ms < 100.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{ms:.0} ms")
    }
}

fn bytes_text(b: u64) -> String {
    let gb = b as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{gb:.2} GB")
    } else {
        format!("{:.0} MB", b as f64 / (1024.0 * 1024.0))
    }
}

fn count_text(n: u64) -> String {
    if n >= 10_000_000 {
        format!("{:.1} M", n as f64 / 1e6)
    } else if n >= 100_000 {
        format!("{:.0} k", n as f64 / 1e3)
    } else {
        super::format::group(n)
    }
}

/// A processor's brand string without the marketing words.
fn cpu_name(brand: &str) -> String {
    let cut = brand.split(" @ ").next().unwrap_or(brand);
    cut.replace("(R)", "")
        .replace("(TM)", "")
        .split_whitespace()
        .filter(|w| !matches!(*w, "Processor" | "CPU") && !w.ends_with("-Core"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bevy's pass names in words.
fn pass_name(p: &str) -> String {
    let known = match p {
        "main_opaque_pass_3d" => "Opaque",
        "main_transparent_pass_3d" => "Transparent",
        "main_alpha_mask_pass_3d" => "Alpha mask",
        "per_view_shadow_pass" => "Shadows",
        "early prepass" => "Depth prepass",
        "late prepass" => "Late prepass",
        "ssao" => "Ambient occlusion",
        "smaa" => "Anti-aliasing",
        "bloom" => "Bloom",
        "tonemapping" => "Tone mapping",
        "upscaling" => "Upscaling",
        "clustering" => "Light clusters",
        "early_mesh_preprocessing" => "Mesh preprocessing",
        "ui" | "ui_pass" => "Interface",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    let s = p.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// Everything the texts show, worked out once per update.
#[derive(Default)]
struct Figures {
    texts: Vec<(Txt, String, Color)>,
    meters: [(f32, bool); 3],
    passes: usize,
}

#[allow(clippy::too_many_arguments)]
fn figures(
    ui: &UiState,
    frames: &mut Frames,
    now: f64,
    probe: &PerfProbe,
    proc_: &Proc,
    sim: &SimWorld,
    clock: SimClock,
    entities: usize,
    window: Option<&Window>,
    quality: GraphicsQuality,
) -> Figures {
    let mut f = Figures::default();
    let mut put = |tag: Txt, s: String, c: Color| f.texts.push((tag, s, c));
    let dash = || "\u{2014}".to_string();
    let stats = frame_stats(&frames.times, now);
    let over = |ms: f32| if ms > BUDGET_MS { t::WARNING } else { t::TEXT };
    if let Some(s) = stats {
        put(Txt::SmallFps, format!("{:.0}", s.fps), t::TEXT);
        let small_ms = if s.ms < 100.0 {
            format!("{:.1}", s.ms)
        } else {
            format!("{:.0}", s.ms)
        };
        put(Txt::SmallMs, small_ms, over(s.ms));
        put(Txt::Fps, format!("{:.0}", s.fps), t::TEXT);
        put(Txt::Avg, format!("{:.0} fps", s.fps_avg), t::TEXT);
        put(
            Txt::Low,
            format!("{:.0} fps", s.low_1),
            over(1000.0 / s.low_1.max(1e-3)),
        );
        put(Txt::Ms, ms_text(s.ms), over(s.ms));
        put(
            Txt::Max,
            format!("slowest {}", ms_text(s.max_ms)),
            if s.max_ms > 2.0 * BUDGET_MS {
                t::WARNING
            } else {
                t::TEXT_MUTED
            },
        );
    }
    if ui.perf != PerfDetail::Full {
        return f;
    }
    let frame_ms = stats.map_or(0.0, |s| s.ms);
    // Where the frame goes.
    let main_ms = (frames.main.1 > 0).then(|| (frames.main.0 / frames.main.1 as f64) as f32);
    frames.main = (0.0, 0);
    let render = probe.render.as_ref();
    let render_ms = render.and_then(|r| r.render_cpu_ms);
    let gpu_ms = render.and_then(|r| r.gpu_ms);
    let mut meter = |i: usize, tag: Txt, v: Option<f32>| match v {
        Some(v) => {
            f.meters[i] = ((v / frame_ms.max(v).max(1e-3)).clamp(0.0, 1.0), v > BUDGET_MS);
            f.texts.push((tag, ms_text(v), over(v)));
        }
        None => {
            f.meters[i] = (0.0, false);
            f.texts.push((tag, dash(), t::TEXT_MUTED));
        }
    };
    meter(0, Txt::CpuMain, main_ms);
    meter(1, Txt::CpuRender, render_ms);
    meter(2, Txt::Gpu, gpu_ms);
    let mut put = |tag: Txt, s: String, c: Color| f.texts.push((tag, s, c));
    let limit = match (render, gpu_ms) {
        (None, _) => "Renderer figures need the full game".to_string(),
        (Some(r), None) => format!("GPU time is not measured on {}", r.backend),
        (Some(_), Some(g)) => {
            let cpu = main_ms.unwrap_or(0.0).max(render_ms.unwrap_or(0.0));
            let vsync = window.is_some_and(|w| {
                matches!(
                    w.present_mode,
                    PresentMode::AutoVsync | PresentMode::Fifo | PresentMode::FifoRelaxed
                )
            });
            if frame_ms <= 0.0 {
                String::new()
            } else if g >= 0.85 * frame_ms {
                "The GPU sets the pace".to_string()
            } else if cpu >= 0.85 * frame_ms {
                if main_ms.unwrap_or(0.0) >= render_ms.unwrap_or(0.0) {
                    "The game update sets the pace".to_string()
                } else {
                    "The render thread sets the pace".to_string()
                }
            } else if vsync {
                "Waiting for the display (vsync)".to_string()
            } else {
                "Waiting for the window to present".to_string()
            }
        }
    };
    put(Txt::Limit, limit, t::TEXT_MUTED);
    if let Some(r) = render {
        f.passes = r.passes.len().min(PASSES);
        for (i, (name, ms)) in r.passes.iter().take(PASSES).enumerate() {
            f.texts.push((Txt::PassName(i), pass_name(name), t::TEXT_MUTED));
            f.texts.push((Txt::PassMs(i), ms_text(*ms), t::TEXT));
        }
    }
    let mut put = |tag: Txt, s: String, c: Color| f.texts.push((tag, s, c));
    // Simulation.
    let w = &sim.world;
    if frames.clock != Some(clock) {
        frames.clock = Some(clock);
        frames.ticks.clear();
    }
    frames.ticks.push_back((now, w.time.tick));
    while frames.ticks.front().is_some_and(|s| now - s.0 > RATE_S) {
        frames.ticks.pop_front();
    }
    let steps: Vec<f32> = frames.steps.iter().map(|s| s.1).collect();
    if steps.is_empty() {
        put(Txt::SimStep, dash(), t::TEXT_MUTED);
    } else {
        let avg = steps.iter().sum::<f32>() / steps.len() as f32;
        let max = steps.iter().copied().fold(0.0, f32::max);
        put(
            Txt::SimStep,
            format!("{} · max {}", ms_text(avg), ms_text(max)),
            t::TEXT,
        );
    }
    let target = if clock.paused {
        0.0
    } else {
        (TICKS_PER_SECOND * clock.speed_steps()) as f32
    };
    let rate = match (frames.ticks.front(), frames.ticks.back()) {
        (Some(a), Some(b)) if b.0 - a.0 >= 0.9 => Some((b.1 - a.1) as f32 / (b.0 - a.0) as f32),
        _ => None,
    };
    match rate {
        Some(r) => put(Txt::SimRate, format!("{r:.0} of {target:.0}"), t::TEXT),
        None => put(Txt::SimRate, format!("\u{2014} of {target:.0}"), t::TEXT_MUTED),
    }
    let (state, color) = match rate {
        _ if clock.paused => ("Paused".to_string(), t::TEXT_MUTED),
        None => ("Measuring".to_string(), t::TEXT_MUTED),
        Some(r) if r >= 0.95 * target => ("Yes".to_string(), t::TEXT),
        Some(r) => (
            format!("No · {:.0} % of the speed", 100.0 * r / target.max(1.0)),
            t::WARNING,
        ),
    };
    put(Txt::SimState, state, color);
    // Resources.
    match &proc_.last {
        Some(p) => {
            match p.cores {
                Some(c) => put(
                    Txt::ProcCpu,
                    format!(
                        "{c:.1} of {} cores · {:.0} %",
                        p.threads,
                        100.0 * c / p.threads.max(1) as f32
                    ),
                    t::TEXT,
                ),
                None => put(Txt::ProcCpu, dash(), t::TEXT_MUTED),
            }
            put(Txt::ProcMem, bytes_text(p.memory), t::TEXT);
            put(
                Txt::SysMem,
                format!("{} of {}", bytes_text(p.system.0), bytes_text(p.system.1)),
                t::TEXT,
            );
            if !p.cpu_name.is_empty() {
                put(Txt::Cpu, cpu_name(&p.cpu_name), t::TEXT);
            }
        }
        None => {
            for tag in [Txt::ProcCpu, Txt::ProcMem, Txt::SysMem] {
                put(tag, dash(), t::TEXT_MUTED);
            }
        }
    }
    match render.and_then(|r| r.gpu_memory) {
        Some((used, _)) => put(Txt::GpuMem, bytes_text(used), t::TEXT),
        None => put(Txt::GpuMem, "not reported".into(), t::TEXT_MUTED),
    }
    // Scene.
    put(Txt::Entities, count_text(entities as u64), t::TEXT);
    match render {
        Some(r) => {
            put(
                Txt::Draws,
                format!("{} · {} shadow", count_text(r.draw_calls.0 as u64), r.draw_calls.1),
                t::TEXT,
            );
            match r.triangles {
                Some(n) => put(Txt::Tris, count_text(n), t::TEXT),
                None => put(Txt::Tris, "not reported".into(), t::TEXT_MUTED),
            }
        }
        None => {
            put(Txt::Draws, dash(), t::TEXT_MUTED);
            put(Txt::Tris, dash(), t::TEXT_MUTED);
        }
    }
    let budget = citysim_sim::traffic::vehicle_budget(w);
    let cars = match render {
        Some(r) => format!(
            "{} of {} · {} seen",
            count_text(r.vehicles.0 as u64),
            count_text(budget as u64),
            count_text(r.vehicles.1 as u64)
        ),
        None => format!(
            "{} of {}",
            count_text(w.vehicles.len() as u64),
            count_text(budget as u64)
        ),
    };
    put(Txt::Cars, cars, t::TEXT);
    put(Txt::Citizens, count_text(w.citizens.len() as u64), t::TEXT);
    put(Txt::Buildings, count_text(w.buildings.len() as u64), t::TEXT);
    // Setup.
    match render {
        Some(r) if !r.adapter.is_empty() => {
            put(Txt::Card, r.adapter.clone(), t::TEXT);
            let api = if r.driver.is_empty() {
                r.backend.clone()
            } else {
                format!("{} · {}", r.backend, r.driver)
            };
            put(Txt::Api, api, t::TEXT);
        }
        _ => {
            put(Txt::Card, dash(), t::TEXT_MUTED);
            put(Txt::Api, dash(), t::TEXT_MUTED);
        }
    }
    if let Some(w) = window {
        let q = match quality {
            GraphicsQuality::Low => "Low",
            GraphicsQuality::Medium => "Medium",
            GraphicsQuality::High => "High",
        };
        let sync = match w.present_mode {
            PresentMode::AutoVsync | PresentMode::Fifo | PresentMode::FifoRelaxed => "vsync",
            _ => "no vsync",
        };
        put(
            Txt::Screen,
            format!("{} \u{00d7} {} · {q} · {sync}", w.physical_width(), w.physical_height()),
            t::TEXT,
        );
    } else {
        put(Txt::Screen, dash(), t::TEXT_MUTED);
    }
    f
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_texts(
    time: Res<Time<Real>>,
    ui: Res<UiState>,
    mut frames: ResMut<Frames>,
    probe: Res<PerfProbe>,
    proc_: Res<Proc>,
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    quality: Res<GraphicsQuality>,
    window: Query<&Window, With<PrimaryWindow>>,
    entities: Query<Entity>,
    mut texts: Query<(&Txt, &mut Text, &mut TextColor)>,
    mut meters: Query<(&Meter, &mut Node, &mut BackgroundColor), Without<PassRow>>,
    mut rows: Query<(&PassRow, &mut Node), Without<Meter>>,
) {
    let now = time.elapsed_secs();
    if now - frames.text_t < TEXT_S && frames.text_t > 0.0 {
        return;
    }
    frames.text_t = now;
    let full = ui.perf == PerfDetail::Full;
    let f = figures(
        &ui,
        &mut frames,
        time.elapsed_secs_f64(),
        &probe,
        &proc_,
        &sim,
        *clock,
        if full { entities.iter().count() } else { 0 },
        window.single().ok(),
        *quality,
    );
    for (tag, mut text, mut color) in &mut texts {
        let small = matches!(tag, Txt::SmallFps | Txt::SmallMs);
        if small == full {
            continue;
        }
        if let Some((_, s, c)) = f.texts.iter().find(|(t, ..)| t == tag) {
            if text.0 != *s {
                text.0 = s.clone();
            }
            if color.0 != *c {
                color.0 = *c;
            }
        }
    }
    if !full {
        return;
    }
    for (m, mut node, mut bg) in &mut meters {
        let i = match m {
            Meter::CpuMain => 0,
            Meter::CpuRender => 1,
            Meter::Gpu => 2,
        };
        let (v, hot) = f.meters[i];
        let w = percent(v * 100.0);
        if node.width != w {
            node.width = w;
        }
        let c = if hot { t::WARNING } else { t::ACCENT };
        if bg.0 != c {
            bg.0 = c;
        }
    }
    for (r, mut node) in &mut rows {
        let d = if r.0 < f.passes { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_graphs(
    time: Res<Time<Real>>,
    ui: Res<UiState>,
    mut frames: ResMut<Frames>,
    graphs: Query<&Graph>,
    mut mats: ResMut<Assets<GraphMaterial>>,
    mut texts: Query<(&Txt, &mut Text)>,
    mut tag: Query<(&mut Node, &mut Visibility), With<BudgetTag>>,
) {
    let now = time.elapsed_secs();
    if now - frames.graph_t < GRAPH_S && frames.graph_t > 0.0 {
        return;
    }
    frames.graph_t = now;
    let at = time.elapsed_secs_f64();
    for g in &graphs {
        if g.detail != ui.perf {
            continue;
        }
        let cols = columns(&frames.times, at, g.span, g.cols);
        let max = cols.iter().copied().fold(0.0, f32::max);
        let scale = graph_scale(max);
        if let Some(mut m) = mats.get_mut(&g.material) {
            m.set(&cols, scale);
        }
        if g.detail == PerfDetail::Full && frames.scale != scale {
            frames.scale = scale;
            for (t, mut text) in &mut texts {
                if *t == Txt::Scale {
                    text.0 = format!("{scale:.0} ms");
                }
            }
            // The budget tag sits on its line (hidden near the edges).
            let k = BUDGET_MS / scale;
            for (mut node, mut vis) in &mut tag {
                let top = px((1.0 - k) * (GRAPH_H - 2.0) - 14.0);
                if node.top != top {
                    node.top = top;
                }
                vis.set_if_neq(if (0.12..0.85).contains(&k) {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                });
            }
        }
    }
}

/// Keep the full panel clear of the drawer, the getting-started plate and
/// the bar: shrink it (from its top-left corner) when the window is short.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn fit_full(
    time: Res<Time<Real>>,
    ui: Res<UiState>,
    extent: Res<super::tray::TrayExtent>,
    window: Query<&Window, With<PrimaryWindow>>,
    scale: Res<UiScale>,
    mut full: Query<(&mut FullRoot, &mut UiTransform, &ComputedNode)>,
    named: Query<(&Name, &ComputedNode), Without<FullRoot>>,
    mut hint: Local<Option<Entity>>,
    names: Query<(Entity, &Name)>,
) {
    let Ok((mut root, mut tf, computed)) = full.single_mut() else {
        return;
    };
    if ui.perf != PerfDetail::Full {
        return;
    }
    let Ok(w) = window.single() else { return };
    let s = scale.0.max(1e-3);
    let height = w.height() / s;
    let mut bottom = t::BAR_H + 10.0;
    if extent.0 > 0.0 {
        bottom += extent.0 + 8.0;
    }
    if ui.hint_visible {
        if hint.is_none() {
            *hint = names
                .iter()
                .find(|(_, n)| n.as_str() == "Getting started")
                .map(|(e, _)| e);
        }
        if let Some(e) = *hint
            && let Ok((_, c)) = named.get(e)
        {
            bottom = bottom.max(t::BAR_H + 150.0 + c.size.y * c.inverse_scale_factor + 8.0);
        }
    }
    let size = computed.size * computed.inverse_scale_factor;
    if size.y <= 0.0 {
        return;
    }
    let want = ((height - TOP - bottom) / size.y).clamp(0.55, 1.0);
    root.fit += (want - root.fit) * approach(time.delta_secs(), 0.06);
    if (want - root.fit).abs() < 0.002 {
        root.fit = want;
    }
    let k = root.fit;
    let scale_v = Vec2::splat(k);
    // UiTransform scales about the node's centre: keep the top-left corner.
    let shift = Val2::px(-(1.0 - k) * size.x * 0.5, -(1.0 - k) * size.y * 0.5);
    if tf.scale != scale_v {
        tf.scale = scale_v;
    }
    if tf.translation != shift {
        tf.translation = shift;
    }
}

/// Toasts leave the panels room: below the small one, beside the full one.
fn place_toasts(
    ui: Res<UiState>,
    full: Query<(&FullRoot, &ComputedNode)>,
    mut toasts: Query<&mut Node, With<super::toasts::ToastStack>>,
) {
    let (top, left) = match ui.perf {
        PerfDetail::Small => (TOP + SMALL_H + 8.0, 0.0),
        PerfDetail::Full => {
            let w = full
                .single()
                .map(|(r, c)| c.size.x * c.inverse_scale_factor * r.fit)
                .unwrap_or(FULL_W);
            (super::toasts::STACK_TOP, w + 8.0)
        }
        PerfDetail::Off => return,
    };
    for mut n in &mut toasts {
        if n.top != px(top) {
            n.top = px(top);
        }
        if n.left != px(left) {
            n.left = px(left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady(ms: f32, secs: f64) -> VecDeque<(f64, f32)> {
        let mut v = VecDeque::new();
        let mut t = 0.0;
        while t < secs {
            t += ms as f64 / 1000.0;
            v.push_back((t, ms));
        }
        v
    }

    #[test]
    fn steady_frames_read_as_their_rate() {
        let v = steady(10.0, 5.0);
        let now = v.back().unwrap().0;
        let s = frame_stats(&v, now).unwrap();
        assert!((s.fps - 100.0).abs() < 0.5, "{s:?}");
        assert!((s.fps_avg - 100.0).abs() < 0.5, "{s:?}");
        assert!((s.low_1 - 100.0).abs() < 0.5, "{s:?}");
        assert_eq!(s.max_ms, 10.0);
    }

    #[test]
    fn a_stall_shows_in_the_slowest_frames_and_as_a_wide_spike() {
        let mut v = steady(10.0, 3.0);
        let mut t = v.back().unwrap().0 + 0.25;
        v.push_back((t, 250.0));
        for _ in 0..100 {
            t += 0.01;
            v.push_back((t, 10.0));
        }
        let s = frame_stats(&v, t).unwrap();
        assert_eq!(s.max_ms, 250.0);
        // 401 frames: the slowest 4 include the stall.
        assert!(s.low_1 < 20.0, "{s:?}");
        let cols = columns(&v, t, 5.0, 100);
        // 50 ms columns: the stall covers five of them.
        let hot = cols.iter().filter(|&&c| c == 250.0).count();
        assert!((5..=6).contains(&hot), "{hot} {cols:?}");
        // The oldest columns (before the first frame) are unknown.
        assert_eq!(cols[0], -1.0);
        assert_eq!(*cols.last().unwrap(), 10.0);
        assert_eq!(graph_scale(250.0), 200.0);
        assert_eq!(graph_scale(10.0), 25.0);
        assert_eq!(graph_scale(30.0), 50.0);
    }

    #[test]
    fn f3_cycles() {
        let mut d = PerfDetail::Off;
        let mut seen = vec![];
        for _ in 0..3 {
            d = next_detail(d);
            seen.push(d);
        }
        assert_eq!(seen, [PerfDetail::Small, PerfDetail::Full, PerfDetail::Off]);
    }

    #[test]
    fn pass_names_read_as_words() {
        assert_eq!(pass_name("main_opaque_pass_3d"), "Opaque");
        assert_eq!(pass_name("per_view_shadow_pass"), "Shadows");
        assert_eq!(pass_name("some_new_pass"), "Some new pass");
        assert_eq!(cpu_name("AMD Ryzen 5 2600X Six-Core Processor"), "AMD Ryzen 5 2600X");
        assert_eq!(
            cpu_name("Intel(R) Core(TM) i7-8700K CPU @ 3.70GHz"),
            "Intel Core i7-8700K"
        );
    }
}
