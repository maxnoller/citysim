//! Scripted, hands-free runs for checking rendering without a human.
//!
//! * `CITYSIM_AUTOSHOT=out.png` (or `--autoshot out.png`): builds a small
//!   sample city through `SimCommand`s, shows a road ghost, waits
//!   `CITYSIM_AUTOSHOT_DELAY` seconds (default 3), saves a screenshot, exits.
//! * `CITYSIM_DEMO=1` (or `--demo`): drives the real tools with a scripted
//!   pointer and moves the camera for ~15 s, then exits. Used for videos.
//!   `CITYSIM_DEMO_DELAY=secs` holds still before starting (for recorders).

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use citysim_sim::{Cell, Command, Zone};

use crate::GameSet;
use crate::camera::{CameraRig, CameraScripted};
use crate::cursor::{Pointer, PointerScripted};
use crate::sim_driver::SimCommand;
use crate::tools::{Tool, ToolState};

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptMode {
    Interactive,
    Autoshot {
        path: String,
        delay: f32,
    },
    /// `delay`: seconds to hold the first camera keyframe before starting.
    Demo {
        delay: f32,
    },
}

impl ScriptMode {
    pub fn from_env() -> ScriptMode {
        let args: Vec<String> = std::env::args().collect();
        let arg_after = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let delay = std::env::var("CITYSIM_AUTOSHOT_DELAY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3.0);
        if let Some(path) = arg_after("--autoshot").or_else(|| std::env::var("CITYSIM_AUTOSHOT").ok()) {
            return ScriptMode::Autoshot { path, delay };
        }
        if args.iter().any(|a| a == "--demo") || std::env::var("CITYSIM_DEMO").is_ok_and(|v| !v.is_empty() && v != "0")
        {
            let delay = std::env::var("CITYSIM_DEMO_DELAY")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            return ScriptMode::Demo { delay };
        }
        ScriptMode::Interactive
    }
}

pub struct ScriptPlugin {
    pub mode: ScriptMode,
}

#[derive(Resource)]
struct Script {
    mode: ScriptMode,
    /// Seconds since the first Update.
    t: f32,
    frames: u32,
    shot_taken: bool,
    // Demo state.
    step: usize,
    step_t: f32,
    move_from: Option<Cell>,
}

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        if self.mode == ScriptMode::Interactive {
            return;
        }
        app.insert_resource(Script {
            mode: self.mode.clone(),
            t: 0.0,
            frames: 0,
            shot_taken: false,
            step: 0,
            step_t: 0.0,
            move_from: None,
        })
        .insert_resource(PointerScripted)
        .insert_resource(CameraScripted)
        .add_systems(Update, run_script.in_set(GameSet::Input));
    }
}

/// Sample city used by the autoshot (all through the normal command path).
pub fn sample_city() -> Vec<Command> {
    let c = Cell::new;
    vec![
        Command::BuildRoad {
            from: c(11, 128),
            to: c(60, 128),
        },
        Command::BuildRoad {
            from: c(30, 104),
            to: c(30, 156),
        },
        Command::BuildRoad {
            from: c(45, 128),
            to: c(45, 150),
        },
        Command::BuildRoad {
            from: c(30, 150),
            to: c(60, 150),
        },
        Command::BuildRoad {
            from: c(30, 104),
            to: c(52, 126),
        },
        Command::SetZone {
            min: c(31, 129),
            max: c(44, 149),
            zone: Some(Zone::Residential),
        },
        Command::SetZone {
            min: c(12, 124),
            max: c(29, 127),
            zone: Some(Zone::Commercial),
        },
        Command::SetZone {
            min: c(46, 129),
            max: c(60, 149),
            zone: Some(Zone::Industrial),
        },
        Command::SetZone {
            min: c(31, 105),
            max: c(50, 127),
            zone: Some(Zone::Residential),
        },
    ]
}

fn shot_rig() -> CameraRig {
    CameraRig {
        focus: crate::coords::cell_center(Cell::new(40, 137), 0.0),
        yaw: 0.3,
        pitch: 1.0,
        distance: 470.0,
    }
}

#[derive(Clone, Copy)]
enum Step {
    Tool(Tool),
    Move(Cell, f32),
    Press,
    Release,
    Click,
    Cancel,
    Wait(f32),
}

fn demo_steps() -> Vec<Step> {
    use Step::*;
    let c = Cell::new;
    vec![
        Wait(0.8),
        Tool(crate::tools::Tool::Road),
        Move(c(11, 128), 0.4),
        Click,
        Move(c(60, 128), 1.2),
        Click,
        Cancel,
        Move(c(30, 104), 0.5),
        Click,
        Move(c(30, 156), 1.0),
        Click,
        Cancel,
        Move(c(45, 128), 0.4),
        Click,
        Move(c(45, 150), 0.6),
        Click,
        Move(c(60, 150), 0.6),
        Click,
        Cancel,
        Move(c(30, 104), 0.4),
        Click,
        Move(c(52, 126), 0.9),
        Click,
        Cancel,
        Tool(crate::tools::Tool::Zone(Some(Zone::Residential))),
        Move(c(31, 129), 0.4),
        Press,
        Move(c(44, 149), 0.9),
        Release,
        Move(c(31, 105), 0.3),
        Press,
        Move(c(50, 127), 0.7),
        Release,
        Tool(crate::tools::Tool::Zone(Some(Zone::Commercial))),
        Move(c(12, 124), 0.5),
        Press,
        Move(c(29, 127), 0.7),
        Release,
        Tool(crate::tools::Tool::Zone(Some(Zone::Industrial))),
        Move(c(46, 129), 0.4),
        Press,
        Move(c(60, 149), 0.7),
        Release,
        Tool(crate::tools::Tool::Road),
        Move(c(60, 128), 0.3),
        Click,
        Move(c(60, 110), 0.8),
        Wait(1.5),
    ]
}

