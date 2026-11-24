//! Every tunable number in one place.
//!
//! # Time scales
//!
//! The sim runs two clocks off the same fixed tick (`TICKS_PER_SECOND` = 20
//! per real second at 1x):
//!
//! * **Vehicle time is real time.** A tick is 1/20 s for anything that moves:
//!   speeds are real m/s, a street car does 50 km/h = 0.7 m per tick, so cars
//!   look right on screen. Commute times are reported in these seconds
//!   ("vehicle seconds"; at 1x they are real seconds).
//! * **Calendar time is compressed.** A day is [`TICKS_PER_DAY`] ticks = 4 real
//!   minutes at 1x (80 s at 3x), so one in-game hour is 10 real seconds.
//!   Daily things (taxes, upkeep, abandonment, giving up on the city) happen
//!   at midnight; demand, growth and moving in run continuously.
//!
//! The mismatch is the usual city-builder compromise: a 40 s drive is "four
//! hours" of calendar time. Rush hours are therefore spread over several
//! calendar hours (see [`MORNING_DEPART_START_H`]) and only a share of the
//! commuters drives on any given day ([`COMMUTE_DRIVE_SHARE`]), which keeps
//! the number of cars proportional to, but much smaller than, the number of
//! workers.

// ---------------------------------------------------------------- money ----
//
// Targets (checked by the `headless` example and `tests/balance.rs`), for a
// sensible player (the scripted `GridPlayer`: grid of 10x5 blocks built as
// demand asks) over a 30-minute game at 2.5x:
//
// * Starting money pays for the first days, when taxes do not yet cover
//   upkeep and services, and is mostly spent by the middle of the game:
//   cash bottoms out around $6k (about 120 road cells) on days 11-14.
// * At 5,000 residents the daily net (taxes - road upkeep - services) is
//   20-35 % of tax income, about $3k; 100 road cells ($5k) cost 1.5-2 days
//   of it, so every expansion and every rebuild is a real expense.
// * The player never waits more than about a minute for the money for its
//   next block, and never has less than 100 road cells' worth in hand.
// * Cash at 30 minutes stays below $25k (it used to be $127k and climbing).
// * Road upkeep is the biggest cost: a network with much more road per
//   resident than a grid (the `overbuilt` scenario) loses money every day.

/// Money at the start of a new game: the first 600 road cells, or the
/// roads of a town of about 3,500.
pub const STARTING_MONEY: i64 = 30_000;
/// Cost of one cell of newly built road (per cell-to-cell piece of length).
pub const ROAD_COST_PER_CELL: i64 = 50;
/// Cost per cell cleared by the bulldozer (road cell or building cell).
pub const BULLDOZE_COST_PER_CELL: i64 = 1;
/// Daily upkeep per road cell (charged at midnight; can push money negative).
/// A road cell of a sensible grid serves ~6.5 residents and their jobs, who
/// pay ~$10 a day net of services: upkeep takes most of that, so wasted
/// road shows.
pub const ROAD_UPKEEP_PER_CELL_PER_DAY: i64 = 6;
/// Daily cost of city services per resident (police, fire, schools...,
/// abstracted), charged at midnight with road upkeep. Keeps the margin per
/// resident thin, so a bigger city is not automatically a money machine.
pub const SERVICES_COST_PER_RESIDENT_PER_DAY: f32 = 0.6;

/// Tax rate of a new game.
pub const DEFAULT_TAX_RATE: f32 = 0.09;
/// Highest tax rate `SetTaxRate` accepts (lowest is 0.0).
pub const MAX_TAX_RATE: f32 = 0.30;
/// Daily taxable value of one resident; income = value * tax rate.
/// At the default 9 % a resident pays about $1.26 per day.
pub const RESIDENT_TAX_BASE_PER_DAY: f32 = 14.0;
/// Daily taxable value of one filled commercial job.
pub const COMMERCIAL_TAX_BASE_PER_JOB_PER_DAY: f32 = 22.0;
/// Daily taxable value of one filled industrial job.
pub const INDUSTRIAL_TAX_BASE_PER_JOB_PER_DAY: f32 = 18.0;

// --------------------------------------------------------------- zoning ----

/// Zoning is allowed on cells within this Chebyshev distance of a road cell.
pub const ZONE_ROAD_REACH: i32 = 2;

// ----------------------------------------------------------------- time ----

