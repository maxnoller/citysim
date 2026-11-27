//! Driving: lanes, lane changes, junction boxes, signals and priority.
//!
//! Per tick, in this order:
//! 1. Lane changes on multi-lane roads: into the lane the next turn needs
//!    (left lane for left turns, kerb lane for right turns) when close to
//!    the junction, otherwise keep right unless a slow queue ahead can be
//!    overtaken. A change needs a safe gap ahead and behind in the target
//!    lane; `lane_shift` lets renderers ease the car across.
//! 2. Signals advance (actuated by the cars approaching them).
//! 3. Junction entry: the front car of each lane that is close to its stop
//!    line asks to enter the box for its movement. It may only when
//!    * no car inside the box is on a conflicting movement,
//!    * its out-lane has room for it beyond the box (nobody enters a
//!      junction they cannot leave, so a blocked exit never blocks the box),
//!    * the control allows it: green (yellow only when too close to stop);
//!      permissive left turns and give-way approaches yield to conflicting
//!      cars with precedence that would arrive within the critical gap.
//!
//!    Cars are processed in precedence order per junction and each entry
//!    counts at once, so two conflicting cars never enter together.
//! 4. Cars in boxes follow their movement curve (and the car ahead on it)
//!    and join their out-lane at its start; cars on lanes follow the car
//!    ahead, stop at the line when not allowed in, slow for turns.
//! 5. Last resorts: a car stuck inside a box (only possible after a road
//!    edit) is moved out or removed (`deadlocks_resolved`); a car stuck in
//!    a queue for `STUCK_REMOVE_AFTER_S` gives up (`stuck_removed`).

use crate::derived::Access;
use crate::idmap::key_index;
use crate::junction::{Aspect, JunctionControl, JunctionState, Turn};
use crate::roads::{NodeId, RoadNetwork, RoadSegment, SegmentId};
use crate::traffic::{self, DT, Ended, JunctionMove, TrafficState, TripEnd, VehicleId, secs_to_ticks};
use crate::{World, balance};

/// Lanes per direction the lane index space allows (two through lanes and
/// an avenue's turn pocket).
const LANE_SLOTS: usize = 3;

/// Index of a lane: segment slot, direction, lane.
pub(crate) fn lane_key(s: SegmentId, forward: bool, lane: u8) -> usize {
    key_index(s) * 2 * LANE_SLOTS + RoadSegment::dir(forward) * LANE_SLOTS + (lane as usize).min(LANE_SLOTS - 1)
}

/// Stop lines per segment slot: `[setback at a, setback at b]`.
pub(crate) fn build_setbacks(
    roads: &RoadNetwork,
    states: &crate::side::SideTable<NodeId, JunctionState>,
) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; roads.segments.slot_count()];
    for (s, seg) in roads.segments.iter() {
        let at = |n: NodeId| {
            states
                .get(n)
                .and_then(|j| j.arm_of(s).map(|a| j.arms[a].setback))
                .unwrap_or(0.0)
        };
        out[key_index(s)] = [at(seg.a), at(seg.b)];
    }
    out
}

/// Expected wait at the junction a segment direction leads to, by its
/// control and this approach's role there (routing costs).
pub(crate) fn build_route_delays(
    roads: &RoadNetwork,
    states: &crate::side::SideTable<NodeId, JunctionState>,
) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; roads.segments.slot_count()];
    for (s, seg) in roads.segments.iter() {
        let wait = |n: NodeId| {
            let Some(j) = states.get(n).filter(|j| j.arms.len() >= 3) else {
                return 0.0;
            };
            let Some(arm) = j.arm_of(s) else { return 0.0 };
            match j.control {
                JunctionControl::Signal => {
                    let main = j
                        .plan
                        .as_ref()
                        .and_then(|p| p.phases.get(p.main as usize))
                        .is_some_and(|ph| ph.arms.contains(&(arm as u8)));
                    if main {
                        balance::ROUTE_SIGNAL_MAIN_S
                    } else {
                        balance::ROUTE_SIGNAL_SIDE_S
                    }
                }
                JunctionControl::Priority => {
                    if j.arms[arm].major {
                        balance::ROUTE_MAJOR_S
                    } else if j.arms.iter().any(|a| a.major) {
                        balance::ROUTE_GIVE_WAY_S
                    } else {
                        balance::ROUTE_ALL_WAY_S
                    }
                }
            }
        };
        // Travel a -> b waits at b, b -> a at a.
        out[key_index(s)] = [wait(seg.b), wait(seg.a)];
    }
    out
}

/// Extra routing cost of every turn at every junction (see
/// `Derived::route_turns`): drivers avoid turning left across a busy road
/// (the more oncoming lanes, the more so) and crossing a major road from a
/// minor one; at an all-way stop every way costs the same.
pub(crate) fn build_route_turns(
    roads: &RoadNetwork,
    states: &crate::side::SideTable<NodeId, JunctionState>,
) -> Vec<Vec<f32>> {
    let mut out = vec![Vec::new(); roads.nodes.slot_count()];
    for (n, node) in roads.nodes.iter() {
        let deg = node.segments.len();
        let Some(j) = states.get(n).filter(|j| j.arms.len() >= 3 && j.arms.len() == deg) else {
            continue;
        };
        let any_major = j.arms.iter().any(|a| a.major);
        let mut t = vec![0.0f32; deg * deg];
        for (pi, &si) in node.segments.iter().enumerate() {
            let Some(a) = j.arm_of(si) else { continue };
            let arm = &j.arms[a];
            let mut oncoming = arm.opposite.map_or(1.0, |o| j.arms[o as usize].lanes_in as f32);
            // A left turn waiting in a pocket (with its own phase at
            // signals) costs less.
            if j.movements
                .iter()
                .any(|m| m.from_arm as usize == a && m.from_lane >= arm.lanes_in)
            {
                oncoming *= balance::ROUTE_POCKET_LEFT_SHARE;
            }
            let major_lanes: f32 = j
                .arms
                .iter()
                .filter(|x| x.major)
                .map(|x| x.lanes_in as f32)
                .sum::<f32>()
                * 0.5;
            for (po, &so) in node.segments.iter().enumerate() {
                let Some(b) = j.arm_of(so) else { continue };
                let turn = j.movements_between(a, b).next().map(|(_, m)| m.turn);
                let c = match (j.control, turn) {
                    (_, None) => 0.0,
                    (JunctionControl::Signal, Some(Turn::Left)) => balance::ROUTE_LEFT_PER_LANE_S * oncoming,
                    (JunctionControl::Signal, Some(Turn::Right)) => balance::ROUTE_RIGHT_S,
                    (JunctionControl::Signal, _) => 0.0,
                    (JunctionControl::Priority, _) if !any_major => 0.0,
                    (JunctionControl::Priority, Some(turn)) => match (arm.major, turn) {
                        (true, Turn::Left) => balance::ROUTE_LEFT_PER_LANE_S * 0.5 * oncoming,
                        (true, _) => 0.0,
                        (false, Turn::Right) => balance::ROUTE_RIGHT_S,
                        (false, Turn::Left) => balance::ROUTE_LEFT_PER_LANE_S * major_lanes,
                        (false, _) => balance::ROUTE_LEFT_PER_LANE_S * 0.5 * major_lanes,
                    },
                };
                t[pi * deg + po] = c;
            }
        }
        out[key_index(n)] = t;
    }
    out
}

/// Turn pocket lengths per segment slot (`junction::pocket_lengths`).
pub(crate) fn build_pockets(
    roads: &RoadNetwork,
    states: &crate::side::SideTable<NodeId, JunctionState>,
) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; roads.segments.slot_count()];
    for (s, seg) in roads.segments.iter() {
        if seg.kind.pocket_lane().is_some() {
            out[key_index(s)] = crate::junction::pocket_lengths(roads, states, s);
        }
    }
    out
}

/// Where the turn pocket of segment `s` in direction `forward` opens, as
/// progress along travel, given its stop line (`None`: no pocket).
fn pocket_start(pockets: &[[f32; 2]], s: SegmentId, forward: bool, stop: f32) -> Option<f32> {
    let len = pockets.get(key_index(s))?[usize::from(!forward)];
    (len > 0.0).then_some(stop - len)
}

