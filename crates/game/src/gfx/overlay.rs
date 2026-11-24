//! Data overlays drawn in 3D. `OverlayMode::Traffic`: each segment drawn
//! per direction, green (free) to red (jammed) by `congestion_in`.

use bevy::prelude::*;

use super::heights;
use crate::shared::{GameSet, OverlayMode, SimWorld, sim_to_world};

pub struct OverlayPlugin;

impl Plugin for OverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, draw_traffic.in_set(GameSet::Render));
    }
}

fn draw_traffic(mode: Res<OverlayMode>, sim: Res<SimWorld>, mut gizmos: Gizmos) {
    if *mode != OverlayMode::Traffic {
        return;
    }
    let roads = &sim.world.roads;
    for seg in roads.segments.values() {
        let a = sim_to_world(roads.node_pos(seg.a), heights::OVERLAY);
        let b = sim_to_world(roads.node_pos(seg.b), heights::OVERLAY);
        let dir = (b - a).normalize_or_zero();
        let side = Vec3::new(-dir.z, 0.0, dir.x) * 1.5;
        for forward in [true, false] {
            let t = seg.congestion_in(forward);
            let color = Color::srgb(t.min(0.5) * 2.0, (1.0 - t).min(0.5) * 2.0, 0.1);
            let off = if forward { side } else { -side };
            gizmos.line(a + off, b + off, color);
        }
    }
}
