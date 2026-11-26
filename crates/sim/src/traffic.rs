//! Vehicles, pathfinding and congestion. Uses only the road graph.
//!
//! Model (see `balance` for the numbers):
//! * A trip starts at the road point nearest its origin building (or at the
//!   highway entry) and ends at the road point nearest its destination. The
//!   route is an A* path whose edge cost is the current travel time.
//! * Each segment direction is a single-file lane. Cars accelerate towards
//!   the segment's speed limit, slowed by a volume-delay curve of that
//!   lane's load / capacity, and keep a time headway plus a minimum gap to
//!   the car ahead, so queues form and are visible.
//! * Junctions (nodes with three or more segments) let one car through per
//!   `JUNCTION_HEADWAY_S`, approaches served round-robin. A car only enters
//!   the next segment if there is room at its start.
//! * Road edits: vehicles are re-anchored by position, broken routes are
//!   re-planned (budgeted per tick), cars whose road vanished are removed and
//!   their trip ends as if they had arrived.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::citizens::{CitizenId, Location, TravelMode};
use crate::derived::{Access, access_pos};
use crate::idmap::key_index;
use crate::junction::{self, JunctionState, Turn};
use crate::path::{self, Target};
use crate::roads::RoadSegment;
use crate::side::SideTable;
use crate::{
    BuildingId, HouseholdId, NodeId, RoadNetwork, SegmentId, TICKS_PER_SECOND, World, balance, geom, households,
};

new_key_type! {
    /// Stable id of a [`Vehicle`].
    pub struct VehicleId;
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Vehicle {
    /// Position in sim metres, on the segment centre line (renderers offset
    /// to the driving lane using `heading` and `lane`, see
    /// `RoadKind::lane_offset_m`). Inside a junction (`junction` is
    /// `Some`) it is the exact point on the turn path, lane offset included.
    pub pos: [f32; 2],
    /// Heading in radians, `atan2(dy, dx)` in sim space.
    pub heading: f32,
    /// Segment currently driven on.
    pub segment: Option<SegmentId>,
    /// Metres travelled along `segment` from the node it entered by
    /// (`0..=segment.length`; from `a` if `forward`, else from `b`).
    pub progress: f32,
    /// True when driving from the segment's `a` node towards `b`.
    pub forward: bool,
    /// Current speed, m/s.
    pub speed: f32,
    /// Remaining segments after the current one. The last one is the
    /// segment the destination lies on (the car stops part-way along it).
    pub route: Vec<SegmentId>,
    /// Last node the trip passes (where it enters its final segment), or
    /// `None` when the whole trip stays on one segment.
    pub destination: Option<NodeId>,
    pub household: Option<HouseholdId>,
    /// Stable random number for purely visual variation.
    #[serde(default)]
    pub seed: u32,
    /// Lane on the current segment direction: 0 is the kerb-side (right)
    /// lane, 1 the next one left (avenues and the highway have two).
    pub lane: u8,
    /// Lateral position still to cover after a lane change, in lanes
    /// (+1: the car came from the lane to its left, -1: from the right);
    /// eases to 0 over about a second. Renderers add it to `lane`.
    pub lane_shift: f32,
    /// Set while the car is inside a junction box.
    pub junction: Option<JunctionMove>,
    /// Slowing down (brake lights).
    pub braking: bool,
    /// Allowed into the junction ahead at the last tick (cars that must
    /// give way yield to it).
    pub go: bool,
}

impl Vehicle {
    /// Standing (or crawling) still.
    pub fn stopped(&self) -> bool {
        self.speed < balance::WAIT_SPEED
    }
}

/// A vehicle's way through a junction box. While it is set, the vehicle's
/// `segment` / `progress` stay at the end of the segment it left; `pos` and
/// `heading` follow the movement's curve
/// (`junction::JunctionState::movements[movement]`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JunctionMove {
    pub node: NodeId,
    /// Segment the vehicle came from and the one it turns into.
    pub from: SegmentId,
    pub to: SegmentId,
    /// Direction it will drive `to` in (`a -> b`).
    pub to_forward: bool,
    /// Index into the junction's `movements`.
    pub movement: u16,
    pub turn: Turn,
    pub from_lane: u8,
    pub to_lane: u8,
    /// Metres travelled along the turn so far, and its length.
    pub distance: f32,
    pub length: f32,
}

/// Why a car is on the road.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TripPurpose {
    ToWork,
    ToHome,
    /// Newcomer driving in from the highway to their new home.
    MoveIn,
    /// Household that left town driving out to the highway.
    MoveOut,
    /// Shopping run between a home (or workplace) and a shop, both ways.
    Shopping,
    /// Goods delivery: industry to shop, or to / from the highway. Renderers
    /// may draw these as trucks (look up `world.traffic.trips`).
    Goods,
    /// A night out: to a leisure venue (any commercial building) and back.
    Leisure,
}

impl TripPurpose {
    pub const ALL: [TripPurpose; 7] = [
        TripPurpose::ToWork,
        TripPurpose::ToHome,
        TripPurpose::MoveIn,
        TripPurpose::MoveOut,
        TripPurpose::Shopping,
        TripPurpose::Goods,
        TripPurpose::Leisure,
    ];
    pub const COUNT: usize = 7;