/// Make sure the stop lines and routing delays match the junctions.
pub(crate) fn ensure_tables(world: &mut World) {
    if world.cache.setbacks_key != Some(world.traffic.junctions_version) {
        world.cache.setbacks = build_setbacks(&world.roads, &world.traffic.junction_states);
        world.cache.pockets = build_pockets(&world.roads, &world.traffic.junction_states);
        world.cache.setbacks_key = Some(world.traffic.junctions_version);
    }
    let key = (world.traffic.junctions_version, world.traffic.control_version);
    if world.cache.route_delay_key != Some(key) {
        world.cache.route_delay = build_route_delays(&world.roads, &world.traffic.junction_states);
        world.cache.route_turns = build_route_turns(&world.roads, &world.traffic.junction_states);
        world.cache.route_delay_key = Some(key);
    }
}

/// Drivable stretch of a segment direction, metres from the node it is
/// entered by: from the end of the box behind to the stop line ahead.
pub(crate) fn lane_range(setbacks: &[[f32; 2]], s: SegmentId, forward: bool, len: f32) -> (f32, f32) {
    let [at_a, at_b] = setbacks.get(key_index(s)).copied().unwrap_or([0.0, 0.0]);
    let (start, end) = if forward { (at_a, at_b) } else { (at_b, at_a) };
    let stop = len - end;
    if start <= stop {
        (start, stop)
    } else {
        let m = ((start + stop) * 0.5).clamp(0.0, len);
        (m, m)
    }
}

/// Drivable stretch of segment `s` in direction `forward` (public view of
/// the stop lines: `(start, stop)` metres along the direction of travel).
pub fn drivable_range(world: &World, s: SegmentId, forward: bool) -> Option<(f32, f32)> {
    let seg = world.roads.segments.get(s)?;
    let at = |n: NodeId| crate::junction::setback(world, n, s);
    let (a, b) = (at(seg.a), at(seg.b));
    let sb = [[a, b]];
    // A one-entry table indexed like the real one.
    let mut table = vec![[0.0f32; 2]; key_index(s) + 1];
    table[key_index(s)] = sb[0];
    Some(lane_range(&table, s, forward, seg.length))
}

/// How far before a stop line a waiting vehicle's centre stands: half its
/// body plus `STOP_LINE_M` (the front bumper stays behind the line).
pub fn stop_back_m(truck: bool) -> f32 {
    let body = if truck {
        balance::TRUCK_BODY_M[0]
    } else {
        balance::CAR_BODY_M[0]
    };
    body * 0.5 + balance::STOP_LINE_M
}

fn is_truck(trip: Option<&traffic::Trip>) -> bool {
    trip.is_some_and(|t| t.purpose == traffic::TripPurpose::Goods)
}

/// Bits of a float that sort like the float (`total_cmp` order).
fn ordered_bits(x: f32) -> u32 {
    let b = x.to_bits();
    if b >> 31 == 1 { !b } else { b | 0x8000_0000 }
}

fn follow_speed(gap: f32, headway: f32) -> f32 {
    ((gap - balance::MIN_GAP_M) / headway).max(0.0)
}

/// Time headway drivers keep on a road of this kind
/// (`balance::FOLLOW_HEADWAY_S`): narrow streets lined with driveways are
/// driven with more distance than avenues.
fn headway_of(kind: crate::RoadKind) -> f32 {
    match kind {
        crate::RoadKind::Street => balance::STREET_FOLLOW_HEADWAY_S,
        _ => balance::FOLLOW_HEADWAY_S,
    }
}

fn stop_speed(d: f32) -> f32 {
    let d = d.max(0.0);
    (2.0 * balance::BRAKING * d).sqrt().min(d / DT)
}

fn turn_speed(turn: Turn, exit_limit: f32) -> f32 {
    match turn {
        Turn::Straight => exit_limit,
        Turn::Right | Turn::Left => balance::JUNCTION_SPEED,
        Turn::UTurn => balance::UTURN_SPEED,
    }
}

/// Seconds to cover `d` metres starting at `v` m/s, accelerating up to
/// `vmax` (a car pulling away in a queue gets there sooner than its speed
/// says).
fn arrival_s(d: f32, v: f32, vmax: f32) -> f32 {
    let d = d.max(0.0);
    let a = balance::ACCELERATION;
    let vmax = vmax.max(v).max(0.1);
    let ta = (vmax - v) / a;
    let da = (v + vmax) * 0.5 * ta;
    if d <= da {
        ((v * v + 2.0 * a * d).sqrt() - v) / a
    } else {
        ta + (d - da) / vmax
    }
}

/// Speed factor of this driver (`1 ± DRIVER_SPEED_VARIATION`).
fn driver(seed: u32) -> f32 {
    1.0 + balance::DRIVER_SPEED_VARIATION * ((seed % 1001) as f32 / 500.0 - 1.0)
}

/// The next step of a vehicle's route at the end of its segment.
#[derive(Clone, Copy)]
struct Step {
    node: NodeId,
    to: SegmentId,
    to_forward: bool,
}

fn next_step(roads: &RoadNetwork, seg_id: SegmentId, forward: bool, route: &[SegmentId]) -> Option<Step> {
    let seg = roads.segments.get(seg_id)?;
    let node = if forward { seg.b } else { seg.a };
    let to = *route.first()?;
    let ns = roads.segments.get(to)?;
    let to_forward = if ns.a == node {
        true
    } else if ns.b == node {
        false
    } else {
        return None;
    };
    Some(Step { node, to, to_forward })
}

/// The turn from arm `a` to arm `b` and the lanes (bit set) drivers make
/// it from (`Movement::preferred`): the turn pocket alone when the
/// approach has one (`pocket`), else the through lanes' preferred ones.
fn turn_lanes(j: &JunctionState, a: usize, b: usize, pocket: bool) -> Option<(Turn, u8)> {
    let lanes = j.arms.get(a)?.lanes_in;
    let (mut mask, mut pocket_mask) = (0u8, 0u8);
    let mut turn = None;
    for (_, m) in j.movements_between(a, b) {
        turn = Some(m.turn);
        if !m.preferred || m.from_lane >= 8 {
            continue;
        }
        if m.from_lane >= lanes {
            if pocket {
                pocket_mask |= 1 << m.from_lane;
            }
        } else {
            mask |= 1 << m.from_lane;
        }
    }
    let turn = turn?;
    let mask = if pocket_mask != 0 { pocket_mask } else { mask };
    Some((turn, if mask == 0 { u8::MAX } else { mask }))
}

/// The turn a vehicle makes at the end of its segment and the lanes (bit
/// set) drivers make it from (see [`turn_lanes`]).
fn next_turn(
    roads: &RoadNetwork,
    traffic: &TrafficState,
    pockets: &[[f32; 2]],
    v: &traffic::Vehicle,
) -> Option<(Turn, u8)> {
    let s = v.segment?;
    let st = next_step(roads, s, v.forward, &v.route)?;
    let j = traffic.junction_states.get(st.node)?;
    let (a, b) = (j.arm_of(s)?, j.arm_of(st.to)?);
    let pocket = pockets
        .get(key_index(s))
        .is_some_and(|p| p[usize::from(!v.forward)] > 0.0);
    turn_lanes(j, a, b, pocket)
}

/// The front car of a lane, about to reach a junction.
#[derive(Clone, Copy)]
struct Front {
    /// Index into the sorted lane list.
    idx: usize,
    node: NodeId,
    arm: u8,
    movement: usize,
    turn: Turn,
    d_line: f32,
    /// Where this vehicle stops before the line (`stop_back_m`).
    back: f32,
    speed: f32,
    eta: f32,
    wait: u32,
    /// Asked to go last tick (others yield to it).
    go_before: bool,
    to: SegmentId,
    to_forward: bool,
    to_lane: u8,
    exit_key: usize,
    exit_start: f32,
    exit_stop: f32,
    /// Allowed to enter (decided this tick).
    permit: bool,
    /// Refused only because a conflicting car is still in the box (it goes
    /// as soon as that one is past): cars of lower precedence wait for it.
    held_by_box: bool,
    /// Reaches the stop line this tick.
    reaches: bool,
    /// Cars in this lane when it is a turn pocket (0 otherwise).
    pocket_cars: u16,
    /// A minor-road car let in by courtesy: it has waited
    /// `COURTESY_WAIT_S` at its line and every conflicting car with
    /// priority is creeping (a slow queue lets one in).
    courtesy: bool,
}

