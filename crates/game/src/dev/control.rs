//! `CITYSIM_CONTROL=dir`: a file-based control channel for scripted
//! play-throughs (`scripts/play.py`). Development only.
//!
//! The script appends lines `SEQ COMMAND ARGS...` to `dir/cmd`; the game
//! polls the file a few times a second and appends `SEQ ok|err TEXT` to
//! `dir/reply` (some commands reply later, when they finish). Player input
//! itself goes through the real OS (xdotool); this channel only reports
//! where things are on screen, frames the camera, fast-forwards, takes
//! screenshots and reads numbers.
//!
//! Commands:
//! * `screen X Y` — window position (logical px) of cell (X, Y)'s centre:
//!   `X Y inside|outside`.
//! * `hover` — the hovered cell and whether the pointer is over UI.
//! * `preview-road X1 Y1 X2 Y2` — `ok COST`, `blocked X Y` (a building in the
//!   way) or `error ...` (the sim's `CommandError`), without building.
//! * `camera X Z YAW PITCH DIST` (metres) / `look CX CY YAW PITCH DIST`
//!   (cells) / `camera off` — set or clear `CameraOverride`.
//! * `shot PATH` — screenshot of the window, replies once written.
//! * `stats` — one `key=value` line of game numbers.
//! * `turbo N` — N extra sim steps per fixed tick (0 = off; not while paused).
//! * `until-pop N [T]` / `until-day N [T]` / `until-hour H [T]` — T extra
//!   steps per fixed tick (default `turbo` if set, else 30) until reached,
//!   then reply and stop.
//! * `fps`, `counts` (entities, meshes, materials, vehicles, step time).
//! * `window W H` — resize the window. `quit` — exit.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::window::PrimaryWindow;
use citysim_sim::Cell;

use crate::shared::{
    ActiveTool, CameraOverride, CameraPose, HoveredCell, OverlayMode, PointerOverUi, SimClock, SimStep, SimWorld,
    WorldCamera, cell_center,
};

const POLL_SECS: f32 = 0.1;
const DEFAULT_UNTIL_TURBO: u32 = 30;

pub struct ControlPlugin {
    pub dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Goal {
    Pop(u32),
    Day(u32),
    /// Tick of day.
    Hour(u32),
}

#[derive(Resource)]
struct Control {
    dir: PathBuf,
    offset: u64,
    since_poll: f32,
    turbo: u32,
    until: Option<(u64, Goal, u32)>,
    frame_times: VecDeque<f32>,
    step_started: Option<Instant>,
    tick_before: u64,
    /// (seconds per sim step, steps) samples from recent fixed updates.
    step_samples: VecDeque<(f64, u64)>,
}

impl Control {
    fn reply(&self, seq: u64, ok: bool, text: &str) {
        reply_to(&self.dir, seq, ok, text);
    }
}

fn reply_to(dir: &std::path::Path, seq: u64, ok: bool, text: &str) {
    let line = format!("{seq} {} {}\n", if ok { "ok" } else { "err" }, text.replace('\n', " "));
    let res = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("reply"))
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = res {
        warn!("control: cannot write reply: {e}");
    }
}

impl Plugin for ControlPlugin {
    fn build(&self, app: &mut App) {
        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            warn!("control: cannot create {}: {e}", self.dir.display());
        }
        // Start from a clean channel.
        let _ = std::fs::write(self.dir.join("cmd"), "");
        let _ = std::fs::write(self.dir.join("reply"), "");
        info!("control channel at {}", self.dir.display());
        app.insert_resource(Control {
            dir: self.dir.clone(),
            offset: 0,
            since_poll: 0.0,
            turbo: 0,
            until: None,
            frame_times: VecDeque::new(),
            step_started: None,
            tick_before: 0,
            step_samples: VecDeque::new(),
        })
        .add_systems(FixedUpdate, (start_timing.before(SimStep), turbo_steps.after(SimStep)))
        .add_systems(Update, poll_commands);
    }
}

