//! Aggregate numbers for the HUD.

use serde::{Deserialize, Serialize};

use crate::{Occupant, World};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Stats {
    pub population: u32,
    pub jobs: u32,
    pub employed: u32,
    pub households: u32,
    pub buildings: u32,
    pub vehicles_on_road: u32,
    pub road_cells: u32,
}

/// Recompute what can be derived cheaply from state.
pub(crate) fn update(world: &mut World) {
    let road_cells = world.grid.cells.iter().filter(|c| c.occupant == Occupant::Road).count() as u32;
    let s = &mut world.stats;
    s.road_cells = road_cells;
    s.buildings = world.buildings.len() as u32;
    s.households = world.households.len() as u32;
    s.vehicles_on_road = world.vehicles.len() as u32;
    // PHASE2: population, jobs, employed.
}
