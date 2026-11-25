//! Sets `PointerOverUi` from Bevy's UI picking (any hovered UI node).

use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::PointerId;
use bevy::prelude::*;

use crate::shared::{GameSet, PointerOverUi};

pub struct PointerPlugin;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, pointer_over_ui.in_set(GameSet::Input));
    }
}

fn pointer_over_ui(hover: Res<HoverMap>, nodes: Query<(), With<Node>>, mut over: ResMut<PointerOverUi>) {
    let any = hover
        .get(&PointerId::Mouse)
        .is_some_and(|hits| hits.keys().any(|&e| nodes.contains(e)));
    over.set_if_neq(PointerOverUi(any));
}