    /// Index into per-purpose arrays (`ALL` order).
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Where a trip starts or ends.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Place {
    /// The outer end of the highway stub.
    Entry,
    /// The road point nearest to this position (a building centre), metres.
    At([f32; 2]),
}

/// Per-vehicle trip data kept beside [`Vehicle`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trip {
    pub purpose: TripPurpose,
    pub to: Place,
    /// Resolved destination on the network.
    pub dest: Option<Access>,
    /// Tick the trip was requested (commute time counts from here).
    pub requested_tick: u64,
    /// Consecutive ticks below `WAIT_SPEED`.
    pub wait_ticks: u32,
    /// Route is broken; the car waits at the end of its segment for a new one.
    pub needs_route: bool,
    /// In the re-plan queue.
    pub queued: bool,
    /// The citizen driving (none for trucks and for cars of households
    /// leaving the city, see `party`). The people in a car stay in it from
    /// departure to arrival. Passengers: `inspect::occupants`.
    pub citizen: Option<CitizenId>,
    /// The citizen whose trip this car draws (the driver; stays set if the
    /// driver leaves the city mid-trip, for their commute statistics).
    pub owner: Option<CitizenId>,
    /// Where the trip started.
    pub from: Place,
    /// Buildings at the two ends (`None`: the highway entry, or a building
    /// that has since been removed is still named here; check it exists).
    pub from_building: Option<BuildingId>,
    pub to_building: Option<BuildingId>,
    /// People aboard who are no longer citizens (a household moving out of
    /// the city drives away with this many). 0 for citizens' trips (count
    /// them with `inspect::occupants`) and trucks.
    pub party: u8,
    /// Goods aboard a truck, units (0 for cars).
    pub cargo: f32,
    /// Tick the car pulled out onto the road.
    pub started_tick: u64,
}

/// A trip waiting for its car to be put on the road.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Departure {
    pub household: Option<HouseholdId>,
    /// The citizen who will drive.
    pub citizen: Option<CitizenId>,
    pub purpose: TripPurpose,
    pub from: Place,
    pub to: Place,
    pub requested_tick: u64,
    /// See [`Trip::from_building`], [`Trip::to_building`], [`Trip::party`],
    /// [`Trip::cargo`].
    pub from_building: Option<BuildingId>,
    pub to_building: Option<BuildingId>,
    pub party: u8,
    pub cargo: f32,
}

impl Departure {
    /// A trip of `purpose` between two places, requested now.
    pub(crate) fn new(world: &World, purpose: TripPurpose, from: Place, to: Place) -> Departure {
        Departure {
            household: None,
            citizen: None,
            purpose,
            from,
            to,
            requested_tick: world.time.tick,
            from_building: None,
            to_building: None,
            party: 0,
            cargo: 0.0,
        }
    }
}

/// How a trip ended, reported to the household.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TripEnd {
    /// Reached the destination after this many vehicle seconds.
    Arrived(f32),
    /// Gave up after being stuck; counts as arrived, late.
    GaveUp(f32),
    /// The car was removed by a road edit; the trip completes off screen.
    Vanished,
    /// No route exists between origin and destination.
    NoRoute,
    /// The car could not start in time; the trip happens off screen.
    Cancelled,
}

/// A finished trip, reported to the population model.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ended {
    pub household: Option<HouseholdId>,
    pub citizen: Option<CitizenId>,
    pub owner: Option<CitizenId>,
    pub purpose: TripPurpose,
    pub end: TripEnd,
    pub to: Place,
    pub to_building: Option<BuildingId>,
    /// Where the car was when the trip ended (its people go on from there
    /// when it was taken off the road).
    pub at: Option<[f32; 2]>,
}

/// All traffic state that is not on the vehicles themselves.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrafficState {
    pub trips: SideTable<VehicleId, Trip>,
    /// Geometry, control and signal state per node (see `junction`).
    /// Rebuilt when the road graph changes; settings and signal timers
    /// survive edits that keep a junction's arms.
    pub junction_states: SideTable<NodeId, JunctionState>,
    /// `versions.roads` the junction states were built for.
    pub junctions_version: u64,
    /// Trips waiting for a car to start (path budget or blocked driveway).
    pub pending: VecDeque<Departure>,
    /// Vehicles waiting for a new route.
    pub replan: VecDeque<VehicleId>,
    /// `versions.roads` the vehicles were last checked against.
    pub roads_version: u64,
    /// Moving average of completed car commutes (to and from work, on the
    /// road), door to door, vehicle seconds.
    pub avg_commute_s: f32,
    /// Mean speed of the cars on the road that are moving this tick
    /// (faster than `balance::WAIT_SPEED`), m/s. Cars standing at a red
    /// light or in a queue are counted by `stopped_share` instead.
    pub avg_speed: f32,
    /// Most cars on the road at once today / yesterday.
    pub peak_today: u32,
    pub peak_yesterday: u32,
    /// Completed car trips today.
    pub trips_today: u32,
    /// Pending shopping / goods trips (`errands`).
    pub errand_acc: [f32; 2],
    /// Share of segments with a direction above 80 % of capacity, now and
    /// the highest today / yesterday.
    pub congestion_share: f32,
    pub peak_congestion_today: f32,
    pub peak_congestion_yesterday: f32,
    /// Gridlock breakers that fired (all time): a car stuck so long it was
    /// taken off the road, and junction deadlocks that had to be cleared.
    pub stuck_removed: u32,
    pub deadlocks_resolved: u32,
    /// Bumped whenever a junction's control changes (routing costs follow).
    pub control_version: u64,
    /// Share of the cars on the road standing (at a red light, in a queue,
    /// giving way) this tick.
    pub stopped_share: f32,
    /// Moving average of car trips' door-to-door time over their free-flow
    /// estimate (`day::estimate_s`), at least 1: what the roads do to every
    /// trip, on screen or off.
    pub delay_factor: f32,
}

