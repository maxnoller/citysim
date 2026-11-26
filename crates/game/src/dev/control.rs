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
//! * `ready` — ok once the scene draws (shaders built), else err.
//! * `fps`, `counts` (entities, meshes, materials, vehicles, step time).
//! * `window W H` — resize the window; `window` alone: its size `W H`.
//! * `find-home` — the cell under a home near the camera's focus whose
//!   first resident is at home, leaves for work within 1.5 in-game hours
//!   and last drove there in 20-90 s: `ok X Y`.
//! * `find-car [MIN]` — window position (logical px) of a moving car near
//!   the middle of the screen with at least MIN metres (300) still to go.
//! * `node PREFIX` — window position (logical px) of the first visible UI
//!   node whose `Name` starts with PREFIX: `X Y` (people-panel links are
//!   named "Person link", "Place link", "Vehicle link", "Follow").
//! * `quit` — exit.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use bevy::window::PrimaryWindow;
use citysim_sim::Cell;

use crate::shared::{
    ActiveTool, CameraFollow, CameraOverride, CameraPose, HoveredCell, OverlayMode, PointerOverUi, RoadChoice,
    SceneReady, SimClock, SimStep, SimWorld, ToolModes, WorldCamera, cell_center,
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
    /// The next time of day asked for, as an absolute tick (normal sim
    /// steps run between the checks, so an exact tick of day could be
    /// stepped over).
    Hour(u64),
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
        Goal::Hour(t) => world.time.tick >= t,
    }
}

