//! `citysim_sim` — the headless, deterministic city simulation.
//!
//! All game state and rules live here. There is no Bevy dependency: the game
//! crate owns a [`World`], sends it [`Command`]s, calls [`World::step`] on a
//! fixed timestep and draws what it reads from the World's public fields.
//!
//! Coordinates: the map is [`MAP_CELLS`] x [`MAP_CELLS`] cells of
//! [`CELL_SIZE`] metres. Sim space is 2D in metres; the centre of cell
//! `(x, y)` is `((x + 0.5) * CELL_SIZE, (y + 0.5) * CELL_SIZE)`.

pub mod balance;
pub mod buildings;
pub mod command;
pub mod demand;
pub mod economy;
pub mod geom;
pub mod grid;
pub mod growth;
pub mod households;
pub mod idmap;
pub mod rng;
pub mod roads;
pub mod save;
pub mod stats;
pub mod time;
pub mod traffic;
pub mod world;

pub use buildings::{Building, BuildingId, Facing};
pub use command::{Command, CommandError, Outcome, Preview};
pub use demand::Demand;
pub use economy::Economy;
pub use geom::{Cell, CellRect};
pub use grid::{CellData, Grid, Occupant, Zone};
pub use households::{Household, HouseholdId, HouseholdState};
pub use idmap::IdMap;
pub use rng::Rng;
pub use roads::{NodeId, RoadKind, RoadNetwork, RoadNode, RoadSegment, SegmentId};
pub use save::LoadError;
pub use stats::Stats;
pub use time::SimTime;
pub use traffic::{Vehicle, VehicleId};
pub use world::{Versions, World};

/// Cells per side of the square map.
pub const MAP_CELLS: i32 = 256;
/// Edge length of one cell in metres.
pub const CELL_SIZE: f32 = 8.0;
/// Fixed simulation ticks per real second at 1x speed.
pub const TICKS_PER_SECOND: u32 = 20;
/// Map edge length in metres.
pub const MAP_SIZE_M: f32 = MAP_CELLS as f32 * CELL_SIZE;
