//! Households: a home, maybe a job, maybe a car on the road.
//!
//! PHASE2: move-in from the highway entry, job assignment, commuting
//! schedule. Only the types exist so far.

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::{BuildingId, VehicleId, World};

new_key_type! {
    /// Stable id of a [`Household`].
    pub struct HouseholdId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HouseholdState {
    #[default]
    AtHome,
    CommutingToWork,
    AtWork,
    CommutingHome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Household {
    pub home: BuildingId,
    pub job: Option<BuildingId>,
    /// People in the household (counts towards population).
    pub members: u8,
    pub state: HouseholdState,
    /// The car while it is on the road.
    pub vehicle: Option<VehicleId>,
}

/// Per-tick household logic.
pub(crate) fn tick(_world: &mut World) {
    // PHASE2: assign homes/jobs, trigger commutes by time of day,
    // spawn vehicles via `traffic`, remove households whose home is gone.
}