/// Ticks in one in-game day (4 real minutes at 1x).
pub const TICKS_PER_DAY: u32 = 4 * 60 * crate::TICKS_PER_SECOND;
/// Ticks in one in-game hour (10 real seconds at 1x).
pub const TICKS_PER_HOUR: u32 = TICKS_PER_DAY / 24;
/// Time of day a new game starts at, in ticks after midnight (06:00).
pub const START_TICK_OF_DAY: u32 = TICKS_PER_DAY / 4;

/// Morning departures to work are spread from this hour ...
pub const MORNING_DEPART_START_H: f32 = 3.5;
/// ... over this many hours (trapezoid distribution: the middle third of
/// the window, about 06:30-09:30, gets 1.5x the average rate).
pub const MORNING_DEPART_SPREAD_H: f32 = 9.0;
/// Households that have not left for work by this hour skip the day.
pub const MORNING_DEPART_LATEST_H: f32 = 13.0;
/// Evening departures home, same idea (rush centred on 18:00).
pub const EVENING_DEPART_START_H: f32 = 14.0;
pub const EVENING_DEPART_SPREAD_H: f32 = 9.0;
/// Fraction of working households that take the car on a given day (decided
/// per household per day, deterministic). The rest commute "off screen" and
/// switch between home and work instantly. Keeps traffic proportional to
/// the city without every worker being on the road at once.
pub const COMMUTE_DRIVE_SHARE: f32 = 0.5;

// ------------------------------------------------------------- vehicles ----

/// Speed limit of ordinary streets, metres per second (50 km/h).
pub const STREET_SPEED_LIMIT: f32 = 13.9;
/// Speed limit of the highway stub, metres per second (100 km/h).
pub const HIGHWAY_SPEED_LIMIT: f32 = 27.8;
/// Lanes per direction.
pub const STREET_LANES: f32 = 1.0;
pub const HIGHWAY_LANES: f32 = 2.0;
/// Road length one queued vehicle occupies, metres (used for capacity:
/// `capacity = length * lanes / VEHICLE_SPACING_M`). Slightly above
/// `MIN_GAP_M`, so a standing queue reads ~0.9-1.0 and "over 80 %" means
/// cars crawl below ~12 km/h.
pub const VEHICLE_SPACING_M: f32 = 6.0;

/// Distance between the centres of two stopped cars in a queue, metres.
pub const MIN_GAP_M: f32 = 5.5;
/// Time headway drivers keep to the car ahead: allowed speed is
/// `(gap - MIN_GAP_M) / FOLLOW_HEADWAY_S`. 1 s gives about 0.65 cars/s per
/// lane at street speed.
pub const FOLLOW_HEADWAY_S: f32 = 0.5;
/// Acceleration, m/s².
pub const ACCELERATION: f32 = 3.0;
/// Comfortable braking used towards stop lines and junctions, m/s².
pub const BRAKING: f32 = 4.0;
/// Speed at which cars turn at junctions (degree >= 3), m/s. Straight-on
/// traffic keeps its speed.
pub const JUNCTION_SPEED: f32 = 8.0;
/// Cars stop this far before a node they may not enter yet, metres.
pub const STOP_LINE_M: f32 = 0.6;
/// Per-driver speed variation: each car drives at `1 ± this` of the
/// allowed speed (from its seed).
pub const DRIVER_SPEED_VARIATION: f32 = 0.08;
/// Volume-delay curve for driving speed: speed = limit / (1 + A * x^B) with
/// x = load / capacity of that direction. At x = 1 cars drive at half speed
/// even before queueing kicks in.
pub const VDF_ALPHA: f32 = 1.0;
pub const VDF_BETA: f32 = 2.0;

/// Junctions (nodes with three or more segments) work like simple traffic
/// lights: approaches on one straight line (e.g. north and south) form a
/// phase and flow together, limited only by car following, for up to this
/// many cars; then the next waiting approach (round-robin) gets a phase.
pub const PHASE_MAX_CARS: u16 = 6;
/// After the last car of a phase, other phases wait this long (clearing the
/// junction). This is what makes a busy junction a bottleneck.
pub const JUNCTION_CLEARANCE_S: f32 = 0.35;
/// Route planning: fixed cost of passing a junction, seconds (prefers
/// straighter routes with fewer junctions).
pub const ROUTE_JUNCTION_COST_S: f32 = 1.5;
/// Route planning: expected wait per car already on a segment direction
/// that ends at a junction, multiplied by that direction's congestion.
pub const ROUTE_QUEUE_COST_S: f32 = 2.0;