fn start_timing(mut ctl: ResMut<Control>, sim: Res<SimWorld>) {
    ctl.step_started = Some(Instant::now());
    ctl.tick_before = sim.world.time.tick;
}

fn reached(world: &citysim_sim::World, goal: Goal) -> bool {
    match goal {
        Goal::Pop(n) => world.stats.population >= n,
        Goal::Day(d) => world.time.day >= d,
        Goal::Hour(t) => world.time.tick_of_day == t,
    }
}

fn turbo_steps(mut ctl: ResMut<Control>, mut sim: ResMut<SimWorld>, clock: Res<SimClock>) {
    if !clock.paused {
        let n = match ctl.until {
            Some((_, _, n)) => n,
            None => ctl.turbo,
        };
        for _ in 0..n {
            if let Some((seq, goal, _)) = ctl.until
                && reached(&sim.world, goal)
            {
                ctl.until = None;
                let w = &sim.world;
                ctl.reply(
                    seq,
                    true,
                    &format!(
                        "day={} hour={:.2} pop={}",
                        w.time.day,
                        w.time.hour(),
                        w.stats.population
                    ),
                );
                break;
            }
            sim.world.step();
        }
    }
    if let Some(start) = ctl.step_started.take() {
        let steps = sim.world.time.tick - ctl.tick_before;
        if steps > 0 {
            let per = start.elapsed().as_secs_f64() / steps as f64;
            ctl.step_samples.push_back((per, steps));
            while ctl.step_samples.len() > 400 {
                ctl.step_samples.pop_front();
            }
        }
    }
}

fn stats_line(sim: &SimWorld, clock: &SimClock, tool: ActiveTool, overlay: OverlayMode) -> String {
    let w = &sim.world;
    let s = &w.stats;
    let e = &w.economy;
    // Zoned, still empty cells per zone (what a player sees as free land).
    let mut free = [0u32; 3];
    for cd in &w.grid.cells {
        if cd.occupant == citysim_sim::Occupant::Empty
            && let Some(z) = cd.zone
        {
            free[z as usize] += 1;
        }
    }
    format!(
        "day={} hour={:.2} tick={} pop={} households={} unemployment={:.3} jobs={} buildings={} res={} com={} ind={} \
         cars={} money={} income={} expenses={} tax={:.3} commute_s={:.1} speed={:.2} congestion={:.3} \
         demand_r={:.3} demand_c={:.3} demand_i={:.3} paused={} speed_idx={} segments={} nodes={} road_cells={} \
         tool={} overlay={:?} days_logged={} free_r={} free_c={} free_i={}",
        w.time.day,
        w.time.hour(),
        w.time.tick,
        s.population,
        s.households,
        s.unemployment,
        s.jobs,
        s.buildings,
        s.buildings_by_kind[0],
        s.buildings_by_kind[1],
        s.buildings_by_kind[2],
        w.vehicles.len(),
        e.money,
        e.last_day.income(),
        e.last_day.expenses(),
        e.tax_rate,
        s.avg_commute_s,
        s.avg_speed,
        s.congestion_share,
        w.demand.residential,
        w.demand.commercial,
        w.demand.industrial,
        clock.paused,
        clock.speed,
        w.roads.segments.len(),
        w.roads.nodes.len(),
        s.road_cells,
        tool_name(tool),
        overlay,
        e.history.len(),
        free[0],
        free[1],
        free[2],
    )
}

