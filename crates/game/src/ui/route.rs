//! The route of the selected car or person, for the map (`shared::RouteView`)
//! and the side panel's words.
//!
//! A car's way still ahead is traced along the lanes the sim has planned
//! (`Vehicle::route`): the rest of its segment in its lane, then at every
//! junction the turn path (`junction::Movement`) drivers take there from the
//! lane nearest to theirs, then the next segment from the box edge to the
//! stop line, easing across lanes shortly before a turn, as drivers do. The
//! line ends where the car will stop (`Trip::dest`). A trip that has not
//! started (or happens off the map) is traced the same way along the route
//! a car would take now (`inspect::planned_route`).
//!
//! Cheap: nothing runs while nobody travelling is selected; a car's line is
//! re-traced a few times a second and published only when the route changes
//! or the car is a few metres further on.

use bevy::prelude::*;
use citysim_sim::citizens::Activity;
use citysim_sim::inspect::{self, PlannedRoute};
use citysim_sim::{Access, BuildingId, CitizenId, Place, SegmentId, VehicleId, World, drive, junction};

use super::UiState;
use super::panel::PeopleView;
use super::people::Target;
use super::widgets::UiAction;
use crate::shared::{CameraGoTo, GameSet, RouteEnd, RouteState, RouteView, SimWorld};

pub struct RoutePlugin;

impl Plugin for RoutePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TripCache>()
            .add_systems(Update, (jump_to_destination, publish_route).in_set(GameSet::Render));
        if std::env::var("CITYSIM_ROUTE_STATS").is_ok_and(|v| v == "1") {
            app.add_systems(Update, log_route_stats);
        }
    }
}

/// Sample spacing along curved roads, metres.
const STEP_M: f32 = 3.0;
/// A lane change before a turn: it starts this far before the stop line and
/// takes `LANE_CHANGE_M`.
const LANE_CHANGE_BEFORE_M: f32 = 42.0;
const LANE_CHANGE_M: f32 = 18.0;
/// The car's line is published again once its head moved this far.
const REPUBLISH_M: f32 = 2.5;

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Length of a polyline, metres.
pub fn path_length(pts: &[[f32; 2]]) -> f32 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

