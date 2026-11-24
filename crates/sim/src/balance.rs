//! Every tunable number in one place.

/// Money at the start of a new game.
pub const STARTING_MONEY: i64 = 50_000;
/// Cost of one cell of newly built road (per cell-to-cell piece of length).
pub const ROAD_COST_PER_CELL: i64 = 10;
/// Cost per cell cleared by the bulldozer (road cell or building cell).
pub const BULLDOZE_COST_PER_CELL: i64 = 1;
/// Daily upkeep per road cell. PHASE2: charged by the daily economy.
pub const ROAD_UPKEEP_PER_CELL_PER_DAY: i64 = 1;

/// Tax rate of a new game.
pub const DEFAULT_TAX_RATE: f32 = 0.09;
/// Highest tax rate `SetTaxRate` accepts (lowest is 0.0).
pub const MAX_TAX_RATE: f32 = 0.30;

/// Zoning is allowed on cells within this Chebyshev distance of a road cell.
pub const ZONE_ROAD_REACH: i32 = 2;

/// Ticks in one in-game day (2 real minutes at 1x).
pub const TICKS_PER_DAY: u32 = 2 * 60 * crate::TICKS_PER_SECOND;
/// Time of day a new game starts at, in ticks after midnight (06:00).
pub const START_TICK_OF_DAY: u32 = TICKS_PER_DAY / 4;

/// Speed limit of ordinary streets, metres per second (50 km/h).
pub const STREET_SPEED_LIMIT: f32 = 13.9;
/// Speed limit of the highway stub, metres per second (100 km/h).
pub const HIGHWAY_SPEED_LIMIT: f32 = 27.8;
/// Lanes per direction.
pub const STREET_LANES: f32 = 1.0;
pub const HIGHWAY_LANES: f32 = 2.0;
/// Road length one queued vehicle occupies, metres (used for capacity).
pub const VEHICLE_SPACING_M: f32 = 8.0;

/// Length of the highway stub in cells, entering from the west map edge.
pub const HIGHWAY_STUB_CELLS: i32 = 12;
