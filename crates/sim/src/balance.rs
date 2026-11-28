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
//! * **Calendar time is compressed.** A day is [`TICKS_PER_DAY`] ticks =
//!   [`DAY_MINUTES`] (16) real minutes at 1x, so one in-game hour is 40 real
//!   seconds. Daily things (taxes, upkeep, wages, rent, abandonment, giving
//!   up on the city) happen at midnight; demand, growth and moving in run
//!   continuously.
//!
//! **Trips take the time they take.** A car moves in real time, and the
//! people in it stay in it from departure until arrival, whatever the clock
//! says; their next activity starts when they arrive. A typical drive of
//! 40-90 real seconds is 1-2¼ in-game hours (the price of a compressed
//! calendar), so workers leave home early enough by the door-to-door time
//! their last commute took (`Citizen::commute_ticks`). Trips made off screen
//! (no free car, or the vehicle budget [`MAX_VEHICLES`] is used up) take the
//! same time as the drive would ([`OFFSCREEN_CALENDAR_FACTOR`] is 1). Every
//! car trip is drawn on the road while the budget allows; rush hours emerge
//! from the workers' shifts.
//!
//! # Day length
//!
//! Round 2 had a 4-minute day. The day is now [`DAY_SCALE`] (4) times as
//! long, which makes a drive a plausible part of a day, and still shows a
//! night every few minutes at the faster speeds (a 30-minute session at
//! 2.5x sees about five days). Every per-day amount of money and every rate
//! per in-game hour or day that sets the pace of the city (growth, moving
//! in, wages, rents, taxes, upkeep, services, exports) is scaled by
//! [`DAY_SCALE`], so the pace per real minute is what it was: a sensible
//! player reaches about 5,000 residents in about 30 real minutes at 2.5x.
//! Routines stay per day (one commute each way, shopping every couple of
//! days, a night out every few days). Ages: one in-game day is one year of
//! age ([`DAYS_PER_YEAR`]), half round 3's pace of life per real minute,
//! so a 30-minute session spans about five years.

// ---------------------------------------------------------------- money ----
//
// Targets (checked by the `headless` example and `tests/balance.rs`), over
// a game at 2.5x:
//
// * A sensible layout stays solvent however it is built. A town runs a
//   small deficit only on its first day while it is still empty, then a
//   clear and growing surplus. That holds for a whole two-district grid
//   laid on day one (`scenarios::prebuilt`), for a player who spends most
//   of the start money on a district on day one and then follows demand
//   (`GridPlayer::build_ahead`), and for one who builds block by block
//   (`GridPlayer`).
// * Construction is the big expense, and running costs grow faster than
//   the city (services cost more per resident in a bigger and denser city,
//   avenues and traffic lights cost upkeep), so money stays a constraint
//   for the whole game: the block-by-block player holds $20-40k before
//   5,000 residents, every sensible layout holds tens of thousands at
//   10,000 (enough for the next district or an avenue upgrade, not
//   hundreds of thousands), and the longest wait for money is a few real
//   minutes.
// * Traffic decides who comes out ahead (round 5): commuting time lost in
//   jams costs business tax (`TRAFFIC_OUTPUT_PER_S`, shown as
//   `economy::TrafficLoss`), jammed residents are unhappy, move nearer work
//   or leave, and newcomers stay away. A road hierarchy earns back its
//   extra construction and upkeep; the same blocks on streets only run
//   into a deficit as they grow past 12,000 residents.
// * A network far bigger than its town needs (the pre-built `overbuilt`
//   mesh, ~9,000 cells) loses money every day.

/// Money at the start of a new game: the first 460 road cells.
pub const STARTING_MONEY: i64 = 30_000;
/// Cost of one cell of newly built road (per cell-to-cell piece of length).
/// The main money sink: an expanding city spends most of its surplus here.
pub const ROAD_COST_PER_CELL: i64 = 65;
/// Cost of one cell of avenue (round 5: 140 -> 90, round 6: 75; a hierarchy
/// must be affordable to the player who builds it). Upgrading a street to
/// an avenue costs the difference; downgrading is free (no refund).
pub const AVENUE_COST_PER_CELL: i64 = 75;
/// Cost per cell cleared by the bulldozer (road cell or building cell).
pub const BULLDOZE_COST_PER_CELL: i64 = 1;
/// Daily upkeep per road cell (charged at midnight; can push money negative).
/// Low enough that a sensible network built ahead of its town (2-3 road
/// cells per resident while it fills) is carried by the town after a day
/// or two; a mesh many times bigger than its town still sinks it.
/// ($1 per 4 real minutes, as in round 2.)
pub const ROAD_UPKEEP_PER_CELL_PER_DAY: i64 = 4;
/// Daily upkeep per avenue cell (four lanes, more to maintain; round 5:
/// 10 -> 8, round 6: 6).
pub const AVENUE_UPKEEP_PER_CELL_PER_DAY: i64 = 6;
/// Daily upkeep of a junction with traffic lights (power, maintenance),
/// part of road upkeep. Busy junctions get lights automatically, so a city
/// whose traffic needs many lights pays for them.
pub const SIGNAL_UPKEEP_PER_DAY: i64 = 80;
/// Daily cost of city services per resident (police, fire, schools...,
/// abstracted), charged at midnight with road upkeep. Keeps the margin per
/// resident moderate (residents and their jobs pay ~$9 a day in taxes at
/// the default rate), so a bigger city is not a money machine (round 5:
/// 3.0 -> 3.6, with a gentler size term below).
pub const SERVICES_COST_PER_RESIDENT_PER_DAY: f32 = 3.6;
/// A bigger city needs more complex services (hospitals, a fire brigade
/// with ladders, transit, courts): each resident costs up to this much more
/// per day, `* pop² / (pop² + SERVICES_CITY_SIZE_POP²)`: little in a town
/// ($0.1 at 3,000 residents), $0.6 at 10,000, $1.1 at 20,000. Total
/// services therefore grow faster than the population, but a well-run
/// bigger city keeps netting more per day as it grows (round 6: 2.2 at
/// 9,000 made net income fall beyond 8,000 residents).
pub const SERVICES_CITY_SIZE_PER_RESIDENT: f32 = 1.5;
/// Population at which half of [`SERVICES_CITY_SIZE_PER_RESIDENT`] applies.
pub const SERVICES_CITY_SIZE_POP: f32 = 12_000.0;
/// The cost of density: extra services per resident per day by the level
/// of their home `[1, 2, 3]` (apartment blocks need more fire cover,
/// policing and rubbish collection than houses).
pub const SERVICES_DENSITY_PER_RESIDENT: [f32; 3] = [0.0, 0.4, 1.0];

/// Traffic costs business: every commute's time beyond
/// `TRAFFIC_FREE_FACTOR` times its free-flow estimate (rush-hour traffic in
/// a city that works costs nothing; round 6: 1.6 -> 2.0) is output lost at
/// `TRAFFIC_OUTPUT_PER_S` dollars per vehicle second ($10 an in-game hour
/// of one worker), and the business tax on it (tax rate times that)
/// is not paid, up to `TRAFFIC_LOSS_MAX_SHARE` of the day's business tax
/// (`economy::TrafficLoss`, taken off the commercial and industrial tax
/// lines).
pub const TRAFFIC_FREE_FACTOR: f32 = 2.0;
pub const TRAFFIC_OUTPUT_PER_S: f32 = 0.9;
pub const TRAFFIC_LOSS_MAX_SHARE: f32 = 0.7;

/// Debt: every midnight with money below zero cuts city services by this
/// share (services cost less, but all demand falls), down to
/// [`SERVICE_LEVEL_MIN`]; every midnight in the black restores this much.
pub const SERVICE_CUT_PER_DAY: f32 = 0.25;
pub const SERVICE_RECOVER_PER_DAY: f32 = 0.5;
pub const SERVICE_LEVEL_MIN: f32 = 0.3;
/// Demand lost per unit of missing services (residential; jobs half).
pub const SERVICE_DEMAND_WEIGHT: f32 = 1.6;
/// Demand lost while money is below zero (nobody invests in a broke city).
pub const DEBT_DEMAND_PENALTY: f32 = 0.15;
/// After this many midnights in debt the city is in crisis: each night
/// `(1 - service level) * CRISIS_LEAVE_SHARE_PER_DAY` of households leave.
pub const DEBT_CRISIS_DAYS: u16 = 2;
pub const CRISIS_LEAVE_SHARE_PER_DAY: f32 = 0.25;

/// Tax rate of a new game.
pub const DEFAULT_TAX_RATE: f32 = 0.09;
/// Highest tax rate `SetTaxRate` accepts (lowest is 0.0).
pub const MAX_TAX_RATE: f32 = 0.30;
/// Round 3: no longer used by the sim (the residential tax is taken from wages, [`WAGE_BY_TIER`]); kept for compatibility.
/// Daily taxable value of one resident; income = value * tax rate.
/// At the default 9 % a resident pays about $5 per day.
pub const RESIDENT_TAX_BASE_PER_DAY: f32 = 56.0;
/// Daily taxable value of one filled commercial job.
pub const COMMERCIAL_TAX_BASE_PER_JOB_PER_DAY: f32 = 68.0;
/// Daily taxable value of one filled industrial job.
pub const INDUSTRIAL_TAX_BASE_PER_JOB_PER_DAY: f32 = 56.0;

// --------------------------------------------------------------- zoning ----

/// Zoning is allowed on cells within this Chebyshev distance of a road cell.
pub const ZONE_ROAD_REACH: i32 = 2;

// ----------------------------------------------------------------- time ----