fn tool_name(t: ActiveTool) -> &'static str {
    use citysim_sim::Zone;
    match t {
        ActiveTool::Inspect => "inspect",
        ActiveTool::Road => "road",
        ActiveTool::Bulldoze => "bulldoze",
        ActiveTool::Zone(Some(Zone::Residential)) => "zone-r",
        ActiveTool::Zone(Some(Zone::Commercial)) => "zone-c",
        ActiveTool::Zone(Some(Zone::Industrial)) => "zone-i",
        ActiveTool::Zone(None) => "dezone",
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct Ctx<'w, 's> {
    sim: Res<'w, SimWorld>,
    clock: Res<'w, SimClock>,
    tool: Res<'w, ActiveTool>,
    overlay: Res<'w, OverlayMode>,
    hovered: Res<'w, HoveredCell>,
    over_ui: Res<'w, PointerOverUi>,
    camera: ResMut<'w, CameraOverride>,
    window: Query<'w, 's, &'static mut Window, With<PrimaryWindow>>,
    cams: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<WorldCamera>>,
    entities: Query<'w, 's, ()>,
    meshes: Res<'w, Assets<Mesh>>,
    materials: Res<'w, Assets<StandardMaterial>>,
    exit: MessageWriter<'w, AppExit>,
}

fn poll_commands(mut commands: Commands, time: Res<Time<Real>>, mut ctl: ResMut<Control>, mut cx: Ctx) {
    let dt = time.delta_secs();
    ctl.frame_times.push_back(dt);
    while ctl.frame_times.len() > 60 {
        ctl.frame_times.pop_front();
    }
    ctl.since_poll += dt;
    if ctl.since_poll < POLL_SECS {
        return;
    }
    ctl.since_poll = 0.0;
    let Ok(bytes) = std::fs::read(ctl.dir.join("cmd")) else {
        return;
    };
    if (bytes.len() as u64) <= ctl.offset {
        return;
    }
    let new = &bytes[ctl.offset as usize..];
    // Only consume complete lines.
    let Some(end) = new.iter().rposition(|&b| b == b'\n') else {
        return;
    };
    ctl.offset += end as u64 + 1;
    let text = String::from_utf8_lossy(&new[..end]).to_string();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(seq) = parts.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let Some(name) = parts.next() else { continue };
        let args: Vec<&str> = parts.collect();
        if let Err(e) = run(&mut commands, &mut ctl, &mut cx, seq, name, &args) {
            ctl.reply(seq, false, &e);
        }
    }
}

fn num<T: std::str::FromStr>(args: &[&str], i: usize) -> Result<T, String> {
    args.get(i)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("argument {} missing or invalid", i + 1))
}

