//! RCI demand.

use serde::{Deserialize, Serialize};

use crate::World;

/// Demand per zone type, each in `-1.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Demand {
    pub residential: f32,
    pub commercial: f32,
    pub industrial: f32,
}

pub(crate) fn tick(_world: &mut World) {
    // PHASE2: compute demand from population, jobs and employment.
}