/// Length of an in-game day at 1x, real minutes.
pub const DAY_MINUTES: u32 = 16;
/// The day is this many times as long as round 2's 4-minute day; per-day
/// amounts and per-hour rates that set the city's pace are scaled by it
/// (see the module docs).
pub const DAY_SCALE: u32 = DAY_MINUTES / 4;
/// Ticks in one in-game day (16 real minutes at 1x).
pub const TICKS_PER_DAY: u32 = DAY_MINUTES * 60 * crate::TICKS_PER_SECOND;
/// Ticks in one in-game hour (40 real seconds at 1x).
pub const TICKS_PER_HOUR: u32 = TICKS_PER_DAY / 24;
/// Time of day a new game starts at, in ticks after midnight (09:00: a
/// new city opens on a bright morning).
pub const START_TICK_OF_DAY: u32 = TICKS_PER_DAY * 3 / 8;

/// Round 3: no longer used by the sim (departures follow each worker's shift, [`INDUSTRY_START_H`] and below); kept for compatibility.
/// Morning departures to work are spread from this hour ...
pub const MORNING_DEPART_START_H: f32 = 3.5;
/// Round 3: no longer used by the sim (departures follow each worker's shift); kept for compatibility.
/// ... over this many hours (trapezoid distribution: the middle third of
/// the window, about 06:30-09:30, gets 1.5x the average rate).
pub const MORNING_DEPART_SPREAD_H: f32 = 9.0;
/// Round 3: no longer used by the sim (a shift is skipped when less than [`MIN_WORK_H`] would be left); kept for compatibility.
/// Households that have not left for work by this hour skip the day.
pub const MORNING_DEPART_LATEST_H: f32 = 13.0;
/// Round 3: no longer used by the sim (workers go home when their shift ends); kept for compatibility.
/// Evening departures home, same idea (rush centred on 18:00).
pub const EVENING_DEPART_START_H: f32 = 14.0;
/// Round 3: no longer used by the sim (workers go home when their shift ends); kept for compatibility.
pub const EVENING_DEPART_SPREAD_H: f32 = 9.0;
/// No longer used by the sim: every car trip is drawn on the road while the
/// vehicle budget allows ([`MAX_VEHICLES`]); kept for compatibility (round 3
/// drew 17 % of commutes).
pub const COMMUTE_DRIVE_SHARE: f32 = 1.0;

// ------------------------------------------------------------- vehicles ----

/// Speed limit of ordinary streets, metres per second (50 km/h).
pub const STREET_SPEED_LIMIT: f32 = 11.1;
/// Speed limit of the highway stub, metres per second (100 km/h).
pub const HIGHWAY_SPEED_LIMIT: f32 = 27.8;
/// Speed limit of avenues, metres per second (60 km/h).
pub const AVENUE_SPEED_LIMIT: f32 = 16.7;
/// Lanes per direction.
pub const STREET_LANES: u8 = 1;
pub const HIGHWAY_LANES: u8 = 2;
pub const AVENUE_LANES: u8 = 2;
/// Road length one queued vehicle occupies, metres (used for capacity:
/// `capacity = length * lanes / VEHICLE_SPACING_M`). Slightly above
/// `MIN_GAP_M`, so a standing queue reads ~0.9-1.0 and "over 80 %" means
/// cars crawl below ~12 km/h.
pub const VEHICLE_SPACING_M: f32 = 6.0;

/// Distance between the centres of two stopped cars in a queue, metres.
pub const MIN_GAP_M: f32 = 5.5;
/// Time headway drivers keep to the car ahead: allowed speed is
/// `(gap - MIN_GAP_M) / headway`. On avenues and the highway this one
/// (a queue discharges at about 0.6 cars/s per lane, an empty lane carries
/// about 0.7 at speed); on streets `STREET_FOLLOW_HEADWAY_S`.
pub const FOLLOW_HEADWAY_S: f32 = 1.1;
/// ... on streets: 2.75 m lanes lined with driveways and kerbside activity
/// are driven with more distance (about 0.5 cars/s, 1,350 an hour, per lane
/// from a queue). 2.6 s made a small town of streets give up trips at its
/// T junctions (queues backed up across them); 2.2 s keeps it as it was.
pub const STREET_FOLLOW_HEADWAY_S: f32 = 2.2;
/// Acceleration, m/s².
pub const ACCELERATION: f32 = 3.5;
/// Comfortable braking used towards stop lines and junctions, m/s².
pub const BRAKING: f32 = 4.0;
/// Speed at which cars turn at junctions (degree >= 3), m/s. Straight-on
/// traffic keeps its speed.
pub const JUNCTION_SPEED: f32 = 9.0;
/// A car waiting at a stop line stands with its front bumper this far
/// behind the line, metres (its centre half a body length further back:
/// `drive::stop_back_m`), so turning cars pass clear of its nose.
pub const STOP_LINE_M: f32 = 0.5;
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
/// A car facing yellow keeps going (clears the junction) when faster than
/// this, m/s: it could not stop comfortably at the line.
pub const YELLOW_GO_SPEED: f32 = 4.0;
/// Speed of a U-turn through a junction box, m/s.
pub const UTURN_SPEED: f32 = 3.0;
/// The front car of a lane decides whether it may enter the junction
/// ahead from this far (or its braking distance, if longer), metres.
pub const DECIDE_DISTANCE_M: f32 = 45.0;
/// A car may enter a junction behind a queue on its out-lane that is
/// moving off faster than this (m/s) without a full car spacing yet, when
/// that lane is long (the queue cannot stop right behind the box).
pub const EXIT_MOVING_SPEED: f32 = 3.0;
/// How far ahead (seconds) a car about to enter a junction counts the room
/// a moving out-lane frees for it.
pub const EXIT_LOOKAHEAD_S: f32 = 2.0;
/// ... only when the out-lane still has storage for this many more cars
/// beyond everyone on it, heading for it and this car.
pub const EXIT_STORAGE_MARGIN_CARS: f32 = 1.0;
/// No lane changes and no cars pulling out of driveways this close behind
/// a junction box, metres (cars leaving the box need the room).
pub const NO_LANE_CHANGE_AFTER_BOX_M: f32 = 10.0;
/// All-way stops (crossings of equal roads): a car may only enter once it
/// stood at the line, i.e. slower than this (m/s) within this distance of
/// it (m).
pub const STOP_SIGN_SPEED: f32 = 1.0;
pub const STOP_SIGN_DISTANCE_M: f32 = 1.5;
/// ... after standing there this long, seconds (a real stop).
pub const STOP_DWELL_S: f32 = 1.0;
/// All-way stops: once the box is empty the next car sets off this much
/// later, seconds (looking both ways).
pub const ALL_WAY_REACTION_S: f32 = 1.2;
/// A car slower than this (m/s) that is in the wrong lane for its next turn
/// asks the car behind in the lane it needs to leave it a gap
/// (`MERGE_ROOM_M` more than a standing queue's spacing), unless it is
/// within `MERGE_MIN_LINE_M` of the stop line (it turns from where it is).
pub const MERGE_ASK_SPEED: f32 = 3.0;
pub const MERGE_ROOM_M: f32 = 1.2;
pub const MERGE_MIN_LINE_M: f32 = 8.0;
/// Critical gaps, seconds: a permissive left turn (and same-class cars)
/// leaves this much time to a conflicting car with precedence, a car on a
/// give-way approach this much to the major road.
pub const PERMISSIVE_GAP_S: f32 = 2.5;
pub const GIVE_WAY_GAP_S: f32 = 3.5;
/// Courtesy at a priority junction: a minor-road car that has stood this
/// long at its line goes as if it had priority while every conflicting car
/// with priority is slower than [`COURTESY_SPEED`] (a creeping queue lets
/// one in), seconds and m/s.
pub const COURTESY_WAIT_S: f32 = 20.0;
pub const COURTESY_SPEED: f32 = 4.0;
/// A car not moving inside a junction box for this long is taken off the
/// road (deadlock breaker; only road edits can cause it).
pub const BOX_DEADLOCK_S: f32 = 20.0;
/// A lane change takes this long (renderers ease `lane_shift` over it), s.
pub const LANE_CHANGE_S: f32 = 1.2;
/// Cars move into the lane their next turn needs within this distance of
/// the stop line, metres.
pub const LANE_PREPARE_M: f32 = 150.0;
/// Extra stop-line distance beyond the carriageways of the crossing roads,
/// metres: room for a pedestrian crossing and the kerb radius (junctions of
/// three or more roads).
pub const JUNCTION_MARGIN_M: f32 = 2.5;
/// The same for a bend (two roads meeting at an angle): kerb radius only.
pub const BEND_MARGIN_M: f32 = 0.5;
/// Two pieces of road meeting at a slight kink (turning by more than about
/// 1.8°: cosine below this) get a bend box sized to the kink.
pub const BEND_KINK_COS: f32 = 0.9995;
/// Two movements through a junction whose paths come closer than this
/// (centre lines, metres) conflict even when they do not cross: a car is
/// about 1.9 m wide, plus room so the bodies never brush.
pub const MOVEMENT_CLEARANCE_M: f32 = 2.6;
/// ... two movements from neighbouring lanes of one approach, which start
/// side by side like the lanes: closer than this (a car's width plus a
/// little) only where they really close in on each other.
pub const NEIGHBOUR_CLEARANCE_M: f32 = 2.2;
/// Turn pockets: before a junction an avenue approach gets a short
/// left-turn lane cut into its median (lane `RoadKind::pocket_lane`), as
/// long as `POCKET_MAX_M`, never shorter than `POCKET_MIN_M` (no pocket on
/// a block too short for it). When both ends of a segment have one, they
/// share the drivable length and leave `POCKET_GAP_M` of median between
/// them; a pocket alone leaves `NO_LANE_CHANGE_AFTER_BOX_M` plus this gap
/// behind the box it follows. Left turners move into it from the inner
/// lane; one that finds it full waits at its entry, `POCKET_ENTRY_M` in,
/// so the queue spills back into the inner lane.
pub const POCKET_MAX_M: f32 = 60.0;
/// (Pockets longer than this only where they take at most half the
/// block's drivable length.)
pub const POCKET_LONG_M: f32 = 40.0;
pub const POCKET_MIN_M: f32 = 11.0;
pub const POCKET_GAP_M: f32 = 6.0;
pub const POCKET_ENTRY_M: f32 = 4.0;
/// Offset of the pocket lane's centre from the centre line (right of
/// travel), metres, and the avenue median's half-width (the pocket is cut
/// into it).
pub const POCKET_OFFSET_M: f32 = 0.0;
pub const AVENUE_MEDIAN_HALF_M: f32 = 1.0;
/// Signalled junctions give approaches with a turn pocket a protected left
/// phase (called by a left turner standing at the pocket's line; both
/// opposite pockets turn together), before the through phase.
pub const POCKET_LEFT_PHASES: bool = true;
/// ... called only once this many cars stand in the pocket (fewer clear
/// out permissively at the end of the through green).
pub const POCKET_CALL_CARS: u16 = 3;
/// Maximum greens at junctions whose approaches have pockets: the left
/// phase, and the main through phases (longer, so the left phase's lost
/// time is spread over more cars).
pub const POCKET_LEFT_MAX_GREEN_S: f32 = 14.0;
pub const POCKET_MAIN_MAX_GREEN_S: f32 = 40.0;
/// ... but no longer than this while one of its pockets is full.
pub const POCKET_FULL_MAX_GREEN_S: f32 = 30.0;
/// Right turns of the other road go during a left-arrow phase.
pub const RIGHT_TURN_OVERLAP: bool = true;
/// Routing: a left turn from a turn pocket costs this share of the usual
/// left-turn cost (`ROUTE_LEFT_PER_LANE_S`): it waits out of the way.
pub const ROUTE_POCKET_LEFT_SHARE: f32 = 0.5;
/// Length of the merge box where a road changes its number of lanes
/// without a junction (split between both sides of the node), metres.
pub const LANE_MERGE_TAPER_M: f32 = 14.0;