pub(crate) const DT: f32 = 1.0 / TICKS_PER_SECOND as f32;

pub(crate) fn secs_to_ticks(s: f32) -> u64 {
    (s * TICKS_PER_SECOND as f32).round() as u64
}

fn lane_of(s: SegmentId, forward: bool) -> usize {
    key_index(s) * 2 + RoadSegment::dir(forward)
}

/// Shortest route (by current travel time) from `from` to `to` as a list of
/// segments.
pub fn shortest_path(roads: &RoadNetwork, from: NodeId, to: NodeId) -> Option<Vec<SegmentId>> {
    let mut scratch = path::PathScratch::default();
    let mut budget = i64::MAX;
    scratch
        .search(roads, &[], &[(from, 0.0)], Target::Node(to), &mut budget)
        .map(|f| f.segments)
}

/// Queue a trip. The car appears once a route is found and the driveway is
/// clear.
pub(crate) fn request(world: &mut World, household: Option<HouseholdId>, purpose: TripPurpose, from: Place, to: Place) {
    let mut d = Departure::new(world, purpose, from, to);
    d.household = household;
    request_trip(world, d);
}

/// Queue a trip (see [`Departure`]).
pub(crate) fn request_trip(world: &mut World, dep: Departure) {
    world.traffic.pending.push_back(dep);
}

/// A household left the city while `party` of its members were in car
/// `id`: the car takes them out to the highway instead.
pub(crate) fn move_out(world: &mut World, id: VehicleId, party: u8) {
    let Some(t) = world.traffic.trips.get_mut(id) else {
        return;
    };
    t.purpose = TripPurpose::MoveOut;
    t.to = Place::Entry;
    t.to_building = None;
    t.citizen = None;
    t.party = party;
    t.needs_route = true;
    queue_replan(&mut world.traffic, id, true);
    if let Some(v) = world.vehicles.get_mut(id) {
        v.household = None;
    }
}

/// The people in car `id` change their mind: it heads for `to` instead
/// (they turn back home when the shop or venue they were driving to has
/// closed).
pub(crate) fn redirect(world: &mut World, id: VehicleId, to: Place, to_building: Option<BuildingId>) {
    let Some(t) = world.traffic.trips.get_mut(id) else {
        return;
    };
    t.to = to;
    t.to_building = to_building;
    t.needs_route = true;
    queue_replan(&mut world.traffic, id, true);
}

/// Cars allowed on the road (and waiting to start) at once: 125 per 1,000
/// residents, at least 150, at most `balance::MAX_VEHICLES`.
pub fn vehicle_budget(world: &World) -> usize {
    (world.citizens.len() * balance::VEHICLES_PER_1000 / 1000).clamp(balance::MIN_VEHICLE_BUDGET, balance::MAX_VEHICLES)
}

/// Is there room in the vehicle budget ([`vehicle_budget`]) for one more
/// car?
pub fn has_room(world: &World) -> bool {
    world.vehicles.len() + world.traffic.pending.len() < vehicle_budget(world)
}

/// [`TrafficState::delay_factor`], at least 1.
pub fn delay_factor(world: &World) -> f32 {
    world.traffic.delay_factor.max(1.0)
}

fn place_pos(world: &World, p: Place) -> Option<[f32; 2]> {
    match p {
        Place::At(p) => Some(p),
        Place::Entry => world.roads.entry.and_then(|e| world.roads.nodes.get(e)).map(|n| n.pos),
    }
}

fn resolve(world: &World, place: Place) -> Option<Access> {
    match place {
        Place::Entry => {
            let e = world.roads.entry?;
            let s = *world.roads.nodes.get(e)?.segments.first()?;
            let seg = &world.roads.segments[s];
            Some(Access {
                segment: s,
                from_a: if seg.a == e { 0.0 } else { seg.length },
            })
        }
        Place::At(p) => world.cache.access_near(&world.roads, p),
    }
}

pub(crate) fn set_pose(roads: &RoadNetwork, v: &mut Vehicle) {
    let Some(s) = v.segment else { return };
    let Some(seg) = roads.segments.get(s) else { return };
    let p = seg.pose_dir(v.forward, v.progress.clamp(0.0, seg.length), 0.0);
    v.pos = p.pos;
    v.heading = p.heading();
}

fn route_valid(roads: &RoadNetwork, v: &Vehicle, dest: Option<Access>) -> bool {
    let Some(seg) = v.segment.and_then(|s| roads.segments.get(s)) else {
        return false;
    };
    let mut node = if v.forward { seg.b } else { seg.a };
    let mut last = v.segment.expect("checked");
    for &r in &v.route {
        let Some(rs) = roads.segments.get(r) else {
            return false;
        };
        if rs.a == node {
            node = rs.b;
        } else if rs.b == node {
            node = rs.a;
        } else {
            return false;
        }
        last = r;
    }
    dest.is_none_or(|d| d.segment == last)
}