fn push(out: &mut Vec<[f32; 2]>, p: [f32; 2]) {
    if out.last().is_none_or(|&q| dist(p, q) > 0.05) {
        out.push(p);
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Segment `s` driven in direction `fwd` from `from` to `to` metres (along
/// travel), `off0` metres right of the centre line at the start and `off1`
/// at the end (the change happens shortly before the end, as a car moves
/// over for its turn). Curved roads are sampled at whole multiples of
/// `STEP_M` from the segment's start, so the samples ahead of a moving car
/// stay put.
#[allow(clippy::too_many_arguments)]
fn trace_segment(
    world: &World,
    s: SegmentId,
    fwd: bool,
    from: f32,
    to: f32,
    off0: f32,
    off1: f32,
    change_by: Option<f32>,
    out: &mut Vec<[f32; 2]>,
) {
    let Some(seg) = world.roads.segments.get(s) else {
        return;
    };
    let (from, to) = (from.clamp(0.0, seg.length), to.clamp(0.0, seg.length));
    if to < from {
        return;
    }
    // Where the lane change is done: given (a turn pocket's mouth), else
    // a little before the stop line.
    let change_end = change_by
        .unwrap_or(to - (LANE_CHANGE_BEFORE_M - LANE_CHANGE_M))
        .clamp(from, to);
    let change_start = (change_end - LANE_CHANGE_M).max(from);
    let offset = |d: f32| {
        if (off1 - off0).abs() < 0.01 {
            off0
        } else if change_end - change_start < 0.5 {
            if d >= change_end { off1 } else { off0 }
        } else {
            off0 + (off1 - off0) * smoothstep((d - change_start) / (change_end - change_start))
        }
    };
    let mut ds = vec![from, to];
    if !seg.curve.is_straight() {
        let mut d = (from / STEP_M).floor() * STEP_M + STEP_M;
        while d < to {
            ds.push(d);
            d += STEP_M;
        }
    }
    if (off1 - off0).abs() >= 0.01 {
        let mut d = change_start;
        while d < change_end {
            ds.push(d);
            d += 2.0;
        }
        ds.push(change_end);
    }
    ds.sort_by(f32::total_cmp);
    ds.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    for d in ds {
        if d < from - 1e-3 || d > to + 1e-3 {
            continue;
        }
        push(out, seg.pose_dir(fwd, d, offset(d)).pos);
    }
}

/// Lane offset (metres right of the centre line) of lane `lane` on `s`.
fn lane_off(world: &World, s: SegmentId, lane: f32) -> f32 {
    let Some(seg) = world.roads.segments.get(s) else {
        return 0.0;
    };
    let l = lane.round().clamp(0.0, (seg.lane_count().max(1) - 1) as f32) as u8;
    seg.kind.lane_offset_m(l)
}

/// From `seg` (driven `fwd`, `progress` metres along, `off` metres right of
/// the centre line, in lane `lane`) along `route` to `end`.
#[allow(clippy::too_many_arguments)]
fn trace_route(
    world: &World,
    mut seg_id: SegmentId,
    mut fwd: bool,
    mut progress: f32,
    mut off: f32,
    mut lane: u8,
    route: &[SegmentId],
    end: Option<Access>,
    out: &mut Vec<[f32; 2]>,
) {
    for i in 0..=route.len() {
        let Some(seg) = world.roads.segments.get(seg_id) else {
            return;
        };
        let len = seg.length;
        let node = if fwd { seg.b } else { seg.a };
        let Some(&to) = route.get(i) else {
            // The last segment: up to where the car stops.
            let stop = end
                .filter(|a| a.segment == seg_id)
                .map_or(len, |a| if fwd { a.from_a } else { len - a.from_a });
            trace_segment(world, seg_id, fwd, progress, stop.max(progress), off, off, None, out);
            return;
        };
        let Some(next) = world.roads.segments.get(to) else {
            trace_segment(world, seg_id, fwd, progress, len, off, off, None, out);
            return;
        };
        let to_fwd = if next.a == node {
            true
        } else if next.b == node {
            false
        } else {
            // Not joined: the route is stale (the car re-plans).
            trace_segment(world, seg_id, fwd, progress, len, off, off, None, out);
            return;
        };
        // The turn drivers take here from the lane nearest to this one.
        let movement = world.traffic.junction_states.get(node).and_then(|j| {
            let (a, b) = (j.arm_of(seg_id)?, j.arm_of(to)?);
            j.movements_between(a, b)
                .map(|(_, m)| m)
                .min_by_key(|m| (!m.preferred, (m.from_lane as i32 - lane as i32).abs(), m.to_lane))
        });
        match movement {
            Some(m) => {
                let stop = len - junction::setback(world, node, seg_id);
                let want = seg.kind.lane_offset_m(m.from_lane);
                // Into a left-turn pocket just past where it opens.
                let by = (seg.kind.pocket_lane() == Some(m.from_lane))
                    .then(|| junction::turn_pocket(world, seg_id, fwd).map(|p| p.start_m + 8.0))
                    .flatten();
                trace_segment(world, seg_id, fwd, progress, stop.max(progress), off, want, by, out);
                // (Round a roundabout's ring, not across its island.)
                for p in m.points(0.5) {
                    push(out, p);
                }
                lane = m.to_lane;
                progress = junction::setback(world, node, to);
            }
            None => {
                // A plain bend: the lanes run straight on.
                trace_segment(world, seg_id, fwd, progress, len, off, off, None, out);
                lane = lane.min(next.lane_count().saturating_sub(1));
                progress = 0.0;
            }
        }
        seg_id = to;
        fwd = to_fwd;
        off = lane_off(world, seg_id, lane as f32);
    }
}

/// The way still ahead of a car, from its exact lane pose to where it will
/// stop. `None` for a car not on the road.
pub fn vehicle_path(world: &World, id: VehicleId) -> Option<Vec<[f32; 2]>> {
    let v = world.vehicles.get(id)?;
    let end = world.traffic.trips.get(id).and_then(|t| t.dest);
    let mut out = Vec::with_capacity(64);
    push(&mut out, drive::vehicle_pose(world, v).0);
    match &v.junction {
        Some(jm) => {
            let m = world
                .traffic
                .junction_states
                .get(jm.node)
                .and_then(|j| j.movements.get(jm.movement as usize));
            if let Some(m) = m {
                let mut d = jm.distance;
                while d < m.length {
                    push(&mut out, m.pose_at(d).0);
                    d += 1.0;
                }
                push(&mut out, m.pose_at(m.length).0);
            }
            let route = match v.route.first() {
                Some(&s) if s == jm.to => &v.route[1..],
                _ => &v.route[..],
            };
            let progress = junction::setback(world, jm.node, jm.to);
            let off = lane_off(world, jm.to, jm.to_lane as f32);
            trace_route(
                world,
                jm.to,
                jm.to_forward,
                progress,
                off,
                jm.to_lane,
                route,
                end,
                &mut out,
            );
        }
        None => {
            let s = v.segment?;
            let seg = world.roads.segments.get(s)?;
            let off = drive::lane_offset(seg.kind, v.lane, v.lane_shift);
            trace_route(world, s, v.forward, v.progress, off, v.lane, &v.route, end, &mut out);
        }
    }
    Some(out)
}

/// The way a car would take for a planned route, from where it pulls out.
pub fn planned_path(world: &World, r: &PlannedRoute) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(64);
    let Some(seg) = world.roads.segments.get(r.start.segment) else {
        return out;
    };
    let progress = if r.forward {
        r.start.from_a
    } else {
        seg.length - r.start.from_a
    };
    let off = seg.kind.lane_offset_m(0);
    trace_route(
        world,
        r.start.segment,
        r.forward,
        progress,
        off,
        0,
        &r.segments,
        Some(r.end),
        &mut out,
    );
    out
}

/// What a selected person or car is up to, as a route.
#[derive(Debug, Clone, PartialEq)]
pub struct Trip {
    pub state: RouteState,
    pub points: Vec<[f32; 2]>,
    pub origin: Option<RouteEnd>,
    pub destination: Option<RouteEnd>,
    /// The car on the road, if any.
    pub vehicle: Option<VehicleId>,
    /// Under way off the map: where they should be by now (`points` then
    /// start there).
    pub at: Option<[f32; 2]>,
}

fn building_end(world: &World, b: BuildingId) -> Option<RouteEnd> {
    world.buildings.get(b).map(|bb| RouteEnd {
        pos: bb.center(),
        building: Some(b),
    })
}

fn place_end(world: &World, p: Place, b: Option<BuildingId>) -> Option<RouteEnd> {
    if let Some(e) = b.and_then(|b| building_end(world, b)) {
        return Some(e);
    }
    match p {
        Place::At(pos) => Some(RouteEnd { pos, building: None }),
        Place::Entry => world
            .roads
            .entry
            .and_then(|e| world.roads.nodes.get(e))
            .map(|n| RouteEnd {
                pos: n.pos,
                building: None,
            }),
    }
}

/// A car's trip: its line, where it set off and where it goes.
pub fn vehicle_trip(world: &World, id: VehicleId) -> Option<Trip> {
    let t = world.traffic.trips.get(id)?;
    Some(Trip {
        state: RouteState::Driving,
        points: vehicle_path(world, id)?,
        origin: place_end(world, t.from, t.from_building),
        destination: place_end(world, t.to, t.to_building),
        vehicle: Some(id),
        at: None,
    })
}

/// Route searches and traces of planned trips, and the time they took
/// (`CITYSIM_ROUTE_STATS=1` logs them every 5 s).
pub static PLANNED_TRACES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static PLANNED_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A planned trip between two buildings (`None` without a road route).
fn planned_trip(world: &World, state: RouteState, from: BuildingId, to: BuildingId) -> Option<Trip> {
    let t0 = std::time::Instant::now();
    let out = planned_trip_now(world, state, from, to);
    PLANNED_TRACES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    PLANNED_NS.fetch_add(t0.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    out
}

fn planned_trip_now(world: &World, state: RouteState, from: BuildingId, to: BuildingId) -> Option<Trip> {
    let r = inspect::planned_route(world, from, to)?;
    Some(Trip {
        state,
        points: planned_path(world, &r),
        origin: building_end(world, from),
        destination: building_end(world, to),
        vehicle: None,
        at: None,
    })
}

/// A person's trip: the car they are in, a trip off the map, or the next
/// trip they will make.
pub fn person_trip(world: &World, id: CitizenId) -> Option<Trip> {
    let p = inspect::plan(world, id)?;
    person_trip_of(world, &p)
}

fn person_trip_of(world: &World, p: &inspect::DayPlan) -> Option<Trip> {
    if let Some(t) = p.trip {
        if let Some(car) = t.vehicle.filter(|c| world.vehicles.contains_key(*c)) {
            return vehicle_trip(world, car);
        }
        let from = t.from?;
        return planned_trip(world, RouteState::Unseen, from, t.to).or_else(|| {
            Some(Trip {
                state: RouteState::Unseen,
                points: Vec::new(),
                origin: building_end(world, from),
                destination: building_end(world, t.to),
                vehicle: None,
                at: None,
            })
        });
    }
    let next = p.next.filter(|n| n.activity == Activity::Travelling)?;
    let (from, to) = (p.inside?, next.place?);
    if from == to {
        return None;
    }
    planned_trip(world, RouteState::Planned, from, to)
}

/// What a person's planned (or off-screen) trip depends on: the roads, the
/// trip they are on (and whether its car is on the map), and where they are
/// and go next.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TripKey {
    person: CitizenId,
    roads: u64,
    trip: Option<(Option<BuildingId>, BuildingId, Option<VehicleId>)>,
    inside: Option<BuildingId>,
    next: Option<(Activity, Option<BuildingId>)>,
}

/// The selected person's trip, traced once and kept until something it
/// depends on changes ([`TripKey`]). A trip in a car on the map follows the
/// car, so it is traced each time it is asked for (no route search: the
/// car carries its route).
#[derive(Resource, Default)]
pub struct TripCache {
    kept: Option<(TripKey, Option<Trip>)>,
}

impl TripCache {
    pub fn person(&mut self, world: &World, id: CitizenId) -> Option<Trip> {
        let p = inspect::plan(world, id)?;
        let car = p
            .trip
            .and_then(|t| t.vehicle)
            .filter(|c| world.vehicles.contains_key(*c));
        if car.is_some() {
            return person_trip_of(world, &p);
        }
        let key = TripKey {
            person: id,
            roads: world.versions.roads,
            trip: p.trip.map(|t| (t.from, t.to, t.vehicle)),
            inside: p.inside,
            next: p.next.map(|n| (n.activity, n.place)),
        };
        let fresh = !matches!(&self.kept, Some((k, _)) if *k == key);
        if fresh {
            self.kept = Some((key, person_trip_of(world, &p)));
        }
        let trip = self.kept.as_ref().and_then(|k| k.1.as_ref())?;
        // Under way off the map: from where they should be by now.
        match p.trip {
            Some(t) if trip.state == RouteState::Unseen => Some(under_way(world, &t, trip)),
            _ => Some(trip.clone()),
        }
    }
}

/// An off-screen trip (on foot, a lift, a car beyond those drawn) as it
/// stands now: the share of its time gone, along its way (a straight line
/// when there is no way by road), gives where they should be; the way
/// still ahead starts there.
fn under_way(world: &World, t: &inspect::TripView, trip: &Trip) -> Trip {
    let mut pts = trip.points.clone();
    if pts.len() < 2
        && let (Some(o), Some(d)) = (trip.origin, trip.destination)
    {
        pts = vec![o.pos, d.pos];
    }
    let eta = t.eta_tick.unwrap_or(t.expected_tick);
    let span = eta.saturating_sub(t.departed_tick).max(1) as f32;
    let f = (world.time.tick.saturating_sub(t.departed_tick) as f32 / span).clamp(0.0, 1.0);
    let (at, rest) = split_at(&pts, path_length(&pts) * f);
    Trip {
        points: rest,
        at,
        ..trip.clone()
    }
}

/// The point `d` metres along `pts`, and the part of `pts` from there on.
fn split_at(pts: &[[f32; 2]], d: f32) -> (Option<[f32; 2]>, Vec<[f32; 2]>) {
    let mut left = d;
    for (i, w) in pts.windows(2).enumerate() {
        let l = dist(w[0], w[1]);
        if left <= l {
            let k = if l > 1e-6 { left / l } else { 0.0 };
            let p = [w[0][0] + (w[1][0] - w[0][0]) * k, w[0][1] + (w[1][1] - w[0][1]) * k];
            let mut rest = vec![p];
            rest.extend_from_slice(&pts[i + 1..]);
            return (Some(p), rest);
        }
        left -= l;
    }
    let last = pts.last().copied();
    (last, last.into_iter().collect())
}

/// "Going to ›": the camera moves to the destination (following stops;
/// the selection and its route stay).
fn jump_to_destination(
    mut actions: MessageReader<UiAction>,
    route: Res<RouteView>,
    mut ui: ResMut<UiState>,
    mut go: MessageWriter<CameraGoTo>,
) {
    for a in actions.read() {
        if *a == UiAction::JumpToDestination
            && let Some(d) = route.destination
        {
            ui.following = false;
            go.write(CameraGoTo {
                focus: d.pos,
                distance: None,
            });
        }
    }
}

fn log_route_stats(time: Res<Time<Real>>, mut since: Local<f32>) {
    use std::sync::atomic::Ordering::Relaxed;
    *since += time.delta_secs();
    if *since < 5.0 {
        return;
    }
    let n = PLANNED_TRACES.swap(0, Relaxed);
    let ns = PLANNED_NS.swap(0, Relaxed);
    info!(
        "route stats: {:.1} planned traces/s, {:.3} ms/s, {:.3} ms each",
        n as f32 / *since,
        ns as f32 / 1e6 / *since,
        if n > 0 { ns as f32 / 1e6 / n as f32 } else { 0.0 }
    );
    *since = 0.0;
}

/// What publishing remembers between frames.
#[derive(Default)]
struct Published {
    target: Option<Target>,
    age: f32,
    /// The head point of the line last published.
    head: Option<[f32; 2]>,
}

/// Seconds between re-traces of a car's line, and of a planned trip.
const CAR_EVERY_S: f32 = 0.15;
const PLAN_EVERY_S: f32 = 0.5;

/// Keep `RouteView` on the selected person or car.
fn publish_route(
    time: Res<Time<Real>>,
    sim: Res<SimWorld>,
    view: Res<PeopleView>,
    mut route: ResMut<RouteView>,
    mut cache: ResMut<TripCache>,
    mut last: Local<Published>,
) {
    let target = view
        .current()
        .filter(|t| matches!(t, Target::Person(_) | Target::Vehicle(_)));
    let Some(target) = target else {
        if route.state != RouteState::None || !route.points.is_empty() {
            *route = RouteView::default();
        }
        last.target = None;
        return;
    };
    last.age += time.delta_secs();
    let fresh = last.target != Some(target) || sim.is_changed() && last.age > 10.0;
    let every = if route.state == RouteState::Driving || route.traveller.is_some() {
        CAR_EVERY_S
    } else {
        PLAN_EVERY_S
    };
    if !fresh && last.age < every {
        return;
    }
    last.age = 0.0;
    last.target = Some(target);
    let w = &sim.world;
    let trip = match target {
        Target::Person(c) => cache.person(w, c),
        Target::Vehicle(v) => vehicle_trip(w, v),
        Target::Place(_) => None,
    };
    let Some(trip) = trip else {
        if *route != RouteView::default() {
            *route = RouteView::default();
        }
        last.head = None;
        return;
    };
    let new = RouteView {
        state: trip.state,
        points: trip.points,
        origin: trip.origin,
        destination: trip.destination,
        traveller: trip.at,
    };
    // A moving car: publish again only once it is a few metres further on
    // or anything else changed (the samples ahead of it stay put).
    let same_rest = new.state == route.state
        && new.traveller.is_some() == route.traveller.is_some()
        && new.origin == route.origin
        && new.destination == route.destination
        && new.points.len() == route.points.len()
        && new.points.get(1..) == route.points.get(1..);
    let head_moved = match (new.points.first(), last.head) {
        (Some(&a), Some(b)) => dist(a, b) > REPUBLISH_M,
        _ => true,
    };
    if same_rest && !head_moved {
        return;
    }
    last.head = new.points.first().copied();
    if *route != new {
        *route = new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_add_up() {
        assert_eq!(path_length(&[[0.0, 0.0], [3.0, 4.0], [3.0, 10.0]]), 11.0);
        assert_eq!(path_length(&[]), 0.0);
    }

    #[test]
    fn a_car_in_town_has_a_line_to_where_it_stops() {
        let mut w = citysim_sim::World::new(3);
        citysim_sim::scenarios::prebuilt(&mut w, |w| {
            citysim_sim::scenarios::organic(w);
        });
        // Into the morning rush.
        for _ in 0..citysim_sim::balance::TICKS_PER_DAY / 2 {
            w.step();
        }
        let mut checked = 0;
        let ids: Vec<VehicleId> = w.vehicles.keys().collect();
        for id in ids {
            let Some(t) = vehicle_trip(&w, id) else { continue };
            let car = &w.vehicles[id];
            assert!(!t.points.is_empty());
            // It starts at the car.
            assert!(dist(t.points[0], drive::vehicle_pose(&w, car).0) < 0.01);
            // No detours or missing pieces: about as long as the road still
            // ahead (the last segment only partly, setbacks and turn paths
            // shorten or lengthen it a little).
            if car.junction.is_none()
                && let Some(s) = car.segment
            {
                let len = |s| w.roads.segments.get(s).map_or(0.0, |g| g.length);
                let last = car.route.last().map_or(len(s), |&l| len(l));
                let ahead = len(s) - car.progress + car.route.iter().map(|&r| len(r)).sum::<f32>();
                let drawn = path_length(&t.points);
                assert!(drawn <= ahead + 60.0, "{drawn} m drawn, {ahead} m ahead");
                assert!(drawn >= ahead - last - 60.0, "{drawn} m drawn, {ahead} m ahead");
            }
            // It ends where the car will stop.
            if let Some(dest) = w.traffic.trips.get(id).and_then(|t| t.dest)
                && car.junction.is_none()
                && !car.route.is_empty()
            {
                let seg = &w.roads.segments[dest.segment];
                let stop = seg.curve.point_at(dest.from_a, 0.0);
                assert!(dist(*t.points.last().unwrap(), stop) < 6.0);
            }
            checked += 1;
        }
        assert!(checked > 5, "only {checked} cars");
    }
}