// -------------------------------------------------------------- signals ----

/// Minimum / maximum green of a phase, seconds of vehicle time. A phase
/// ends early once nobody approaches on green (gap-out), never before its
/// minimum, and is skipped when nobody waits for it.
pub const SIGNAL_MIN_GREEN_S: f32 = 4.0;
pub const SIGNAL_MAIN_MAX_GREEN_S: f32 = 20.0;
pub const SIGNAL_SIDE_MAX_GREEN_S: f32 = 15.0;
/// Protected left-turn phases on multi-lane approaches. Off: in a grid of
/// short blocks the extra phases lengthen every cycle and queues spill back
/// into the junctions behind (measured: about a quarter more standing time
/// per trip in the avenue city); left turns are permissive and clear out
/// at the end of green instead.
pub const SIGNAL_LEFT_PHASES: bool = false;
/// Protected left-turn phase (avenues), when `SIGNAL_LEFT_PHASES`.
pub const SIGNAL_LEFT_MIN_GREEN_S: f32 = 4.0;
pub const SIGNAL_LEFT_MAX_GREEN_S: f32 = 8.0;
/// Yellow and all-red clearance, seconds.
pub const SIGNAL_YELLOW_S: f32 = 2.5;
pub const SIGNAL_ALL_RED_S: f32 = 1.0;
/// Does the all-red wait (up to three times as long) for the box to empty?
/// Off: cars let in on the next green yield to those still in the box
/// anyway.
pub const SIGNAL_ALL_RED_WAITS_FOR_BOX: bool = false;
/// A green is held only for vehicles that reach the line within this many
/// seconds (passage time); vehicles further back call their phase.
pub const SIGNAL_PASSAGE_S: f32 = 2.5;
/// Queue-based gap-out: a through green whose approaches have at most
/// [`SIGNAL_DISCHARGED_CARS`] standing cars left ends (after its minimum)
/// when another approach has [`SIGNAL_SWITCH_QUEUE_CARS`] standing, even
/// if more cars are arriving.
pub const SIGNAL_DISCHARGED_CARS: u16 = 1;
/// (A car near the line slower than this, m/s, is still pulling away from
/// a queue: it holds the green whatever stands elsewhere.)
pub const SIGNAL_DISCHARGE_SPEED: f32 = 7.0;
/// (Nor while the cars within [`SIGNAL_DETECT_M`] of the line on its
/// through lanes, times this, are at least as many as those standing for
/// the other phase.)
pub const SIGNAL_SWITCH_RATIO: u32 = 2;
/// Signals that are not coordinated serve next the phase with the most
/// cars standing for it (left phases: the cars in the pockets), not the
/// next one in plan order; a phase whose front car has waited this long is
/// served first.
pub const SIGNAL_BY_QUEUE: bool = true;
pub const SIGNAL_STARVED_S: f32 = 60.0;
/// A car standing in a turn pocket counts this many times (a full pocket
/// spills into the inner lane and blocks it).
pub const SIGNAL_POCKET_WEIGHT: u32 = 3;
/// With this many cars in a pocket its left phase runs before its own
/// through phase (a full pocket spills into the inner lane).
pub const SIGNAL_LEFT_FIRST_CARS: u16 = 4;
pub const SIGNAL_SWITCH_QUEUE_CARS: u16 = 4;
/// Vehicles closer than this to the stop line call their phase, metres.
pub const SIGNAL_DETECT_M: f32 = 40.0;
/// Common cycle of coordinated signals along avenues, seconds ...
pub const SIGNAL_COORD_CYCLE_S: f32 = 60.0;
/// ... of which the avenue keeps at least this share once green ...
pub const SIGNAL_COORD_MAIN_SHARE: f32 = 0.5;
/// ... and offsets follow a platoon driving at this speed, m/s.
pub const SIGNAL_PROGRESSION_SPEED: f32 = 14.0;
/// Auto control turns an all-way junction (equal roads, no major one)
/// into signals when its busiest minute of vehicle time yesterday let in
/// plus left queued at least this many vehicles (about what an all-way
/// stop passes: signals every short block cost more than they bring
/// below it), and back below the lower number.
pub const AUTO_SIGNAL_PER_MINUTE: u16 = 30;
pub const AUTO_PRIORITY_PER_MINUTE: u16 = 18;
/// Junctions with a major road (an avenue through streets, a street
/// through a T): signals once the minor approaches' busiest minute (cars
/// from them let in plus those still queued) reaches this, back to
/// priority below the second (`junction::judge_busy`).
pub const AUTO_SIGNAL_MINOR_PER_MINUTE: u16 = 30;
pub const AUTO_PRIORITY_MINOR_PER_MINUTE: u16 = 18;

/// Route planning: fixed cost of passing a junction, seconds (prefers
/// straighter routes with fewer junctions).
pub const ROUTE_JUNCTION_COST_S: f32 = 1.5;
/// Route planning: expected wait at a junction by how the approach is
/// controlled, seconds (replaces `ROUTE_JUNCTION_COST_S` once junctions
/// are known): through on the major road, giving way, an all-way stop, a
/// signal's main road and its side roads. Makes trips prefer arterials.
pub const ROUTE_MAJOR_S: f32 = 0.5;
pub const ROUTE_GIVE_WAY_S: f32 = 4.0;
pub const ROUTE_ALL_WAY_S: f32 = 5.0;
pub const ROUTE_SIGNAL_MAIN_S: f32 = 3.0;
pub const ROUTE_SIGNAL_SIDE_S: f32 = 8.0;
/// Route planning: turning back the way the car came at a node, seconds
/// (only when nothing else works).
pub const ROUTE_UTURN_COST_S: f32 = 30.0;
/// Routing cost of turns on top of the approach's wait
/// (`drive::build_route_turns`): a left turn at a signal per oncoming lane
/// it must find gaps in (half that from a major road at a priority
/// junction; crossing or turning left out of a minor road per major lane),
/// a right turn.
pub const ROUTE_LEFT_PER_LANE_S: f32 = 8.0;
pub const ROUTE_RIGHT_S: f32 = 1.0;
/// Route planning: expected wait per car already on a segment direction
/// that ends at a junction, multiplied by that direction's congestion.
pub const ROUTE_QUEUE_COST_S: f32 = 2.0;

