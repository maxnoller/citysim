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
use crate::{World, balance, path};

/// Lanes per direction the lane index space allows.
const LANE_SLOTS: usize = 2;

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

fn follow_speed(gap: f32) -> f32 {
    ((gap - balance::MIN_GAP_M) / balance::FOLLOW_HEADWAY_S).max(0.0)
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
    /// Allowed to enter (decided this tick).
    permit: bool,
    /// Reaches the stop line this tick.
    reaches: bool,
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
    if world.cache.setbacks_key != Some(world.traffic.junctions_version) {
        world.cache.setbacks = build_setbacks(&world.roads, &world.traffic.junction_states);
        world.cache.setbacks_key = Some(world.traffic.junctions_version);
    }

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

    lane_changes(world);

    let World {
        roads,
        vehicles,
        traffic,
        cache,
        ..
    } = world;
    let roads: &RoadNetwork = roads;
    let traffic: &mut TrafficState = traffic;
    let setbacks: &[[f32; 2]] = &cache.setbacks;
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
    cars.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
    boxed.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(b.2.total_cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    let mut rear = vec![f32::INFINITY; nkeys];
    let mut rear_speed = vec![0.0f32; nkeys];
    for &(k, p, id) in &cars {
        let r = &mut rear[k as usize];
        if p < *r {
            *r = p;
            rear_speed[k as usize] = vehicles[id].speed;
        }
    }
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
        let (exit_start, _) = lane_range(setbacks, st.to, st.to_forward, to_len);
        let reach = (v.speed + balance::ACCELERATION * DT) * DT + 0.02;
        fronts.push(Front {
            idx: i,
            node: st.node,
            arm: a as u8,
            movement: m,
            turn: mv.turn,
            d_line,
            speed: v.speed,
            eta: if v.speed > 0.5 {
                d_line / v.speed
            } else {
                (2.0 * d_line / balance::ACCELERATION).sqrt()
            },
            wait: trip.map_or(0, |t| t.wait_ticks),
            go_before: v.go,
            to: st.to,
            to_forward: st.to_forward,
            to_lane: mv.to_lane,
            exit_key: lane_key(st.to, st.to_forward, mv.to_lane),
            exit_start,
            permit: false,
            reaches: d_line <= reach,
        });
    }
    fronts.sort_by_key(|f| (key_index(f.node), f.idx));
    let mut front_of = vec![usize::MAX; cars.len()];
    for (p, f) in fronts.iter().enumerate() {
        front_of[f.idx] = p;
    }

    // Signals.
    tick_signals(traffic, roads, &fronts, &boxed, now);

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
        let j = traffic.junction_states.get_mut(node).expect("front has a junction");
        let mut occupied: Vec<usize> = boxed[b0..b1].iter().map(|b| b.1 as usize).collect();
        // Precedence order.
        let mut order: Vec<usize> = (k..e).collect();
        order.sort_by(|&x, &y| {
            let (fx, fy) = (&fronts[x], &fronts[y]);
            class(j, fy)
                .cmp(&class(j, fx))
                .then(fx.eta.total_cmp(&fy.eta))
                .then(fy.wait.cmp(&fx.wait))
                .then(fx.idx.cmp(&fy.idx))
        });
        for (rank, &fi) in order.iter().enumerate() {
            let f = fronts[fi];
            let mut ok = !occupied.iter().any(|&o| j.conflict(f.movement, o));
            // Room beyond the box.
            if ok {
                // Room for this car behind the last one on the out-lane
                // (and those still in the box heading there), or a queue
                // that is moving off.
                let room = rear[f.exit_key] - f.exit_start;
                let ahead = balance::VEHICLE_SPACING_M * inbox[f.exit_key] as f32;
                let empty = rear[f.exit_key].is_infinite();
                ok = if empty {
                    inbox[f.exit_key] == 0 || roads.segments[f.to].length > ahead + balance::MIN_GAP_M
                } else {
                    room >= ahead + balance::MIN_GAP_M
                        || (rear_speed[f.exit_key] > balance::EXIT_MOVING_SPEED && room >= ahead + 2.0)
                };
            }
            if ok {
                ok = match j.control {
                    JunctionControl::Signal => match j.movement_aspect(f.movement) {
                        Some(Aspect::Green) => true,
                        Some(Aspect::Yellow) => {
                            f.speed * f.speed / (2.0 * balance::BRAKING) > f.d_line + 0.5 && f.speed > 1.0
                        }
                        _ => false,
                    },
                    JunctionControl::Priority => true,
                };
            }
            if ok {
                // Yield to conflicting cars with precedence arriving soon.
                let mine = class(j, &f);
                for &oi in &order[..rank] {
                    let o = &fronts[oi];
                    if !(o.go_before || o.permit) || !j.conflict(f.movement, o.movement) {
                        continue;
                    }
                    if j.control == JunctionControl::Signal && j.movement_aspect(o.movement) != Some(Aspect::Green) {
                        continue;
                    }
                    let theirs = class(j, o);
                    let gap = gap_s(j, mine, theirs);
                    let blocks = if theirs == mine {
                        // Earlier in the order: arrives first.
                        o.eta < f.eta + gap
                    } else {
                        o.eta < f.eta + gap
                    };
                    if blocks {
                        ok = false;
                        break;
                    }
                }
            }
            fronts[fi].permit = ok;
            if ok && f.reaches {
                occupied.push(f.movement);
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
            match lead {
                Some(d) => target = target.min(follow_speed(d - jm.distance)),
                None => {
                    if rear[ek].is_finite() {
                        target = target.min(follow_speed(m.length - jm.distance + rear[ek] - exit_start));
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

    // Move cars on lanes, lane by lane, front to back.
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
            let mut target = seg.speed_limit * driver(v.seed) * path::vdf(seg.congestion_in(v.forward));
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
            if let Some(lp) = leader {
                target = target.min(follow_speed(lp - v.progress));
                if let Some(a) = arrive_at {
                    target = target.min(stop_speed(a - v.progress + 3.0));
                }
            } else if let Some(a) = arrive_at {
                target = target.min(stop_speed(a - v.progress + 3.0));
            } else if let Some(f) = front {
                let j = traffic.junction_states.get(f.node).expect("front junction");
                let m = &j.movements[f.movement];
                let tv = turn_speed(f.turn, roads.segments[f.to].speed_limit);
                target = target.min((tv * tv + 2.0 * balance::BRAKING * f.d_line).sqrt());
                if f.permit {
                    // The nearest car ahead from this lane inside the box,
                    // else the rear of the out-lane.
                    let mut gap = f32::INFINITY;
                    let in_lane = (m.from_arm, m.from_lane);
                    let slot = key_index(f.node) as u32;
                    let lo = boxed.partition_point(|b| b.0 < slot);
                    let hi = boxed.partition_point(|b| b.0 <= slot);
                    for b in &boxed[lo..hi] {
                        if let Some(bm) = j.movements.get(b.1 as usize)
                            && (bm.from_arm, bm.from_lane) == in_lane
                        {
                            gap = gap.min(f.d_line + b.2);
                        }
                    }
                    if gap.is_infinite() && rear[f.exit_key].is_finite() {
                        gap = f.d_line + m.length + rear[f.exit_key] - f.exit_start;
                    }
                    if gap.is_finite() {
                        target = target.min(follow_speed(gap));
                    }
                    if f.reaches {
                        enter = Some(f);
                    }
                } else {
                    target = target.min(stop_speed(f.d_line - balance::STOP_LINE_M));
                }
            } else {
                // Junction too far to decide, or no way on: stop at the line.
                target = target.min(stop_speed(stop - v.progress - balance::STOP_LINE_M));
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
                let secs = trip.map_or(0.0, |t| (now - t.requested_tick.min(now)) as f32 * DT);
                finished = Some(TripEnd::Arrived(secs));
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
                        j.entered_minute = j.entered_minute.saturating_add(1);
                        j.clear_at = now;
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
        if v.speed < balance::WAIT_SPEED {
            t.wait_ticks += 1;
        } else {
            t.wait_ticks = 0;
        }
        if v.junction.is_some() && t.wait_ticks >= box_stuck_ticks {
            unbox.push(id);
        } else if t.wait_ticks >= stuck_ticks {
            let secs = (now - t.requested_tick.min(now)) as f32 * DT;
            done.push((id, TripEnd::GaveUp(secs)));
            traffic.stuck_removed += 1;
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
    for (id, f) in done {
        traffic::remove_vehicle(world, id, f, ended);
    }
}

fn tick_signals(
    traffic: &mut TrafficState,
    roads: &RoadNetwork,
    fronts: &[Front],
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
                && let Some(d) = per_arm.get_mut(f.arm as usize)
            {
                if matches!(f.turn, Turn::Left | Turn::UTurn) {
                    d.left = true;
                } else {
                    d.through = true;
                }
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
    // Cars on multi-lane roads, by lane, sorted by progress (rear first).
    let mut lanes: Vec<(u32, f32, f32, VehicleId)> = vehicles
        .iter()
        .filter(|(_, v)| v.junction.is_none())
        .filter_map(|(id, v)| {
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
    for &(key, p, speed, id) in &lanes {
        let v = &vehicles[id];
        if v.lane_shift.abs() > 0.05 {
            continue;
        }
        let s = v.segment.expect("listed");
        let seg = &roads.segments[s];
        let n = seg.lane_count();
        let (_, stop) = lane_range(setbacks, s, v.forward, seg.length);
        let d_line = stop - p;
        // Lanes the next movement can be made from.
        let mut need: Option<u8> = None;
        if let Some(st) = next_step(roads, s, v.forward, &v.route)
            && let Some(j) = traffic.junction_states.get(st.node)
            && let (Some(a), Some(b)) = (j.arm_of(s), j.arm_of(st.to))
        {
            let allowed: Vec<u8> = j.movements_between(a, b).map(|(_, m)| m.from_lane).collect();
            if !allowed.is_empty() && !allowed.contains(&v.lane) && d_line < balance::LANE_PREPARE_M {
                need = allowed.iter().copied().min_by_key(|l| l.abs_diff(v.lane));
            }
            if need.is_none() && d_line < balance::LANE_PREPARE_M && !allowed.is_empty() {
                // Already in a usable lane near the junction: stay.
                continue;
            }
        } else if v.route.is_empty() && v.lane > 0 {
            need = Some(0);
        }
        let want = match need {
            Some(l) => l,
            None => {
                if d_line < 20.0 {
                    continue;
                }
                let my = key;
                let (g_me, s_me, _, _) = around(my, p, id);
                let other = if v.lane == 0 { 1 } else { 0 };
                let okey = lane_key(s, v.forward, other) as u32;
                let (g_o, _, _, _) = around(okey, p, id);
                if v.lane == 0 {
                    // Overtake a slow car or queue ahead.
                    if g_me < 40.0 && s_me + 2.0 < speed.max(seg.speed_limit * 0.6) && g_o > g_me + 15.0 {
                        1
                    } else {
                        continue;
                    }
                } else if g_o > 50.0 || g_o >= g_me {
                    // Keep right.
                    0
                } else {
                    continue;
                }
            }
        };
        if want == v.lane || want >= n {
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