fn queue_replan(traffic: &mut TrafficState, id: VehicleId, urgent: bool) {
    if let Some(t) = traffic.trips.get_mut(id)
        && !t.queued
    {
        t.queued = true;
        if urgent {
            traffic.replan.push_front(id);
        } else {
            traffic.replan.push_back(id);
        }
    }
}

pub(crate) fn remove_vehicle(world: &mut World, id: VehicleId, end: TripEnd, ended: &mut Vec<Ended>) {
    let car = world.vehicles.remove(id);
    let at = car.as_ref().map(|v| v.pos);
    let household = car.and_then(|v| v.household);
    if let Some(trip) = world.traffic.trips.remove(id) {
        if let TripEnd::Arrived(s) | TripEnd::GaveUp(s) = end {
            world.traffic.trips_today += 1;
            if matches!(trip.purpose, TripPurpose::ToWork | TripPurpose::ToHome) {
                let t = &mut world.traffic;
                t.avg_commute_s = if t.avg_commute_s == 0.0 {
                    s
                } else {
                    t.avg_commute_s + (s - t.avg_commute_s) * 0.02
                };
            }
            // Commutes only (errands are short and may turn back), each
            // weighted by its length so a minute's drive counts more than
            // a hop round the corner.
            if let TripEnd::Arrived(s) = end
                && matches!(trip.purpose, TripPurpose::ToWork | TripPurpose::ToHome)
                && let (Some(a), Some(b)) = (place_pos(world, trip.from), place_pos(world, trip.to))
            {
                let est = crate::day::estimate_s(a, b);
                let r = (s / est).clamp(0.5, balance::DELAY_FACTOR_MAX);
                let k = 0.01 * (est / 60.0).min(3.0);
                let t = &mut world.traffic;
                t.delay_factor = if t.delay_factor == 0.0 {
                    r.max(1.0)
                } else {
                    (t.delay_factor + (r - t.delay_factor) * k).max(1.0)
                };
            }
        }
        ended.push(Ended {
            household,
            citizen: trip.citizen,
            owner: trip.owner,
            purpose: trip.purpose,
            end,
            to: trip.to,
            to_building: trip.to_building,
            at,
        });
    }
}

/// Re-anchor vehicles after the road graph changed. Called from
/// `World::apply` and at the start of every traffic tick (no-op when
/// nothing changed).
pub(crate) fn sync_network(world: &mut World, ended: &mut Vec<Ended>) {
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    world.cache.ensure_zoning(&world.zoning);
    sync_junctions(world);
    if world.traffic.roads_version == world.versions.roads {
        return;
    }
    world.traffic.roads_version = world.versions.roads;
    let ids: Vec<VehicleId> = world.vehicles.keys().collect();
    for id in ids {
        // Inside a junction box: keep the movement if the junction still
        // has it, else put the car back at the stop line it came from.
        let mut ok = false;
        if let Some(jm) = world.vehicles[id].junction {
            let kept = world.traffic.junction_states.get(jm.node).and_then(|j| {
                let (a, b) = (j.arm_of(jm.from)?, j.arm_of(jm.to)?);
                let m = j.movement(a, jm.from_lane, b)?;
                Some((m, j.movements[m].length, j.movements[m].to_lane, j.arms[a].setback))
            });
            let seg_ok = world.roads.segments.contains_key(jm.from) && world.roads.segments.contains_key(jm.to);
            let v = &mut world.vehicles[id];
            match kept {
                Some((m, length, to_lane, setback)) if seg_ok => {
                    // The segment it left may have been cut or merged:
                    // stay at its stop line.
                    let from_len = world.roads.segments[jm.from].length;
                    v.progress = (from_len - setback).clamp(0.0, from_len);
                    let jm = v.junction.as_mut().expect("checked");
                    jm.movement = m as u16;
                    jm.length = length;
                    jm.to_lane = to_lane;
                    jm.distance = jm.distance.min(length);
                    ok = true;
                }
                _ => {
                    v.junction = None;
                    if let Some(seg) = world.roads.segments.get(jm.from) {
                        v.segment = Some(jm.from);
                        v.progress = v.progress.min(seg.length);
                        set_pose(&world.roads, v);
                    }
                }
            }
        }
        let v = &world.vehicles[id];
        if !ok
            && let Some(s) = v.segment
            && let Some(seg) = world.roads.segments.get(s)
            && v.progress <= seg.length + 0.01
        {
            let mut probe = v.clone();
            set_pose(&world.roads, &mut probe);
            ok = geom::dist(probe.pos, v.pos) < 0.5;
        }
        if !ok && !relocate(world, id) {
            remove_vehicle(world, id, TripEnd::Vanished, ended);
            continue;
        }
        // Refresh the destination and check the route still leads there.
        let to = world.traffic.trips.get(id).map(|t| t.to);
        let dest = to.and_then(|p| resolve(world, p));
        if let Some(t) = world.traffic.trips.get_mut(id) {
            t.dest = dest;
        }
        let valid = dest.is_some() && route_valid(&world.roads, &world.vehicles[id], dest);
        if let Some(t) = world.traffic.trips.get_mut(id) {
            if !valid {
                t.needs_route = true;
            }
            // Everyone reconsiders their route (new roads may be faster);
            // broken routes first.
            queue_replan(&mut world.traffic, id, !valid);
        } else if !valid {
            // Untracked vehicle (no trip): just let it finish its segment.
            world.vehicles[id].route.clear();
        }
    }
}

