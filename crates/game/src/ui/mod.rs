//! UI: all 2D interface, tools / input that produce sim `Command`s,
//! pause/speed, overlays & quality switches, save/load. Talks to the rest of
//! the game only through `crate::shared` (never import from `crate::gfx`).
//!
//! The interface is a game HUD docked to the screen edges: the bottom bar
//! (`bar.rs`: money, residents, demand, tool slots, clock, speed), the
//! build tray standing on it (`tray.rs`, `budget.rs`), the right-edge
//! selection panel (`panel.rs`), the top edge (`top.rs`: city plate, alert
//! ribbons, system buttons), the cursor readout (`context.rs`), menus.
//! Colours, sizes and fonts live in `theme.rs`; angular plates in
//! `plate.rs`; reusable pieces in `widgets.rs`.
//! `CITYSIM_UI_SHOTS=dir` captures every UI state to `dir`; `CITYSIM_UI_TOUR=1`
//! plays a scripted walk through the interface for videos (`shots.rs`).

mod bar;
mod budget;
mod context;
mod controls;
mod fiscal;
mod format;
mod fsnap;
mod loading;
mod menu;
mod panel;
mod people;
mod plan;
mod plate;
mod pointer;
mod readout;
mod save_load;
mod settings;
mod shapes;
mod shots;
mod theme;
mod toasts;
mod tools;
mod tooltip;
mod top;
mod tray;
mod widgets;

use bevy::camera::ClearColorConfig;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResized};
use citysim_sim::Zone;

use crate::shared::WorldCamera;

/// The drawers of the bar that are not tools: only one at a time, and none
/// while a tool's build tray is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoTray {
    Budget,
    People,
    Maps,
    Stats,
}

/// Interface state that only ui cares about.
#[derive(Resource, Debug)]
pub struct UiState {
    /// The open info drawer (budget, city people, maps, statistics).
    pub tray: Option<InfoTray>,
    pub menu_open: bool,
    /// Clock state to restore when the menu closes.
    pub paused_before_menu: bool,
    /// Getting-started plate shown (new games only).
    pub hint_visible: bool,
    /// Zone type the Zones slot selects.
    pub last_zone: Option<Zone>,
    /// Settings open (inside the pause menu).
    pub settings_open: bool,
    /// Key reference open (inside the pause menu).
    pub keys_open: bool,
    /// Index into `settings::UI_SCALES`.
    pub ui_scale: usize,
    /// The camera follows the person or car of the side panel.
    pub following: bool,
    /// The bar is in its compact form (narrow window).
    pub compact: bool,
    /// The keyboard works the open drawer (F6; arrows, Enter, Esc).
    pub tray_focus: bool,
}

impl UiState {
    pub fn budget_open(&self) -> bool {
        self.tray == Some(InfoTray::Budget)
    }
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            tray: None,
            menu_open: false,
            paused_before_menu: false,
            hint_visible: false,
            last_zone: Some(Zone::Residential),
            settings_open: false,
            keys_open: false,
            ui_scale: 1,
            following: false,
            compact: false,
            tray_focus: false,
        }
    }
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiState>()
            .init_resource::<theme::UiFonts>()
            .add_plugins((
                plate::PlatePlugin,
                widgets::WidgetsPlugin,
                pointer::PointerPlugin,
                tools::ToolsPlugin,
                controls::ControlsPlugin,
                save_load::SaveLoadPlugin,
                bar::BarPlugin,
                top::TopPlugin,
                tray::TrayPlugin,
                budget::BudgetPlugin,
                context::ContextPlugin,
                panel::PanelPlugin,
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

/// `UiScale` for a logical window size: 1.0 at the 1920x1080 design size,
/// growing a little slower than the window on bigger screens, and shrinking
/// slower than the window on smaller ones (the bar compacts instead, so
/// text stays readable at 1280x720).
pub fn ui_scale_for(size: Vec2) -> f32 {
    let ratio = (size / theme::DESIGN_SIZE).min_element();
    if ratio >= 1.0 {
        1.0 + (ratio - 1.0) * 0.9
    } else {
        (1.0 - (1.0 - ratio) * 0.6).max(0.55)
    }
}

/// The final scale: the base, times the player's interface scale, but
/// never so large that the smallest layout no longer fits the window.
pub fn final_scale(size: Vec2, mult: f32) -> f32 {
    let fit = (size / theme::MIN_LAYOUT).min_element();
    (ui_scale_for(size) * mult).min(fit).max(0.4)
}

fn scale_ui(
    mut resized: MessageReader<WindowResized>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut state: ResMut<UiState>,
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
        let s = final_scale(size, mult);
        if (scale.0 - s).abs() > 1e-3 {
            scale.0 = s;
        }
        let compact = size.x / s < theme::COMPACT_W;
        if state.compact != compact {
            state.compact = compact;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_scale_grows_with_the_window() {
        assert_eq!(ui_scale_for(Vec2::new(1920.0, 1080.0)), 1.0);
        assert!((ui_scale_for(Vec2::new(2560.0, 1440.0)) - 1.3).abs() < 1e-3);
        assert!((ui_scale_for(Vec2::new(1280.0, 720.0)) - 0.8).abs() < 1e-3);
    }

    #[test]
    fn the_smallest_layout_always_fits() {
        for size in [
            Vec2::new(1280.0, 720.0),
            Vec2::new(1366.0, 768.0),
            Vec2::new(1920.0, 1080.0),
        ] {
            for (mult, _) in settings::UI_SCALES {
                let s = final_scale(size, mult);
                assert!(size.x / s >= theme::MIN_LAYOUT.x - 0.5, "{size} x{mult}");
                assert!(size.y / s >= theme::MIN_LAYOUT.y - 0.5, "{size} x{mult}");
            }
        }
    }
}
