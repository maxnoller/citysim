//! `CITYSIM_AUTOSHOT=out.png`: wait, capture the window, exit.
//! Without a demo it first builds `sample_city()` through `SimCommand`s.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use citysim_sim::{Cell, Command, RoadKind, Zone};

use super::{DemoKind, ShotConfig, ShotView};
use bevy::window::PrimaryWindow;

use crate::shared::{CameraOverride, CameraPose, GameSet, OverlayMode, SceneReady, SimCommand, cell_center};

pub struct AutoshotPlugin {
    pub shot: ShotConfig,
    pub demo: Option<DemoKind>,
}

#[derive(Resource)]
struct Autoshot {
    shot: ShotConfig,
    demo: Option<DemoKind>,
    t: f32,
    frames: u32,
    taken: bool,
}

impl Plugin for AutoshotPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Autoshot {
            shot: self.shot.clone(),
            demo: self.demo,
            t: 0.0,
            frames: 0,
            taken: false,
        })
        .add_systems(Update, run_autoshot.in_set(GameSet::Input));
    }
}

/// Sample city used when no demo is running (all through the command path).
pub fn sample_city() -> Vec<Command> {
    let c = Cell::new;
    vec![
        Command::BuildRoad {
            from: c(11, 128),
            to: c(60, 128),
            kind: RoadKind::Avenue,
        },
        Command::BuildRoad {
            from: c(30, 104),
            to: c(30, 156),
            kind: RoadKind::Street,
        },
        Command::BuildRoad {
            from: c(45, 128),
            to: c(45, 150),
            kind: RoadKind::Street,
        },
        Command::BuildRoad {
            from: c(30, 150),
            to: c(60, 150),
            kind: RoadKind::Street,
        },
        Command::BuildRoad {
            from: c(30, 104),
            to: c(52, 126),
            kind: RoadKind::Street,
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

/// Reproducible camera presets per scenario.
pub fn view_pose(view: ShotView, demo: Option<DemoKind>) -> CameraPose {
    // (focus cell, wide distance, street-level focus cell)
    let (focus, wide, street) = match demo {
        Some(kind) => {
            let (focus, wide) = super::scenario::focus(kind);
            (focus, wide, super::scenario::street(kind))
        }
        _ => (Cell::new(40, 137), 470.0, Cell::new(30, 128)),
    };
    match view {
        ShotView::Wide => CameraPose {
            focus: cell_center(focus, 0.0),
            yaw: 0.3,
            pitch: 1.0,
            distance: wide,
        },
        ShotView::Close => CameraPose {
            focus: cell_center(focus, 0.0),
            yaw: 0.5,
            pitch: 0.8,
            distance: 200.0,
        },
        ShotView::Street => CameraPose {
            focus: cell_center(street, 0.0),
            yaw: 0.9,
            pitch: 0.35,
            distance: 70.0,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run_autoshot(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut state: ResMut<Autoshot>,
    mut camera: ResMut<CameraOverride>,
    mut overlay: ResMut<OverlayMode>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    mut out: MessageWriter<SimCommand>,
    mut exit: MessageWriter<AppExit>,
    ready: Res<SceneReady>,
) {
    let first = state.frames == 0;
    state.frames += 1;
    // The delay counts from when the scene really draws (shaders built).
    if !first && ready.0 {
        state.t += time.delta_secs().min(0.25);
    }
    if first && state.demo.is_none() {
        for cmd in sample_city() {
            out.write(SimCommand(cmd));
        }
    }
    camera.0 = Some(view_pose(state.shot.view, state.demo));
    overlay.set_if_neq(state.shot.overlay);
    // Ignore wherever the real mouse is, for reproducible shots.
    if let Ok(mut w) = window.single_mut() {
        w.set_cursor_position(None);
        // `CITYSIM_SHOT_SIZE=1920x1080`: the window size for the shot.
        if first
            && let Some((x, y)) = std::env::var("CITYSIM_SHOT_SIZE").ok().and_then(|s| {
                let (a, b) = s.split_once('x')?;
                Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
            })
        {
            w.resolution.set_physical_resolution(x, y);
        }
    }
    if state.t > state.shot.delay + 60.0 {
        error!("autoshot timed out");
        exit.write(AppExit::error());
    }
    if !state.taken && state.t >= state.shot.delay && state.frames > 30 {
        state.taken = true;
        let path = state.shot.path.clone();
        info!("autoshot: saving {path}");
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path))
            .observe(|_: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                exit.write(AppExit::Success);
            });
    }
}