/// Rebuild junction geometry / control for the current road graph (no-op
/// when it did not change).
pub(crate) fn sync_junctions(world: &mut World) {
    if world.traffic.junctions_version == world.versions.roads {
        return;
    }
    world.traffic.junctions_version = world.versions.roads;
    let roads = &world.roads;
    let states = &mut world.traffic.junction_states;
    states.retain_keys(|n| roads.nodes.contains_key(n));
    for n in roads.nodes.keys() {
        let old = states.remove(n);
        states.insert(n, junction::build_state(roads, n, old.as_ref()));
    }
}

/// Put a vehicle on the segment under its position, keeping its heading.
fn relocate(world: &mut World, id: VehicleId) -> bool {
    let v = &world.vehicles[id];
    let pos = v.pos;
    let dir = [libm::cosf(v.heading), libm::sinf(v.heading)];
    let c = crate::Cell::from_pos(pos);
    let mut best: Option<(f32, SegmentId, f32)> = None;
    for y in c.y - 1..=c.y + 1 {
        for x in c.x - 1..=c.x + 1 {
            for &s in world.cache.segments_in(crate::Cell::new(x, y)) {
                let Some(seg) = world.roads.segments.get(s) else {
                    continue;
                };
                let (from_a, d, _) = seg.nearest(pos);
                if d < 1.0 && best.is_none_or(|b| d < b.0) {
                    best = Some((d, s, from_a));
                }
            }
        }
    }
    let Some((_, s, from_a)) = best else {
        return false;
    };
    let forward = geom::dot(world.roads.segments[s].pose(from_a, 0.0).dir, dir) >= 0.0;
    let len = world.roads.segments[s].length;
    let v = &mut world.vehicles[id];
    v.segment = Some(s);
    v.forward = forward;
    v.progress = if forward { from_a } else { len - from_a };
    set_pose(&world.roads, v);
    true
}

/// Per-tick vehicle movement and segment load bookkeeping.
pub(crate) fn tick(world: &mut World) {
    let mut ended = Vec::new();
    sync_network(world, &mut ended);
    if world.time.tick_of_day == 0 {
        world.traffic.peak_yesterday = world.traffic.peak_today;
        world.traffic.peak_today = 0;
        world.traffic.peak_congestion_yesterday = world.traffic.peak_congestion_today;
        world.traffic.peak_congestion_today = 0.0;
        world.traffic.trips_today = 0;
    }
    crate::drive::step(world, &mut ended);
    let total = balance::PATH_EXPANSIONS_PER_TICK as i64;
    let mut budget = total;
    replan_some(world, &mut budget, &mut ended);
    // Every road edit queues all cars for a re-plan; departures keep at
    // least half the budget so a busy editor cannot starve them.
    let mut budget = budget.max(total / 2);
    depart_some(world, &mut budget, &mut ended);
    update_loads(world);
    for e in ended {
        households::trip_ended(world, e);
    }
}

fn replan_some(world: &mut World, budget: &mut i64, ended: &mut Vec<Ended>) {
    let mut tries = world.traffic.replan.len();
    while tries > 0 && *budget > 0 {
        tries -= 1;
        let Some(id) = world.traffic.replan.pop_front() else {
            break;
        };
        let Some(trip) = world.traffic.trips.get_mut(id) else {
            continue;
        };
        trip.queued = false;
        let to = trip.to;
        let Some(v) = world.vehicles.get(id) else { continue };
        if v.junction.is_some() {
            // Crossing a junction: its next segment is committed. Try
            // again once it is out of the box.
            if let Some(t) = world.traffic.trips.get_mut(id) {
                t.queued = true;
            }
            world.traffic.replan.push_back(id);
            continue;
        }
        let Some(s) = v.segment.filter(|&s| world.roads.segments.contains_key(s)) else {
            continue;
        };
        let dest = resolve(world, to);
        let Some(dest) = dest else {
            remove_vehicle(world, id, TripEnd::Vanished, ended);
            continue;
        };
        let seg = &world.roads.segments[s];
        let here_from_a = if v.forward { v.progress } else { seg.length - v.progress };
        let (route, last) =
            if dest.segment == s && (dest.from_a - here_from_a) * if v.forward { 1.0 } else { -1.0 } >= 0.0 {
                (Vec::new(), None)
            } else {
                let end = if v.forward { seg.b } else { seg.a };
                let remaining = path::drive_time(seg, v.forward, seg.length - v.progress);
                let cache = &mut world.cache;
                match cache.path.search(
                    &world.roads,
                    &cache.route_delay,
                    &[(end, remaining)],
                    Target::Access(dest),
                    budget,
                ) {
                    Some(found) => (found.segments, Some(found.last_node)),
                    None => {
                        remove_vehicle(world, id, TripEnd::Vanished, ended);
                        continue;
                    }
                }
            };
        let v = &mut world.vehicles[id];
        v.route = route;
        v.destination = last;
        let t = world.traffic.trips.get_mut(id).expect("checked");
        t.dest = Some(dest);
        t.needs_route = false;
    }
}

