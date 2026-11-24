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

use crate::derived::{Access, access_pos};
use crate::idmap::key_index;
use crate::path::{self, Target};
use crate::roads::RoadSegment;
use crate::side::SideTable;
use crate::{HouseholdId, NodeId, RoadNetwork, SegmentId, TICKS_PER_SECOND, World, balance, geom, households};

new_key_type! {
    /// Stable id of a [`Vehicle`].
    pub struct VehicleId;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vehicle {
    /// Position in sim metres, on the segment centre line (renderers offset
    /// to the driving lane using `heading`).
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
    /// Shopping run between a home and a shop (no household attached).
    Shopping,
    /// Goods delivery: industry to shop, or to / from the highway. Renderers
    /// may draw these as trucks (look up `world.traffic.trips`).
    Goods,
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
}

/// A trip waiting for its car to be put on the road.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Departure {
    pub household: Option<HouseholdId>,
    pub purpose: TripPurpose,
    pub from: Place,
    pub to: Place,
    pub requested_tick: u64,
}

/// Junction control state per node slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Junction {
    /// Approaches outside the current phase may not enter before this tick
    /// (last crossing + clearance time).
    pub busy_until: u64,
    /// Index (into the node's `segments`) of the approach that opened the
    /// current phase; approaches parallel to it (straight across) share it.
    pub last_served: u8,
    /// Cars let through in the current phase.
    #[serde(default)]
    pub phase_count: u16,
}

/// Unit direction from node `n` along its `i`-th segment.
fn approach_dir(roads: &RoadNetwork, n: NodeId, i: usize) -> [f32; 2] {
    let node = &roads.nodes[n];
    let Some(&s) = node.segments.get(i) else {
        return [0.0, 0.0];
    };
    let other = roads.nodes[roads.segments[s].other(n)].pos;
    let d = geom::sub(other, node.pos);
    let l = geom::dot(d, d).sqrt().max(1e-6);
    [d[0] / l, d[1] / l]
}

/// Do two approaches of a junction move in the same signal phase (they lie
/// on one straight line, or are the same approach)?
fn same_phase(roads: &RoadNetwork, n: NodeId, a: usize, b: usize) -> bool {
    a == b || geom::cross(approach_dir(roads, n, a), approach_dir(roads, n, b)).abs() < 0.35
}

