//! One save slot: F5 saves, F9 loads `citysim.citysave` in the working dir.

use bevy::prelude::*;
use citysim_sim::World;

use crate::shared::{GameSet, SimWorld};

pub const SAVE_PATH: &str = "citysim.citysave";

pub struct SaveLoadPlugin;

impl Plugin for SaveLoadPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, save_load_keys.in_set(GameSet::Input));
    }
}

fn save_load_keys(keys: Res<ButtonInput<KeyCode>>, mut sim: ResMut<SimWorld>) {
    if keys.just_pressed(KeyCode::F5) {
        match std::fs::write(SAVE_PATH, sim.world.save()) {
            Ok(()) => info!("saved to {SAVE_PATH}"),
            Err(e) => error!("save failed: {e}"),
        }
    }
    if keys.just_pressed(KeyCode::F9) {
        let loaded = std::fs::read(SAVE_PATH)
            .map_err(|e| e.to_string())
            .and_then(|b| World::load(&b).map_err(|e| e.to_string()));
        match loaded {
            Ok(world) => {
                sim.replace(world);
                info!("loaded {SAVE_PATH}");
            }
            Err(e) => error!("load failed: {e}"),
        }
    }
}
