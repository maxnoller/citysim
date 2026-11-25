//! UI: all 2D interface, tools / input that produce sim `Command`s,
//! pause/speed, overlays & quality switches, save/load. Talks to the rest of
//! the game only through `crate::shared` (never import from `crate::gfx`).
//!
//! Design tokens live in `theme.rs`; reusable pieces in `widgets.rs`.
//! `CITYSIM_UI_SHOTS=dir` captures every UI state to `dir`; `CITYSIM_UI_TOUR=1`
//! plays a ~30 s scripted walk through the interface for videos (`shots.rs`).

mod budget;
mod context;
mod controls;
mod demand;
mod fiscal;
mod format;
mod fsnap;
mod hud;
mod legend;
mod loading;
mod menu;
mod people;
mod plan;
mod pointer;
mod readout;
mod save_load;
mod settings;
mod shapes;
mod shots;
mod theme;
mod toasts;
mod toolbar;
mod toolopts;
mod tools;
mod tooltip;
mod widgets;

use bevy::camera::ClearColorConfig;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResized};
use citysim_sim::Zone;

use crate::shared::WorldCamera;

/// Interface state that only ui cares about.
#[derive(Resource, Debug)]
pub struct UiState {
    pub budget_open: bool,
    pub menu_open: bool,
    /// Clock state to restore when the menu closes.
    pub paused_before_menu: bool,
    /// Getting-started card shown (new games only).
    pub hint_visible: bool,
    /// Zone type the Zone button selects.
    pub last_zone: Option<Zone>,
    /// Settings card open (inside the pause menu).
    pub settings_open: bool,
    /// Index into `settings::UI_SCALES`.
    pub ui_scale: usize,
    /// The camera follows the person or car of the people panel.
    pub following: bool,
    /// The city-wide people view is open.
    pub city_people_open: bool,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            budget_open: false,
            menu_open: false,
            paused_before_menu: false,
            hint_visible: false,
            last_zone: Some(Zone::Residential),
            settings_open: false,
            ui_scale: 1,
            following: false,
            city_people_open: false,
        }
    }
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiState>()
            .init_resource::<theme::UiFonts>()
            .add_plugins((
                widgets::WidgetsPlugin,
                pointer::PointerPlugin,
                tools::ToolsPlugin,
                controls::ControlsPlugin,
                save_load::SaveLoadPlugin,
                hud::HudPlugin,
                toolbar::ToolbarPlugin,
                toolopts::ToolOptsPlugin,
                demand::DemandPlugin,
                context::ContextPlugin,
                legend::LegendPlugin,
                budget::BudgetPlugin,
                toasts::ToastsPlugin,
                menu::MenuPlugin,
                tooltip::TooltipPlugin,
            ))
            .add_plugins((fiscal::FiscalPlugin, settings::SettingsPlugin, loading::LoadingPlugin))
            .add_systems(PostStartup, fallback_ui_camera)
            .add_systems(PreUpdate, scale_ui);
        if let Some(dir) = std::env::var_os("CITYSIM_UI_SHOTS").filter(|d| !d.is_empty()) {
            app.add_plugins(shots::ShotsPlugin { dir: dir.into() });
        } else if std::env::var("CITYSIM_UI_TOUR").is_ok_and(|v| !v.is_empty() && v != "0") {
            app.add_plugins(shots::TourPlugin);
        }
    }
}

/// UI renders on the world camera when gfx provides one; otherwise (gfx
/// plugin absent) spawn a plain 2D camera so the UI is still visible.
fn fallback_ui_camera(mut commands: Commands, world_cams: Query<(), With<WorldCamera>>) {
    if world_cams.is_empty() {
        commands.spawn((
            Name::new("UI fallback camera"),
            Camera2d,
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.1, 0.1, 0.12)),
                ..default()
            },
        ));
    }
}

/// `UiScale` for a logical window size: 1.0 at the 1280x720 design size,
/// growing (a little slower than the window) on bigger screens so the HUD
/// keeps its proportions without becoming huge; shrinking on small ones.
pub fn ui_scale_for(size: Vec2) -> f32 {
    let ratio = (size / theme::DESIGN_SIZE).min_element();
    if ratio >= 1.0 {
        1.0 + (ratio - 1.0) * 0.8
    } else {
        ratio.max(0.6)
    }
}

fn scale_ui(
    mut resized: MessageReader<WindowResized>,
    window: Query<&Window, With<PrimaryWindow>>,
    state: Res<UiState>,
    mut scale: ResMut<UiScale>,
    mut first: Local<bool>,
) {
    if resized.read().count() == 0 && *first && !state.is_changed() {
        return;
    }
    *first = true;
    if let Ok(w) = window.single() {
        let mult = settings::UI_SCALES[state.ui_scale.min(settings::UI_SCALES.len() - 1)].0;
        let size = Vec2::new(w.width(), w.height());
        // Larger than the window can hold would cut panels off: the design
        // size must always fit.
        let s = (ui_scale_for(size) * mult).min((size / theme::DESIGN_SIZE).min_element().max(0.6));
        if (scale.0 - s).abs() > 1e-3 {
            scale.0 = s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_scale_grows_with_the_window() {
        assert_eq!(ui_scale_for(Vec2::new(1280.0, 720.0)), 1.0);
        assert!((ui_scale_for(Vec2::new(2560.0, 1440.0)) - 1.8).abs() < 1e-5);
        assert!(ui_scale_for(Vec2::new(1920.0, 1080.0)) > 1.3);
        assert!(ui_scale_for(Vec2::new(1024.0, 600.0)) < 1.0);
    }
}