/// Is the move from segment `from` into segment `to` at node `n` a turn?
fn is_turn(roads: &RoadNetwork, n: NodeId, from: SegmentId, to: SegmentId) -> bool {
    let node = &roads.nodes[n];
    let i = node.segments.iter().position(|&s| s == from);
    let j = node.segments.iter().position(|&s| s == to);
    match (i, j) {
        // Straight on: the outgoing direction is opposite the incoming one.
        (Some(i), Some(j)) => geom::dot(approach_dir(roads, n, i), approach_dir(roads, n, j)) > -0.9,
        _ => true,
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

pub(crate) type Ended = (Option<HouseholdId>, TripPurpose, TripEnd);

/// All traffic state that is not on the vehicles themselves.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrafficState {
    pub trips: SideTable<VehicleId, Trip>,
    /// Indexed by node slot (`idmap::key_index`).
    pub junctions: Vec<Junction>,
    /// Trips waiting for a car to start (path budget or blocked driveway).
    pub pending: VecDeque<Departure>,
    /// Vehicles waiting for a new route.
    pub replan: VecDeque<VehicleId>,
    /// `versions.roads` the vehicles were last checked against.
    pub roads_version: u64,
    /// Moving average of completed car commutes, vehicle seconds.
    pub avg_commute_s: f32,
    /// Mean speed of the cars on the road this tick, m/s.
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
}

const DT: f32 = 1.0 / TICKS_PER_SECOND as f32;

fn secs_to_ticks(s: f32) -> u64 {
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
        .search(roads, &[(from, 0.0)], Target::Node(to), &mut budget)
        .map(|f| f.segments)
}

/// Queue a trip. The car appears once a route is found and the driveway is
/// clear.
pub(crate) fn request(world: &mut World, household: Option<HouseholdId>, purpose: TripPurpose, from: Place, to: Place) {
    world.traffic.pending.push_back(Departure {
        household,
        purpose,
        from,
        to,
        requested_tick: world.time.tick,
    });
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

fn set_pose(roads: &RoadNetwork, v: &mut Vehicle) {
    let Some(s) = v.segment else { return };
    let Some(seg) = roads.segments.get(s) else { return };
    let (pa, pb) = roads.segment_ends(s);
    let (from, to) = if v.forward { (pa, pb) } else { (pb, pa) };
    let f = if seg.length > 0.0 {
        (v.progress / seg.length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    v.pos = [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f];
    v.heading = (to[1] - from[1]).atan2(to[0] - from[0]);
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

fn remove_vehicle(world: &mut World, id: VehicleId, end: TripEnd, ended: &mut Vec<Ended>) {
    let household = world.vehicles.remove(id).and_then(|v| v.household);
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
        }
        ended.push((household, trip.purpose, end));
    }
}

/// Re-anchor vehicles after the road graph changed. Called from
/// `World::apply` and at the start of every traffic tick (no-op when
/// nothing changed).
pub(crate) fn sync_network(world: &mut World, ended: &mut Vec<Ended>) {
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    let n = world.roads.nodes.slot_count();
    if world.traffic.junctions.len() < n {
        world.traffic.junctions.resize(n, Junction::default());
    }
    if world.traffic.roads_version == world.versions.roads {
        return;
    }
    world.traffic.roads_version = world.versions.roads;
    let ids: Vec<VehicleId> = world.vehicles.keys().collect();
    for id in ids {
        // Still where it thinks it is?
        let v = &world.vehicles[id];
        let mut ok = false;
        if let Some(s) = v.segment
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

/// Put a vehicle on the segment under its position, keeping its heading.
fn relocate(world: &mut World, id: VehicleId) -> bool {
    let v = &world.vehicles[id];
    let pos = v.pos;
    let dir = [v.heading.cos(), v.heading.sin()];
    let c = crate::Cell::from_pos(pos);
    let mut best: Option<(f32, SegmentId, f32)> = None;
    for y in c.y - 1..=c.y + 1 {
        for x in c.x - 1..=c.x + 1 {
            for &s in world.cache.segments_in(crate::Cell::new(x, y)) {
                let Some(seg) = world.roads.segments.get(s) else {
                    continue;
                };
                let (pa, pb) = world.roads.segment_ends(s);
                let ab = geom::sub(pb, pa);
                let t = (geom::dot(geom::sub(pos, pa), ab) / geom::dot(ab, ab)).clamp(0.0, 1.0);
                let p = [pa[0] + ab[0] * t, pa[1] + ab[1] * t];
                let d = geom::dist(p, pos);
                if d < 1.0 && best.is_none_or(|b| d < b.0) {
                    best = Some((d, s, t * seg.length));
                }
            }
        }
    }
    let Some((_, s, from_a)) = best else {
        return false;
    };
    let (pa, pb) = world.roads.segment_ends(s);
    let forward = geom::dot(geom::sub(pb, pa), dir) >= 0.0;
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
    move_vehicles(world, &mut ended);
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

struct Request {
    node_slot: usize,
    node: NodeId,
    approach: usize,
    order_idx: usize,
    next_lane: usize,
}

fn move_vehicles(world: &mut World, ended: &mut Vec<Ended>) {
    let now = world.time.tick;
    let nlanes = world.roads.segments.slot_count() * 2;

    // Lanes: vehicles grouped by (segment, direction), front first.
    let mut order: Vec<(u32, f32, VehicleId)> = Vec::with_capacity(world.vehicles.len());
    let mut lost: Vec<VehicleId> = Vec::new();
    for (id, v) in world.vehicles.iter() {
        match v.segment.filter(|&s| world.roads.segments.contains_key(s)) {
            Some(s) => order.push((lane_of(s, v.forward) as u32, v.progress, id)),
            None => lost.push(id),
        }
    }
    for id in lost {
        remove_vehicle(world, id, TripEnd::Vanished, ended);
    }
    order.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut lane_rear = vec![f32::INFINITY; nlanes];
    for &(lane, p, _) in &order {
        let r = &mut lane_rear[lane as usize];
        *r = r.min(p);
    }

    let World {
        roads,
        vehicles,
        traffic,
        ..
    } = world;
    let roads: &RoadNetwork = roads;

    // Junction requests from lane fronts about to reach their end node.
    let mut requests: Vec<Request> = Vec::new();
    for (i, &(lane, _, id)) in order.iter().enumerate() {
        if i > 0 && order[i - 1].0 == lane {
            continue;
        }
        let v = &vehicles[id];
        let Some(next) = next_step(roads, v) else { continue };
        let seg = &roads.segments[v.segment.expect("in order")];
        let d_end = seg.length - v.progress;
        let reach = (v.speed + balance::ACCELERATION * DT).min(seg.speed_limit * 1.2) * DT;
        if d_end > reach + balance::STOP_LINE_M + 0.05 {
            continue;
        }
        if lane_rear[next.lane] < balance::MIN_GAP_M {
            continue;
        }
        let approach = roads.nodes[next.node]
            .segments
            .iter()
            .position(|&x| Some(x) == v.segment)
            .unwrap_or(0);
        requests.push(Request {
            node_slot: key_index(next.node),
            node: next.node,
            approach,
            order_idx: i,
            next_lane: next.lane,
        });
    }
    let mut granted = vec![false; order.len()];
    let mut lane_taken: Vec<usize> = Vec::new();
    requests.sort_by_key(|r| (r.node_slot, r.approach));
    let mut k = 0;
    while k < requests.len() {
        let mut e = k;
        while e < requests.len() && requests[e].node_slot == requests[k].node_slot {
            e += 1;
        }
        let node = requests[k].node;
        let deg = roads.nodes[node].segments.len();
        if deg <= 2 {
            for r in &requests[k..e] {
                if !lane_taken.contains(&r.next_lane) {
                    lane_taken.push(r.next_lane);
                    granted[r.order_idx] = true;
                }
            }
        } else {
            // Signal-like phases: approaches on one straight line flow
            // together for up to PHASE_MAX_CARS cars, then (after the
            // clearance time) the next waiting approach gets its phase.
            let j = &mut traffic.junctions[requests[k].node_slot];
            let cur = (j.last_served as usize).min(deg - 1);
            let reqs = &requests[k..e];
            let in_phase = |a: usize| same_phase(roads, node, a, cur);
            let any_in = reqs.iter().any(|r| in_phase(r.approach));
            let others = reqs.iter().filter(|r| !in_phase(r.approach));
            let waiting_others = others.clone().count() > 0;
            let phase = if any_in && (j.phase_count < balance::PHASE_MAX_CARS || !waiting_others) {
                if j.phase_count >= balance::PHASE_MAX_CARS {
                    j.phase_count = 0;
                }
                Some(cur)
            } else if waiting_others && j.busy_until <= now {
                let pick = others
                    .min_by_key(|r| (r.approach + deg - cur - 1) % deg)
                    .expect("waiting");
                j.last_served = pick.approach as u8;
                j.phase_count = 0;
                Some(pick.approach)
            } else {
                None
            };
            if let Some(p) = phase {
                for r in reqs {
                    if same_phase(roads, node, r.approach, p) && !lane_taken.contains(&r.next_lane) {
                        lane_taken.push(r.next_lane);
                        granted[r.order_idx] = true;
                        j.phase_count = j.phase_count.saturating_add(1);
                        j.busy_until = now + secs_to_ticks(balance::JUNCTION_CLEARANCE_S);
                    }
                }
            }
        }
        k = e;
    }

    // Move, lane by lane, front to back.
    let mut done: Vec<(VehicleId, TripEnd)> = Vec::new();
    let replan_ticks = secs_to_ticks(balance::REPLAN_AFTER_WAIT_S) as u32;
    let stuck_ticks = secs_to_ticks(balance::STUCK_REMOVE_AFTER_S) as u32;
    let mut i = 0;
    while i < order.len() {
        let lane = order[i].0;
        let mut leader: Option<f32> = None;
        while i < order.len() && order[i].0 == lane {
            let id = order[i].2;
            let is_front = leader.is_none();
            let v = &mut vehicles[id];
            let s = v.segment.expect("in order");
            let seg = &roads.segments[s];
            let len = seg.length;
            let driver = 1.0 + balance::DRIVER_SPEED_VARIATION * ((v.seed % 1001) as f32 / 500.0 - 1.0);
            let mut target = seg.speed_limit * driver * path::vdf(seg.congestion_in(v.forward));
            let d_end = len - v.progress;
            let trip = traffic.trips.get_mut(id);
            let mut cross: Option<Next> = None;
            let mut arrive_at: Option<f32> = None;
            if v.route.is_empty() {
                let dest = trip.as_ref().and_then(|t| if t.needs_route { None } else { t.dest });
                match dest {
                    Some(d) if d.segment == s => {
                        let stop = if v.forward { d.from_a } else { len - d.from_a };
                        arrive_at = Some(stop.clamp(0.0, len));
                    }
                    _ if trip.is_none() => arrive_at = Some(len),
                    _ => {}
                }
            }
            if let Some(lp) = leader {
                target = target.min(follow_speed(lp - v.progress));
                if let Some(stop) = arrive_at {
                    target = target.min(stop_speed(stop - v.progress + 3.0));
                }
            } else if v.route.is_empty() {
                target = match arrive_at {
                    Some(stop) if trip.is_some() => target.min(stop_speed(stop - v.progress + 3.0)),
                    Some(_) => target,
                    None => target.min(stop_speed(d_end - balance::STOP_LINE_M)),
                };
            } else {
                let next = next_step(roads, v);
                match next {
                    Some(n) if granted[i] => {
                        target = target.min(follow_speed(d_end + lane_rear[n.lane].min(1e6)));
                        cross = Some(n);
                    }
                    Some(n) => {
                        let deg = roads.nodes[n.node].segments.len();
                        let eta = d_end / v.speed.max(1.0);
                        let open = deg <= 2
                            || traffic.junctions.get(key_index(n.node)).is_some_and(|j| {
                                let me = roads.nodes[n.node].segments.iter().position(|&x| x == s).unwrap_or(0);
                                (same_phase(roads, n.node, me, j.last_served as usize)
                                    && j.phase_count < balance::PHASE_MAX_CARS)
                                    || j.busy_until <= now + secs_to_ticks(eta)
                            });
                        let clear = lane_rear[n.lane] >= balance::MIN_GAP_M && open;
                        if clear {
                            target = target.min(follow_speed(d_end + lane_rear[n.lane].min(1e6)));
                        } else {
                            target = target.min(stop_speed(d_end - balance::STOP_LINE_M));
                        }
                        if deg >= 3 && is_turn(roads, n.node, s, n.segment) {
                            let jv = balance::JUNCTION_SPEED;
                            target = target.min((jv * jv + 2.0 * balance::BRAKING * d_end.max(0.0)).sqrt());
                        }
                    }
                    None => target = target.min(stop_speed(d_end - balance::STOP_LINE_M)),
                }
            }
            let speed = (v.speed + balance::ACCELERATION * DT).min(target).max(0.0);
            let mut adv = speed * DT;
            v.speed = speed;
            let mut finished: Option<TripEnd> = None;
            if let Some(stop) = arrive_at
                && v.progress + adv >= stop - 0.05
            {
                let secs = trip
                    .as_ref()
                    .map_or(0.0, |t| (now - t.requested_tick.min(now)) as f32 * DT);
                finished = Some(TripEnd::Arrived(secs));
            }
            if finished.is_none() {
                if let Some(n) = cross {
                    let overflow = (v.progress + adv - len).max(0.0);
                    v.route.remove(0);
                    v.segment = Some(n.segment);
                    v.forward = n.forward;
                    v.progress = overflow.min(roads.segments[n.segment].length);
                    leader = Some(len + v.progress);
                } else {
                    if v.progress + adv > len - 0.01 {
                        adv = (len - 0.01 - v.progress).max(0.0);
                        v.speed = adv / DT;
                    }
                    v.progress += adv;
                    leader = Some(v.progress);
                }
                set_pose(roads, v);
            } else if !is_front {
                leader = Some(v.progress);
            }
            if let Some(t) = trip {
                if v.speed < balance::WAIT_SPEED {
                    t.wait_ticks += 1;
                } else {
                    t.wait_ticks = 0;
                }
                if finished.is_none() {
                    if t.wait_ticks >= stuck_ticks {
                        let secs = (now - t.requested_tick.min(now)) as f32 * DT;
                        finished = Some(TripEnd::GaveUp(secs));
                    } else if t.wait_ticks > 0 && t.wait_ticks % replan_ticks == 0 && !t.queued {
                        t.queued = true;
                        traffic.replan.push_back(id);
                    }
                }
            }
            if let Some(f) = finished {
                done.push((id, f));
            }
            i += 1;
        }
    }
    for (id, f) in done {
        remove_vehicle(world, id, f, ended);
    }
}

/// Speed allowed when the car ahead (or a free lane start) is `gap` metres
/// ahead.
fn follow_speed(gap: f32) -> f32 {
    ((gap - balance::MIN_GAP_M) / balance::FOLLOW_HEADWAY_S).max(0.0)
}

/// Speed from which the car can still stop within `d` metres.
fn stop_speed(d: f32) -> f32 {
    let d = d.max(0.0);
    (2.0 * balance::BRAKING * d).sqrt().min(d / DT)
}

#[derive(Clone, Copy)]
struct Next {
    node: NodeId,
    segment: SegmentId,
    forward: bool,
    lane: usize,
}

/// The node at the end of the current segment and the segment after it.
fn next_step(roads: &RoadNetwork, v: &Vehicle) -> Option<Next> {
    let seg = roads.segments.get(v.segment?)?;
    let node = if v.forward { seg.b } else { seg.a };
    let next = *v.route.first()?;
    let ns = roads.segments.get(next)?;
    let forward = if ns.a == node {
        true
    } else if ns.b == node {
        false
    } else {
        return None;
    };
    Some(Next {
        node,
        segment: next,
        forward,
        lane: lane_of(next, forward),
    })
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
                match world
                    .cache
                    .path
                    .search(&world.roads, &[(end, remaining)], Target::Access(dest), budget)
                {
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
    let mut lanes: Option<Vec<(usize, f32)>> = None;
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
        let waited = world.time.tick.saturating_sub(dep.requested_tick);
        if waited > secs_to_ticks(balance::MAX_DEPARTURE_WAIT_S) {
            ended.push((dep.household, dep.purpose, TripEnd::Cancelled));
            continue;
        }
        let (Some(from), Some(to)) = (resolve(world, dep.from), resolve(world, dep.to)) else {
            ended.push((dep.household, dep.purpose, TripEnd::NoRoute));
            continue;
        };
        let from_seg = world.roads.segments[from.segment].clone();
        // Cars per lane, for the driveway check (built on first use).
        let lanes = lanes.get_or_insert_with(|| {
            let mut l: Vec<(usize, f32)> = world
                .vehicles
                .values()
                .filter_map(|v| v.segment.map(|s| (lane_of(s, v.forward), v.progress)))
                .collect();
            l.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            l
        });
        let blocked = |fwd: bool| {
            let lane = lane_of(from.segment, fwd);
            let progress = if fwd {
                from.from_a
            } else {
                from_seg.length - from.from_a
            };
            let start = lanes.partition_point(|&(l, _)| l < lane);
            lanes[start..]
                .iter()
                .take_while(|&&(l, _)| l == lane)
                .any(|&(_, p)| (p - progress).abs() < balance::SPAWN_CLEARANCE_M)
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
            match world
                .cache
                .path
                .search(&world.roads, &sources, Target::Access(to), budget)
            {
                Some(found) => (found.first == from_seg.b, found.segments, Some(found.last_node)),
                None => {
                    ended.push((dep.household, dep.purpose, TripEnd::NoRoute));
                    continue;
                }
            }
        };
        let progress = if forward {
            from.from_a
        } else {
            from_seg.length - from.from_a
        };
        if from.segment == to.segment && (to.from_a - from.from_a).abs() < 2.0 {
            ended.push((dep.household, dep.purpose, TripEnd::Arrived(0.0)));
            continue;
        }
        // Driveway clear in the chosen direction?
        if blocked(forward) {
            world.traffic.pending.push_back(dep);
            continue;
        }
        let lane = lane_of(from.segment, forward);
        let pos = lanes.partition_point(|&(l, p)| l < lane || (l == lane && p < progress));
        lanes.insert(pos, (lane, progress));
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
            },
        );
        if let Some(h) = dep.household.and_then(|h| world.households.get_mut(h)) {
            h.vehicle = Some(id);
        }
    }
}

fn update_loads(world: &mut World) {
    for seg in world.roads.segments.values_mut() {
        seg.load = [0.0; 2];
    }
    let mut speed_sum = 0.0;
    for v in world.vehicles.values() {
        speed_sum += v.speed;
        if let Some(seg) = v.segment.and_then(|s| world.roads.segments.get_mut(s)) {
            seg.load[RoadSegment::dir(v.forward)] += 1.0;
        }
    }
    let n = world.vehicles.len();
    let nseg = world.roads.segments.len();
    let jammed = world.roads.segments.values().filter(|s| s.congestion() > 0.8).count();
    let t = &mut world.traffic;
    t.avg_speed = if n > 0 { speed_sum / n as f32 } else { 0.0 };
    t.peak_today = t.peak_today.max(n as u32);
    t.congestion_share = if nseg > 0 { jammed as f32 / nseg as f32 } else { 0.0 };
    t.peak_congestion_today = t.peak_congestion_today.max(t.congestion_share);
}

/// Queue a household-less trip between two positions (tests, tools).
pub fn debug_request(world: &mut World, from: [f32; 2], to: [f32; 2]) {
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