/// A car that reaches its destination on a one-lane street stands in the
/// lane this long before it is gone (parking, turning into a driveway):
/// everyone behind waits. On wider roads it pulls in at once. Seconds.
pub const PARK_DWELL_S: f32 = 0.0;
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
/// Vehicle budget: at most this many cars on the road and waiting to start
/// at once (a 20,000-resident city at rush hour). A trip that would go over
/// it happens off screen, taking the estimated drive time.
pub const MAX_VEHICLES: usize = 2_500;
/// ... and at most this many per 1,000 residents (the same share in a
/// smaller city; never fewer than [`MIN_VEHICLE_BUDGET`]).
pub const VEHICLES_PER_1000: usize = 125;
pub const MIN_VEHICLE_BUDGET: usize = 150;
/// Share of citizens' car trips that are real cars on the road (the rest
/// happen off screen and take the delay the drawn cars measure,
/// `traffic::delay_factor`): all of them in a small town, falling to
/// `CAR_SHARE_LARGE` between `CAR_SHARE_FROM_POP` and `CAR_SHARE_TO_POP`
/// residents. A fixed share, whatever the traffic: the network carries the
/// cars of this share and the layout decides how well (see
/// `traffic::car_share`).
pub const CAR_SHARE_LARGE: f32 = 0.4;
pub const CAR_SHARE_FROM_POP: f32 = 1_500.0;
pub const CAR_SHARE_TO_POP: f32 = 5_000.0;
/// Lunches (midday trips to and from venues) are drawn at most at this
/// share, the big city's (`traffic::drawn_lunch`).
pub const LUNCH_CAR_SHARE: f32 = CAR_SHARE_LARGE;
/// Goods deliveries and exports driven as real trucks, as a share of the
/// car share (trucks make up about a tenth of the traffic).
pub const TRUCK_SHARE: f32 = 0.5;
/// Cap of a car trip's time over its free-flow estimate in the delay
/// factor (`TrafficState::delay_factor`).
pub const DELAY_FACTOR_MAX: f32 = 8.0;
/// A trip whose car could not start within this many seconds (blocked
/// driveway, saturated network) happens off screen instead and counts as a
/// hopeless commute.
pub const MAX_DEPARTURE_WAIT_S: f32 = 40.0;
/// A car only pulls out of a driveway if no car on that lane is within this
/// distance of the spot, metres.
pub const SPAWN_CLEARANCE_M: f32 = 6.0;
/// A car does not pull out when a standing car is less than this far ahead
/// of its driveway in its lane (a queue): it waits for the queue to move.
pub const SPAWN_QUEUE_M: f32 = 25.0;
/// How far from a building (centre) its road access point may be, cells.
pub const ACCESS_SEARCH_CELLS: i32 = 4;

// -------------------------------------------------------------- errands ----

/// Errand traffic is generated every this many ticks.
pub const ERRAND_INTERVAL_TICKS: u64 = 10;
/// Round 3: no longer used by the sim (shopping trips come from households' goods, [`SHOP_BELOW_DAYS`]); kept for compatibility.
/// One-way shopping runs per household per day (half home -> shop, half
/// back), mostly 10:00-20:00.
pub const SHOP_TRIPS_PER_HOUSEHOLD_PER_DAY: f32 = 0.15;
/// Round 3: no longer used by the sim (deliveries follow shop sales, [`TRUCK_LOAD`]; exports: [`EXPORT_TRIPS_PER_INDUSTRIAL_JOB_PER_DAY`]); kept for compatibility.
/// Goods trips per filled industrial job per day, mostly 05:00-19:00.
pub const GOODS_TRIPS_PER_INDUSTRIAL_JOB_PER_DAY: f32 = 0.15;
/// Round 3: no longer used by the sim (see [`GOODS_IMPORT_SHARE`]); kept for compatibility.
/// Share of goods trips that leave for / come from the highway.
pub const GOODS_VIA_HIGHWAY_SHARE: f32 = 0.2;
/// Round 3: no longer used by the sim (see [`SHOP_SAMPLES`], [`DELIVERY_SAMPLES`]); kept for compatibility.
/// Errand destinations: nearest of this many random candidates.
pub const ERRAND_SAMPLES: usize = 3;

// ----------------------------------------------------------- households ----

/// Round 3: no longer used by the sim (households arrive by type, [`ARRIVAL_WEIGHTS`]); kept for compatibility.
/// Household sizes 1..=4 are drawn with these weights (mean ~2.6).
pub const HOUSEHOLD_SIZE_WEIGHTS: [u32; 4] = [20, 30, 25, 25];
/// Every this many ticks unemployed households look for jobs ...
pub const JOB_MATCH_INTERVAL_TICKS: u64 = 10;
/// ... at most this many per round ...
pub const JOB_MATCH_PER_ROUND: usize = 60;
/// ... each comparing this many random vacancies and taking the nearest.
pub const JOB_MATCH_SAMPLES: usize = 10;
/// Move-ins per in-game hour per household of free housing (fills new
/// houses within a few real minutes; round 2's 0.6 per 10-second hour) ...
pub const MOVE_IN_FILL_PER_HOUR: f32 = 2.4;
/// ... at most this many per hour ...
pub const MOVE_IN_MAX_PER_HOUR: f32 = 480.0;
/// Newcomers arrive between these hours; at night at this share of the
/// mean rate (the day's total is unchanged).
pub const MOVE_IN_DAY_H: [f32; 2] = [6.0, 21.0];
pub const MOVE_IN_NIGHT_SHARE: f32 = 0.2;
/// ... and only while the city's attractiveness (residential demand
/// ignoring empty housing) is above this.
pub const MOVE_IN_MIN_DEMAND: f32 = -0.25;
/// Round 3: no longer used by the sim (jobless households leave when their money runs out, [`BROKE_LEAVE_DAYS`]); kept for compatibility.
/// A household unemployed at this many consecutive midnights leaves town.
pub const UNEMPLOYED_LEAVE_DAYS: u8 = 2;
/// A car commute longer than this (vehicle seconds, door to door: waiting
/// to pull out included) is hopeless (round 3: 150 s, when only a sample of
/// the commuters drove; with every car trip on the road the peaks are
/// honest and longer) ...
pub const HOPELESS_COMMUTE_S: f32 = 300.0;
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
/// [`RES_VACANCY_OK`] (round 3: 0.14 instead of 0.10, as newcomers leave
/// a place free in bigger homes for births, [`RESERVE_FROM_CAPACITY`]) (vacancy = empty places / (capacity + 2 *
/// `DEMAND_SMALL_CITY`)).
pub const RES_VACANCY_WEIGHT: f32 = 2.5;
pub const RES_VACANCY_OK: f32 = 0.14;
/// Commercial jobs (shops) wanted per resident. No longer used by the sim
/// (round 3 wants jobs per member of the workforce,
/// [`SHOP_JOBS_PER_WORKER`]).
pub const SHOP_JOBS_PER_RESIDENT: f32 = 0.2;
/// Commercial jobs (shops) wanted per member of the workforce (industry
/// wants [`INDUSTRY_PER_SHOP`] times as many): ~1.1 jobs per worker.
pub const SHOP_JOBS_PER_WORKER: f32 = 0.44;
/// Industrial jobs wanted per commercial job.
pub const INDUSTRY_PER_SHOP: f32 = 1.5;
/// Weight of the relative job shortfall on C / I demand.
pub const JOB_SHORTFALL_WEIGHT: f32 = 1.4;
/// Extra C / I demand per unit of unemployment rate (a quarter to each kind
/// of work, the other half by their shortfall of jobs, `demand::target`: 9 %
/// out of work with only industry short adds about 0.6 to industrial
/// demand).
pub const UNEMPLOYMENT_JOB_BOOST: f32 = 4.5;
/// `demand::jobs_view`: below this share of the workforce out of work
/// nothing is wrong; fewer free zoned cells of the short kind of work than
/// this counts as no land zoned.
pub const JOBLESS_OK: f32 = 0.03;
pub const JOBLESS_LAND_CELLS: u32 = 24;
/// C / I demand falls by this per unit of unfilled-job rate above
/// [`JOB_VACANCY_OK`].
pub const JOB_VACANCY_WEIGHT: f32 = 2.0;
pub const JOB_VACANCY_OK: f32 = 0.12;
/// Softens ratios in a tiny city (added to denominators).
pub const DEMAND_SMALL_CITY: f32 = 40.0;
/// Demand change per unit of tax rate above [`DEFAULT_TAX_RATE`]. At 30 %
/// tax all demand drops by ~0.8.
pub const TAX_DEMAND_SLOPE: f32 = 4.0;
/// Below the default rate demand rises only gently, by this per unit of
/// tax rate, and by at most [`LOW_TAX_DEMAND_MAX`]: zero tax is no growth
/// button (it is a fast way into debt).
pub const LOW_TAX_DEMAND_SLOPE: f32 = 1.0;
pub const LOW_TAX_DEMAND_MAX: f32 = 0.06;
/// Average car commutes longer than this (vehicle seconds) cut residential
/// demand (round 3: 70 s with a sample of the commuters on the road; every
/// car trip is on the road now, so rush hours are fuller) ...
pub const COMMUTE_OK_S: f32 = 130.0;
/// The city's traffic (`traffic::delay_factor`, commutes over free-flow
/// time): up to `DELAY_OK_FACTOR` nobody minds; beyond it residential
/// demand drops by `DELAY_DEMAND_PER_UNIT` per unit, and commuters' commute
/// happiness falls to zero at `DELAY_HOPELESS_FACTOR`.
pub const DELAY_OK_FACTOR: f32 = 2.5;
pub const DELAY_HOPELESS_FACTOR: f32 = 5.0;
pub const DELAY_DEMAND_PER_UNIT: f32 = 0.5;
/// ... by 1.0 per this many extra seconds.
pub const COMMUTE_DEMAND_SCALE_S: f32 = 180.0;

// --------------------------------------------------------------- growth ----

