//! One save slot in the platform's data directory. F5 / the menu saves,
//! F9 / the menu loads; a toast reports the result. Writing the file, and
//! reading and decoding a save, run on a background task (a big city took a
//! frame of 15-25 ms otherwise); a load asked for while a save is still being
//! written waits for it.
//!
//! Location: `$XDG_DATA_HOME/citysim` (default `~/.local/share/citysim`) on
//! Linux, `~/Library/Application Support/CitySim` on macOS,
//! `%APPDATA%\CitySim` on Windows. `CITYSIM_SAVE_DIR` overrides it.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::tasks::futures::check_ready;
use bevy::tasks::{IoTaskPool, Task, block_on};
use citysim_sim::World;

use super::UiState;
use super::toasts::{Toast, ToastKind};
use super::widgets::UiAction;
use crate::shared::{GameSet, SimClock, SimWorld};

pub const SAVE_FILE: &str = "city.citysave";

pub struct SaveLoadPlugin;

impl Plugin for SaveLoadPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Pending>()
            .add_systems(Update, save_load.in_set(GameSet::Input));
    }
}

/// Directory holding the save slot.
pub fn save_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CITYSIM_SAVE_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("CitySim");
        }
    } else if cfg!(target_os = "macos") {
        if let Some(h) = home() {
            return h.join("Library/Application Support/CitySim");
        }
    } else if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(xdg).join("citysim");
    } else if let Some(h) = home() {
        return h.join(".local/share/citysim");
    }
    PathBuf::from(".")
}

pub fn save_path() -> PathBuf {
    save_dir().join(SAVE_FILE)
}

/// Write via a temporary file so a crash never leaves half a save.
fn write_save(bytes: &[u8]) -> std::io::Result<PathBuf> {
    let dir = save_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(SAVE_FILE);
    let tmp = dir.join(format!("{SAVE_FILE}.tmp"));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn read_save() -> Result<World, String> {
    let path = save_path();
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "No saved city yet. Press F5 to save.".to_string(),
        _ => format!("Couldn't read the save: {e}"),
    })?;
    World::load(&bytes).map_err(|e| format!("Couldn't load: {e}"))
}

/// Save and load work under way on background tasks.
#[derive(Resource, Default)]
pub(super) struct Pending {
    save: Option<Task<std::io::Result<PathBuf>>>,
    load: Option<Task<Result<World, String>>>,
    /// A load waits for the save before it.
    load_wanted: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn save_load(
    keys: Res<ButtonInput<KeyCode>>,
    mut pending: ResMut<Pending>,
    mut actions: MessageReader<UiAction>,
    mut sim: ResMut<SimWorld>,
    mut clock: ResMut<SimClock>,
    mut ui: ResMut<UiState>,
    mut toasts: MessageWriter<Toast>,
) {
    let mut save = keys.just_pressed(KeyCode::F5);
    let mut load = keys.just_pressed(KeyCode::F9);
    for a in actions.read() {
        save |= *a == UiAction::Save;
        load |= *a == UiAction::Load;
    }
    if save {
        // Encoding needs the world (here, between frames); the file is written
        // on a task.
        if let Some(t) = pending.save.take() {
            report_save(block_on(t), &mut toasts);
        }
        let bytes = sim.world.save();
        pending.save = Some(IoTaskPool::get().spawn(async move { write_save(&bytes) }));
    }
    if let Some(t) = pending.save.as_mut()
        && let Some(res) = check_ready(t)
    {
        pending.save = None;
        report_save(res, &mut toasts);
    }
    pending.load_wanted |= load;
    if pending.load_wanted && pending.save.is_none() && pending.load.is_none() {
        pending.load_wanted = false;
        pending.load = Some(IoTaskPool::get().spawn(async { read_save() }));
    }
    let loaded = pending.load.as_mut().and_then(check_ready);
    if loaded.is_some() {
        pending.load = None;
    }
    if let Some(res) = loaded {
        match res {
            Ok(world) => {
                let day = world.time.day;
                sim.replace(world);
                info!("loaded {}", save_path().display());
                toasts.write(Toast::new(ToastKind::Success, format!("City loaded · day {day}")));
                // Back to the game, paused, so the player can look first.
                if ui.menu_open {
                    ui.menu_open = false;
                }
                clock.paused = true;
                ui.hint_visible = false;
            }
            Err(e) => {
                warn!("load failed: {e}");
                let kind = if e.starts_with("No saved") {
                    ToastKind::Info
                } else {
                    ToastKind::Error
                };
                toasts.write(Toast::new(kind, e));
            }
        }
    }
}

fn report_save(res: std::io::Result<PathBuf>, toasts: &mut MessageWriter<Toast>) {
    match res {
        Ok(path) => {
            info!("saved to {}", path.display());
            toasts.write(Toast::new(ToastKind::Success, "City saved"));
        }
        Err(e) => {
            error!("save failed: {e}");
            toasts.write(Toast::new(ToastKind::Error, format!("Couldn't save: {e}")));
        }
    }
}
