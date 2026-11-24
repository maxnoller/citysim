//! Global keys and the handler for every [`UiAction`].
//!
//! Keys: Space pause, 1/2/3 speed, O cycles overlays, T traffic overlay,
//! G graphics quality. Escape: cancels the tool action in progress (done by
//! `tools.rs`), else closes the menu / budget, else drops back to Inspect,
//! else opens the pause menu. Right click with nothing in progress also
//! drops back to Inspect.

use bevy::prelude::*;

use super::UiState;
use super::budget::TAX_STEP;
use super::tools::ToolState;
use super::widgets::{UiAction, button_clicks};
use crate::shared::{ActiveTool, GameSet, GraphicsQuality, OverlayMode, PointerOverUi, SimClock, SimCommand, SimWorld};
use citysim_sim::{Command, balance};

pub struct ControlsPlugin;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (button_clicks, control_keys, handle_actions)
                .chain()
                .in_set(GameSet::Input),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn control_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    over_ui: Res<PointerOverUi>,
    tools: Res<ToolState>,
    tool: Res<ActiveTool>,
    quality: Res<GraphicsQuality>,
    mut ui: ResMut<UiState>,
    mut out: MessageWriter<UiAction>,
) {
    if keys.just_pressed(KeyCode::Escape) && !tools.busy() {
        if ui.menu_open {
            out.write(UiAction::CloseMenu);
        } else if ui.budget_open {
            ui.budget_open = false;
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
    for (i, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3]
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
    sim: Res<SimWorld>,
    mut cmds: MessageWriter<SimCommand>,
    mut exit: MessageWriter<AppExit>,
) {
    // O cycles overlays (kept from phase 1).
    if keys.just_pressed(KeyCode::KeyO) && !ui.menu_open {
        *overlay = match *overlay {
            OverlayMode::None => OverlayMode::Zones,
            OverlayMode::Zones => OverlayMode::Traffic,
            OverlayMode::Traffic => OverlayMode::None,
        };
    }
    for action in actions.read() {
        match *action {
            UiAction::Tool(t) => {
                let new = if *tool == t && t != ActiveTool::Inspect {
                    ActiveTool::Inspect
                } else {
                    t
                };
                tool.set_if_neq(new);
            }
            UiAction::ZoneTool => {
                let new = if matches!(*tool, ActiveTool::Zone(_)) {
                    ActiveTool::Inspect
                } else {
                    ActiveTool::Zone(ui.last_zone)
                };
                tool.set_if_neq(new);
            }
            UiAction::Overlay(o) => {
                *overlay = if *overlay == o { OverlayMode::None } else { o };
            }
            UiAction::TogglePause => clock.paused = !clock.paused,
            UiAction::Speed(i) => {
                clock.speed = i;
                clock.paused = false;
            }
            UiAction::ToggleBudget => ui.budget_open = !ui.budget_open,
            UiAction::TaxStep(n) => {
                let r = sim.world.economy.tax_rate + n as f32 * TAX_STEP / 5.0;
                let r = ((r / TAX_STEP).round() * TAX_STEP).clamp(0.0, balance::MAX_TAX_RATE);
                cmds.write(SimCommand(Command::SetTaxRate(r)));
            }
            UiAction::OpenMenu => {
                if !ui.menu_open {
                    ui.menu_open = true;
                    ui.budget_open = false;
                    ui.paused_before_menu = clock.paused;
                    clock.paused = true;
                }
            }
            UiAction::CloseMenu => {
                if ui.menu_open {
                    ui.menu_open = false;
                    clock.paused = ui.paused_before_menu;
                }
            }
            UiAction::Quality(q) => *quality = q,
            UiAction::Quit => {
                exit.write(AppExit::Success);
            }
            UiAction::DismissHint => ui.hint_visible = false,
            // Handled in save_load.rs.
            UiAction::Save | UiAction::Load => {}
        }
    }
}