/// Camera keyframes for the demo: (time, rig).
fn demo_camera(t: f32) -> CameraRig {
    let focus = |x, y| crate::coords::cell_center(Cell::new(x, y), 0.0);
    let keys = [
        (
            0.0,
            CameraRig {
                focus: focus(24, 128),
                yaw: 0.2,
                pitch: 1.1,
                distance: 380.0,
            },
        ),
        (
            5.0,
            CameraRig {
                focus: focus(38, 134),
                yaw: 0.5,
                pitch: 1.0,
                distance: 470.0,
            },
        ),
        (
            10.0,
            CameraRig {
                focus: focus(40, 136),
                yaw: 0.9,
                pitch: 0.95,
                distance: 480.0,
            },
        ),
        (
            16.0,
            CameraRig {
                focus: focus(40, 134),
                yaw: 1.4,
                pitch: 0.9,
                distance: 520.0,
            },
        ),
    ];
    let i = keys.iter().rposition(|(kt, _)| *kt <= t).unwrap_or(0);
    let (t0, a) = keys[i];
    let Some(&(t1, b)) = keys.get(i + 1) else { return a };
    let s = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    let s = s * s * (3.0 - 2.0 * s);
    CameraRig {
        focus: a.focus.lerp(b.focus, s),
        yaw: a.yaw + (b.yaw - a.yaw) * s,
        pitch: a.pitch + (b.pitch - a.pitch) * s,
        distance: a.distance + (b.distance - a.distance) * s,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_script(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut script: ResMut<Script>,
    mut pointer: ResMut<Pointer>,
    mut tools: ResMut<ToolState>,
    mut rigs: Query<&mut CameraRig>,
    mut out: MessageWriter<SimCommand>,
    mut exit: MessageWriter<AppExit>,
) {
    let first = script.frames == 0;
    script.frames += 1;
    if !first {
        script.t += time.delta_secs().min(0.25);
    }
    pointer.pressed = false;
    pointer.released = false;
    pointer.cancel = false;
    if script.t > 90.0 {
        error!("scripted run timed out");
        exit.write(AppExit::error());
        return;
    }
    match script.mode.clone() {
        ScriptMode::Interactive => {}
        ScriptMode::Autoshot { path, delay } => {
            if first {
                for cmd in sample_city() {
                    out.write(SimCommand(cmd));
                }
                for mut rig in &mut rigs {
                    *rig = shot_rig();
                }
                tools.tool = Tool::Road;
                tools.anchor = Some(Cell::new(60, 128));
                pointer.cell = Some(Cell::new(60, 110));
            }
            if !script.shot_taken && script.t >= delay && script.frames > 30 {
                script.shot_taken = true;
                info!("autoshot: saving {path}");
                commands
                    .spawn(Screenshot::primary_window())
                    .observe(save_to_disk(path))
                    .observe(|_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                        exit.write(AppExit::Success);
                    });
            }
        }
        ScriptMode::Demo { delay } => {
            if script.t < delay {
                for mut rig in &mut rigs {
                    *rig = demo_camera(0.0);
                }
                return;
            }
            let t = script.t - delay;
            for mut rig in &mut rigs {
                *rig = demo_camera(t);
            }
            let steps = demo_steps();
            let dt = if first { 0.0 } else { time.delta_secs().min(0.25) };
            script.step_t += dt;
            // Run instantaneous steps until one takes time.
            while let Some(&step) = steps.get(script.step) {
                let done = match step {
                    Step::Tool(tool) => {
                        tools.tool = tool;
                        tools.anchor = None;
                        true
                    }
                    Step::Press => {
                        pointer.pressed = true;
                        pointer.held = true;
                        true
                    }
                    Step::Release => {
                        pointer.released = true;
                        pointer.held = false;
                        true
                    }
                    Step::Click => {
                        pointer.pressed = true;
                        pointer.released = true;
                        true
                    }
                    Step::Cancel => {
                        pointer.cancel = true;
                        true
                    }
                    Step::Wait(secs) => script.step_t >= secs,
                    Step::Move(target, secs) => {
                        let from = *script.move_from.get_or_insert(pointer.cell.unwrap_or(target));
                        let s = (script.step_t / secs).clamp(0.0, 1.0);
                        let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * s).round() as i32;
                        pointer.cell = Some(Cell::new(lerp(from.x, target.x), lerp(from.y, target.y)));
                        s >= 1.0
                    }
                };
                if !done {
                    break;
                }
                script.step += 1;
                script.step_t = 0.0;
                script.move_from = None;
                // One button event per frame so tools see each separately.
                if matches!(step, Step::Press | Step::Release | Step::Click | Step::Cancel) {
                    break;
                }
            }
            if script.step >= steps.len() {
                info!("demo finished");
                exit.write(AppExit::Success);
            }
        }
    }
}
