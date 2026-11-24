//! Buildings grown on zoned land.

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::{CellRect, Zone};

new_key_type! {
    /// Stable id of a [`Building`].
    pub struct BuildingId;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Building {
    /// Zone type the building was grown for.
    pub kind: Zone,
    /// Cells the building stands on; every one has `Occupant::Building(id)`.
    pub footprint: CellRect,
    /// Density level, 1 = smallest. Renderer derives height from it.
    pub level: u8,
    /// Residents (residential) or jobs (commercial/industrial) it can hold.
    pub capacity: u32,
    /// Residents or workers currently there.
    pub occupied: u32,
}
