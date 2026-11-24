//! Sim space <-> Bevy space. Sim `(x, y)` metres maps to Bevy `(x, 0, y)`.

use bevy::prelude::*;
use citysim_sim::Cell;

/// Heights above the ground for flat overlays, to avoid z-fighting.
pub const ZONE_Y: f32 = 0.03;
pub const ROAD_Y: f32 = 0.06;
pub const HIGHWAY_Y: f32 = 0.07;
pub const GHOST_Y: f32 = 0.12;
pub const HIGHLIGHT_Y: f32 = 0.15;

pub fn sim_to_world(p: [f32; 2], height: f32) -> Vec3 {
    Vec3::new(p[0], height, p[1])
}

pub fn world_to_sim(v: Vec3) -> [f32; 2] {
    [v.x, v.z]
}

pub fn cell_center(c: Cell, height: f32) -> Vec3 {
    sim_to_world(c.center(), height)
}

/// Corners of a cell (inset by `inset` metres) at `height`.
pub fn cell_corners(c: Cell, inset: f32, height: f32) -> [Vec3; 4] {
    let s = citysim_sim::CELL_SIZE;
    let (x0, z0) = (c.x as f32 * s + inset, c.y as f32 * s + inset);
    let (x1, z1) = ((c.x + 1) as f32 * s - inset, (c.y + 1) as f32 * s - inset);
    [
        Vec3::new(x0, height, z0),
        Vec3::new(x1, height, z0),
        Vec3::new(x1, height, z1),
        Vec3::new(x0, height, z1),
    ]
}
