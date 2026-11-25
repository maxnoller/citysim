//! People as the interface shows them: names and portraits from a
//! person's seed, and what a person, a building's people, a car and the
//! whole city look like, as plain data (`Sheet`s of lines and links) that
//! any layout can display.

use citysim_sim::{VehicleId, World};

use super::readout::{Line, Tone};

/// What a view shows: a title and lines.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    pub title: String,
    pub lines: Vec<Line>,
}

/// A car or truck.
pub fn vehicle_sheet(world: &World, id: VehicleId) -> Option<Sheet> {
    let v = citysim_sim::inspect::vehicle(world, id)?;
    Some(Sheet {
        title: format!("{:?}", v.purpose),
        lines: vec![Line::new("People", v.occupants.to_string(), Tone::Normal)],
    })
}