fn depart_some(world: &mut World, budget: &mut i64, ended: &mut Vec<Ended>) {
    let mut tries = world.traffic.pending.len().min(balance::DEPARTURES_PER_TICK);
    let mut lanes: Option<Vec<(usize, f32, bool)>> = None;
    let mut marks = std::mem::take(&mut world.cache.depart_marks);
    let mut marked: Vec<usize> = Vec::new();
    while tries > 0 && *budget > 0 {
        tries -= 1;
        let Some(dep) = world.traffic.pending.pop_front() else {
            break;
        };
        if let Some(h) = dep.household
            && !world.households.contains_key(h)
        {
            continue;
        }
        let gone = |end| Ended {
            household: dep.household,
            citizen: dep.citizen,
            owner: dep.citizen,
            purpose: dep.purpose,
            end,
            to: dep.to,
            to_building: dep.to_building,
            at: None,
        };
        let waited = world.time.tick.saturating_sub(dep.requested_tick);
        if waited > secs_to_ticks(balance::MAX_DEPARTURE_WAIT_S) {
            ended.push(gone(TripEnd::Cancelled));
            continue;
        }
        let (Some(from), Some(to)) = (resolve(world, dep.from), resolve(world, dep.to)) else {
            ended.push(gone(TripEnd::NoRoute));
            continue;
        };
        let from_seg = world.roads.segments[from.segment].clone();
        // Cars per lane, for the driveway check (built on first use).
        let lanes = lanes.get_or_insert_with(|| {
            // Only the segments this tick's departures start on (the first
            // `tries` in the queue; blocked ones go to its back).
            marks.resize(world.roads.segments.slot_count(), false);
            marked.push(key_index(from.segment));
            for d in world.traffic.pending.iter().take(tries) {
                if let Some(a) = resolve(world, d.from) {
                    marked.push(key_index(a.segment));
                }
            }
            for &i in &marked {
                marks[i] = true;
            }
            let mut l: Vec<(usize, f32, bool)> = world
                .vehicles
                .values()
                .filter_map(|v| {
                    let s = v.segment?;
                    marks
                        .get(key_index(s))
                        .copied()
                        .unwrap_or(false)
                        .then(|| (lane_of(s, v.forward), v.progress, v.stopped()))
                })
                .collect();
            l.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            l
        });
        // Where the car would start in direction `fwd`: at its access point,
        // but on the road, not inside a junction box.
        let setbacks_ok = world.cache.setbacks_key == Some(world.traffic.junctions_version);
        let setbacks = &world.cache.setbacks;
        let start_at = |fwd: bool| {
            let p = if fwd {
                from.from_a
            } else {
                from_seg.length - from.from_a
            };
            if setbacks_ok {
                let (lo, hi) = crate::drive::lane_range(setbacks, from.segment, fwd, from_seg.length);
                // Not with its nose over the stop line either.
                let hi = if hi < from_seg.length - 0.01 {
                    (hi - crate::drive::stop_back_m(dep.purpose == TripPurpose::Goods)).max(lo.min(hi))
                } else {
                    hi
                };
                let lo = if lo > 0.0 {
                    (lo + balance::NO_LANE_CHANGE_AFTER_BOX_M).min(hi)
                } else {
                    lo
                };
                p.clamp(lo, hi)
            } else {
                p
            }
        };
        let (start_fwd, start_back) = (start_at(true), start_at(false));
        // Blocked: a car right at the driveway, or a standing queue just
        // ahead of it (nobody pulls out into a jam: the trip waits, and in
        // the end goes without the car).
        let blocked = |fwd: bool| {
            let lane = lane_of(from.segment, fwd);
            let progress = if fwd { start_fwd } else { start_back };
            let start = lanes.partition_point(|&(l, _, _)| l < lane);
            lanes[start..]
                .iter()
                .take_while(|&&(l, _, _)| l == lane)
                .any(|&(_, p, stopped)| {
                    (p - progress).abs() < balance::SPAWN_CLEARANCE_M
                        || (stopped && p > progress && p - progress < balance::SPAWN_QUEUE_M)
                })
        };
        if blocked(true) && blocked(false) {
            world.traffic.pending.push_back(dep);
            continue;
        }
        // Starting direction(s) and route.
        let (forward, route, last) = if from.segment == to.segment {
            (to.from_a >= from.from_a, Vec::new(), None)
        } else {
            let sources: Vec<(NodeId, f32)> = match dep.from {
                Place::Entry => {
                    let fwd = from.from_a == 0.0;
                    let end = if fwd { from_seg.b } else { from_seg.a };
                    vec![(end, path::drive_time(&from_seg, fwd, from_seg.length))]
                }
                Place::At(_) => vec![
                    (from_seg.a, path::drive_time(&from_seg, false, from.from_a)),
                    (
                        from_seg.b,
                        path::drive_time(&from_seg, true, from_seg.length - from.from_a),
                    ),
                ],
            };
            let cache = &mut world.cache;
            match cache
                .path
                .search(&world.roads, &cache.route_delay, &sources, Target::Access(to), budget)
            {
                Some(found) => (found.first == from_seg.b, found.segments, Some(found.last_node)),
                None => {
                    ended.push(gone(TripEnd::NoRoute));
                    continue;
                }
            }
        };
        let progress = if forward {
            from.from_a
        } else {
            from_seg.length - from.from_a
        };
        let _ = progress;
        let progress = if forward { start_fwd } else { start_back };
        if from.segment == to.segment && (to.from_a - from.from_a).abs() < 2.0 {
            ended.push(gone(TripEnd::Arrived(0.0)));
            continue;
        }
        // Driveway clear in the chosen direction?
        if blocked(forward) {
            world.traffic.pending.push_back(dep);
            continue;
        }
        let lane = lane_of(from.segment, forward);
        let pos = lanes.partition_point(|&(l, p, _)| l < lane || (l == lane && p < progress));
        lanes.insert(pos, (lane, progress, true));
        let seed = world.rng.next_u64() as u32;
        let mut v = Vehicle {
            pos: [0.0; 2],
            heading: 0.0,
            segment: Some(from.segment),
            progress,
            forward,
            speed: 0.0,
            route,
            destination: last,
            household: dep.household,
            seed,
            ..Vehicle::default()
        };
        set_pose(&world.roads, &mut v);
        let id = world.vehicles.insert(v);
        world.traffic.trips.insert(
            id,
            Trip {
                purpose: dep.purpose,
                to: dep.to,
                dest: Some(to),
                requested_tick: dep.requested_tick,
                wait_ticks: 0,
                needs_route: false,
                queued: false,
                citizen: dep.citizen,
                owner: dep.citizen,
                from: dep.from,
                from_building: dep.from_building,
                to_building: dep.to_building,
                party: dep.party,
                cargo: dep.cargo,
                started_tick: world.time.tick,
            },
        );
        if let Some(c) = dep.citizen {
            board(world, c, id);
        } else if let Some(h) = dep.household.and_then(|h| world.households.get_mut(h))
            && h.citizens.is_empty()
        {
            h.vehicle = Some(id);
        }
    }
    for i in marked {
        marks[i] = false;
    }
    world.cache.depart_marks = marks;
}

