//! Global keys and the handler for every [`UiAction`].
//!
//! Keys: Space pause, 1/2/3 speed, O cycles overlays, T traffic overlay,
//! G graphics quality, F3 the performance panel. Escape: cancels the tool
//! action in progress (done by `tools.rs`), else closes the menu, the city
//! window or the drawer, else drops back to Inspect, else opens the pause
//! menu. Right click with nothing in progress also
//! drops back to Inspect.

use bevy::prelude::*;

use super::budget::TAX_STEP;
use super::tools::ToolState;
use super::widgets::{UiAction, button_clicks};
use super::{CityPage, InfoTray, UiState};
use crate::shared::{
    ActiveTool, CameraInputBlocked, DisplaySettings, GameSet, GraphicsQuality, OverlayMode, PointerOverUi, RoadChoice,
    RoadMode, SimClock, SimCommand, SimWorld, ToolModes,
};
use citysim_sim::{Command, balance};

pub struct ControlsPlugin;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (button_clicks, control_keys, handle_actions)
                .chain()
                .in_set(GameSet::Input),
        )
        .add_systems(
            Update,
            block_camera_input
                .in_set(GameSet::Input)
                .after(handle_actions)
                .after(super::save_load::save_load),
        );
    }
}

/// The pause menu owns keyboard and mouse: tell gfx to leave the camera
/// alone while it is open (after this frame's menu open / close / load).
fn block_camera_input(ui: Res<UiState>, mut blocked: ResMut<CameraInputBlocked>) {
    blocked.set_if_neq(CameraInputBlocked(ui.menu_open || ui.tray_focus));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn control_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    over_ui: Res<PointerOverUi>,
    tools: Res<ToolState>,
    people: Res<super::panel::PeopleView>,
    tool: Res<ActiveTool>,
    quality: Res<GraphicsQuality>,
    mut ui: ResMut<UiState>,
    mut out: MessageWriter<UiAction>,
) {
    // F3 works everywhere, the menu included.
    if keys.just_pressed(KeyCode::F3) {
        out.write(UiAction::Perf(super::perf::next_detail(ui.perf)));
    }
    // Esc in a focused drawer only leaves the focus (`tray::tray_keys`).
    if keys.just_pressed(KeyCode::Escape) && !tools.busy() && !ui.tray_focus {
        if ui.menu_open && ui.settings_open {
            out.write(UiAction::CloseSettings);
        } else if ui.menu_open && ui.keys_open {
            out.write(UiAction::Keys);
        } else if ui.menu_open {
            out.write(UiAction::CloseMenu);
        } else if ui.window.is_some() {
            ui.window = None;
        } else if ui.tray.is_some() {
            ui.tray = None;
        } else if !people.stack.is_empty() {
            out.write(UiAction::ClosePeople);
        } else if *tool != ActiveTool::Inspect {
            out.write(UiAction::Tool(ActiveTool::Inspect));
        } else {
            out.write(UiAction::OpenMenu);
        }
    }
    if ui.menu_open {
        if keys.just_pressed(KeyCode::KeyG) {
            out.write(UiAction::Quality(next_quality(*quality)));
        }
        return;
    }
    if mouse.just_pressed(MouseButton::Right) && !over_ui.0 && !tools.busy() && *tool != ActiveTool::Inspect {
        out.write(UiAction::Tool(ActiveTool::Inspect));
    }
    if keys.just_pressed(KeyCode::Space) {
        out.write(UiAction::TogglePause);
    }
    for (i, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4]
        .into_iter()
        .enumerate()
    {
        if keys.just_pressed(key) {
            out.write(UiAction::Speed(i));
        }
    }
    if keys.just_pressed(KeyCode::KeyT) {
        out.write(UiAction::Overlay(OverlayMode::Traffic));
    }
    if keys.just_pressed(KeyCode::KeyU) && !super::tools::ctrl(&keys) {
        out.write(UiAction::ToggleBudget);
    }
    if keys.just_pressed(KeyCode::KeyP) {
        out.write(UiAction::CityPeople);
    }
    if keys.just_pressed(KeyCode::KeyG) {
        out.write(UiAction::Quality(next_quality(*quality)));
    }
}