/// Has the front car come to a stop at its line (all-way stops: it stood
/// there for `STOP_DWELL_S`)?
fn at_stop(f: &Front) -> bool {
    f.speed < balance::STOP_SIGN_SPEED
        && f.d_line < f.back + balance::STOP_SIGN_DISTANCE_M
        && f.wait >= (balance::STOP_DWELL_S * crate::TICKS_PER_SECOND as f32) as u32
}

/// Precedence class of a front (higher goes first, lower yields).
fn class(j: &JunctionState, f: &Front) -> u8 {
    let left = matches!(f.turn, Turn::Left | Turn::UTurn);
    match j.control {
        JunctionControl::Signal => {
            if j.protected(f.movement) {
                3
            } else {
                1
            }
        }
        JunctionControl::Priority => {
            let major = j.arms.get(f.arm as usize).is_some_and(|a| a.major);
            if !j.arms.iter().any(|a| a.major) {
                // All-way stop: everyone alike.
                return 1;
            }
            if f.courtesy {
                return 3;
            }
            match (major, left) {
                (true, false) => 3,
                (true, true) => 2,
                (false, false) => 1,
                (false, true) => 0,
            }
        }
    }
}

/// Critical gap a car of class `mine` leaves to a conflicting car of
/// class `theirs`, seconds.
fn gap_s(j: &JunctionState, mine: u8, theirs: u8) -> f32 {
    if theirs < mine {
        return 0.0;
    }
    match j.control {
        JunctionControl::Signal => balance::PERMISSIVE_GAP_S,
        JunctionControl::Priority if theirs > mine => {
            if mine <= 1 {
                balance::GIVE_WAY_GAP_S
            } else {
                balance::PERMISSIVE_GAP_S
            }
        }
        // Same class: first come, first served.
        JunctionControl::Priority => balance::PERMISSIVE_GAP_S,
    }
}

