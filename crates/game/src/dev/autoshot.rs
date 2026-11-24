//! `CITYSIM_AUTOSHOT=out.png`: wait, capture the window, exit.
//! Without a demo it first builds `sample_city()` through `SimCommand`s.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use citysim_sim::{Cell, Command, Zone};

use super::{DemoKind, ShotConfig, ShotView};
use bevy::window::PrimaryWindow;

use crate::shared::{CameraOverride, CameraPose, GameSet, OverlayMode, SimCommand, cell_center};

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

/// Reproducible camera presets per scenario.
pub fn view_pose(view: ShotView, demo: Option<DemoKind>) -> CameraPose {
    // (focus cell, wide distance, street-level focus cell)
    let (focus, wide, street) = match demo {
        Some(DemoKind::Town) => (crate::dev_demo::TOWN_CENTER, 650.0, crate::dev_demo::TOWN_STREET_VIEW),
        Some(DemoKind::Stress) => (
            crate::dev_demo::STRESS_CENTER,
            1700.0,
            crate::dev_demo::TOWN_STREET_VIEW,
        ),
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
) {
    let first = state.frames == 0;
    state.frames += 1;
    if !first {
        state.t += time.delta_secs().min(0.25);
    }
    if first && state.demo.is_none() {
        for cmd in sample_city() {
            out.write(SimCommand(cmd));
        }
    }
    if state.demo != Some(DemoKind::Script) {
        camera.0 = Some(view_pose(state.shot.view, state.demo));
        overlay.set_if_neq(state.shot.overlay);
        // Ignore wherever the real mouse is, for reproducible shots.
        if let Ok(mut w) = window.single_mut() {
            w.set_cursor_position(None);
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