/// A car slower than this counts as waiting.
pub const WAIT_SPEED: f32 = 0.5;
/// A car that has been waiting this long looks for a new route.
pub const REPLAN_AFTER_WAIT_S: f32 = 10.0;
/// A car that has been waiting this long gives up: it is removed and its
/// trip ends as if it arrived (gridlock breaker).
pub const STUCK_REMOVE_AFTER_S: f32 = 60.0;
/// Path searches are budgeted: about this many node expansions per tick
/// (at least one search always runs). Others wait in the queue. Re-plans
/// go first; departures always get at least half of it.
pub const PATH_EXPANSIONS_PER_TICK: u32 = 2500;
/// At most this many waiting departures are tried per tick.
pub const DEPARTURES_PER_TICK: usize = 24;
/// A trip whose car could not start within this many seconds (blocked
/// driveway, saturated network) happens off screen instead and counts as a
/// hopeless commute.
pub const MAX_DEPARTURE_WAIT_S: f32 = 40.0;
/// A car only pulls out of a driveway if no car on that lane is within this
/// distance of the spot, metres.
pub const SPAWN_CLEARANCE_M: f32 = 6.0;
/// How far from a building (centre) its road access point may be, cells.
pub const ACCESS_SEARCH_CELLS: i32 = 4;

// -------------------------------------------------------------- errands ----

/// Errand traffic is generated every this many ticks.
pub const ERRAND_INTERVAL_TICKS: u64 = 10;
/// One-way shopping runs per household per day (half home -> shop, half
/// back), mostly 10:00-20:00.
pub const SHOP_TRIPS_PER_HOUSEHOLD_PER_DAY: f32 = 0.3;
/// Goods trips per filled industrial job per day, mostly 05:00-19:00.
pub const GOODS_TRIPS_PER_INDUSTRIAL_JOB_PER_DAY: f32 = 0.25;
/// Share of goods trips that leave for / come from the highway.
pub const GOODS_VIA_HIGHWAY_SHARE: f32 = 0.3;
/// Errand destinations: nearest of this many random candidates.
pub const ERRAND_SAMPLES: usize = 3;

// ----------------------------------------------------------- households ----

/// Household sizes 1..=4 are drawn with these weights (mean ~2.6).
pub const HOUSEHOLD_SIZE_WEIGHTS: [u32; 4] = [20, 30, 25, 25];
/// Every this many ticks unemployed households look for jobs ...
pub const JOB_MATCH_INTERVAL_TICKS: u64 = 10;
/// ... at most this many per round ...
pub const JOB_MATCH_PER_ROUND: usize = 60;
/// ... each comparing this many random vacancies and taking the nearest.
pub const JOB_MATCH_SAMPLES: usize = 6;
/// Move-ins per in-game hour per household of free housing (fills new
/// houses within a couple of hours) ...
pub const MOVE_IN_FILL_PER_HOUR: f32 = 0.6;
/// ... at most this many per hour ...
pub const MOVE_IN_MAX_PER_HOUR: f32 = 120.0;
/// ... and only while the city's attractiveness (residential demand
/// ignoring empty housing) is above this.
pub const MOVE_IN_MIN_DEMAND: f32 = -0.25;
/// A household unemployed at this many consecutive midnights leaves town.
pub const UNEMPLOYED_LEAVE_DAYS: u8 = 2;
/// A car commute longer than this (vehicle seconds) is hopeless ...
pub const HOPELESS_COMMUTE_S: f32 = 150.0;
/// ... and after this many hopeless commutes in a row the household leaves.
pub const HOPELESS_COMMUTES_TO_LEAVE: u8 = 3;
/// Leaving households drive out to the highway.
pub const MOVE_OUT_DRIVES: bool = true;

// --------------------------------------------------------------- demand ----