pub(crate) fn step(world: &mut World, ended: &mut Vec<Ended>) {
    let now = world.time.tick;
    // Stop lines for the current junction geometry.
    ensure_tables(world);

    // Vehicles that lost their road.
    let lost: Vec<VehicleId> = world
        .vehicles
        .iter()
        .filter(|(_, v)| v.segment.is_none_or(|s| !world.roads.segments.contains_key(s)))
        .map(|(id, _)| id)
        .collect();
    for id in lost {
        traffic::remove_vehicle(world, id, TripEnd::Vanished, ended);
    }

    // Lane choices are made every other tick (a car decides a twentieth
    // of a second later; half the cost).
    if now.is_multiple_of(2) {
        lane_changes(world);
    }

    let World {
        roads,
        vehicles,
        traffic,
        cache,
        ..
    } = world;
    let roads: &RoadNetwork = roads;
    let traffic: &mut TrafficState = traffic;
    let setbacks_vec: Vec<[f32; 2]> = std::mem::take(&mut cache.setbacks);
    let setbacks: &[[f32; 2]] = &setbacks_vec;
    let pockets_vec: Vec<[f32; 2]> = std::mem::take(&mut cache.pockets);
    let pockets: &[[f32; 2]] = &pockets_vec;
    let nkeys = roads.segments.slot_count() * 2 * LANE_SLOTS;

    // Lane lists (front first) and box lists.
    let mut cars: Vec<(u32, f32, VehicleId)> = Vec::with_capacity(vehicles.len());
    let mut boxed: Vec<(u32, u16, f32, VehicleId)> = Vec::new();
    for (id, v) in vehicles.iter() {
        let s = v.segment.expect("checked");
        match &v.junction {
            Some(jm) => boxed.push((key_index(jm.node) as u32, jm.movement, jm.distance, id)),
            None => cars.push((lane_key(s, v.forward, v.lane) as u32, v.progress, id)),
        }
    }
    // By lane, front first (descending progress), then id: one integer key.
    cars.sort_unstable_by_key(|c| (((c.0 as u64) << 32) | !ordered_bits(c.1) as u64, c.2));
    boxed.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(b.2.total_cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    let mut rear = vec![f32::INFINITY; nkeys];
    let mut rear_speed = vec![0.0f32; nkeys];
    let mut on_lane = vec![0u16; nkeys];
    // Progress of the front car of each lane.
    let mut lane_front = vec![f32::NEG_INFINITY; nkeys];
    for &(k, p, id) in &cars {
        on_lane[k as usize] = on_lane[k as usize].saturating_add(1);
        lane_front[k as usize] = lane_front[k as usize].max(p);
        let r = &mut rear[k as usize];
        if p < *r {
            *r = p;
            rear_speed[k as usize] = vehicles[id].speed;
        }
    }
    // Cars inside boxes by the lane they came from (key, distance in).
    let mut box_from: Vec<(usize, f32)> = boxed
        .iter()
        .map(|b| {
            let v = &vehicles[b.3];
            let jm = v.junction.expect("boxed");
            (lane_key(jm.from, v.forward, jm.from_lane), jm.distance)
        })
        .collect();
    box_from.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    // Cars inside boxes heading for each out-lane.
    let mut inbox = vec![0u8; nkeys];
    for &(_, _, _, id) in &boxed {
        let jm = vehicles[id].junction.expect("boxed");
        let k = lane_key(jm.to, jm.to_forward, jm.to_lane);
        inbox[k] = inbox[k].saturating_add(1);
    }

    // Fronts close to their stop line.
    let mut fronts: Vec<Front> = Vec::new();
    for (i, &(k, p, id)) in cars.iter().enumerate() {
        if i > 0 && cars[i - 1].0 == k {
            continue;
        }
        let v = &vehicles[id];
        let s = v.segment.expect("checked");
        let seg = &roads.segments[s];
        let (_, stop) = lane_range(setbacks, s, v.forward, seg.length);
        let d_line = (stop - p).max(0.0);
        if d_line > balance::DECIDE_DISTANCE_M.max(v.speed * v.speed / (2.0 * balance::BRAKING) + 5.0) {
            continue;
        }
        let trip = traffic.trips.get(id);
        if trip.is_some_and(|t| t.needs_route) {
            continue;
        }
        let Some(st) = next_step(roads, s, v.forward, &v.route) else {
            continue;
        };
        let Some(j) = traffic.junction_states.get(st.node) else {
            continue;
        };
        let (Some(a), Some(b)) = (j.arm_of(s), j.arm_of(st.to)) else {
            continue;
        };
        let Some(m) = j.movement(a, v.lane, b) else { continue };
        let mv = &j.movements[m];
        let to_len = roads.segments[st.to].length;
        let (exit_start, exit_stop) = lane_range(setbacks, st.to, st.to_forward, to_len);
        let reach = (v.speed + balance::ACCELERATION * DT) * DT + 0.02;
        fronts.push(Front {
            idx: i,
            node: st.node,
            arm: a as u8,
            movement: m,
            turn: mv.turn,
            d_line,
            back: stop_back_m(is_truck(trip)),
            speed: v.speed,
            eta: arrival_s(d_line, v.speed, seg.speed_limit),
            wait: trip.map_or(0, |t| t.wait_ticks),
            go_before: v.go,
            to: st.to,
            to_forward: st.to_forward,
            to_lane: mv.to_lane,
            exit_key: lane_key(st.to, st.to_forward, mv.to_lane),
            exit_start,
            exit_stop,
            permit: false,
            held_by_box: false,
            reaches: d_line <= reach,
            pocket_cars: if seg.kind.pocket_lane().is_some_and(|pl| v.lane >= pl) {
                on_lane[k as usize]
            } else {
                0
            },
            courtesy: false,
        });
    }
    fronts.sort_by_key(|f| (key_index(f.node), f.idx));
    let mut front_of = vec![usize::MAX; cars.len()];
    for (p, f) in fronts.iter().enumerate() {
        front_of[f.idx] = p;
    }

    // Signals. A car whose exit lane is jammed (its last car standing with
    // no room behind it) does not call for green: the light serves those
    // who can go instead of showing green to a queue that cannot move.
    let can_go: Vec<bool> = fronts
        .iter()
        .map(|f| {
            let r = rear[f.exit_key];
            !(r.is_finite()
                && rear_speed[f.exit_key] < balance::WAIT_SPEED
                && r - f.exit_start < balance::MIN_GAP_M + balance::VEHICLE_SPACING_M)
        })
        .collect();
    // Cars within detection of the line in each front's lane (a platoon
    // holds its green; a car or two arriving does not against a queue).
    let near_cars: Vec<u16> = fronts
        .iter()
        .map(|f| {
            let (k, p, _) = cars[f.idx];
            let stop = p + f.d_line;
            cars[f.idx..]
                .iter()
                .take_while(|c| c.0 == k && stop - c.1 <= balance::SIGNAL_DETECT_M)
                .count()
                .min(u16::MAX as usize) as u16
        })
        .collect();
    tick_signals(traffic, roads, &fronts, &can_go, &near_cars, &boxed, now);
    // Lanes whose front car waits at a red light: waiting there is normal
    // (a red can last a whole cycle), so it does not count towards giving
    // up.
    let mut held_by_red = vec![false; nkeys];

    // Entry decisions per junction.
    let mut b0 = 0;
    let mut k = 0;
    while k < fronts.len() {
        let node = fronts[k].node;
        let slot = key_index(node) as u32;
        let mut e = k;
        while e < fronts.len() && fronts[e].node == node {
            e += 1;
        }
        while b0 < boxed.len() && boxed[b0].0 < slot {
            b0 += 1;
        }
        let mut b1 = b0;
        while b1 < boxed.len() && boxed[b1].0 == slot {
            b1 += 1;
        }
        let denials = &mut cache.denials;
        let j = traffic.junction_states.get_mut(node).expect("front has a junction");
        j.ensure_clear();
        // (movement, lane the car really came from, distance along)
        let mut occupied: Vec<(usize, usize, f32)> = boxed[b0..b1]
            .iter()
            .map(|b| {
                let v = &vehicles[b.3];
                let jm = v.junction.expect("boxed");
                (b.1 as usize, lane_key(jm.from, v.forward, jm.from_lane), jm.distance)
            })
            .collect();
        // Conflict with a car in the box: crossing / merging paths it has
        // not yet passed, or the same lane's paths taken by a car from
        // another lane (a car that could not reach its turning lane borrows
        // that lane's path).
        let clash = |j: &JunctionState, m: usize, lane: usize, o: (usize, usize, f32)| {
            j.blocks(o.0, o.2, m) || {
                let (a, b) = (&j.movements[m], &j.movements[o.0]);
                (a.from_arm, a.from_lane) == (b.from_arm, b.from_lane) && lane != o.1
            }
        };
        // Precedence order: class, then who was already let go, then the
        // platoon being served (a busy stream keeps going for a few cars
        // before the junction switches), then first come, first served.
        let in_batch = |f: &Front| {
            j.control == JunctionControl::Priority
                && j.phase_count < balance::PHASE_MAX_CARS
                && (f.arm == j.last_served
                    || j.arms.get(f.arm as usize).and_then(|a| a.opposite) == Some(j.last_served))
        };
        let all_way = j.control == JunctionControl::Priority && !j.arms.iter().any(|a| a.major);
        // All-way stops: the next driver sets off a moment after the box
        // empties (`clear_at`: last tick anyone was in it).
        if !occupied.is_empty() {
            j.clear_at = now;
        }
        let reacted = now >= j.clear_at + secs_to_ticks(balance::ALL_WAY_REACTION_S);
        // Courtesy: a minor-road car that has waited long at its line is let
        // in when every conflicting car with priority is creeping (on an
        // arterial: a queue on a street leaves gaps of its own).
        let arterial_major = j
            .arms
            .iter()
            .any(|a| a.major && a.kind.rank() >= crate::RoadKind::Avenue.rank());
        if j.control == JunctionControl::Priority && !all_way && arterial_major {
            let long = secs_to_ticks(balance::COURTESY_WAIT_S) as u32;
            for x in k..e {
                let f = &fronts[x];
                if f.wait < long || f.d_line > f.back + 1.0 || class(j, f) >= 2 {
                    continue;
                }
                let creeping = (k..e).all(|y| {
                    let o = &fronts[y];
                    y == x
                        || !j.conflict(f.movement, o.movement)
                        || class(j, o) <= class(j, f)
                        || o.speed < balance::COURTESY_SPEED
                });
                fronts[x].courtesy = creeping;
            }
        }
        let mut order: Vec<usize> = (k..e).collect();
        order.sort_by(|&x, &y| {
            let (fx, fy) = (&fronts[x], &fronts[y]);
            if all_way {
                // All-way stop: whoever has stood at the line longest goes
                // first (cars already let go keep their turn).
                return fy
                    .go_before
                    .cmp(&fx.go_before)
                    .then(at_stop(fy).cmp(&at_stop(fx)))
                    .then(fy.wait.cmp(&fx.wait))
                    .then(fx.eta.total_cmp(&fy.eta))
                    .then(fx.idx.cmp(&fy.idx));
            }
            // Committed first: a car let go last tick is pulling up to its
            // line and keeps its turn (it stops behind the line, so it needs
            // a moment to get there; taking the turn back would stall it).
            // Two cars both let go (side by side, merging): the one nearer
            // its line keeps going first (a key that does not flip while
            // they creep, unlike the arrival time).
            let both_go = fx.go_before && fy.go_before;
            fy.go_before
                .cmp(&fx.go_before)
                .then(class(j, fy).cmp(&class(j, fx)))
                .then(if both_go {
                    (fx.d_line - fx.back).total_cmp(&(fy.d_line - fy.back))
                } else {
                    std::cmp::Ordering::Equal
                })
                .then(in_batch(fy).cmp(&in_batch(fx)))
                .then(fx.eta.total_cmp(&fy.eta))
                .then(fy.wait.cmp(&fx.wait))
                .then(fx.idx.cmp(&fy.idx))
        });
        for (rank, &fi) in order.iter().enumerate() {
            let f = fronts[fi];
            let my_lane = cars[f.idx].0 as usize;
            let mut ok = !occupied.iter().any(|&o| clash(j, f.movement, my_lane, o));
            let box_blocked = !ok;
            // A car that could not reach its turning lane takes that lane's
            // path: only while no car waits at that lane's stop line.
            let mv_lane = j.movements[f.movement].from_lane;
            let car = &vehicles[cars[f.idx].2];
            if ok && mv_lane != car.lane {
                let other = lane_key(car.segment.expect("on a lane"), car.forward, mv_lane);
                let stop = car.progress + f.d_line;
                if lane_front[other] > stop - stop_back_m(false) - balance::MIN_GAP_M {
                    ok = false;
                }
            }
            // Room beyond the box.
            if ok {
                let mlen = j.movements[f.movement].length;
                ok = exit_room(roads, &f, mlen, &rear, &rear_speed, &on_lane, &inbox);
            }
            if ok {
                ok = match j.control {
                    JunctionControl::Signal => match j.movement_aspect(f.movement) {
                        Some(Aspect::Green) => true,
                        Some(Aspect::Yellow) => {
                            (f.speed * f.speed / (2.0 * balance::BRAKING) > f.d_line + 0.5 && f.speed > 1.0)
                                // A permissive left turner waiting at the
                                // line clears out at the end of green.
                                || (matches!(f.turn, Turn::Left)
                                    && !j.protected(f.movement)
                                    && f.d_line < f.back + 1.0
                                    && f.speed < balance::STOP_SIGN_SPEED)
                        }
                        _ => false,
                    },
                    // All-way stop (no major road): every car halts at
                    // the line before it goes; once let go it pulls up to
                    // the line and in (it stands a little back from it).
                    JunctionControl::Priority => {
                        j.arms.iter().any(|a| a.major)
                            || f.go_before
                            || (at_stop(&f) && (reacted || !occupied.is_empty()))
                    }
                };
            }
            if ok {
                // Yield to conflicting cars with precedence arriving soon.
                let mine = class(j, &f);
                for &oi in &order[..rank] {
                    let o = &fronts[oi];
                    if !j.conflict(f.movement, o.movement) {
                        continue;
                    }
                    // All-way stop: nobody jumps the turn of a conflicting
                    // car that has waited longer, even while the box keeps
                    // it waiting.
                    if all_way && at_stop(o) && !f.go_before {
                        ok = false;
                        break;
                    }
                    if j.control == JunctionControl::Signal && j.movement_aspect(o.movement) != Some(Aspect::Green) {
                        continue;
                    }
                    let theirs = class(j, o);
                    // A car with precedence waiting at its line only for the
                    // box to clear goes next: nobody of lower precedence
                    // slips in ahead of it (a chain of permissive left turns
                    // would otherwise starve the oncoming stream).
                    if o.held_by_box && theirs > mine && o.d_line < o.back + 1.5 {
                        ok = false;
                        break;
                    }
                    if !(o.go_before || o.permit) {
                        continue;
                    }
                    let gap = gap_s(j, mine, theirs);
                    // Earlier in the order and arriving before this one
                    // could clear; a car already let go ahead of it always.
                    let blocks = (o.go_before && f.go_before) || o.eta < f.eta + gap;
                    if blocks {
                        ok = false;
                        break;
                    }
                }
            }
            fronts[fi].permit = ok;
            fronts[fi].held_by_box = box_blocked && !ok;
            if !ok && j.control == JunctionControl::Signal && j.movement_aspect(f.movement) != Some(Aspect::Green) {
                held_by_red[my_lane] = true;
            }
            if ok {
                let vs = key_index(cars[f.idx].2);
                if let Some(x) = cache.last_denial.get_mut(vs) {
                    *x = 255;
                }
            }
            if !ok && f.d_line < f.back + 1.0 {
                let reason = if j.control == JunctionControl::Signal
                    && !matches!(j.movement_aspect(f.movement), Some(Aspect::Green | Aspect::Yellow))
                {
                    2
                } else if occupied.iter().any(|&o| clash(j, f.movement, my_lane, o)) {
                    0
                } else {
                    let room = rear[f.exit_key] - f.exit_start;
                    if rear[f.exit_key].is_finite()
                        && room < balance::VEHICLE_SPACING_M * inbox[f.exit_key] as f32 + balance::MIN_GAP_M
                    {
                        1
                    } else {
                        3
                    }
                };
                denials[reason] += 1;
                let slot = key_index(node);
                if cache.node_denials.len() <= slot {
                    cache.node_denials.resize(slot + 1, [0; 4]);
                }
                cache.node_denials[slot][reason] += 1;
                let vs = key_index(cars[f.idx].2);
                if cache.last_denial.len() <= vs {
                    cache.last_denial.resize(vs + 1, 255);
                }
                cache.last_denial[vs] = reason as u8;
            }
            if ok && f.reaches {
                occupied.push((f.movement, my_lane, 0.0));
                inbox[f.exit_key] = inbox[f.exit_key].saturating_add(1);
            }
        }
        let _ = j;
        b0 = b1;
        k = e;
    }

    // Move cars inside boxes.
    let mut done: Vec<(VehicleId, TripEnd)> = Vec::new();
    let mut i = 0;
    while i < boxed.len() {
        let (slot, mv, _, _) = boxed[i];
        let mut lead: Option<f32> = None;
        while i < boxed.len() && boxed[i].0 == slot && boxed[i].1 == mv {
            let id = boxed[i].3;
            let v = &mut vehicles[id];
            let mut jm = v.junction.expect("boxed");
            let Some(j) = traffic.junction_states.get(jm.node) else {
                i += 1;
                continue;
            };
            let Some(m) = j.movements.get(jm.movement as usize) else {
                i += 1;
                continue;
            };
            let exit_seg = &roads.segments[jm.to];
            let (exit_start, _) = lane_range(setbacks, jm.to, jm.to_forward, exit_seg.length);
            let mut target = turn_speed(m.turn, exit_seg.speed_limit) * driver(v.seed);
            let ek = lane_key(jm.to, jm.to_forward, jm.to_lane);
            let hw = headway_of(exit_seg.kind);
            match lead {
                Some(d) => target = target.min(follow_speed(d - jm.distance, hw)),
                None => {
                    if rear[ek].is_finite() {
                        target = target.min(follow_speed(m.length - jm.distance + rear[ek] - exit_start, hw));
                    }
                }
            }
            let speed = (v.speed + balance::ACCELERATION * DT).min(target).max(0.0);
            v.braking = speed + 0.01 < v.speed;
            v.speed = speed;
            jm.distance += speed * DT;
            if jm.distance >= m.length {
                let over = jm.distance - m.length;
                v.junction = None;
                v.segment = Some(jm.to);
                v.forward = jm.to_forward;
                v.lane = jm.to_lane;
                v.lane_shift = 0.0;
                let mut p = exit_start + over;
                if rear[ek].is_finite() {
                    p = p.min((rear[ek] - balance::MIN_GAP_M).max(exit_start));
                }
                v.progress = p.min(exit_seg.length);
                if v.route.first() == Some(&jm.to) {
                    v.route.remove(0);
                }
                traffic::set_pose(roads, v);
                rear[ek] = rear[ek].min(v.progress);
                lead = None;
            } else {
                let (p, d) = m.pose_at(jm.distance);
                v.pos = p;
                v.heading = libm::atan2f(d[1], d[0]);
                v.junction = Some(jm);
                lead = Some(jm.distance);
            }
            i += 1;
        }
    }

    // Cars still easing out of a lane, by that lane: followers there keep
    // their distance until the change is done.
    let mut ghosts: Vec<(u32, f32)> = cars
        .iter()
        .filter_map(|&(_, p, id)| {
            let v = &vehicles[id];
            if v.lane_shift.abs() <= 0.05 {
                return None;
            }
            let old = if v.lane_shift > 0.0 {
                v.lane.saturating_add(1)
            } else {
                v.lane.saturating_sub(1)
            };
            Some((lane_key(v.segment?, v.forward, old) as u32, p))
        })
        .collect();
    // Zip merging: a slow car that must change lanes for its next turn
    // (a standing queue leaves no gap) asks for one; the car behind it in
    // the lane it needs holds back a car's length so it can move over.
    for &(_, p, id) in &cars {
        let v = &vehicles[id];
        if v.speed >= balance::MERGE_ASK_SPEED || v.lane_shift.abs() > 0.05 {
            continue;
        }
        let Some(s) = v.segment else { continue };
        let seg = &roads.segments[s];
        if seg.lane_count() < 2 {
            continue;
        }
        let (start, stop) = lane_range(setbacks, s, v.forward, seg.length);
        let d_line = stop - p;
        if !(balance::MERGE_MIN_LINE_M..=balance::LANE_PREPARE_M).contains(&d_line)
            || p < start + balance::NO_LANE_CHANGE_AFTER_BOX_M
        {
            continue;
        }
        // (Towards a turn pocket: the inner lane first.)
        let n = seg.lane_count();
        if let Some((_, mask)) = next_turn(roads, traffic, pockets, v)
            && mask & (1 << v.lane.min(7)) == 0
            && let Some(want) = (0..n)
                .filter(|l| mask & (1 << l) != 0)
                .min_by_key(|l| l.abs_diff(v.lane))
                .or_else(|| (mask & !((1u8 << n) - 1) != 0 && v.lane + 1 < n).then(|| v.lane + 1))
        {
            ghosts.push((lane_key(s, v.forward, want) as u32, p - balance::MERGE_ROOM_M));
        }
    }
    ghosts.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

    // Move cars on lanes, lane by lane, front to back.
    let mut entered: Vec<VehicleId> = Vec::new();
    let mut i = 0;
    while i < cars.len() {
        let key = cars[i].0;
        let mut leader: Option<f32> = None;
        while i < cars.len() && cars[i].0 == key {
            let id = cars[i].2;
            let front = fronts.get(front_of[i]).copied();
            let v = &mut vehicles[id];
            let s = v.segment.expect("checked");
            let seg = &roads.segments[s];
            let len = seg.length;
            let (_, stop) = lane_range(setbacks, s, v.forward, len);
            // Congestion shows up through car following and the stop lines;
            // the volume-delay curve is a routing estimate only (applied
            // here too it slowed every car on a segment with a queue at its
            // far end).
            let mut target = seg.speed_limit * driver(v.seed);
            let hw = headway_of(seg.kind);
            let trip = traffic.trips.get(id);
            let mut arrive_at: Option<f32> = None;
            if v.route.is_empty() {
                let dest: Option<Access> = trip.and_then(|t| if t.needs_route { None } else { t.dest });
                match dest {
                    Some(d) if d.segment == s => {
                        let at = if v.forward { d.from_a } else { len - d.from_a };
                        arrive_at = Some(at.clamp(v.progress.min(stop), stop));
                    }
                    _ if trip.is_none() => arrive_at = Some(stop),
                    _ => {}
                }
            }
            let mut enter: Option<Front> = None;
            // Half way through a lane change the car is still partly in the
            // lane it left: keep the distance to the car ahead there too.
            if v.lane_shift.abs() > 0.05 {
                let old = if v.lane_shift > 0.0 {
                    v.lane.saturating_add(1)
                } else {
                    v.lane.saturating_sub(1)
                };
                let ok = lane_key(s, v.forward, old) as u32;
                let lo = cars.partition_point(|c| c.0 < ok);
                let hi = cars.partition_point(|c| c.0 <= ok);
                let mut ahead: Option<f32> = None;
                for c in &cars[lo..hi] {
                    if c.1 > v.progress {
                        ahead = Some(c.1);
                    } else {
                        break;
                    }
                }
                if let Some(p) = ahead {
                    target = target.min(follow_speed(p - v.progress, hw));
                }
            }
            {
                let lo = ghosts.partition_point(|g| g.0 < key || (g.0 == key && g.1 <= v.progress));
                if let Some(&(gk, gp)) = ghosts.get(lo)
                    && gk == key
                {
                    target = target.min(follow_speed(gp - v.progress, hw));
                }
            }
            // A left turner in the inner lane that has not got into its
            // turn pocket (full) waits at the pocket's entry: the queue
            // spills back into the inner lane.
            if let Some(pl) = seg.kind.pocket_lane()
                && v.lane + 1 == pl
                && arrive_at.is_none()
                && let Some(p0) = pocket_start(pockets, s, v.forward, stop)
            {
                let entry = p0 + balance::POCKET_ENTRY_M;
                if v.progress <= entry + 0.5
                    && entry - v.progress < 40.0
                    && next_turn(roads, traffic, pockets, v).is_some_and(|(_, m)| m & (1 << pl) != 0)
                {
                    target = target.min(stop_speed(entry - v.progress));
                }
            }
            if let Some(lp) = leader {
                target = target.min(follow_speed(lp - v.progress, hw));
                if let Some(a) = arrive_at {
                    target = target.min(stop_speed(a - v.progress + 0.3));
                }
            } else if let Some(a) = arrive_at {
                target = target.min(stop_speed(a - v.progress + 0.3));
            } else if let Some(f) = front {
                let j = traffic.junction_states.get(f.node).expect("front junction");
                let m = &j.movements[f.movement];
                let tv = turn_speed(f.turn, roads.segments[f.to].speed_limit);
                target = target.min((tv * tv + 2.0 * balance::BRAKING * f.d_line).sqrt());
                if f.permit {
                    // The nearest car ahead from this lane inside the box,
                    // else the rear of the out-lane.
                    let mut gap = f32::INFINITY;
                    let here = key as usize;
                    let at = box_from.partition_point(|b| b.0 < here);
                    if let Some(&(k, d)) = box_from.get(at)
                        && k == here
                    {
                        // Sorted by distance: the first is the nearest.
                        gap = f.d_line + d;
                    }
                    if gap.is_infinite() && rear[f.exit_key].is_finite() {
                        gap = f.d_line + m.length + rear[f.exit_key] - f.exit_start;
                    }
                    if gap.is_finite() {
                        target = target.min(follow_speed(gap, hw));
                    }
                    if f.reaches {
                        enter = Some(f);
                    }
                } else {
                    target = target.min(stop_speed(f.d_line - f.back));
                    // A car standing just beyond a short box (a bend, a join
                    // of curves) is closer than the stop line: keep the gap.
                    if rear[f.exit_key].is_finite() {
                        target = target.min(follow_speed(f.d_line + m.length + rear[f.exit_key] - f.exit_start, hw));
                    }
                }
            } else {
                // Junction too far to decide, or no way on: stop at the line.
                target = target.min(stop_speed(stop - v.progress - stop_back_m(is_truck(trip))));
            }
            let speed = (v.speed + balance::ACCELERATION * DT).min(target).max(0.0);
            v.braking = speed + 0.01 < v.speed;
            v.go = front.is_some_and(|f| f.permit);
            let mut adv = speed * DT;
            v.speed = speed;
            let mut finished: Option<TripEnd> = None;
            if let Some(a) = arrive_at
                && v.progress + adv >= a - 0.05
            {
                // Parking: the car stands at the kerb for a moment before it
                // is gone (on a one-lane street everyone behind waits; on
                // an avenue they pass in the other lane).
                let dwell = secs_to_ticks(balance::PARK_DWELL_S) as u32;
                if trip.is_none_or(|t| t.wait_ticks >= dwell) {
                    let secs = trip.map_or(0.0, |t| (now - t.requested_tick.min(now)) as f32 * DT);
                    finished = Some(TripEnd::Arrived(secs));
                } else {
                    v.progress = a.max(v.progress);
                    v.speed = 0.0;
                    adv = 0.0;
                }
            }
            if finished.is_none() {
                match enter {
                    Some(f) if v.progress + adv >= stop - 0.01 => {
                        let j = traffic.junction_states.get_mut(f.node).expect("front junction");
                        let m = &j.movements[f.movement];
                        let over = (v.progress + adv - stop).max(0.0);
                        v.progress = stop;
                        let jm = JunctionMove {
                            node: f.node,
                            from: s,
                            to: f.to,
                            to_forward: f.to_forward,
                            movement: f.movement as u16,
                            turn: f.turn,
                            from_lane: v.lane,
                            to_lane: f.to_lane,
                            distance: over.min(m.length),
                            length: m.length,
                        };
                        let (p, d) = m.pose_at(jm.distance);
                        v.pos = p;
                        v.heading = libm::atan2f(d[1], d[0]);
                        v.junction = Some(jm);
                        v.lane_shift = 0.0;
                        entered.push(id);
                        j.entered_minute = j.entered_minute.saturating_add(1);
                        if j.arm_of(s).is_some_and(|a| !j.arms[a].major) && j.arms.iter().any(|a| a.major) {
                            j.minor_minute = j.minor_minute.saturating_add(1);
                        }
                        j.clear_at = now;
                        cache.turns[match f.turn {
                            Turn::Straight => 0,
                            Turn::Right => 1,
                            Turn::Left => 2,
                            Turn::UTurn => 3,
                        }] += 1;
                        let arm = j.arm_of(s).unwrap_or(0) as u8;
                        let same = arm == j.last_served || j.arms[arm as usize].opposite == Some(j.last_served);
                        if same {
                            j.phase_count = j.phase_count.saturating_add(1);
                        } else {
                            j.last_served = arm;
                            j.phase_count = 1;
                        }
                        leader = Some(stop + jm.distance);
                    }
                    _ => {
                        if v.progress + adv > stop {
                            adv = (stop - v.progress).max(0.0);
                            v.speed = adv / DT;
                        }
                        v.progress += adv;
                        traffic::set_pose(roads, v);
                        leader = Some(v.progress);
                    }
                }
            } else {
                leader = Some(v.progress);
            }
            if v.lane_shift != 0.0 {
                let step = DT / balance::LANE_CHANGE_S;
                v.lane_shift = if v.lane_shift > 0.0 {
                    (v.lane_shift - step).max(0.0)
                } else {
                    (v.lane_shift + step).min(0.0)
                };
            }
            if let Some(f) = finished {
                done.push((id, f));
            }
            i += 1;
        }
    }

    // Time stuck inside a box counts from entering it.
    for id in entered {
        if let Some(t) = traffic.trips.get_mut(id) {
            t.wait_ticks = 0;
        }
    }
    // Waiting, re-planning, giving up; deadlocks inside boxes.
    let replan_ticks = secs_to_ticks(balance::REPLAN_AFTER_WAIT_S) as u32;
    let stuck_ticks = secs_to_ticks(balance::STUCK_REMOVE_AFTER_S) as u32;
    let box_stuck_ticks = secs_to_ticks(balance::BOX_DEADLOCK_S) as u32;
    let mut unbox: Vec<VehicleId> = Vec::new();
    for (id, v) in vehicles.iter() {
        if done.iter().any(|d| d.0 == id) {
            continue;
        }
        let Some(t) = traffic.trips.get_mut(id) else { continue };
        let at_red = v.junction.is_none()
            && v.segment.is_some_and(|s| {
                held_by_red
                    .get(lane_key(s, v.forward, v.lane))
                    .copied()
                    .unwrap_or(false)
            });
        if v.speed >= balance::WAIT_SPEED {
            t.wait_ticks = 0;
        } else if !at_red || now.is_multiple_of(2) {
            // At a red light the timer runs at half speed: giving up takes
            // twice `STUCK_REMOVE_AFTER_S`, longer than any signal cycle.
            t.wait_ticks += 1;
        }
        if v.junction.is_some() && t.wait_ticks >= box_stuck_ticks {
            unbox.push(id);
        } else if t.wait_ticks >= stuck_ticks {
            let secs = (now - t.requested_tick.min(now)) as f32 * DT;
            done.push((id, TripEnd::GaveUp(secs)));
            traffic.stuck_removed += 1;
            if let Some(s) = v.segment
                && let Some(seg) = roads.segments.get(s)
            {
                let (_, stop) = lane_range(setbacks, s, v.forward, seg.length);
                if cache.gave_up.len() < 100_000 {
                    cache.gave_up.push((now, s, v.forward, stop - v.progress, v.lane));
                }
                let r = cache.last_denial.get(key_index(id)).copied().unwrap_or(255);
                cache.gave_up_reasons[(r as usize).min(4)] += 1;
            }
        } else if t.wait_ticks > 0 && t.wait_ticks.is_multiple_of(replan_ticks) && !t.queued && v.junction.is_none() {
            t.queued = true;
            traffic.replan.push_back(id);
        }
    }
    for id in unbox {
        traffic.deadlocks_resolved += 1;
        let secs = traffic
            .trips
            .get(id)
            .map_or(0.0, |t| (now - t.requested_tick.min(now)) as f32 * DT);
        done.push((id, TripEnd::GaveUp(secs)));
    }
    world.cache.setbacks = setbacks_vec;
    world.cache.pockets = pockets_vec;
    for (id, f) in done {
        traffic::remove_vehicle(world, id, f, ended);
    }
}

/// Is there room for the front car `f` beyond the box: behind the last
/// car on its out-lane (and those still in the box heading there)? A
/// moving out-lane counts the room its last car frees by the time this car
/// gets there (a discharging platoon keeps flowing; car following keeps
/// the spacing), as long as the out-lane is long enough for everyone on it
/// (a short block filling up needs the room now).
#[allow(clippy::too_many_arguments)]
fn exit_room(
    roads: &RoadNetwork,
    f: &Front,
    movement_len: f32,
    rear: &[f32],
    rear_speed: &[f32],
    on_lane: &[u16],
    inbox: &[u8],
) -> bool {
    let room = rear[f.exit_key] - f.exit_start;
    let ahead = balance::VEHICLE_SPACING_M * inbox[f.exit_key] as f32;
    let empty = rear[f.exit_key].is_infinite();
    if empty {
        return inbox[f.exit_key] == 0 || roads.segments[f.to].length > ahead + balance::MIN_GAP_M;
    }
    if room >= ahead + balance::MIN_GAP_M {
        return true;
    }
    let v = rear_speed[f.exit_key];
    // Storage left on the out-lane once everyone on it and in the box
    // heading there stands in a queue, and this car too, with a margin: a
    // short block that could fill up needs the room now (a car stuck in the
    // box blocks every crossing movement).
    let long = f.exit_stop - f.exit_start
        > (on_lane[f.exit_key] as f32 + inbox[f.exit_key] as f32 + 1.0 + balance::EXIT_STORAGE_MARGIN_CARS)
            * balance::VEHICLE_SPACING_M;
    if v <= balance::WAIT_SPEED || !long {
        return false;
    }
    // Seconds until this car reaches the box exit.
    let t = ((f.d_line + movement_len) / f.speed.max(2.0)).clamp(0.5, balance::EXIT_LOOKAHEAD_S);
    room + v * t >= ahead + balance::MIN_GAP_M
}

fn tick_signals(
    traffic: &mut TrafficState,
    roads: &RoadNetwork,
    fronts: &[Front],
    can_go: &[bool],
    near_cars: &[u16],
    boxed: &[(u32, u16, f32, VehicleId)],
    now: u64,
) {
    use crate::junction::ArmDemand;
    let mut per_arm: Vec<ArmDemand> = Vec::new();
    let mut k = 0;
    let mut b = 0;
    for n in roads.nodes.keys() {
        let slot = key_index(n);
        while k < fronts.len() && key_index(fronts[k].node) < slot {
            k += 1;
        }
        while b < boxed.len() && (boxed[b].0 as usize) < slot {
            b += 1;
        }
        let Some(j) = traffic.junction_states.get_mut(n) else {
            continue;
        };
        if j.control != JunctionControl::Signal {
            continue;
        }
        per_arm.clear();
        per_arm.resize(j.arms.len(), ArmDemand::default());
        let mut kk = k;
        while kk < fronts.len() && fronts[kk].node == n {
            let f = &fronts[kk];
            if f.d_line <= balance::SIGNAL_DETECT_M
                && can_go[kk]
                && let Some(d) = per_arm.get_mut(f.arm as usize)
            {
                // Within the passage time of the line (or standing at it):
                // holds a green that serves it.
                let near = f.eta <= balance::SIGNAL_PASSAGE_S || f.d_line < f.back + balance::STOP_SIGN_DISTANCE_M;
                if matches!(f.turn, Turn::Left | Turn::UTurn) {
                    // The protected left phase is called by a left turner
                    // stuck at the line (it found no gap) and, from a turn
                    // pocket, only once the pocket fills up (the others go
                    // permissively through the green and clear out at its
                    // end).
                    if f.speed < balance::STOP_SIGN_SPEED
                        && f.d_line < f.back + balance::STOP_SIGN_DISTANCE_M
                        && (f.pocket_cars == 0 || f.pocket_cars >= balance::POCKET_CALL_CARS)
                    {
                        d.left = true;
                    }
                    d.through_left = true;
                    d.hold_left |= near;
                    d.pocket = d.pocket.max(f.pocket_cars);
                } else {
                    d.through = true;
                    d.hold_through |= near;
                    d.near = d.near.saturating_add(near_cars[kk]);
                }
                d.discharging |= near && f.speed < balance::SIGNAL_DISCHARGE_SPEED;
                d.wait_s = d.wait_s.max(f.wait as f32 / crate::TICKS_PER_SECOND as f32);
            }
            kk += 1;
        }
        let box_clear = !(b < boxed.len() && boxed[b].0 as usize == slot);
        crate::junction::tick_signal(j, &per_arm, now, box_clear);
    }
}

/// Lane changes for every car on a multi-lane road (see the module docs).
fn lane_changes(world: &mut World) {
    let World {
        roads,
        vehicles,
        traffic,
        cache,
        ..
    } = world;
    let setbacks: &[[f32; 2]] = &cache.setbacks;
    let pockets: &[[f32; 2]] = &cache.pockets;
    // Cars on multi-lane roads, by lane, sorted by progress (rear first).
    // Cars that just entered a box still count in the lane they left (at
    // the stop line plus their distance in): nobody changes lanes into the
    // tail of a car that is pulling away.
    let mut lanes: Vec<(u32, f32, f32, VehicleId)> = vehicles
        .iter()
        .filter_map(|(id, v)| {
            if let Some(jm) = &v.junction {
                let seg = roads.segments.get(jm.from)?;
                if seg.lane_count() < 2 {
                    return None;
                }
                let (_, stop) = lane_range(setbacks, jm.from, v.forward, seg.length);
                return Some((
                    lane_key(jm.from, v.forward, jm.from_lane) as u32,
                    stop + jm.distance,
                    v.speed,
                    id,
                ));
            }
            let s = v.segment?;
            let seg = roads.segments.get(s)?;
            (seg.lane_count() >= 2).then(|| (lane_key(s, v.forward, v.lane) as u32, v.progress, v.speed, id))
        })
        .collect();
    if lanes.is_empty() {
        return;
    }
    lanes.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.3.cmp(&b.3)));
    // (leader gap, leader speed) and follower gap/speed in lane `key` around `p`.
    let around = |key: u32, p: f32, me: VehicleId| -> (f32, f32, f32, f32) {
        let start = lanes.partition_point(|x| x.0 < key);
        let end = lanes.partition_point(|x| x.0 <= key);
        let run = &lanes[start..end];
        let pos = run.partition_point(|x| x.1 < p || (x.1 == p && x.3 < me));
        let mut ahead = (f32::INFINITY, f32::INFINITY);
        let mut behind = (f32::INFINITY, 0.0);
        for x in &run[pos..] {
            if x.3 != me {
                ahead = (x.1 - p, x.2);
                break;
            }
        }
        for x in run[..pos].iter().rev() {
            if x.3 != me {
                behind = (p - x.1, x.2);
                break;
            }
        }
        (ahead.0, ahead.1, behind.0, behind.1)
    };
    let mut changes: Vec<(VehicleId, u8)> = Vec::new();
    for (i, &(key, p, speed, id)) in lanes.iter().enumerate() {
        let v = &vehicles[id];
        if v.lane_shift.abs() > 0.05 || v.junction.is_some() {
            continue;
        }
        let s = v.segment.expect("listed");
        let seg = &roads.segments[s];
        let n = seg.lane_count();
        let (start, stop) = lane_range(setbacks, s, v.forward, seg.length);
        let d_line = stop - p;
        // Not right behind a junction: cars leaving its box need the room.
        if p < start + balance::NO_LANE_CHANGE_AFTER_BOX_M {
            continue;
        }
        // The turn pocket of this approach: open from `p0` on.
        let pocket_from = pocket_start(pockets, s, v.forward, stop);
        let lanes_here = if pocket_from.is_some_and(|p0| p >= p0) {
            n + 1
        } else {
            n
        };
        // Lanes the next movement can be made from (all lanes when the
        // junction is still far), as a bit set.
        let all: u8 = if n >= 8 { u8::MAX } else { (1u8 << n) - 1 };
        let mut allowed = all;
        let mut need: Option<u8> = None;
        // (A car bound for a turn pocket moves over from twice as far: it
        // may need two lane changes.)
        if d_line < 2.0 * balance::LANE_PREPARE_M
            && let Some(st) = next_step(roads, s, v.forward, &v.route)
            && let Some(j) = traffic.junction_states.get(st.node)
            && let (Some(a), Some(b)) = (j.arm_of(s), j.arm_of(st.to))
            && let Some((_, ok)) = turn_lanes(j, a, b, pocket_from.is_some())
            && ok != u8::MAX
            && (d_line < balance::LANE_PREPARE_M || ok & !all != 0)
        {
            if ok & (1 << v.lane.min(7)) == 0 {
                need = (0..8u8)
                    .filter(|l| ok & (1 << l) != 0)
                    .min_by_key(|l| l.abs_diff(v.lane));
            }
            allowed = ok;
        } else if v.route.is_empty() && v.lane > 0 {
            need = Some(0);
        }
        // In a pocket it no longer needs (a new route): back out.
        if v.lane >= n && need.is_none() && allowed & (1 << v.lane.min(7)) == 0 {
            need = Some(n - 1);
        }
        if v.lane >= n && need.is_none() {
            continue;
        }
        // One lane at a time (from the kerb lane to the pocket takes two).
        let need = need.map(|l| match l.cmp(&v.lane) {
            std::cmp::Ordering::Greater => v.lane + 1,
            std::cmp::Ordering::Less => v.lane - 1,
            std::cmp::Ordering::Equal => l,
        });
        let want = match need {
            Some(l) => l,
            None => {
                if d_line < 12.0 {
                    continue;
                }
                let other = if v.lane == 0 { 1 } else { 0 };
                if allowed & (1 << other) == 0 {
                    continue;
                }
                // The car ahead in this lane is the next entry of the list.
                let (g_me, s_me) = match lanes.get(i + 1) {
                    Some(x) if x.0 == key => (x.1 - p, x.2),
                    _ => (f32::INFINITY, f32::INFINITY),
                };
                let okey = lane_key(s, v.forward, other) as u32;
                let (g_o, _, _, _) = around(okey, p, id);
                let near = d_line < balance::LANE_PREPARE_M;
                if near {
                    // Queue up in the emptier usable lane, and not behind
                    // a car that will wait to turn left across traffic.
                    let ahead_turns_left = g_me < 40.0
                        && lanes.get(i + 1).is_some_and(|x| {
                            x.0 == key
                                && vehicles[x.3].junction.is_none()
                                && next_turn(roads, traffic, pockets, &vehicles[x.3]).is_some_and(|t| {
                                    // (with a pocket only while it waits
                                    // at the pocket's entry)
                                    t.0 == Turn::Left && (t.1 & (1 << (n - 1).min(7)) != 0 || vehicles[x.3].speed < 2.0)
                                })
                        });
                    if (g_o > g_me + 12.0 && g_me < 60.0) || (ahead_turns_left && g_o >= g_me - 6.0) {
                        other
                    } else {
                        continue;
                    }
                } else {
                    // Further back: overtaking and keeping right, but not
                    // out of the lanes the next turn needs when it is
                    // within twice the preparing distance (a queue there
                    // leaves no gap to get back).
                    let turn_ok = |l: u8| {
                        d_line > 2.0 * balance::LANE_PREPARE_M
                            || next_turn(roads, traffic, pockets, v).is_none_or(|(_, m)| {
                                let through = m & ((1u8 << n) - 1);
                                through == 0 || through & (1 << l) != 0
                            })
                    };
                    if v.lane == 0 {
                        // Overtake a slow car or queue ahead.
                        if g_me < 40.0
                            && s_me + 2.0 < speed.max(seg.speed_limit * 0.6)
                            && g_o > g_me + 15.0
                            && turn_ok(1)
                        {
                            1
                        } else {
                            continue;
                        }
                    } else if (g_o > 50.0 || g_o >= g_me) && turn_ok(0) {
                        // Keep right.
                        0
                    } else {
                        continue;
                    }
                }
            }
        };
        if want == v.lane || want >= lanes_here {
            continue;
        }
        let tkey = lane_key(s, v.forward, want) as u32;
        let (ga, _, gb, sb) = around(tkey, p, id);
        let safe_ahead = ga >= balance::MIN_GAP_M + 1.0 + 0.3 * speed;
        let safe_behind = gb >= balance::MIN_GAP_M + 1.0 + 0.6 * sb;
        if safe_ahead && safe_behind {
            changes.push((id, want));
        }
    }
    // Two cars may not take the same gap: keep the first per target lane
    // within a car spacing of each other.
    let mut taken: Vec<(u32, f32)> = Vec::new();
    for (id, want) in changes {
        let v = &vehicles[id];
        let s = v.segment.expect("listed");
        let tkey = lane_key(s, v.forward, want) as u32;
        if taken
            .iter()
            .any(|&(k, p)| k == tkey && (p - v.progress).abs() < balance::MIN_GAP_M + 1.0)
        {
            continue;
        }
        taken.push((tkey, v.progress));
        let v = &mut vehicles[id];
        v.lane_shift = v.lane as f32 - want as f32;
        v.lane = want;
    }
}