fn turbo_steps(mut ctl: ResMut<Control>, mut sim: ResMut<SimWorld>, clock: Res<SimClock>) {
    if !clock.paused {
        let n = match ctl.until {
            Some((_, _, n)) => n,
            None => ctl.turbo,
        };
        // Turbo is per real-time tick; fast speeds split a tick into
        // several fixed updates.
        let k = clock.fixed_per_tick();
        let n = n.div_ceil(k);
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

fn stats_line(sim: &SimWorld, clock: &SimClock, tool: ActiveTool, overlay: OverlayMode, extra: &str) -> String {
    let w = &sim.world;
    let s = &w.stats;
    let e = &w.economy;
    // Zoned, still empty cells per zone (what a player sees as free land).
    let mut free = [0u32; 3];
    for (_, zc) in w.zoning.cells() {
        if zc.free()
            && let Some(z) = zc.zone
        {
            free[z as usize] += 1;
        }
    }
    format!(
        "day={} hour={:.2} tick={} pop={} households={} unemployment={:.3} jobs={} buildings={} res={} com={} ind={} \
         cars={} money={} income={} expenses={} tax={:.3} commute_s={:.1} speed={:.2} congestion={:.3} \
         demand_r={:.3} demand_c={:.3} demand_i={:.3} paused={} speed_idx={} segments={} nodes={} road_cells={} \
         tool={} overlay={:?} days_logged={} free_r={} free_c={} free_i={} {}",
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
        extra,
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
    ready: Res<'w, SceneReady>,
    modes: Res<'w, ToolModes>,
    nodes: Query<
        'w,
        's,
        (
            &'static Name,
            &'static bevy::ui::UiGlobalTransform,
            &'static InheritedVisibility,
        ),
    >,
    kind: Res<'w, RoadChoice>,
    follow: Res<'w, CameraFollow>,
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
                kind: citysim_sim::RoadKind::Street,
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
        "stats" => {
            // Tool modes, road kind and camera follow, for scripts that switch
            // them with real keys.
            let extra = format!(
                "road_mode={:?} zone_mode={:?} bulldoze_mode={:?} road_kind={:?} following={}",
                cx.modes.road,
                cx.modes.zone,
                cx.modes.bulldoze,
                *cx.kind,
                cx.follow.focus.is_some()
            );
            ctl.reply(
                seq,
                true,
                &stats_line(&cx.sim, &cx.clock, *cx.tool, *cx.overlay, &extra),
            )
        }
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
                    let day = citysim_sim::balance::TICKS_PER_DAY as u64;
                    let t = (h.rem_euclid(24.0) / 24.0 * day as f32) as u64;
                    let w = &cx.sim.world;
                    let ahead = (t + day - w.time.tick_of_day as u64 % day) % day;
                    Goal::Hour(w.time.tick + if ahead == 0 { day } else { ahead })
                }
            };
            if let Some((old, _, _)) = ctl.until.take() {
                ctl.reply(old, false, "replaced by a newer until");
            }
            if reached(&cx.sim.world, goal) && !matches!(goal, Goal::Hour(_)) {
                ctl.reply(seq, true, "already reached");
            } else if cx.clock.paused {
                // Turbo only runs while the clock does: fail instead of hanging.
                return Err("the game is paused".to_string());
            } else {
                let n = match args.get(1) {
                    Some(_) => num(args, 1)?,
                    None if ctl.turbo > 0 => ctl.turbo,
                    None => DEFAULT_UNTIL_TURBO,
                };
                ctl.until = Some((seq, goal, n));
            }
        }
        "ready" => {
            if cx.ready.0 {
                ctl.reply(seq, true, "ready");
            } else {
                return Err("still loading".into());
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
        "window" if args.is_empty() => {
            let win = cx.window.single().map_err(|_| "no window".to_string())?;
            ctl.reply(seq, true, &format!("{:.0} {:.0}", win.width(), win.height()));
        }
        "find-home" => {
            let w = &cx.sim.world;
            let focus = cx
                .cams
                .single()
                .ok()
                .and_then(|(cam, tf)| {
                    let win = cx.window.single().ok()?;
                    let ray = cam
                        .viewport_to_world(tf, Vec2::new(win.width(), win.height()) * 0.5)
                        .ok()?;
                    ray.plane_intersection_point(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))
                })
                .map_or([1024.0, 1024.0], |p| [p.x, p.z]);
            // The first resident (the first one its people panel lists) is
            // at home and leaves for work within two in-game hours.
            let now = w.time.tick;
            let hours = 3 * citysim_sim::balance::TICKS_PER_HOUR as u64 / 2;
            // ... and whose last commute was short (an interesting drive to
            // follow, not a jam at the door).
            let worker_at_home = |b: citysim_sim::BuildingId| {
                citysim_sim::inspect::residents(w, b).next().is_some_and(|c| {
                    w.citizens.get(c).is_some_and(|ct| {
                        ct.job.is_some()
                            && ct.location.building() == Some(b)
                            && (20.0..240.0).contains(&ct.last_commute_s)
                    }) && citysim_sim::inspect::plan(w, c).and_then(|p| p.next).is_some_and(|n| {
                        n.purpose == Some(citysim_sim::TripPurpose::ToWork)
                            && n.at_tick.is_some_and(|t| t >= now && t < now + hours)
                    })
                })
            };
            let best = w
                .buildings
                .iter()
                .filter(|(id, b)| b.kind == citysim_sim::Zone::Residential && b.occupied > 0 && worker_at_home(*id))
                .map(|(_, b)| b.center())
                .min_by(|a, b| {
                    let d = |p: [f32; 2]| (p[0] - focus[0]).powi(2) + (p[1] - focus[1]).powi(2);
                    d(*a).total_cmp(&d(*b))
                })
                .ok_or("no lived-in home")?;
            let c = Cell::from_pos(best);
            ctl.reply(seq, true, &format!("ok {} {}", c.x, c.y));
        }
        "find-car" => {
            // A car on screen (clear of the bar and the side panel) with at
            // least MIN metres still to drive: its window position.
            let min: f32 = num(args, 0).unwrap_or(300.0);
            let w = &cx.sim.world;
            let win = cx.window.single().map_err(|_| "no window".to_string())?;
            let (cam, tf) = cx.cams.single().map_err(|_| "no camera".to_string())?;
            let size = Vec2::new(win.width(), win.height());
            let centre = size * 0.5;
            let len = |s: citysim_sim::SegmentId| w.roads.segments.get(s).map_or(0.0, |g| g.length);
            let hit = w
                .vehicles
                .iter()
                .filter(|(_, v)| v.junction.is_none() && !v.stopped())
                .filter_map(|(_, v)| {
                    let ahead = v.segment.map_or(0.0, |s| len(s) - v.progress) + v.route.iter().map(|&s| len(s)).sum::<f32>();
                    if ahead < min {
                        return None;
                    }
                    let pos = citysim_sim::drive::vehicle_pose(w, v).0;
                    let p = cam.world_to_viewport(tf, crate::shared::sim_to_world(pos, 0.5)).ok()?;
                    let inside = p.x > 120.0 && p.x < size.x - 460.0 && p.y > 110.0 && p.y < size.y - 170.0;
                    inside.then_some(p)
                })
                .min_by(|a, b| a.distance(centre).total_cmp(&b.distance(centre)))
                .ok_or("no car on screen with that far to go")?;
            ctl.reply(seq, true, &format!("{:.0} {:.0}", hit.x, hit.y));
        }
        "node" => {
            let prefix = args.join(" ");
            let win = cx.window.single().map_err(|_| "no window".to_string())?;
            let hit = cx
                .nodes
                .iter()
                .find(|(n, _, v)| v.get() && n.as_str().starts_with(&prefix))
                .map(|(_, tf, _)| tf.translation / win.scale_factor())
                .ok_or_else(|| format!("no node named {prefix}"))?;
            ctl.reply(seq, true, &format!("{:.0} {:.0}", hit.x, hit.y));
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
