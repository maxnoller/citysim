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
    /// Which side of the footprint faces the road it is accessed from.
    #[serde(default)]
    pub facing: Facing,
    /// Stable random number for purely visual variation (model, colour...).
    #[serde(default)]
    pub seed: u32,
    /// `world.time.tick` when the building appeared (for grow-in animations).
    #[serde(default)]
    pub created_tick: u64,
}

impl Building {
    /// Footprint size in cells `(width along x, depth along y)`.
    pub fn size(&self) -> (i32, i32) {
        (self.footprint.width(), self.footprint.height())
    }
}

/// One of the four sides of a cell or footprint, in sim axes
/// (sim `+y` is Bevy `+z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Facing {
    /// Front faces `+x`.
    PlusX,
    /// Front faces `+y`.
    #[default]
    PlusY,
    /// Front faces `-x`.
    MinusX,
    /// Front faces `-y`.
    MinusY,
}

impl Facing {
    pub const ALL: [Facing; 4] = [Facing::PlusX, Facing::PlusY, Facing::MinusX, Facing::MinusY];

    /// Unit cell offset towards the front.
    pub fn offset(self) -> (i32, i32) {
        match self {
            Facing::PlusX => (1, 0),
            Facing::PlusY => (0, 1),
            Facing::MinusX => (-1, 0),
            Facing::MinusY => (0, -1),
        }
    }

    /// Direction of the front in sim space, radians, `atan2(dy, dx)`.
    pub fn angle(self) -> f32 {
        let (dx, dy) = self.offset();
        libm::atan2f(dy as f32, dx as f32)
    }
}