/// Growth runs every this many ticks.
pub const GROWTH_INTERVAL_TICKS: u64 = 10;
/// New buildings (or upgrades) per in-game hour at demand 1.0, per zone
/// `[residential, commercial, industrial]`. Scales linearly with demand.
/// (Round 2's `[5.0, 2.7, 2.8]` per 10-second hour, per real minute.)
pub const GROWTH_PER_HOUR: [f32; 3] = [20.0, 10.8, 11.2];
/// Fraction of growth events that upgrade an existing full building
/// instead of building a new one (when both are possible).
pub const UPGRADE_SHARE: f32 = 0.2;
/// A building can upgrade once it is at least this full ...
pub const UPGRADE_MIN_FULL: f32 = 0.9;
/// ... and at least this old (ticks; 2 real minutes at 1x).
pub const UPGRADE_MIN_AGE_TICKS: u64 = TICKS_PER_DAY as u64 / 8;
/// Highest level per zone `[R, C, I]`.
pub const MAX_LEVEL: [u8; 3] = [3, 3, 2];
/// Residents (R) or jobs (C, I) per footprint cell at levels 1, 2, 3.
/// (Round 3: every adult has a job of their own, ~0.6 workers per resident
/// instead of round 2's ~0.45, so job buildings hold about a quarter more
/// jobs and the city needs the same land for its jobs as before.)
pub const CAPACITY_PER_CELL: [[u32; 3]; 3] = [[4, 9, 18], [4, 8, 13], [5, 10, 10]];
/// Chance (percent) that a new C / I building tries a 2x2 footprint.
pub const BIG_FOOTPRINT_PERCENT: [u32; 3] = [0, 35, 60];
/// A building empty at this many consecutive midnights is abandoned.
pub const ABANDON_EMPTY_DAYS: u8 = 2;
/// No longer used by the sim (kept so existing code still compiles): it was
/// "abandoned at this many consecutive midnights without road access",
/// which killed a building whose road was cut at 23:59 for a rebuild.
/// Abandonment for lost road access now uses
/// [`ABANDON_UNREACHABLE_GRACE_TICKS`]. `BuildingMeta::unreachable_days`
/// still counts those midnights for display.
pub const ABANDON_UNREACHABLE_DAYS: u8 = 1;
/// A building that has been continuously without road access to the
/// highway for this long (sim ticks, one in-game day: 16 real minutes at
/// 1x, 5⅓ at 3x) is abandoned. Reconnecting it at any point before that
/// resets the clock, so a player can cut a road and rebuild it.
pub const ABANDON_UNREACHABLE_GRACE_TICKS: u64 = TICKS_PER_DAY as u64;
/// Road access of every building is checked this often (ticks; one in-game
/// hour), so the grace period above ends within an hour of running out.
pub const REACHABILITY_CHECK_INTERVAL_TICKS: u64 = TICKS_PER_HOUR as u64;

// ------------------------------------------------------------------ map ----

/// Length of the highway stub in cells, entering from the west map edge.
pub const HIGHWAY_STUB_CELLS: i32 = 12;

// =============================================================== people ====
//
// Round 3: every resident is a citizen (see `citizens`, `day`, `life`).
//
// # The calendar
//
// A day is 16 real minutes at 1x. Ages run on a compressed calendar: one
// in-game day is a year ([`DAYS_PER_YEAR`]), so a 30-minute session at 2.5x
// (~5 days) spans about five years of life: babies are born, teens grow up,
// couples form, adults retire and the old die, but a typical resident
// still lives in the city for many sessions (a lifetime of 80 years is 80
// days, 21 hours of play at 1x). Chances "per day" below are per year.

/// In-game days per year of age.
pub const DAYS_PER_YEAR: u32 = 1;
/// Days in a week (shifts have two days off a week, staggered).
pub const DAYS_PER_WEEK: u32 = 7;
/// Ages (years) at which life stages start.
pub const TEEN_AGE_YEARS: u32 = 13;
pub const ADULT_AGE_YEARS: u32 = 18;
/// Retirement age.
pub const SENIOR_AGE_YEARS: u32 = 65;
/// Teens from this age may take a part-time job in a shop.
pub const TEEN_JOB_AGE_YEARS: u32 = 16;
/// Journal lines kept per citizen.
pub const JOURNAL_CAP: usize = 12;

// --------------------------------------------------------- households ----

/// Household types arriving `[single, couple, family, seniors]` (relative
/// weights before the city's own pull; see `households::arrivals`). A new
/// town draws people of working age and young families; seniors are about
/// a tenth of newcomers, and their share grows as the town ages in place.
pub const ARRIVAL_WEIGHTS: [f32; 4] = [30.0, 24.0, 38.0, 8.0];
/// Free jobs per member of the workforce scale the working types by
/// `0.6 + this * free share` (0.6..1.4).
pub const ARRIVAL_JOBS_PULL: f32 = 4.0;
/// Age ranges (years) of arriving heads of household, skewed to the young
/// end: singles, couples, parents (their children are 0-17, mostly young),
/// seniors.
pub const ARRIVAL_SINGLE_YEARS: [u32; 2] = [19, 45];
pub const ARRIVAL_COUPLE_YEARS: [u32; 2] = [21, 50];
pub const ARRIVAL_PARENT_YEARS: [u32; 2] = [23, 45];
pub const ARRIVAL_SENIOR_YEARS: [u32; 2] = [63, 82];
/// Average size of an arriving household (sets the arrival rate per free
/// home place, like round 2's 2.6 people per household).
pub const ARRIVAL_AVG_SIZE: f32 = 2.6;
/// Families arriving with one parent.
pub const SINGLE_PARENT_SHARE: f32 = 0.15;
/// Children of arriving families: 1, 2, 3 with these weights.
pub const FAMILY_CHILDREN_WEIGHTS: [f32; 3] = [30.0, 40.0, 30.0];
/// Senior households that are couples.
pub const SENIOR_COUPLE_SHARE: f32 = 0.5;
/// Households with two drivers owning two cars (the rest share one).
pub const TWO_CAR_SHARE: f32 = 0.3;
/// Families with young children (under 13) where one parent stays at home
/// (out of the workforce, does the shopping) until the youngest is 13 ...
pub const HOMEMAKER_SHARE: f32 = 0.35;
/// ... and couples without young children where one partner does.
pub const STAY_AT_HOME_SHARE: f32 = 0.15;
/// Grown-up children living with their parents study (not in the
/// workforce) until this age.
pub const STUDENT_UNTIL_YEARS: u32 = 22;
/// Old saves: share of 1-2 member households without a job that become
/// seniors.
pub const LEGACY_SENIOR_SHARE: f32 = 0.25;
/// Old saves: savings of generated households, days of costs.
pub const LEGACY_SAVINGS_DAYS: f32 = 6.0;
/// The arrival rate scales with `1 + (avg happiness - neutral) * this`.
pub const ARRIVAL_HAPPINESS_WEIGHT: f32 = 1.5;

// -------------------------------------------------------------- money ----
//
// Per day, whole dollars. A household's money is real: wages (and
// pensions) come in at midnight, the city's residential tax is taken from
// the wages, rent goes out at midnight, goods are paid for in the shop.
// The wages are set so that the residential tax per resident is about what
// round 2's flat per-resident tax was (~$14 of taxable value per resident
// per 4 real minutes). All amounts are [`DAY_SCALE`] times round 3's.

/// Daily wage by job tier `[basic, skilled, professional]`.
pub const WAGE_BY_TIER: [u32; 3] = [92, 112, 132];
/// Daily wage of a teen's part-time job.
pub const PART_TIME_WAGE: u32 = 36;
/// Daily pension of a retiree (not taxed).
pub const PENSION_PER_DAY: i64 = 60;
/// Daily rent per resident place by the home's level `[1, 2, 3]`.
pub const RENT_PER_PLACE: [f32; 3] = [12.0, 14.4, 16.8];
/// Rents rise with residential demand: `* (1 + this * demand)`.
pub const RENT_MARKET_WEIGHT: f32 = 0.2;
/// Goods used per day by a person / a child, units.
pub const GOODS_PER_PERSON_PER_DAY: f32 = 1.0;
pub const GOODS_PER_CHILD_PER_DAY: f32 = 0.6;
/// Price of one unit of goods, dollars.
pub const GOODS_PRICE: f32 = 8.0;
/// A night out costs each adult this much.
pub const LEISURE_COST: i64 = 12;
/// Savings of an arriving household: this many days of rent and goods,
/// plus up to one more (random). Without income they last about two days.
pub const ARRIVAL_SAVINGS_DAYS: f32 = 1.0;
/// Goods an arriving household brings along, days.
pub const ARRIVAL_GOODS_DAYS: f32 = 2.0;
/// Money below this many days of costs: spending is cut (essentials only,
/// no nights out).
pub const AUSTERITY_BELOW_DAYS: f32 = 2.0;
/// A broke household moves to a home whose rent is below this share of
/// its current one; finding none, it leaves.
pub const CHEAPER_HOME_FACTOR: f32 = 0.85;
/// A household broke at this many midnights in a row (money below zero,
/// or below a day's costs with no income) leaves; on the first it looks for
/// a cheaper home.
pub const BROKE_LEAVE_DAYS: u8 = 2;
/// Newcomers leave one place free in homes of at least this many places,
/// for births and moves within the city.
pub const RESERVE_FROM_CAPACITY: u32 = 8;
/// Homes compared when looking for a home.
pub const HOME_SEARCH_SAMPLES: usize = 12;
/// A grown-up child moving out takes this share of the household's money.
pub const MOVE_OUT_MONEY_SHARE: f32 = 0.25;

// --------------------------------------------------------------- jobs ----

