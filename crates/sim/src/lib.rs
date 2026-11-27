//! `citysim_sim` — the headless, deterministic city simulation.
//!
//! All game state and rules live here. There is no Bevy dependency: the game
//! crate owns a [`World`], sends it [`Command`]s, calls [`World::step`] on a
//! fixed timestep and draws what it reads from the World's public fields.
//!
//! # People (round 3)
//!
//! Every resident is a [`Citizen`] in a [`Household`]: an age on a
//! compressed calendar (`balance::DAYS_PER_YEAR` days a year), a life stage,
//! maybe a job of their own (workplace, wage by tier, shift), what they are
//! doing and where they are (inside a building, or on a trip: in a car drawn
//! on the road, waiting for it, or off screen), a leisure need, happiness
//! with its components and a short journal. Households hold the members,
//! money, rent, goods and one or two cars. The rules: `day` (each citizen's
//! day: work shifts, shopping, leisure, sleep; their trips), `life`
//! (payday, ageing, births, moving out, couples, deaths, happiness, leaving,
//! the job search), `households` (arrivals by household type, leaving),
//! `errands` (goods deliveries driven by shop sales), `people`
//! (bookkeeping). Read API for the interface: [`inspect`]. Round-2 fields
//! (`Household::job`, `Stats::population`, `Stats::workforce`, the budget)
//! are derived from the citizens.
//!
//! Coordinates: the map is [`MAP_CELLS`] x [`MAP_CELLS`] cells of
//! [`CELL_SIZE`] metres. Sim space is 2D in metres; the centre of cell
//! `(x, y)` is `((x + 0.5) * CELL_SIZE, (y + 0.5) * CELL_SIZE)`.

pub mod balance;
pub(crate) mod blocks;
pub(crate) mod build;
pub mod buildings;
pub mod capacity;
pub mod citizens;
pub mod command;
pub mod curve;
pub(crate) mod day;
pub mod demand;
pub mod derived;
pub mod drive;
pub mod economy;
pub mod errands;
pub mod geom;
pub mod grid;
pub mod growth;
pub mod households;
pub mod idmap;
pub mod inspect;
pub mod junction;
pub mod life;
pub mod path;
pub mod people;
pub mod restore;
pub mod rng;
pub mod roads;
pub mod save;
pub mod scenarios;
pub mod side;
pub mod stats;
pub mod time;
pub mod traffic;
pub mod world;
pub mod zoning;

#[cfg(test)]
mod people_tests;
#[cfg(test)]
mod trip_tests;

pub use buildings::{Building, BuildingId, Facing, Footprint};
pub use citizens::{
    Activity, Citizen, CitizenId, Education, Happiness, HappinessPart, HouseholdRole, Job, JobTier, JournalEntry,
    Journey, LifeEvent, LifeStage, Location, Shift, TravelMode,
};
pub use command::{
    Command, CommandError, JunctionChange, NodeChange, Outcome, PieceChange, PlannedNode, PlannedPiece, Preview,
};
pub use curve::{Curve, Pose};
pub use demand::{Demand, JoblessReason, JobsView};
pub use derived::Access;
pub use economy::{
    Attendance, Budget, DaySummary, Economy, FiscalReason, FiscalState, FiscalStatus, RunningCosts, TrafficLoss,
};
pub use geom::{Cell, CellRect};
pub use grid::{CellData, GridView, Occupant, Zone};
pub use growth::{BuildingMeta, GrowthState};
pub use households::{Household, HouseholdId, HouseholdKind, HouseholdState, Ledger};
pub use idmap::IdMap;
pub use inspect::{CityPeople, TripReason};
pub use junction::{
    ApproachView, Aspect, JunctionControl, JunctionSetting, JunctionState, JunctionView, SignalStage, Turn,
};
pub use people::{CommuteMoves, LifeCounts, People, PeopleDay, PlacePeople, TripTimes};
pub use restore::{BuildingRecord, RestoreProblem};
pub use rng::Rng;
pub use roads::{NodeId, RoadKind, RoadNetwork, RoadNode, RoadSegment, SegmentId};
pub use save::LoadError;
pub use side::SideTable;
pub use stats::Stats;
pub use time::SimTime;
pub use traffic::{Departure, JunctionMove, Place, TrafficState, Trip, TripPurpose, Vehicle, VehicleId};
pub use world::{Versions, World};
pub use zoning::{Block, Blocked, CellRef, Lattice, LatticeId, Side, ZoneCell, Zoning};

/// Cells per side of the square map.
pub const MAP_CELLS: i32 = 256;
/// Edge length of one cell in metres.
pub const CELL_SIZE: f32 = 8.0;
/// Fixed simulation ticks per real second at 1x speed.
pub const TICKS_PER_SECOND: u32 = 20;
/// Map edge length in metres.
pub const MAP_SIZE_M: f32 = MAP_CELLS as f32 * CELL_SIZE;