/// Lateral offset of a vehicle from the centre line, metres to the right of
/// travel, including an unfinished lane change.
pub fn lane_offset(kind: crate::RoadKind, lane: u8, shift: f32) -> f32 {
    let here = kind.lane_offset_m(lane);
    if shift > 0.0 {
        here + shift * (kind.lane_offset_m(lane.saturating_add(1)) - here)
    } else if shift < 0.0 {
        here - shift * (kind.lane_offset_m(lane.saturating_sub(1)) - here)
    } else {
        here
    }
}

/// Exact drawing pose of a vehicle: position in its lane (or on its
/// junction curve) and unit travel direction. `Vehicle::pos` is on the
/// centre line; this adds the lane offset.
pub fn vehicle_pose(world: &World, v: &traffic::Vehicle) -> ([f32; 2], [f32; 2]) {
    if let Some(jm) = &v.junction
        && let Some(m) = world
            .traffic
            .junction_states
            .get(jm.node)
            .and_then(|j| j.movements.get(jm.movement as usize))
    {
        return m.pose_at(jm.distance);
    }
    let Some(seg) = v.segment.and_then(|s| world.roads.segments.get(s)) else {
        return (v.pos, [1.0, 0.0]);
    };
    let off = lane_offset(seg.kind, v.lane, v.lane_shift);
    let p = seg.pose_dir(v.forward, v.progress.clamp(0.0, seg.length), off);
    (p.pos, p.dir)
}