/// Job search: a vacancy's score is its wage times `1 - min(distance /
/// this, 0.9)` (nearby and better paid wins).
pub const JOB_DIST_SCALE_M: f32 = 1000.0;
/// Shift starts (hours) per workplace: industry, offices (commercial level
/// 3) and shops (early shift; the late shift starts
/// [`LATE_SHIFT_OFFSET_H`] later).
pub const INDUSTRY_START_H: [f32; 2] = [6.5, 8.5];
pub const OFFICE_START_H: [f32; 2] = [8.5, 9.5];
pub const SHOP_START_H: [f32; 2] = [8.0, 9.5];
pub const LATE_SHIFT_OFFSET_H: f32 = 4.0;
/// The late shift's personal offset, +- hours (triangular; narrower than
/// [`SHIFT_PERSONAL_SPREAD_H`]: it starts after the lunch hour and ends
/// after the evening rush, soon after the shops close; starting an hour
/// earlier put its commute into the midday of a small town still busy with
/// the morning's).
pub const LATE_SHIFT_SPREAD_H: f32 = 0.75;
/// Shop workers on the late shift (shops stay staffed into the evening).
pub const LATE_SHIFT_SHARE: f32 = 0.35;
/// Industrial workers on the night shift (from ~21:00).
pub const NIGHT_SHIFT_SHARE: f32 = 0.08;
/// Shift lengths, hours.
pub const SHIFT_H: f32 = 8.5;
pub const PART_TIME_SHIFT_H: f32 = 4.0;
/// Personal offset from the workplace's start, +- hours (triangular).
pub const SHIFT_PERSONAL_SPREAD_H: f32 = 1.8;
/// Day-to-day wobble of a shift start, +- hours.
pub const SHIFT_DAILY_WOBBLE_H: f32 = 0.4;
/// Workers leave home the expected door-to-door time of the trip before
/// their shift starts (their last commute's, `Citizen::commute_ticks`, times
/// [`COMMUTE_TIME_MARGIN`]; the estimate before the first), plus this much
/// slack and up to [`COMMUTE_LEAD_SPREAD_H`] more (personal).
pub const COMMUTE_LEAD_H: f32 = 0.15;
pub const COMMUTE_LEAD_SPREAD_H: f32 = 0.5;
pub const COMMUTE_TIME_MARGIN: f32 = 1.1;
/// A shift is skipped when less than this would be left of it.
pub const MIN_WORK_H: f32 = 1.0;

// ---------------------------------------------------------- the day ----

/// Citizens think in this many rotating buckets: each tick one bucket
/// checks whether its citizens' activities are over (a decision comes at
/// most this many ticks late).
pub const THINK_BUCKETS: u64 = 10;
/// Waking and bedtime (hours), +- [`SLEEP_SPREAD_H`] personal.
pub const WAKE_H: f32 = 6.5;
pub const BED_H: f32 = 22.5;
pub const SLEEP_SPREAD_H: f32 = 1.0;
/// Someone staying in at home looks again after this long.
pub const HOME_RECHECK_H: f32 = 1.0;
/// Shops are visited between these hours ...
pub const SHOP_OPEN_H: f32 = 8.0;
pub const SHOP_CLOSE_H: f32 = 19.5;
/// Nobody sets off shopping who would arrive later than this before
/// closing, hours; a shopper whose car arrives after closing finds the shop
/// shut and goes home; a visit ends at closing time.
pub const SHOP_LAST_ENTRY_BEFORE_CLOSE_H: f32 = 0.25;
/// ... when the household has less than this many days of goods; the
/// shopper buys up to [`BUY_UP_TO_DAYS`] (essentials only in austerity).
pub const SHOP_BELOW_DAYS: f32 = 1.0;
/// (Each adult has a usual shopping time between [`SHOP_OPEN_H`] and an
/// hour before [`SHOP_CLOSE_H`].)
pub const BUY_UP_TO_DAYS: f32 = 3.0;
pub const AUSTERITY_BUY_DAYS: f32 = 1.2;
/// Shopping needs this much free time before the next shift, hours, and
/// takes this long in the shop.
pub const SHOP_FREE_H: f32 = 2.0;
pub const SHOP_DURATION_H: f32 = 0.75;
/// Shops compared when choosing one; a shop without all that is wanted
/// counts as this much farther, metres.
pub const SHOP_SAMPLES: usize = 6;
pub const SHOP_SHORT_STOCK_M: f32 = 600.0;
/// Leisure need grows by this per hour awake (1.5x at work) and falls by
/// [`LEISURE_HOME_RESTORE_PER_HOUR`] less at home; a night out restores
/// [`LEISURE_RESTORE_PER_HOUR`] by venue level `[1, 2, 3]`.
pub const LEISURE_NEED_PER_HOUR: f32 = 0.009;
pub const LEISURE_NEED_WORK_FACTOR: f32 = 1.5;
pub const LEISURE_HOME_RESTORE_PER_HOUR: f32 = 0.0055;
pub const LEISURE_RESTORE_PER_HOUR: [f32; 3] = [0.2, 0.3, 0.4];
/// Citizens go out when their need is above this, between these hours,
/// with at least [`LEISURE_FREE_H`] before their next shift.
pub const LEISURE_GO_NEED: f32 = 0.55;
/// Each citizen has a usual time to go out between [`LEISURE_OPEN_H`] and
/// two hours before [`LEISURE_CLOSE_H`]; above this need they go earlier.
pub const LEISURE_URGENT_NEED: f32 = 0.85;
pub const LEISURE_OPEN_H: f32 = 10.0;
pub const LEISURE_CLOSE_H: f32 = 19.0;
/// Venues close at this hour: nights out end then at the latest, and
/// nobody sets off who would not have at least [`LEISURE_DURATION_H`] there
/// before it.
pub const LEISURE_LAST_H: f32 = 21.0;
pub const LEISURE_FREE_H: f32 = 4.0;
/// Nights out are planned to be home by this hour in today's traffic
/// (`traffic::delay_factor` on the drive both ways): nobody sets off who
/// would not make it with a full visit, and a visit ends in time for it
/// (it lasts at least a quarter of an hour).
pub const LEISURE_HOME_BY_H: f32 = 22.0;
/// A night out lasts this long plus up to the spread (personal), hours.
pub const LEISURE_DURATION_H: f32 = 1.25;
pub const LEISURE_DURATION_SPREAD_H: f32 = 1.0;
/// Venues compared when choosing one; a venue's score is its restoration
/// rate over `1 + distance / this`.
pub const LEISURE_SAMPLES: usize = 6;
pub const LEISURE_DIST_SCALE_M: f32 = 400.0;
/// The same for a midday visit (lunch nearby).
pub const MIDDAY_DIST_SCALE_M: f32 = 150.0;
/// The middle of the day: someone at home with the day free (no shift
/// before [`MIDDAY_FREE_H`] hours from now: a day off, or no job) goes out
/// for lunch on a share of days ([`MIDDAY_OUT_SHARE`]), setting off at a
/// personal time in `MIDDAY_OUT_H` (an hour's window: one look at home falls
/// in it, `HOME_RECHECK_H`), to a venue chosen like a night out; the visit
/// lasts [`MIDDAY_VISIT_H`] plus up to [`MIDDAY_VISIT_SPREAD_H`]. Children
/// at home come along.
pub const MIDDAY_OUT_H: [f32; 2] = [10.0, 13.0];
pub const MIDDAY_FREE_H: f32 = 4.0;
pub const MIDDAY_OUT_SHARE: f32 = 0.25;
pub const MIDDAY_VISIT_H: f32 = 0.75;
pub const MIDDAY_VISIT_SPREAD_H: f32 = 0.5;
/// Lunch breaks: on a share of days ([`LUNCH_BREAK_SHARE`], per worker and
/// day) a worker whose shift runs over the middle of the day leaves work at
/// a personal time in `LUNCH_BREAK_H` for the nearest of a few venues
/// (`LUNCH_SAMPLES`; not their own workplace) at most [`LUNCH_MAX_TRIP_H`]
/// away, stays [`LUNCH_BREAK_VISIT_H`] and goes back to work (only with
/// the car they came in, or on foot; the shift's end is unchanged). Not
/// when less than [`LUNCH_MIN_LEFT_H`] of the shift would be left.
pub const LUNCH_BREAK_SHARE: f32 = 0.3;
/// Lunch out gives way to slow roads: with commutes taking up to the first
/// value times their free-flow time (`traffic::delay_factor`) everyone who
/// would go out for lunch (or take a lunch break out) does; from the second
/// nobody does; linear between. A town whose roads are already slow is not
/// loaded further at midday (in a small town every trip is a real car).
pub const LUNCH_DELAY: [f32; 2] = [2.0, 3.0];
pub const LUNCH_BREAK_H: [f32; 2] = [11.0, 12.5];
pub const LUNCH_BREAK_VISIT_H: f32 = 0.5;
pub const LUNCH_MAX_TRIP_H: f32 = 0.75;
pub const LUNCH_MIN_LEFT_H: f32 = 2.0;
pub const LUNCH_SAMPLES: usize = 6;
/// A visit to a venue that starts before this hour is a midday visit (its
/// length above; later ones are nights out).
pub const MIDDAY_VISIT_BEFORE_H: f32 = 15.5;
/// No longer used by the sim (every car trip is drawn while the vehicle
/// budget allows, [`MAX_VEHICLES`]); kept for compatibility.
pub const SHOP_DRIVE_SHARE: f32 = 1.0;
pub const LEISURE_DRIVE_SHARE: f32 = 1.0;
/// Off-screen trips: straight-line distance times this, at this speed
/// (m/s), plus a fixed time (s).
pub const OFFSCREEN_ROUTE_FACTOR: f32 = 1.3;
pub const OFFSCREEN_SPEED: f32 = 10.0;
pub const OFFSCREEN_BASE_S: f32 = 6.0;
/// Off-screen trips take their estimated drive time (vehicle seconds) times
/// this in ticks of vehicle time: exactly as long as the drive on the road
/// would (round 3 compressed them five-fold into calendar time, so a trip
/// on screen and one off screen took wildly different times).
pub const OFFSCREEN_CALENDAR_FACTOR: f32 = 1.0;
/// No longer used by the sim: a trip by car ends when the car arrives
/// (round 3 let people out on the calendar). Kept for compatibility.
pub const TRIP_DUE_FACTOR: f32 = 3.0;
pub const MIN_TRIP_H: f32 = 0.5;
pub const MAX_TRIP_H: f32 = 2.0;
/// Who walks: a route along the pavements of up to `WALK_NEAR_M` is walked
/// by `WALK_SHARE_NEAR` of the people making it, the share falling by `1/e`
/// every `WALK_DECAY_M` beyond, nobody beyond `WALK_MAX_M`
/// (`walk::walk_share`); commutes by `WALK_COMMUTE_FACTOR` of that (people
/// with a car mostly drive to work). Older people walk `WALK_SENIOR_RANGE`
/// of these distances.
pub const WALK_MAX_M: f32 = 650.0;
pub const WALK_NEAR_M: f32 = 120.0;
pub const WALK_SHARE_NEAR: f32 = 0.95;
pub const WALK_DECAY_M: f32 = 200.0;
pub const WALK_COMMUTE_FACTOR: f32 = 1.0;
/// Nobody walks a way that takes longer than this many in-game hours at
/// their pace (people walk in real time like the cars drive: 4 h is about
/// 160 s, some 280 m), to work `WALK_COMMUTE_MAX_H`.
pub const WALK_MAX_H: f32 = 4.0;
pub const WALK_COMMUTE_MAX_H: f32 = 3.0;
pub const WALK_SENIOR_RANGE: f32 = 0.7;
/// A worker's lunch on foot: venues within this straight-line distance,
/// at most `LUNCH_WALK_MAX_TRIP_H` in-game hours away on foot.
pub const WALK_LUNCH_MAX_M: f32 = 260.0;
pub const LUNCH_WALK_MAX_TRIP_H: f32 = 1.5;
/// A walk to a venue for leisure takes at most this many in-game hours.
pub const WALK_TRIP_MAX_H: f32 = WALK_MAX_H;
/// Walking pace of an adult, m/s (vehicles move in real time, so do
/// people; a few blocks take a couple of in-game hours, like a drive across
/// town), varied by `WALK_SPEED_SPREAD` per person, and by stage of life
/// (`WALK_STAGE_FACTOR`: children, teens, adults, seniors). A party walks
/// at its slowest member's pace.
pub const WALK_SPEED: f32 = 1.75;
pub const WALK_SPEED_SPREAD: f32 = 0.1;
pub const WALK_STAGE_FACTOR: [f32; 4] = [0.82, 1.04, 1.0, 0.78];
/// Pavement widths beside the carriageway (streets, avenues), metres;
/// people walk along their middle. Highways have none.
pub const STREET_PAVEMENT_M: f32 = 1.25;
pub const AVENUE_PAVEMENT_M: f32 = 1.5;
/// Zebra crossings at junctions: from `ZEBRA_INSET_M` to `ZEBRA_INSET_M +
/// ZEBRA_WIDTH_M` inside the stop line (in the junction margin), only on
/// arms whose stop line stands at least `ZEBRA_WIDTH_M + 1` from the node.
pub const ZEBRA_INSET_M: f32 = 0.4;
pub const ZEBRA_WIDTH_M: f32 = 1.75;
/// Mid-block crossings on stretches of road between stop lines at least
/// this long, one per `MIDBLOCK_SPACING_M` (evenly spaced).
pub const MIDBLOCK_MIN_M: f32 = 150.0;
pub const MIDBLOCK_SPACING_M: f32 = 130.0;
/// Route choice: a crossing costs this many metres of pavement (people
/// do not cross back and forth for nothing).
pub const WALK_CROSSING_COST_M: f32 = 8.0;
/// The door: the middle of a lot's front edge, this far inside the lot.
pub const DOOR_INSET_M: f32 = 1.5;
/// People walked in full on the map per 1,000 residents (and at least
/// `MIN_WALKERS`, at most `MAX_WALKERS`); beyond it walks are timed off
/// screen as before.
pub const WALKERS_PER_1000: usize = 300;
pub const MIN_WALKERS: usize = 200;
pub const MAX_WALKERS: usize = 8_000;
/// Crossings without signals: people step onto the zebra in a group (later
/// arrivals join within `PED_GROUP_S` of the first); once a car has stood
/// waiting for the crossing `CAR_PATIENCE_S`, people let the cars go for
/// `CAR_TURN_S` before they step on again.
pub const PED_GROUP_S: f32 = 2.0;
/// Without signals people let a moving car that would reach the zebra
/// before they are across pass first, for up to `PED_PATIENCE_S` (then
/// they step out and it stops, if it can).
pub const PED_PATIENCE_S: f32 = 40.0;
/// Crossing a main road without signals (the major road of a give-way
/// junction, a mid-block zebra): people wait for a gap long enough to
/// cross before the next car arrives, for up to this many seconds.
pub const PED_MAIN_PATIENCE_S: f32 = 40.0;
/// At signals people step onto the zebra only in the first this many
/// seconds of the green that lets them cross (the walk light; then it
/// flashes and the turning cars have the rest of the green).
pub const WALK_INTERVAL_S: f32 = 6.0;
pub const CAR_PATIENCE_S: f32 = 3.0;
pub const CAR_TURN_S: f32 = 6.0;
/// A car that cannot stop short of a zebra: its braking distance plus this
/// margin (people never step in front of it).
pub const PED_SAFETY_M: f32 = 1.5;
/// A trip made off screen takes at most this many in-game hours in a city
/// whose traffic is fine, longer in proportion as it gets worse
/// (`* max(1, delay_factor / DELAY_OK_FACTOR)`): nobody's trip takes most
/// of a day unless the roads are gridlocked.
pub const OFFSCREEN_MAX_TRIP_H: f32 = 2.5;
/// Without a free car a trip takes this many times as long (stands in for
/// lifts, buses and walking until stage 3).
pub const CARLESS_TIME_FACTOR: f32 = 1.8;
/// Car-sharing: a member taking the household's last car gives a lift to a
/// member leaving for work within this many hours to a workplace within
/// this distance of theirs.
pub const CARSHARE_WINDOW_H: f32 = 1.5;
pub const CARSHARE_RADIUS_M: f32 = 800.0;