/// The car of `driver` started: they and their passengers are in it.
fn board(world: &mut World, driver: CitizenId, v: VehicleId) {
    let Some(c) = world.citizens.get_mut(driver) else {
        return;
    };
    let Location::Travelling(j) = &mut c.location else {
        return;
    };
    j.vehicle = Some(v);
    let hid = c.household;
    let members = world
        .households
        .get(hid)
        .map(|h| h.citizens.clone())
        .unwrap_or_default();
    for m in members {
        if let Some(mc) = world.citizens.get_mut(m)
            && let Location::Travelling(j) = &mut mc.location
            && j.mode == TravelMode::Passenger
            && j.driver == Some(driver)
        {
            j.vehicle = Some(v);
        }
    }
    crate::people::refresh_household(world, hid);
}

fn update_loads(world: &mut World) {
    for seg in world.roads.segments.values_mut() {
        seg.load = [0.0; 2];
    }
    let mut speed_sum = 0.0;
    let mut moving = 0u32;
    for v in world.vehicles.values() {
        if !v.stopped() {
            speed_sum += v.speed;
            moving += 1;
        }
        if let Some(seg) = v.segment.and_then(|s| world.roads.segments.get_mut(s)) {
            seg.load[RoadSegment::dir(v.forward)] += 1.0;
        }
    }
    // Queues per approach, per-minute entry counts (auto control).
    for j in world.traffic.junction_states.values_mut() {
        j.queue.iter_mut().for_each(|q| *q = 0);
    }
    for v in world.vehicles.values() {
        if !v.stopped() || v.junction.is_some() {
            continue;
        }
        let Some(seg) = v.segment.and_then(|s| world.roads.segments.get(s)) else {
            continue;
        };
        let end = if v.forward { seg.b } else { seg.a };
        if let Some(j) = world.traffic.junction_states.get_mut(end)
            && let Some(arm) = j.arm_of(v.segment.expect("checked"))
        {
            j.queue[arm] = j.queue[arm].saturating_add(1);
        }
    }
    let minute = 60 * TICKS_PER_SECOND as u64;
    if world.time.tick.is_multiple_of(minute) {
        for j in world.traffic.junction_states.values_mut() {
            // Demand, not just what got through: those let in plus those
            // still queued (a jammed all-way stop lets in fewer cars than
            // the signal threshold however long its queues).
            let waiting: u16 = j.queue.iter().fold(0u16, |a, &q| a.saturating_add(q));
            j.peak_minute_today = j.peak_minute_today.max(j.entered_minute.saturating_add(waiting));
            j.entered_minute = 0;
        }
    }
    if world.time.tick_of_day == 0 {
        for j in world.traffic.junction_states.values_mut() {
            j.peak_minute_yesterday = j.peak_minute_today.max(j.entered_minute);
            j.peak_minute_today = 0;
            let busy = if j.busy {
                j.peak_minute_yesterday >= balance::AUTO_PRIORITY_PER_MINUTE
            } else {
                j.peak_minute_yesterday >= balance::AUTO_SIGNAL_PER_MINUTE
            };
            if busy != j.busy {
                j.busy = busy;
                let setting = j.setting;
                junction::set_setting(j, setting);
                world.traffic.control_version += 1;
            }
        }
    }
    let n = world.vehicles.len();
    let nseg = world.roads.segments.len();
    let jammed = world.roads.segments.values().filter(|s| s.congestion() > 0.8).count();
    let t = &mut world.traffic;
    t.avg_speed = if moving > 0 { speed_sum / moving as f32 } else { 0.0 };
    t.stopped_share = if n > 0 { 1.0 - moving as f32 / n as f32 } else { 0.0 };
    t.peak_today = t.peak_today.max(n as u32);
    t.congestion_share = if nseg > 0 { jammed as f32 / nseg as f32 } else { 0.0 };
    t.peak_congestion_today = t.peak_congestion_today.max(t.congestion_share);
}

