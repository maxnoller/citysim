//! `CITYSIM_UI_SHOTS=dir` (best with `CITYSIM_DEMO=town`): walks through
//! every interface state and saves `dir/NN-name.png` for each, then exits.
//! States: idle HUD, inspect a building and a road, a tooltip, road preview
//! (valid and blocked), zone drag with the flyout, bulldoze drag, budget,
//! toasts, pause menu, first-run hint, and the HUD at 2560x1440.
//! `CITYSIM_UI_SHOTS_ONLY=name,name` limits it to some states.
//!
//! Drives the real ui through synthetic cursor / mouse input and resources.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::ui::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use citysim_sim::{Cell, Occupant, Zone};

use super::UiState;
use super::toasts::{Toast, ToastKind};
use super::tools::{Action, ToolState};
use super::widgets::{UiAction, UiButton};
use crate::shared::{
    ActiveTool, CameraOverride, CameraPose, GameSet, OverlayMode, SimClock, SimWorld, WorldCamera, cell_center,
};

pub struct ShotsPlugin {
    pub dir: PathBuf,
}

impl Plugin for ShotsPlugin {
    fn build(&self, app: &mut App) {
        let only: Option<Vec<String>> = std::env::var("CITYSIM_UI_SHOTS_ONLY")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.split(',').map(|p| p.trim().to_string()).collect());
        let steps: Vec<Step> = steps()
            .into_iter()
            .filter(|s| only.as_ref().is_none_or(|o| o.iter().any(|n| n == s.name)))
            .collect();
        app.insert_resource(Runner {
            dir: self.dir.clone(),
            steps,
            index: 0,
            t: 0.0,
            entered: false,
            shot: false,
            frames: 0,
        })
        .init_resource::<Pointer>()
        .add_systems(
            Update,
            run_shots.in_set(GameSet::Input).before(super::widgets::button_clicks),
        )
        .add_systems(
            Update,
            drive_pointer
                .in_set(GameSet::Input)
                .after(run_shots)
                .before(super::widgets::button_clicks),
        );
    }
}