fn run(
    commands: &mut Commands,
    ctl: &mut Control,
    cx: &mut Ctx,
    seq: u64,
    name: &str,
    args: &[&str],
) -> Result<(), String> {
    match name {
        "screen" => {
            let cell = Cell::new(num(args, 0)?, num(args, 1)?);
            let (cam, tf) = cx.cams.single().map_err(|_| "no world camera".to_string())?;
            let win = cx.window.single().map_err(|_| "no window".to_string())?;
            let p = cam
                .world_to_viewport(tf, cell_center(cell, 0.0))
                .map_err(|e| format!("not visible: {e:?}"))?;
            let inside = p.x >= 0.0 && p.y >= 0.0 && p.x < win.width() && p.y < win.height();
            ctl.reply(
                seq,
                true,
                &format!("{:.0} {:.0} {}", p.x, p.y, if inside { "inside" } else { "outside" }),
            );
        }
        "preview-road" => {
            // What the sim says about a road (no change): `ok COST` or the
            // CommandError, e.g. `BlockedByBuilding(Cell { x: 20, y: 125 })`.
            let cmd = citysim_sim::Command::BuildRoad {
                from: Cell::new(num(args, 0)?, num(args, 1)?),
                to: Cell::new(num(args, 2)?, num(args, 3)?),
            };
            match cx.sim.world.preview(&cmd) {
                Ok(p) => ctl.reply(seq, true, &format!("ok {}", p.cost)),
                Err(citysim_sim::CommandError::BlockedByBuilding(c)) => {
                    ctl.reply(seq, true, &format!("blocked {} {}", c.x, c.y))
                }
                Err(e) => ctl.reply(seq, true, &format!("error {e:?}")),
            }
        }
        "hover" => {
            let c = cx.hovered.0.map_or("none".to_string(), |c| format!("{} {}", c.x, c.y));
            ctl.reply(seq, true, &format!("cell={c} over_ui={}", cx.over_ui.0));
        }
        "camera" | "look" => {
            if args.first() == Some(&"off") {
                cx.camera.0 = None;
            } else {
                let (a, b): (f32, f32) = (num(args, 0)?, num(args, 1)?);
                let focus = if name == "look" {
                    cell_center(Cell::new(a as i32, b as i32), 0.0)
                } else {
                    Vec3::new(a, 0.0, b)
                };
                cx.camera.0 = Some(CameraPose {
                    focus,
                    yaw: num(args, 2)?,
                    pitch: num(args, 3)?,
                    distance: num(args, 4)?,
                });
            }
            ctl.reply(seq, true, "camera set");
        }
        "shot" => {
            let path = args.first().ok_or("shot needs a path")?.to_string();
            let dir = ctl.dir.clone();
            let p2 = path.clone();
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path))
                .observe(move |_: On<ScreenshotCaptured>| reply_to(&dir, seq, true, &p2));
        }
        "stats" => ctl.reply(seq, true, &stats_line(&cx.sim, &cx.clock, *cx.tool, *cx.overlay)),
        "turbo" => {
            ctl.turbo = num(args, 0)?;
            ctl.reply(seq, true, &format!("turbo={}", ctl.turbo));
        }
        "until-pop" | "until-day" | "until-hour" => {
            let goal = match name {
                "until-pop" => Goal::Pop(num(args, 0)?),
                "until-day" => Goal::Day(num(args, 0)?),
                _ => {
                    let h: f32 = num(args, 0)?;
                    let t = (h.rem_euclid(24.0) / 24.0 * citysim_sim::balance::TICKS_PER_DAY as f32) as u32;
                    Goal::Hour(t)
                }
            };
            if let Some((old, _, _)) = ctl.until.take() {
                ctl.reply(old, false, "replaced by a newer until");
            }
            if reached(&cx.sim.world, goal) && !matches!(goal, Goal::Hour(_)) {
                ctl.reply(seq, true, "already reached");
            } else {
                let n = match args.get(1) {
                    Some(_) => num(args, 1)?,
                    None if ctl.turbo > 0 => ctl.turbo,
                    None => DEFAULT_UNTIL_TURBO,
                };
                ctl.until = Some((seq, goal, n));
            }
        }
        "fps" => {
            let total: f32 = ctl.frame_times.iter().sum();
            let fps = if total > 0.0 {
                ctl.frame_times.len() as f32 / total
            } else {
                0.0
            };
            ctl.reply(seq, true, &format!("fps={fps:.1}"));
        }
        "counts" => {
            let (mut sum, mut steps, mut max) = (0.0f64, 0u64, 0.0f64);
            for &(per, n) in &ctl.step_samples {
                sum += per * n as f64;
                steps += n;
                max = max.max(per);
            }
            let mean_ms = if steps > 0 { sum / steps as f64 * 1e3 } else { 0.0 };
            ctl.reply(
                seq,
                true,
                &format!(
                    "entities={} meshes={} materials={} vehicles={} buildings={} step_mean_ms={mean_ms:.3} \
                     step_max_ms={:.3} step_samples={steps}",
                    cx.entities.iter().count(),
                    cx.meshes.len(),
                    cx.materials.len(),
                    cx.sim.world.vehicles.len(),
                    cx.sim.world.buildings.len(),
                    max * 1e3,
                ),
            );
        }
        "window" => {
            let (w, h): (u32, u32) = (num(args, 0)?, num(args, 1)?);
            let mut win = cx.window.single_mut().map_err(|_| "no window".to_string())?;
            win.resolution.set(w as f32, h as f32);
            ctl.reply(seq, true, &format!("window {w}x{h}"));
        }
        "quit" => {
            ctl.reply(seq, true, "bye");
            cx.exit.write(AppExit::Success);
        }
        other => return Err(format!("unknown command {other}")),
    }
    Ok(())
}