// -------------------------------------------------------------- goods ----

/// Shelf space per shop job, units.
pub const SHOP_STOCK_PER_JOB: f32 = 30.0;
/// A shop orders a delivery when its stock falls below this share ...
pub const RESTOCK_BELOW: f32 = 0.5;
/// ... and a truck brings this many units.
pub const TRUCK_LOAD: f32 = 50.0;
/// Deliveries come from outside the city (through the highway) this often,
/// otherwise from the nearest of a few factories with workers.
pub const GOODS_IMPORT_SHARE: f32 = 0.1;
/// A delivery comes from the nearest of this many random factories.
pub const DELIVERY_SAMPLES: usize = 8;
/// Delivery trucks sent per errand round at most.
pub const DELIVERIES_PER_ROUND: usize = 8;
/// Industry ships goods out of the city: trucks per filled industrial job
/// per day (mostly 05:00-19:00; round 3's 0.03 per 4-minute day).
pub const EXPORT_TRIPS_PER_INDUSTRIAL_JOB_PER_DAY: f32 = 0.12;

// ------------------------------------------------------- life events ----

/// Births: a household with two adults, one of them aged
/// [`BIRTH_MIN_YEARS`]..=[`BIRTH_MAX_YEARS`], fewer than [`MAX_CHILDREN`]
/// children, room at home and money has a child with this chance per day.
pub const BIRTH_CHANCE_PER_DAY: f32 = 0.3;
pub const BIRTH_MIN_YEARS: u32 = 20;
/// A couple wanting a child in a full home moves to one with room whose
/// rent is at most this factor of its current one.
pub const ROOMIER_HOME_RENT_FACTOR: f32 = 1.3;
pub const BIRTH_MAX_YEARS: u32 = 42;
pub const MAX_CHILDREN: u32 = 4;
/// Couples put off having a child while city services are cut below this
/// level (debt).
pub const BIRTH_MIN_SERVICE_LEVEL: f32 = 0.8;
/// Grown-up children from this age move out with this chance per day when
/// a home is free (otherwise they stay and are unhappy about it).
pub const MOVE_OUT_MIN_YEARS: u32 = 19;
pub const MOVE_OUT_CHANCE_PER_DAY: f32 = 0.3;
/// Singles of these ages find a partner (a single within
/// [`COUPLE_MAX_AGE_GAP_YEARS`], among [`COUPLE_SAMPLES`] random
/// households) with this chance per day and move in together.
pub const COUPLE_CHANCE_PER_DAY: f32 = 0.06;
pub const COUPLE_MIN_YEARS: u32 = 20;
pub const COUPLE_MAX_YEARS: u32 = 50;
pub const COUPLE_MAX_AGE_GAP_YEARS: u32 = 12;
pub const COUPLE_SAMPLES: usize = 12;
/// Old age: from this age the yearly chance of dying is
/// `DEATH_RATE_AT_START * DEATH_RATE_GROWTH^(age - DEATH_FROM_YEARS)`; nobody
/// lives past [`MAX_AGE_YEARS`].
pub const DEATH_FROM_YEARS: u32 = 70;
pub const DEATH_RATE_AT_START: f32 = 0.015;
pub const DEATH_RATE_GROWTH: f32 = 1.11;
pub const MAX_AGE_YEARS: u32 = 104;

