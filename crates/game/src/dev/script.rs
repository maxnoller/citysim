//! `CITYSIM_DEMO=script`: drives the real tools for ~15 s by injecting
//! input (cursor position, mouse buttons, Escape) and setting `ActiveTool`,
//! while moving the camera through `CameraOverride`; exits when done.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use citysim_sim::{Cell, Zone};

use crate::shared::{ActiveTool, CameraOverride, CameraPose, GameSet, WorldCamera, cell_center};

pub struct ScriptPlugin {
    pub delay: f32,
}

#[derive(Resource)]
struct Script {
    delay: f32,
    t: f32,
    frames: u32,
    step: usize,
    step_t: f32,
    /// Current scripted pointer cell.
    cell: Option<Cell>,
    move_from: Option<Cell>,
    held: bool,
}

impl Plugin for ScriptPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Script {
            delay: self.delay,
            t: 0.0,
            frames: 0,
            step: 0,
            step_t: 0.0,
            cell: None,
            move_from: None,
            held: false,
        })
        .add_systems(Update, run_script.in_set(GameSet::Input));
    }
}

#[derive(Clone, Copy)]
enum Step {
    Tool(ActiveTool),
    Move(Cell, f32),
    Press,
    Release,
    Click,
    Cancel,
    Wait(f32),
}

fn steps() -> Vec<Step> {
    use Step::*;
    let c = Cell::new;
    let zone = |z| Tool(ActiveTool::Zone(Some(z)));
    vec![
        Wait(0.8),
        Tool(ActiveTool::Road),
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
        zone(Zone::Residential),
        Move(c(31, 129), 0.4),
        Press,
        Move(c(44, 149), 0.9),
        Release,
        Move(c(31, 105), 0.3),
        Press,
        Move(c(50, 127), 0.7),
        Release,
        zone(Zone::Commercial),
        Move(c(12, 124), 0.5),
        Press,
        Move(c(29, 127), 0.7),
        Release,
        zone(Zone::Industrial),
        Move(c(46, 129), 0.4),
        Press,
        Move(c(60, 149), 0.7),
        Release,
        Tool(ActiveTool::Road),
        Move(c(60, 128), 0.3),
        Click,
        Move(c(60, 110), 0.8),
        Wait(1.5),
    ]
}

fn camera_at(t: f32) -> CameraPose {
    let focus = |x, y| cell_center(Cell::new(x, y), 0.0);
    let keys = [
        (
            0.0,
            CameraPose {
                focus: focus(24, 128),
                yaw: 0.2,
                pitch: 1.1,
                distance: 380.0,
            },
        ),
        (
            5.0,
            CameraPose {
                focus: focus(38, 134),
                yaw: 0.5,
                pitch: 1.0,
                distance: 470.0,
            },
        ),
        (
            10.0,
            CameraPose {
                focus: focus(40, 136),
                yaw: 0.9,
                pitch: 0.95,
                distance: 480.0,
            },
        ),
        (
            16.0,
            CameraPose {
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
    CameraPose {
        focus: a.focus.lerp(b.focus, s),
        yaw: a.yaw + (b.yaw - a.yaw) * s,
        pitch: a.pitch + (b.pitch - a.pitch) * s,
        distance: a.distance + (b.distance - a.distance) * s,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_script(
    time: Res<Time<Real>>,
    mut script: ResMut<Script>,
    mut tool: ResMut<ActiveTool>,
    mut camera: ResMut<CameraOverride>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    mut exit: MessageWriter<AppExit>,
) {
    let first = script.frames == 0;
    script.frames += 1;
    let dt = if first { 0.0 } else { time.delta_secs().min(0.25) };
    script.t += dt;
    // Synthetic key presses last one frame.
    keys.release(KeyCode::Escape);
    if !script.held {
        mouse.release(MouseButton::Left);
    }
    let t = (script.t - script.delay).max(0.0);
    camera.0 = Some(camera_at(t));
    if script.t < script.delay {
        return;
    }
    script.step_t += dt;
    let steps = steps();
    while let Some(&step) = steps.get(script.step) {
        let done = match step {
            Step::Tool(t) => {
                *tool = t;
                true
            }
            Step::Press => {
                mouse.press(MouseButton::Left);
                script.held = true;
                true
            }
            Step::Release => {
                mouse.release(MouseButton::Left);
                script.held = false;
                true
            }
            Step::Click => {
                mouse.press(MouseButton::Left);
                mouse.release(MouseButton::Left);
                true
            }
            Step::Cancel => {
                keys.press(KeyCode::Escape);
                true
            }
            Step::Wait(secs) => script.step_t >= secs,
            Step::Move(target, secs) => {
                let current = script.cell.unwrap_or(target);
                let from = *script.move_from.get_or_insert(current);
                let s = (script.step_t / secs).clamp(0.0, 1.0);
                let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * s).round() as i32;
                script.cell = Some(Cell::new(lerp(from.x, target.x), lerp(from.y, target.y)));
                s >= 1.0
            }
        };
        if !done {
            break;
        }
        script.step += 1;
        script.step_t = 0.0;
        script.move_from = None;
        if matches!(step, Step::Press | Step::Release | Step::Click | Step::Cancel) {
            break;
        }
    }
    // Put the OS cursor over the scripted cell so hover picking sees it.
    if let (Some(cell), Ok((cam, cam_tf)), Ok(mut win)) = (script.cell, cams.single(), window.single_mut())
        && let Ok(screen) = cam.world_to_viewport(cam_tf, cell_center(cell, 0.0))
    {
        win.set_cursor_position(Some(screen));
    }
    if script.step >= steps.len() {
        info!("script demo finished");
        exit.write(AppExit::Success);
    }
}