/// Debug counters: front cars refused entry at the stop line, by reason
/// `[conflict in box, no room beyond, signal, yielding]` (not saved).
pub fn debug_denials(world: &World) -> [u64; 4] {
    world.cache.denials
}

/// Debug: denials at the stop line of node `n` by reason `[conflict in box,
/// no room beyond, signal, yielding]` (not saved).
pub fn debug_node_denials(world: &World, n: NodeId) -> [u32; 4] {
    world.cache.node_denials.get(key_index(n)).copied().unwrap_or([0; 4])
}

/// Debug: cars that gave up waiting, `(tick, segment, forward, metres
/// before the stop line, lane)` (not saved).
pub fn debug_gave_up(world: &World) -> &[(u64, SegmentId, bool, f32, u8)] {
    &world.cache.gave_up
}

/// Debug: why the cars that gave up were last refused at a stop line
/// `[conflict in box, no room beyond, signal, yielding, never refused]`.
pub fn debug_gave_up_reasons(world: &World) -> [u32; 5] {
    world.cache.gave_up_reasons
}

/// Debug counters: junction entries by turn `[straight, right, left,
/// u-turn]` (not saved).
pub fn debug_turns(world: &World) -> [u64; 4] {
    world.cache.turns
}

/// Queue a household-less trip between two positions (tests, tools).
pub fn debug_request(world: &mut World, from: [f32; 2], to: [f32; 2]) {
    crate::people::count_trip(world, TripPurpose::ToWork);
    request(world, None, TripPurpose::ToWork, Place::At(from), Place::At(to));
}

/// Where a trip's car currently is, for debugging and tests.
pub fn trip_of(world: &World, id: VehicleId) -> Option<&Trip> {
    world.traffic.trips.get(id)
}

/// Point on the road a vehicle is heading for, if known.
pub fn destination_pos(world: &World, id: VehicleId) -> Option<[f32; 2]> {
    let d = world.traffic.trips.get(id)?.dest?;
    access_pos(&world.roads, d)
}

/// Every car's body as an oriented rectangle at its exact drawing pose
/// (`drive::vehicle_pose`), corners in order.
pub fn car_bodies(world: &World) -> Vec<(VehicleId, [[f32; 2]; 4])> {
    world
        .vehicles
        .iter()
        .map(|(id, v)| {
            let (p, d) = crate::drive::vehicle_pose(world, v);
            let truck = world
                .traffic
                .trips
                .get(id)
                .is_some_and(|t| t.purpose == TripPurpose::Goods);
            let [l, w] = if truck {
                balance::TRUCK_BODY_M
            } else {
                balance::CAR_BODY_M
            };
            let r = [-d[1], d[0]];
            let c = |a: f32, b: f32| [p[0] + d[0] * a + r[0] * b, p[1] + d[1] * a + r[1] * b];
            (
                id,
                [
                    c(l / 2.0, -w / 2.0),
                    c(l / 2.0, w / 2.0),
                    c(-l / 2.0, w / 2.0),
                    c(-l / 2.0, -w / 2.0),
                ],
            )
        })
        .collect()
}

/// Pairs of bodies (indices) overlapping by more than `tol` metres, with
/// the depth of the overlap.
pub fn overlapping_bodies(bodies: &[(VehicleId, [[f32; 2]; 4])], tol: f32) -> Vec<(usize, usize, f32)> {
    let mut grid: std::collections::BTreeMap<(i32, i32), Vec<usize>> = Default::default();
    let cell = 8.0;
    for (k, (_, q)) in bodies.iter().enumerate() {
        let c = [(q[0][0] + q[2][0]) * 0.5, (q[0][1] + q[2][1]) * 0.5];
        grid.entry(((c[0] / cell).floor() as i32, (c[1] / cell).floor() as i32))
            .or_default()
            .push(k);
    }
    let depth = |a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]| -> f32 {
        let mut best = f32::INFINITY;
        for poly in [a, b] {
            for k in 0..4 {
                let e = geom::sub(poly[(k + 1) % 4], poly[k]);
                let l = geom::dot(e, e).sqrt().max(1e-6);
                let n = [-e[1] / l, e[0] / l];
                let proj = |q: &[[f32; 2]; 4]| {
                    q.iter()
                        .map(|p| geom::dot(*p, n))
                        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)))
                };
                let (alo, ahi) = proj(a);
                let (blo, bhi) = proj(b);
                best = best.min(ahi.min(bhi) - alo.max(blo));
            }
        }
        best
    };
    let mut out = Vec::new();
    for (&(gx, gy), list) in &grid {
        for &i in list {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let Some(other) = grid.get(&(gx + dx, gy + dy)) else {
                        continue;
                    };
                    for &j in other {
                        if j <= i {
                            continue;
                        }
                        let d = depth(&bodies[i].1, &bodies[j].1);
                        if d > tol {
                            out.push((i, j, d));
                        }
                    }
                }
            }
        }
    }
    out
}

/// Debug: wall-clock time the last zone-block regeneration took, ns (not
/// saved; for benchmarks).
pub fn debug_last_block_sync_ns(world: &World) -> u64 {
    world.cache.last_block_sync_ns
}