// ---------------------------------------------------------- happiness ----

/// Weights of the happiness components `[employment, commute,
/// affordability, access, taxes, finances, housing]`.
/// (Round 5: the commute weighs as much as having a job: a resident stuck in
/// traffic every day is clearly unhappy about it.)
pub const HAPPINESS_WEIGHTS: [f32; 7] = [0.22, 0.30, 0.16, 0.12, 0.08, 0.08, 0.04];
/// A worker's commute happiness is the worse of its length
/// (`COMMUTE_OK_S` .. `HOPELESS_COMMUTE_S`) and of how much longer than the
/// free-flow time it took them: fine up to `COMMUTE_RATIO_OK` times,
/// nothing left at `COMMUTE_RATIO_BAD` (their own trip, on screen or off,
/// over its free-flow time plus `COMMUTE_RATIO_SLACK_S`: one red light does
/// not make a short hop "slow").
pub const COMMUTE_RATIO_OK: f32 = 1.3;
pub const COMMUTE_RATIO_BAD: f32 = 3.0;
pub const COMMUTE_RATIO_SLACK_S: f32 = 15.0;
/// A household whose workers all have commute happiness below this at a
/// midnight is fed up with the traffic: with this chance it moves to a free
/// home nearer the first worker's job (at least `COMMUTE_MOVE_NEARER` times
/// as close), and failing that leaves the city.
pub const COMMUTE_FED_UP_BELOW: f32 = 0.25;
pub const COMMUTE_FED_UP_CHANCE: f32 = 0.2;
pub const COMMUTE_MOVE_NEARER: f32 = 0.7;
/// Happiness of a newcomer (and of every component before the first
/// midnight).
pub const HAPPINESS_START: f32 = 0.88;
/// City average at which demand and arrivals are neither pushed nor pulled.
pub const HAPPINESS_NEUTRAL: f32 = 0.88;
/// Residential demand per unit of average happiness above neutral.
pub const HAPPINESS_DEMAND_WEIGHT: f32 = 0.5;
/// A citizen below this counts as unhappy (city statistics).
pub const UNHAPPY_BELOW: f32 = 0.65;
/// A household whose average stays below this for this many midnights
/// leaves the city.
pub const UNHAPPY_LEAVE_BELOW: f32 = 0.45;
pub const UNHAPPY_LEAVE_DAYS: u8 = 2;
/// Rent burden (rent / income) that is fine / unbearable.
pub const RENT_BURDEN_OK: f32 = 0.25;
pub const RENT_BURDEN_BAD: f32 = 0.8;
/// Savings (days of costs) that feel safe.
pub const RESERVE_GOOD_DAYS: f32 = 5.0;
/// Distance to the nearest shop / venue that is fine / too far, metres.
pub const ACCESS_GOOD_M: f32 = 300.0;
pub const ACCESS_BAD_M: f32 = 1800.0;
/// A small (level-1) venue counts as this much farther for leisure access.
pub const ACCESS_SMALL_VENUE_M: f32 = 250.0;

/// City statistics (`people.stats`) are recomputed this often (ticks),
/// gathered over the first [`PEOPLE_STATS_CHUNKS`] ticks of each interval.
pub const PEOPLE_STATS_INTERVAL_TICKS: u64 = 100;
pub const PEOPLE_STATS_CHUNKS: u64 = 20;
/// The nightly household review is spread over this many ticks after
/// midnight (~14 in-game minutes).
pub const NIGHT_CHUNKS: u32 = 48;

// ------------------------------------------------- free-form road geometry ----

/// Tightest centre-line radius a street / avenue may be built with, metres.
pub const STREET_MIN_RADIUS_M: f32 = 12.0;
pub const AVENUE_MIN_RADIUS_M: f32 = 24.0;
/// Shortest segment a command may leave behind, metres (between any two
/// nodes, including those made by splits and crossings).
pub const MIN_SEGMENT_M: f32 = 4.0;
/// Most arms a node may have.
pub const MAX_NODE_ARMS: usize = 6;
/// Smallest angle between two arms at a node, degrees.
pub const MIN_ARM_ANGLE_DEG: f32 = 25.0;
/// A road end this close to an existing node joins it; this close to an
/// existing road (and farther from its nodes) splits it into a T, metres.
pub const SNAP_NODE_M: f32 = 1.5;
pub const SNAP_ROAD_M: f32 = 0.75;
/// Two roads may not run closer side by side than this (centre lines,
/// metres) except where they meet at a node.
pub const MIN_ROAD_SPACING_M: f32 = 8.0;
/// Lateral acceleration drivers accept in curves, m/s² (speed limit on a
/// curve of radius `r` is `sqrt(CORNER_ACCEL * r)`).
pub const CORNER_ACCEL: f32 = 2.5;
/// Zone blocks: cells per block side, from the road (48 m: land between
/// streets up to about 100 m apart can be zoned all the way in; where the
/// rows of two roads meet, the cell nearer its own road keeps the land).
/// The data structures allow up to [`MAX_BLOCK_DEPTH`].
pub const BLOCK_DEPTH: u8 = 6;
pub const MAX_BLOCK_DEPTH: u8 = 6;
/// Rows of a lot that hold the building (its capacity); the rest of a
/// deeper lot is its yard.
pub const BUILT_DEPTH: u8 = 2;
/// Lattices of two blocks coincide (share cells) when their directions are
/// parallel or perpendicular within this many degrees and their offset is a
/// whole number of cells within this fraction of a cell, at both ends of
/// the newer block.
pub const LATTICE_ANGLE_TOL_DEG: f32 = 0.2;
pub const LATTICE_OFFSET_TOL: f32 = 0.02;
/// Car and truck body sizes the traffic rules keep apart (length, width),
/// metres; renderers should draw vehicles within them. Queues keep
/// `MIN_GAP_M` between centres, so no body may be longer than that.
pub const CAR_BODY_M: [f32; 2] = [4.4, 1.9];
pub const TRUCK_BODY_M: [f32; 2] = [5.3, 2.2];

// ---------------------------------------------------------- roundabouts ----

/// Price to build a roundabout by size (mini, small, medium, large), on top
/// of taking down what stands on its land. A signalled junction costs
/// nothing to build but `SIGNAL_UPKEEP_PER_DAY` a day.
pub const ROUNDABOUT_COST: [i64; 4] = [1_200, 3_000, 6_500, 11_000];
/// Upkeep of a roundabout per day by size (planting, kerbs, lamps), part
/// of road upkeep.
pub const ROUNDABOUT_UPKEEP_PER_DAY: [i64; 4] = [15, 25, 40, 60];
/// Pavement round a ring, metres.
pub const RING_PAVEMENT_M: f32 = 2.5;
/// Verge beyond the pavement that the ring's land includes, metres.
pub const RING_VERGE_M: f32 = 0.5;
/// Give-way line distance outside the ring's outer kerb along an arm.
pub const RING_GIVE_WAY_OUTSIDE_M: f32 = 1.0;
/// Road an arm keeps between the ring's give-way line and the next
/// junction's box, metres (a ring may not swallow its neighbour).
pub const RING_MIN_ARM_M: f32 = 10.0;
/// Roads a ring takes at most.
pub const RING_MAX_ARMS: usize = 6;
/// Least sweep round the ring between one arm's entry and the next arm's
/// exit, radians.
pub const RING_MIN_SWEEP: f32 = 0.12;
/// How far before an arm a ring lane turns off for it, relative to where
/// an entry joins (1 = alike).
pub const RING_EXIT_SPREAD: f32 = 1.0;
/// Sideways acceleration drivers accept on the ring: speed = sqrt(a r).
pub const RING_LATERAL_ACCEL: f32 = 2.4;
/// Slowest speed on a ring (the mini roundabout), m/s.
pub const RING_MIN_SPEED: f32 = 3.0;
/// Paths within this of a conflict point pass it, metres.
pub const RING_CONFLICT_M: f32 = 2.0;
/// Margin beyond a car's rear before a conflict point counts as cleared.
pub const RING_CLEAR_M: f32 = 1.0;
/// An entering car goes only when the next car on the ring reaches its
/// merge point at least this long after the entering car's rear cleared
/// it, seconds (with the time it takes to get there from where it stands,
/// a critical gap of about 3.5 s from a stand).
pub const RING_SAFETY_S: f32 = 0.8;
/// The same for a car leaving across the outer lane.
pub const RING_CROSS_SAFETY_S: f32 = 0.8;
/// Time headway on the ring, seconds.
pub const RING_HEADWAY_S: f32 = 1.0;
/// How far ahead along its path a car on the ring looks, metres.
pub const RING_LOOKAHEAD_M: f32 = 22.0;
/// Room a car on the ring keeps ahead of its body to another's (a
/// standing queue's bumper gap), metres.
pub const RING_FRONT_GAP_M: f32 = 1.0;
/// Room it keeps to its sides, metres.
pub const RING_SIDE_GAP_M: f32 = 0.3;
/// A roundabout's zebra across an arm: its middle this far out from the
/// give-way line (a car length: a car leaving stops between ring and zebra).
pub const RING_ZEBRA_M: f32 = 6.2;
/// Trucks go round a ring at this share of a car's speed.
pub const RING_TRUCK_SPEED: f32 = 0.85;
/// Routing: expected wait at a roundabout's give-way line, seconds.
pub const ROUTE_RING_S: f32 = 2.5;