/// Demand is recomputed every this many ticks ...
pub const DEMAND_INTERVAL_TICKS: u64 = 20;
/// ... and moves this fraction of the way to the new target each time
/// (time constant ~ 7 real seconds at 1x; prevents wild swings).
pub const DEMAND_SMOOTHING: f32 = 0.12;
/// Residential demand of an empty city / of a balanced one.
pub const RES_BASE_DEMAND: f32 = 0.35;
/// Weight of (free jobs - unemployed) / households on residential demand.
pub const RES_JOBS_WEIGHT: f32 = 2.0;
/// Residential demand falls by this per unit of housing vacancy above
/// [`RES_VACANCY_OK`] (vacancy = empty places / (capacity + 2 *
/// `DEMAND_SMALL_CITY`)).
pub const RES_VACANCY_WEIGHT: f32 = 2.5;
pub const RES_VACANCY_OK: f32 = 0.10;
/// Commercial jobs (shops) wanted per resident.
pub const SHOP_JOBS_PER_RESIDENT: f32 = 0.2;
/// Industrial jobs wanted per commercial job.
pub const INDUSTRY_PER_SHOP: f32 = 1.5;
/// Weight of the relative job shortfall on C / I demand.
pub const JOB_SHORTFALL_WEIGHT: f32 = 1.4;
/// Extra C / I demand per unit of unemployment rate.
pub const UNEMPLOYMENT_JOB_BOOST: f32 = 1.5;
/// C / I demand falls by this per unit of unfilled-job rate above
/// [`JOB_VACANCY_OK`].
pub const JOB_VACANCY_WEIGHT: f32 = 2.0;
pub const JOB_VACANCY_OK: f32 = 0.12;
/// Softens ratios in a tiny city (added to denominators).
pub const DEMAND_SMALL_CITY: f32 = 40.0;
/// Demand change per unit of tax rate above [`DEFAULT_TAX_RATE`] (negative
/// above, positive below). At 30 % tax all demand drops by ~0.8.
pub const TAX_DEMAND_SLOPE: f32 = 4.0;
/// Average car commutes longer than this (vehicle seconds) cut residential
/// demand ...
pub const COMMUTE_OK_S: f32 = 60.0;
/// ... by 1.0 per this many extra seconds.
pub const COMMUTE_DEMAND_SCALE_S: f32 = 120.0;

// --------------------------------------------------------------- growth ----

/// Growth runs every this many ticks.
pub const GROWTH_INTERVAL_TICKS: u64 = 10;
/// New buildings (or upgrades) per in-game hour at demand 1.0, per zone
/// `[residential, commercial, industrial]`. Scales linearly with demand.
pub const GROWTH_PER_HOUR: [f32; 3] = [9.5, 3.5, 3.2];
/// Fraction of growth events that upgrade an existing full building
/// instead of building a new one (when both are possible).
pub const UPGRADE_SHARE: f32 = 0.2;
/// A building can upgrade once it is at least this full ...
pub const UPGRADE_MIN_FULL: f32 = 0.9;
/// ... and at least this old (ticks).
pub const UPGRADE_MIN_AGE_TICKS: u64 = TICKS_PER_DAY as u64 / 2;
/// Highest level per zone `[R, C, I]`.
pub const MAX_LEVEL: [u8; 3] = [3, 3, 2];
/// Residents (R) or jobs (C, I) per footprint cell at levels 1, 2, 3.
pub const CAPACITY_PER_CELL: [[u32; 3]; 3] = [[4, 9, 18], [3, 6, 10], [4, 8, 8]];
/// Chance (percent) that a new C / I building tries a 2x2 footprint.
pub const BIG_FOOTPRINT_PERCENT: [u32; 3] = [0, 35, 60];
/// A building empty at this many consecutive midnights is abandoned.
pub const ABANDON_EMPTY_DAYS: u8 = 3;
/// No longer used by the sim (kept so existing code still compiles): it was
/// "abandoned at this many consecutive midnights without road access",
/// which killed a building whose road was cut at 23:59 for a rebuild.
/// Abandonment for lost road access now uses
/// [`ABANDON_UNREACHABLE_GRACE_TICKS`]. `BuildingMeta::unreachable_days`
/// still counts those midnights for display.
pub const ABANDON_UNREACHABLE_DAYS: u8 = 1;
/// A building that has been continuously without road access to the
/// highway for this long (sim ticks, one in-game day: 4 real minutes at
/// 1x, 80 s at 3x) is abandoned. Reconnecting it at any point before that
/// resets the clock, so a player can cut a road and rebuild it.
pub const ABANDON_UNREACHABLE_GRACE_TICKS: u64 = TICKS_PER_DAY as u64;
/// Road access of every building is checked this often (ticks; one in-game
/// hour), so the grace period above ends within an hour of running out.
pub const REACHABILITY_CHECK_INTERVAL_TICKS: u64 = TICKS_PER_HOUR as u64;

// ------------------------------------------------------------------ map ----

/// Length of the highway stub in cells, entering from the west map edge.
pub const HIGHWAY_STUB_CELLS: i32 = 12;