/// Where the synthetic pointer is and whether the left button is held.
#[derive(Resource, Default, Clone, Copy)]
struct Pointer {
    target: PointerTarget,
    hold: bool,
    /// Tool action to force (re-applied every frame: a tool change resets it).
    action: Option<Action>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum PointerTarget {
    #[default]
    None,
    Cell(Cell),
    Button(UiAction),
}

type Setup = fn(&mut World);

struct Step {
    name: &'static str,
    /// Seconds to let the state settle before the capture.
    wait: f32,
    setup: Setup,
}

#[derive(Resource)]
struct Runner {
    dir: PathBuf,
    steps: Vec<Step>,
    index: usize,
    t: f32,
    entered: bool,
    shot: bool,
    frames: u32,
}

fn pose() -> CameraPose {
    CameraPose {
        focus: cell_center(Cell::new(48, 130), 0.0),
        yaw: 0.35,
        pitch: 0.92,
        distance: 360.0,
    }
}

fn reset(world: &mut World) {
    world.resource_mut::<CameraOverride>().0 = Some(pose());
    *world.resource_mut::<ActiveTool>() = ActiveTool::Inspect;
    *world.resource_mut::<OverlayMode>() = OverlayMode::None;
    world.resource_mut::<ToolState>().action = Action::Idle;
    let mut ui = world.resource_mut::<UiState>();
    ui.budget_open = false;
    ui.hint_visible = false;
    ui.menu_open = false;
    world.resource_mut::<SimClock>().paused = false;
    *world.resource_mut::<Pointer>() = Pointer::default();
}

/// A building cell (prefers one near the camera focus) and an empty cell
/// a few cells west of it on the same row.
fn find_building(world: &World) -> Option<Cell> {
    let sim = world.resource::<SimWorld>();
    let focus = Cell::new(48, 130);
    sim.world
        .buildings
        .values()
        .map(|b| b.footprint.min)
        .min_by_key(|c| c.chebyshev(focus))
}

fn find_road(world: &World) -> Option<Cell> {
    let sim = &world.resource::<SimWorld>().world;
    let focus = Cell::new(44, 128);
    let mut best: Option<(i32, Cell)> = None;
    for y in focus.y - 20..focus.y + 20 {
        for x in focus.x - 20..focus.x + 20 {
            let c = Cell::new(x, y);
            if sim.grid.get(c).is_some_and(|cd| cd.occupant == Occupant::Road)
                && sim.roads.segment_through(c.center()).is_some()
            {
                let d = c.chebyshev(focus);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, c));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

/// A buildable new road from an existing road cell across empty land.
fn find_new_road(world: &World) -> Option<(Cell, Cell)> {
    let sim = &world.resource::<SimWorld>().world;
    let focus = Cell::new(46, 126);
    let mut best: Option<(i32, Cell, Cell)> = None;
    for y in focus.y - 24..focus.y + 24 {
        for x in focus.x - 24..focus.x + 24 {
            let start = Cell::new(x, y);
            if !sim.grid.is_road(start) {
                continue;
            }
            for (dx, dy) in [(1, 1), (1, -1), (0, 1), (1, 0)] {
                let end = start.offset(dx * 9, dy * 9);
                let cmd = citysim_sim::Command::BuildRoad { from: start, to: end };
                if let Ok(p) = sim.preview(&cmd)
                    && p.cost >= 8 * citysim_sim::balance::ROAD_COST_PER_CELL
                {
                    let d = start.chebyshev(focus);
                    if best.is_none_or(|(bd, ..)| d < bd) {
                        best = Some((d, start, end));
                    }
                }
            }
        }
    }
    best.map(|(_, a, b)| (a, b))
}

fn steps() -> Vec<Step> {
    vec![
        Step {
            name: "idle",
            wait: 6.0,
            setup: reset,
        },
        Step {
            name: "inspect-building",
            wait: 1.0,
            setup: |w| {
                reset(w);
                if let Some(c) = find_building(w) {
                    w.resource_mut::<Pointer>().target = PointerTarget::Cell(c);
                }
            },
        },
        Step {
            name: "inspect-road",
            wait: 1.0,
            setup: |w| {
                reset(w);
                if let Some(c) = find_road(w) {
                    w.resource_mut::<Pointer>().target = PointerTarget::Cell(c);
                }
            },
        },
        Step {
            name: "tooltip",
            wait: 1.2,
            setup: |w| {
                reset(w);
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Tool(ActiveTool::Road));
            },
        },
        Step {
            name: "road-preview",
            wait: 1.0,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                let Some((start, end)) = find_new_road(w) else { return };
                w.resource_mut::<Pointer>().action = Some(Action::Road { start, held: false });
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(end);
            },
        },
        Step {
            name: "road-blocked",
            wait: 1.0,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                let Some(b) = find_building(w) else { return };
                let start = Cell::new(b.x, b.y - 8);
                w.resource_mut::<Pointer>().action = Some(Action::Road { start, held: false });
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(Cell::new(b.x, b.y + 3));
            },
        },
        Step {
            name: "zone-drag",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Commercial));
                w.resource_mut::<Pointer>().action = Some(Action::Rect {
                    start: Cell::new(36, 112),
                });
                let mut p = w.resource_mut::<Pointer>();
                p.target = PointerTarget::Cell(Cell::new(46, 118));
                p.hold = true;
            },
        },
        Step {
            name: "bulldoze-drag",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Bulldoze;
                let b = find_building(w).unwrap_or(Cell::new(48, 130));
                w.resource_mut::<Pointer>().action = Some(Action::Rect {
                    start: b.offset(-3, -3),
                });
                let mut p = w.resource_mut::<Pointer>();
                p.target = PointerTarget::Cell(b.offset(3, 2));
                p.hold = true;
            },
        },
        Step {
            name: "traffic-overlay",
            wait: 1.5,
            setup: |w| {
                reset(w);
                *w.resource_mut::<OverlayMode>() = OverlayMode::Traffic;
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Overlay(OverlayMode::Traffic));
            },
        },
        Step {
            name: "budget",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<UiState>().budget_open = true;
            },
        },
        Step {
            name: "toasts",
            wait: 0.8,
            setup: |w| {
                reset(w);
                for (k, s) in [
                    (ToastKind::Success, "City saved"),
                    (ToastKind::Error, "Not enough money: costs $1,240, you have $860"),
                    (ToastKind::Error, "Roads must be straight or at 45 degrees"),
                    (
                        ToastKind::Warning,
                        "Nothing zoned: zones must be within 2 cells of a road",
                    ),
                ] {
                    w.write_message(Toast::new(k, s));
                }
            },
        },
        Step {
            name: "pause-menu",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::OpenMenu);
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Save);
            },
        },
        Step {
            name: "first-run",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<UiState>().hint_visible = true;
                w.resource_mut::<SimClock>().paused = true;
            },
        },
        Step {
            name: "1440p",
            wait: 3.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<UiState>().budget_open = true;
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Residential));
                w.resource_mut::<Pointer>().target =
                    PointerTarget::Button(UiAction::Tool(ActiveTool::Zone(Some(Zone::Industrial))));
                if let Ok(mut win) = w.query_filtered::<&mut Window, With<PrimaryWindow>>().single_mut(w) {
                    win.resolution.set(2560.0, 1440.0);
                }
            },
        },
    ]
}