fn next_quality(q: GraphicsQuality) -> GraphicsQuality {
    match q {
        GraphicsQuality::High => GraphicsQuality::Low,
        GraphicsQuality::Low => GraphicsQuality::Medium,
        GraphicsQuality::Medium => GraphicsQuality::High,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_actions(
    keys: Res<ButtonInput<KeyCode>>,
    mut actions: MessageReader<UiAction>,
    mut tool: ResMut<ActiveTool>,
    mut overlay: ResMut<OverlayMode>,
    mut clock: ResMut<SimClock>,
    mut quality: ResMut<GraphicsQuality>,
    mut ui: ResMut<UiState>,
    mut modes: ResMut<ToolModes>,
    mut kind: ResMut<RoadChoice>,
    mut opts: ResMut<super::tools::ToolOptions>,
    mut display: ResMut<DisplaySettings>,
    sim: Res<SimWorld>,
    mut cmds: MessageWriter<SimCommand>,
    mut exit: MessageWriter<AppExit>,
) {
    // O cycles overlays; the maps tray shows while one is on.
    if keys.just_pressed(KeyCode::KeyO) && !ui.menu_open {
        *overlay = match *overlay {
            OverlayMode::None => OverlayMode::Zones,
            OverlayMode::Zones => OverlayMode::Traffic,
            OverlayMode::Traffic => OverlayMode::None,
        };
        if *overlay == OverlayMode::None {
            if ui.tray == Some(InfoTray::Maps) {
                ui.tray = None;
            }
        } else {
            open_tray(&mut ui, &mut tool, InfoTray::Maps);
        }
    }
    for action in actions.read() {
        match *action {
            UiAction::Tool(ActiveTool::Road) if *tool == ActiveTool::Road && modes.road == RoadMode::Junction => {
                modes.road = RoadMode::Straight;
            }
            UiAction::Tool(t) => {
                // A bar slot clicked again puts the tool away; the zone
                // pictures in the tray are a choice, not a switch.
                let new = if *tool == t && !matches!(t, ActiveTool::Inspect | ActiveTool::Zone(_)) {
                    ActiveTool::Inspect
                } else {
                    t
                };
                tool.set_if_neq(new);
                ui.tray = None;
                if new != ActiveTool::Inspect {
                    ui.window = None;
                }
            }
            UiAction::ZoneTool => {
                let new = if matches!(*tool, ActiveTool::Zone(_)) {
                    ActiveTool::Inspect
                } else {
                    ActiveTool::Zone(ui.last_zone)
                };
                tool.set_if_neq(new);
                ui.tray = None;
                ui.window = None;
            }
            UiAction::Overlay(o) => {
                // The maps tray picks one; T toggles traffic.
                *overlay = if *overlay == o && ui.tray != Some(InfoTray::Maps) {
                    OverlayMode::None
                } else {
                    o
                };
                if *overlay != OverlayMode::None
                    && ui.tray.is_none()
                    && ui.window.is_none()
                    && *tool == ActiveTool::Inspect
                {
                    ui.tray = Some(InfoTray::Maps);
                }
            }
            UiAction::Maps => toggle_tray(&mut ui, &mut tool, InfoTray::Maps),
            UiAction::Stats => toggle_window(&mut ui, &mut tool, CityPage::Stats),
            UiAction::CityPeople => toggle_window(&mut ui, &mut tool, CityPage::People),
            UiAction::CityPage(p) => open_window(&mut ui, &mut tool, p),
            UiAction::CloseWindow => ui.window = None,
            UiAction::Demolish => toggle_demolish(&mut tool, &mut ui),
            UiAction::TogglePause => clock.paused = !clock.paused,
            UiAction::Speed(i) => {
                clock.speed = i;
                clock.paused = false;
            }
            UiAction::ToggleBudget => toggle_window(&mut ui, &mut tool, CityPage::Budget),
            UiAction::TaxStep(n) => {
                let r = sim.world.economy.tax_rate + n as f32 * TAX_STEP / 5.0;
                let r = ((r / TAX_STEP).round() * TAX_STEP).clamp(0.0, balance::MAX_TAX_RATE);
                cmds.write(SimCommand(Command::SetTaxRate(r)));
            }
            UiAction::OpenMenu => {
                if !ui.menu_open {
                    ui.menu_open = true;
                    ui.paused_before_menu = clock.paused;
                    clock.paused = true;
                }
            }
            UiAction::CloseMenu => {
                if ui.menu_open {
                    ui.menu_open = false;
                    ui.settings_open = false;
                    ui.keys_open = false;
                    clock.paused = ui.paused_before_menu;
                }
            }
            UiAction::Quality(q) => *quality = q,
            UiAction::Quit => {
                exit.write(AppExit::Success);
            }
            UiAction::DismissHint => ui.hint_visible = false,
            UiAction::RoadMode(m) => {
                // The toolbar's Junctions button toggles like a tool.
                if m == RoadMode::Junction && *tool == ActiveTool::Road && modes.road == m {
                    tool.set_if_neq(ActiveTool::Inspect);
                } else {
                    modes.road = m;
                    tool.set_if_neq(ActiveTool::Road);
                    ui.tray = None;
                    ui.window = None;
                }
            }
            UiAction::ZoneMode(m) => modes.zone = m,
            UiAction::BulldozeMode(m) => modes.bulldoze = m,
            UiAction::RoadKind(k) => {
                kind.set_if_neq(k);
                if modes.road == RoadMode::Junction {
                    modes.road = RoadMode::Straight;
                }
            }
            UiAction::BlockSize(d) => opts.step_block(d),
            UiAction::Snap(r) => opts.toggle(r),
            UiAction::ParallelStep(d) => opts.step_parallel(d),
            UiAction::ZoneDepth(d) => opts.zone_depth = d,
            UiAction::JunctionBrush(b) => opts.junction_brush = b,
            UiAction::OpenSettings => {
                ui.settings_open = true;
                ui.keys_open = false;
            }
            UiAction::CloseSettings => ui.settings_open = false,
            UiAction::Keys => {
                ui.keys_open = !ui.keys_open;
                ui.settings_open = false;
            }
            UiAction::UiScale(i) => ui.ui_scale = i,
            UiAction::EdgeScroll(on) => display.edge_scroll = on,
            UiAction::LotMarkers(on) => display.lot_markers = on,
            UiAction::Perf(d) => ui.perf = d,
            // Handled in tools.rs and people_panel.rs.
            UiAction::Undo
            | UiAction::Person(_)
            | UiAction::Place(_)
            | UiAction::Vehicle(_)
            | UiAction::Follow
            | UiAction::StopFollow
            | UiAction::PeopleBack
            | UiAction::ClosePeople
            | UiAction::SetJunction(_)
            | UiAction::Fold(_)
            | UiAction::JumpToDestination
            | UiAction::None => {}
            // Handled in save_load.rs.
            UiAction::Save | UiAction::Load => {}
        }
    }
}

/// Open an info drawer: tools step back to Inspect so their trays make room
/// (and the city window closes: one or the other).
fn open_tray(ui: &mut UiState, tool: &mut ResMut<ActiveTool>, tray: InfoTray) {
    ui.tray = Some(tray);
    ui.window = None;
    tool.set_if_neq(ActiveTool::Inspect);
}

/// Open the city window on `page`: tools step back to Inspect (the window
/// is for looking; a build tool closes it), the maps drawer goes down.
fn open_window(ui: &mut UiState, tool: &mut ResMut<ActiveTool>, page: CityPage) {
    ui.window = Some(page);
    ui.tray = None;
    tool.set_if_neq(ActiveTool::Inspect);
}

fn toggle_window(ui: &mut UiState, tool: &mut ResMut<ActiveTool>, page: CityPage) {
    if ui.window == Some(page) {
        ui.window = None;
    } else {
        open_window(ui, tool, page);
    }
}

/// The bulldozer switch (button and B): on from whatever tool is in hand,
/// off back to that tool.
pub(super) fn toggle_demolish(tool: &mut ActiveTool, ui: &mut UiState) {
    if *tool == ActiveTool::Bulldoze {
        *tool = ui.before_demolish.take().unwrap_or(ActiveTool::Inspect);
    } else {
        ui.before_demolish = Some(*tool);
        *tool = ActiveTool::Bulldoze;
        ui.window = None;
        ui.tray = None;
    }
}

fn toggle_tray(ui: &mut UiState, tool: &mut ResMut<ActiveTool>, tray: InfoTray) {
    if ui.tray == Some(tray) {
        ui.tray = None;
    } else {
        open_tray(ui, tool, tray);
    }
}
