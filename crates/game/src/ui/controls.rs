//! Global keys: Space pause, 1/2/3 speed, O overlay, G graphics quality.

use bevy::prelude::*;

use crate::shared::{GameSet, GraphicsQuality, OverlayMode, SimClock};

pub struct ControlsPlugin;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, control_keys.in_set(GameSet::Input));
    }
}

fn control_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut clock: ResMut<SimClock>,
    mut overlay: ResMut<OverlayMode>,
    mut quality: ResMut<GraphicsQuality>,
) {
    if keys.just_pressed(KeyCode::Space) {
        clock.paused = !clock.paused;
    }
    for (i, key) in [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3]
        .into_iter()
        .enumerate()
    {
        if keys.just_pressed(key) {
            clock.speed = i;
            clock.paused = false;
        }
    }
    if keys.just_pressed(KeyCode::KeyO) {
        *overlay = match *overlay {
            OverlayMode::None => OverlayMode::Zones,
            OverlayMode::Zones => OverlayMode::Traffic,
            OverlayMode::Traffic => OverlayMode::None,
        };
    }
    if keys.just_pressed(KeyCode::KeyG) {
        *quality = match *quality {
            GraphicsQuality::High => GraphicsQuality::Low,
            GraphicsQuality::Low => GraphicsQuality::Medium,
            GraphicsQuality::Medium => GraphicsQuality::High,
        };
    }
}