fn run_shots(world: &mut World) {
    let dt = world.resource::<Time<Real>>().delta_secs().min(0.25);
    let mut r = world.remove_resource::<Runner>().expect("runner");
    r.frames += 1;
    if r.frames > 3 {
        r.t += dt;
    }
    if r.index >= r.steps.len() {
        if r.t > 1.0 {
            info!("ui shots done");
            world.write_message(AppExit::Success);
        }
        world.insert_resource(r);
        return;
    }
    let step = &r.steps[r.index];
    if !r.entered {
        r.entered = true;
        r.t = 0.0;
        (step.setup)(world);
    } else if !r.shot && r.t >= step.wait {
        r.shot = true;
        let path = r.dir.join(format!("{:02}-{}.png", r.index + 1, step.name));
        info!("ui shot {}", path.display());
        world.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
    } else if r.shot && r.t >= step.wait + 0.6 {
        r.index += 1;
        r.entered = false;
        r.shot = false;
        r.t = 0.0;
    }
    world.insert_resource(r);
}

#[allow(clippy::too_many_arguments)]
fn drive_pointer(
    pointer: Res<Pointer>,
    mut tools: ResMut<ToolState>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    buttons: Query<(&UiButton, &UiGlobalTransform, &InheritedVisibility)>,
) {
    if let Some(a) = pointer.action
        && tools.action != a
    {
        tools.action = a;
    }
    let Ok(mut win) = window.single_mut() else { return };
    let pos = match pointer.target {
        // Park the pointer just outside the window so nothing is hovered.
        PointerTarget::None => Some(Vec2::new(-4.0, -4.0)),
        PointerTarget::Cell(c) => cams
            .single()
            .ok()
            .and_then(|(cam, tf)| cam.world_to_viewport(tf, cell_center(c, 0.0)).ok()),
        PointerTarget::Button(a) => buttons
            .iter()
            .find(|(b, _, v)| b.action == a && v.get())
            .map(|(_, tf, _)| tf.translation / win.scale_factor()),
    };
    if win.cursor_position() != pos {
        win.set_cursor_position(pos);
    }
    if pointer.hold {
        if !mouse.pressed(MouseButton::Left) {
            mouse.press(MouseButton::Left);
        }
    } else if mouse.pressed(MouseButton::Left) {
        mouse.release(MouseButton::Left);
    }
}
