//! Vehicles: one entity per `world.vehicles` entry with a shared low-poly
//! model: a car (shape x colour from the vehicle seed) or, for goods trips
//! (`TripPurpose::Goods`), a small delivery truck.
//!
//! Placement uses only the road graph and each vehicle's `segment`,
//! `forward` and `progress` (never the grid):
//! * vehicles drive in the right-hand lane, offset along the segment's exact
//!   direction (so diagonals and highways are consistent);
//! * each lane maps the sim's `0..length` (node centre to node centre) onto
//!   the drawn stretch between junction surfaces with one fixed linear map
//!   per lane: where the sim holds the head of a queue (`STOP_LINE_M` short
//!   of the node) the vehicle stands at the drawn stop line, and the map
//!   never compresses below `MIN_COMPRESSION`, so vehicles queued at the
//!   sim's minimum spacing (5.5 m) never overlap, whatever trip they are on;
//! * the first metres of a lane (the part of the sim lane that lies inside
//!   the junction) are drawn as a curve from where the vehicle left its
//!   previous lane, driven by sim distance (not time), so spacing through a
//!   junction follows the sim's too;
//! * positions are sampled each sim tick and interpolated between ticks.

use std::collections::HashMap;

use bevy::prelude::*;
use citysim_sim::{RoadKind, SegmentId, TripPurpose, Vehicle, VehicleId, World, balance};

use super::noise::hash;
use super::palette;
use super::road_geom::{self, ROAD_Y, lane_offset};
use super::scene::WorldMaterials;
use super::vehicle_gen::{self, SHAPES};
use crate::shared::{GameSet, SimWorld, WorldCamera};

/// Sim metres over which a vehicle eases into a lane with a different
/// lateral offset where a straight road changes kind.
const LANE_BLEND_M: f32 = 8.0;
/// If a vehicle moves further than this in one tick it teleported.
const TELEPORT_M: f32 = 30.0;
/// Smallest lane compression: the sim queues at `MIN_GAP_M` (5.5 m) and
/// the longest vehicle (the truck) is 4.8 m, so 0.9 keeps 4.95 m.
const MIN_COMPRESSION: f32 = 0.9;
/// Where the centre of a vehicle waiting at a junction with crosswalks
/// stands, metres from the junction edge: the stop line is drawn 3.0-3.35 m
/// out (`road_geom`), so even a truck's nose stays behind it.
const STOP_BACK: f32 = 3.45 + vehicle_gen::TRUCK_LEN * 0.5;

/// Camera distance beyond which vehicles use the simplified model (with a
/// little hysteresis so they do not flicker between models).
const LOD_FAR: f32 = 480.0;
const LOD_NEAR: f32 = 440.0;

/// Number of car models (shape x colour); truck models follow them.
const CAR_MODELS: usize = SHAPES as usize * palette::CAR_BODIES.len();

#[derive(Resource)]
pub struct VehicleModels {
    /// Cars (`[shape * colours + colour]`), then trucks (one per livery):
    /// (detailed, distant).
    meshes: Vec<(Handle<Mesh>, Handle<Mesh>)>,
}

impl VehicleModels {
    pub fn count(&self) -> usize {
        self.meshes.len()
    }

    fn mesh(&self, model: usize, lod: bool) -> Handle<Mesh> {
        let (near, far) = &self.meshes[model];
        if lod { far.clone() } else { near.clone() }
    }
}

/// One direction of travel on a segment, as drawn.
#[derive(Debug, Clone, Copy)]
pub struct Lane {
    /// The node the lane starts at, and the unit travel direction.
    pub origin: Vec2,
    pub dir: Vec2,
    /// Sim progress drawn at `e0`, where the run's straight lane starts
    /// after the junction it came through; below it the vehicle is still
    /// crossing that junction. Negative on later lanes of a run.
    pub p0: f32,
    /// Drawn position of `p0`, metres from `origin` (negative: on an
    /// earlier segment of the run).
    pub e0: f32,
    /// Drawn metres per sim metre from `p0` on.
    pub k: f32,
    /// The node where the run starts, metres from `origin` (`<= 0`).
    pub t_min: f32,
    /// Lowest drawn position: the run's start after its junction.
    floor: f32,
    /// The run this lane belongs to (lanes of one run share a drawn
    /// coordinate: `t - t_min`).
    pub run: u32,
    /// Straight part of the lane, metres from `origin`; before / after it
    /// the lane follows the bend curves (degree-2 corners), if any.
    pub straight: (f32, f32),
    bend_in: Option<Crossing>,
    bend_out: Option<Crossing>,
}

impl Lane {
    /// Lane point and travel direction for `progress` sim metres, `offset`
    /// metres to the right of travel.
    pub fn point(&self, progress: f32, offset: f32) -> (Vec2, Vec2) {
        self.at(self.t(progress), offset)
    }

    /// Drawn metres from `origin` for `progress`. A vehicle that joined
    /// the run inside the junction behind it (leaving a lot there) waits at
    /// the junction edge until the map catches up.
    pub fn t(&self, progress: f32) -> f32 {
        (self.e0 + (progress - self.p0) * self.k).max(self.floor)
    }

    /// Lane point and travel direction `t` drawn metres from `origin`.
    fn at(&self, t: f32, offset: f32) -> (Vec2, Vec2) {
        if t < self.straight.0
            && let Some(c) = &self.bend_in
        {
            return c.along(c.len() - (self.straight.0 - t));
        }
        if t > self.straight.1
            && let Some(c) = &self.bend_out
        {
            return c.along(t - self.straight.1);
        }
        // Right of travel in sim space is (-dir.y, dir.x).
        (
            self.origin + self.dir * t + Vec2::new(-self.dir.y, self.dir.x) * offset,
            self.dir,
        )
    }
}

/// Drawn geometry of one segment's lanes.
#[derive(Debug, Clone, Copy)]
pub struct LaneGeom {
    pub kind: RoadKind,
    /// Sim length (`progress` runs `0..=len`).
    pub len: f32,
    /// Travel `a -> b`, and `b -> a`.
    pub fwd: Lane,
    pub back: Lane,
}

impl LaneGeom {
    pub fn lane(&self, forward: bool) -> &Lane {
        if forward { &self.fwd } else { &self.back }
    }

    pub fn point(&self, forward: bool, progress: f32, offset: f32) -> (Vec2, Vec2) {
        self.lane(forward).point(progress.clamp(0.0, self.len), offset)
    }
}

/// One direction of travel on one segment, for building the lane maps.
#[derive(Debug, Clone, Copy)]
struct LaneEnds {
    origin: Vec2,
    dir: Vec2,
    /// Sim length and drawn (node to node) length.
    len: f32,
    geo: f32,
    /// Setback and degree of the start and end node.
    s0: f32,
    deg0: usize,
    s1: f32,
    deg1: usize,
    start: citysim_sim::NodeId,
    end: citysim_sim::NodeId,
    kind: RoadKind,
}

/// Lane geometry for every segment, from the road layout.
///
/// Lanes are mapped per run: a run continues through degree-2 nodes
/// (straight on, or around a bend curve) and ends at junctions and dead
/// ends. Each run gets one linear map from sim progress to drawn distance
/// along it: the head of a queue at the run's end (`STOP_LINE_M` short of
/// the node) stands at the drawn stop line, the first sim metres are spent
/// crossing the junction behind (`p0`), and the map never compresses below
/// `MIN_COMPRESSION`, so vehicles queued at the sim's spacing never overlap.
pub fn lane_geoms(world: &World) -> HashMap<SegmentId, LaneGeom> {
    let layout = road_geom::layout(&world.roads);
    let mut ends: HashMap<(SegmentId, bool), LaneEnds> = HashMap::new();
    for s in &layout.segs {
        let Some(id) = s.id else { continue };
        let Some(seg) = world.roads.segments.get(id) else {
            continue;
        };
        let (len, geo, dir) = (seg.length.max(0.1), s.len(), s.dir());
        for fwd in [true, false] {
            let e = if fwd {
                (s.a, dir, s.s_a, s.deg_a, s.s_b, s.deg_b, seg.a, seg.b)
            } else {
                (s.b, -dir, s.s_b, s.deg_b, s.s_a, s.deg_a, seg.b, seg.a)
            };
            ends.insert(
                (id, fwd),
                LaneEnds {
                    origin: e.0,
                    dir: e.1,
                    len,
                    geo,
                    s0: e.2,
                    deg0: e.3,
                    s1: e.4,
                    deg1: e.5,
                    start: e.6,
                    end: e.7,
                    kind: seg.kind,
                },
            );
        }
    }
    // The lane continuing `lane` through a degree-2 node at its end (or,
    // with `back`, the lane leading into its start).
    let neighbour = |lane: (SegmentId, bool), back: bool| -> Option<(SegmentId, bool)> {
        let e = ends.get(&lane)?;
        let (deg, node) = if back { (e.deg0, e.start) } else { (e.deg1, e.end) };
        if deg != 2 {
            return None;
        }
        let other = *world.roads.nodes.get(node)?.segments.iter().find(|&&x| x != lane.0)?;
        let seg = world.roads.segments.get(other)?;
        // Leaving `node` along `other` is forward when `node` is its `a`.
        let fwd = if back { seg.b == node } else { seg.a == node };
        ends.contains_key(&(other, fwd)).then_some((other, fwd))
    };
    // The curve from lane `a`'s straight end to lane `b`'s straight start
    // around their shared degree-2 node (`None` when straight on).
    let bend = |a: &LaneEnds, b: &LaneEnds| -> Option<Crossing> {
        if a.dir.dot(b.dir) > 0.9995 {
            return None;
        }
        let off = lane_offset(b.kind, 1);
        let right = |d: Vec2| Vec2::new(-d.y, d.x);
        let pa = b.origin - a.dir * a.s1 + right(a.dir) * off;
        let pb = b.origin + b.dir * b.s0 + right(b.dir) * off;
        Some(Crossing::new(
            pa,
            turn_control(pa, a.dir, pb, b.dir),
            pb,
            0.0,
            1.0,
            None,
        ))
    };
    let stop = |deg: usize| if deg >= 3 { STOP_BACK } else { 0.0 };
    let mut keys: Vec<(SegmentId, bool)> = ends.keys().copied().collect();
    keys.sort();
    let mut done: std::collections::HashSet<(SegmentId, bool)> = std::collections::HashSet::new();
    let mut lanes: Vec<((SegmentId, bool), Lane)> = Vec::new();
    let mut runs = 0u32;
    for start in keys {
        if done.contains(&start) {
            continue;
        }
        // Walk back to the start of the run (a loop starts anywhere).
        let mut first = start;
        while let Some(prev) = neighbour(first, true) {
            if prev == start {
                break;
            }
            first = prev;
        }
        let mut run = vec![first];
        while let Some(next) = neighbour(*run.last().expect("run"), false) {
            if run.contains(&next) {
                break;
            }
            run.push(next);
        }
        let es: Vec<LaneEnds> = run.iter().map(|l| ends[l]).collect();
        let bends: Vec<Option<Crossing>> = es.windows(2).map(|w| bend(&w[0], &w[1])).collect();
        // Drawn offset of each lane's origin along the run: through a bend
        // the run follows the curve instead of the corner.
        let mut offs = vec![0.0f32; es.len()];
        for i in 1..es.len() {
            let corner = match &bends[i - 1] {
                Some(c) => c.len() - es[i - 1].s1 - es[i].s0,
                None => 0.0,
            };
            offs[i] = offs[i - 1] + es[i - 1].geo + corner;
        }
        let (head, tail) = (es[0], es[es.len() - 1]);
        let sim_total: f32 = es.iter().map(|e| e.len).sum();
        let e0 = head.s0;
        // The head of a queue (sim: `STOP_LINE_M` before the node) stands
        // at the drawn stop line.
        let e1 = (offs[es.len() - 1] + tail.geo - tail.s1 - stop(tail.deg1)).max(e0 + 0.1);
        let p_stop = (sim_total - balance::STOP_LINE_M).max(0.5);
        // Sim distance spent crossing the junction behind us: about what
        // the drive from the previous stop line to `e0` measures on screen,
        // so a vehicle keeps roughly sim speed through the junction.
        let crossing = if head.deg0 >= 3 {
            (head.s0 * 2.0 + stop(head.deg0) - balance::STOP_LINE_M).max(0.0)
        } else {
            0.0
        };
        let drawn = e1 - e0;
        let p0 = crossing
            .max(p_stop - drawn / MIN_COMPRESSION)
            .clamp(0.0, (p_stop - 0.5).max(0.0));
        let k = drawn / (p_stop - p0).max(0.1);
        let mut sim_off = 0.0;
        for (i, e) in es.iter().enumerate() {
            done.insert(run[i]);
            let bend_in = if i > 0 { bends[i - 1] } else { None };
            let bend_out = bends.get(i).copied().flatten();
            lanes.push((
                run[i],
                Lane {
                    origin: e.origin,
                    dir: e.dir,
                    p0: p0 - sim_off,
                    e0: e0 - offs[i],
                    k,
                    t_min: -offs[i],
                    floor: e0 - offs[i],
                    run: runs,
                    straight: (
                        if bend_in.is_some() { e.s0 } else { f32::NEG_INFINITY },
                        if bend_out.is_some() {
                            e.geo - e.s1
                        } else {
                            f32::INFINITY
                        },
                    ),
                    bend_in,
                    bend_out,
                },
            ));
            sim_off += e.len;
        }
        runs += 1;
    }
    let mut out: HashMap<SegmentId, LaneGeom> = HashMap::new();
    for ((id, fwd), lane) in lanes {
        let e = ends[&(id, fwd)];
        let g = out.entry(id).or_insert(LaneGeom {
            kind: e.kind,
            len: e.len,
            fwd: lane,
            back: lane,
        });
        if fwd {
            g.fwd = lane;
        } else {
            g.back = lane;
        }
    }
    out
}

/// Samples of a crossing curve's arc length.
const ARC_STEPS: usize = 12;

/// A vehicle driving through a junction onto its current lane: a curve
/// from where it left the previous lane to the start of the straight lane,
/// driven by sim distance at an even speed along the curve.
#[derive(Debug, Clone, Copy)]
struct Crossing {
    p0: Vec2,
    p1: Vec2,
    p2: Vec2,
    /// Sim progress (on the new lane) where the curve starts (negative:
    /// before the node) and ends.
    u0: f32,
    u1: f32,
    /// Arc length from `p0` at Bezier parameter `i / ARC_STEPS`.
    arc: [f32; ARC_STEPS + 1],
    /// The run the vehicle left.
    from_run: Option<u32>,
    /// Only easing sideways onto the next lane of the same run.
    blend: bool,
    /// The run the curve leads onto.
    to_run: Option<u32>,
}

impl Crossing {
    fn new(p0: Vec2, p1: Vec2, p2: Vec2, u0: f32, u1: f32, from_run: Option<u32>) -> Self {
        let mut arc = [0.0; ARC_STEPS + 1];
        let mut last = p0;
        for i in 1..=ARC_STEPS {
            let (p, _) = bezier(p0, p1, p2, i as f32 / ARC_STEPS as f32);
            arc[i] = arc[i - 1] + last.distance(p);
            last = p;
        }
        Crossing {
            p0,
            p1,
            p2,
            u0,
            u1,
            arc,
            from_run,
            blend: false,
            to_run: None,
        }
    }

    fn len(&self) -> f32 {
        self.arc[ARC_STEPS]
    }

    /// Point and direction `d` metres along the curve, continuing straight
    /// on past either end.
    fn along(&self, d: f32) -> (Vec2, Vec2) {
        let len = self.len();
        if d < 0.0 {
            let dir = (self.p1 - self.p0)
                .normalize_or(self.p2 - self.p0)
                .normalize_or(Vec2::X);
            return (self.p0 + dir * d, dir);
        }
        if d > len {
            let dir = (self.p2 - self.p1)
                .normalize_or(self.p2 - self.p0)
                .normalize_or(Vec2::X);
            return (self.p2 + dir * (d - len), dir);
        }
        let (p, h) = self.at_arc(d);
        (p, Vec2::from_angle(h))
    }

    /// Position and heading `d` metres along the curve.
    fn at_arc(&self, d: f32) -> (Vec2, f32) {
        let target = d.clamp(0.0, self.len());
        let i = self.arc.partition_point(|&a| a < target).clamp(1, ARC_STEPS);
        let (a0, a1) = (self.arc[i - 1], self.arc[i]);
        let t = (i as f32 - 1.0 + if a1 > a0 { (target - a0) / (a1 - a0) } else { 1.0 }) / ARC_STEPS as f32;
        let (p, d) = bezier(self.p0, self.p1, self.p2, t);
        let d = if d.length_squared() > 1e-6 {
            d
        } else {
            self.p2 - self.p0
        };
        (p, d.y.atan2(d.x))
    }

    /// Distance along the curve at sim progress `u`. At both ends the
    /// curve is driven at the lanes' scale (`k` on the new lane, 1 on the
    /// old one), so spacing behind and ahead follows the sim; the
    /// difference between the curve's length and its sim distance is taken
    /// up in the middle of the turn (monotone cubic Hermite).
    fn arc_at(&self, u: f32, k: f32) -> f32 {
        let len = self.len();
        let total = (self.u1 - self.u0).max(1e-3);
        // x: remaining sim distance, 0 at the end of the curve, 1 at its start.
        let x = ((self.u1 - u) / total).clamp(0.0, 1.0);
        let delta = (len / total).max(1e-3);
        let (mut a, mut b) = (k / delta, 1.0 / delta);
        let m = a * a + b * b;
        if m > 9.0 {
            let tau = 3.0 / m.sqrt();
            a *= tau;
            b *= tau;
        }
        let (x2, x3) = (x * x, x * x * x);
        let remaining = (x3 - 2.0 * x2 + x) * a + (-2.0 * x3 + 3.0 * x2) + (x3 - x2) * b;
        len - remaining.clamp(0.0, 1.0) * len
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Car,
    Truck,
}

#[derive(Component)]
struct VehicleVisual {
    prev: (Vec2, f32),
    cur: (Vec2, f32),
    seg: Option<(SegmentId, bool)>,
    /// Last sampled progress (for the next junction crossing).
    progress: f32,
    seed: u32,
    crossing: Option<Crossing>,
    /// Distance along the crossing curve (while crossing).
    arc: f32,
    /// Where the vehicle is drawn on its straight run (`None` while
    /// crossing a junction or off the network).
    lane: Option<RunPos>,
    kind: Kind,
    model: usize,
    /// The model the entity's mesh shows.
    mesh_model: Option<usize>,
    lod: bool,
}

/// A vehicle's drawn place on a straight run.
#[derive(Debug, Clone, Copy)]
struct RunPos {
    run: u32,
    /// Drawn metres along the run.
    t: f32,
    seg: SegmentId,
    forward: bool,
    offset: f32,
}

impl VehicleVisual {
    fn new(v: &Vehicle, kind: Kind, lanes: &HashMap<SegmentId, LaneGeom>) -> Self {
        let (p, d) = lane_sample(lanes, v);
        let h = d.y.atan2(d.x);
        VehicleVisual {
            prev: (p, h),
            cur: (p, h),
            seg: v.segment.map(|s| (s, v.forward)),
            progress: v.progress,
            seed: v.seed,
            crossing: None,
            arc: 0.0,
            lane: run_pos(lanes, v),
            kind,
            model: model_index(v.seed, kind),
            mesh_model: None,
            lod: false,
        }
    }

    /// Take the sim's newest state of the vehicle (once per sim tick). A
    /// reused id (new seed) starts afresh; a new trip purpose only swaps
    /// the model (`model` changes).
    fn step(&mut self, v: &Vehicle, kind: Kind, lanes: &HashMap<SegmentId, LaneGeom>) {
        if self.seed != v.seed {
            *self = VehicleVisual {
                lod: self.lod,
                mesh_model: self.mesh_model,
                ..VehicleVisual::new(v, kind, lanes)
            };
            return;
        }
        let now = advance(self, v, lanes);
        self.prev = self.cur;
        self.cur = now;
        if self.prev.0.distance(self.cur.0) > TELEPORT_M {
            self.prev = self.cur;
        }
        if kind != self.kind {
            self.kind = kind;
            self.model = model_index(v.seed, kind);
        }
    }
}

#[derive(Default)]
struct SyncState {
    entities: HashMap<VehicleId, Entity>,
    lanes: HashMap<SegmentId, LaneGeom>,
    roads_key: Option<(u64, u64)>,
    last_fixed: f64,
    last_tick: u64,
    generation: u64,
    /// The sim advanced on the most recent fixed update.
    moving: bool,
}

pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup)
            .add_systems(Update, sync_vehicles.in_set(GameSet::Render));
    }
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    let mut handles = Vec::new();
    for shape in 0..SHAPES {
        for &c in &palette::CAR_BODIES {
            handles.push((
                meshes.add(vehicle_gen::car_model(c, shape, false).build()),
                meshes.add(vehicle_gen::car_model(c, shape, true).build()),
            ));
        }
    }
    for &(cab, cargo, stripe) in &palette::TRUCK_LIVERIES {
        handles.push((
            meshes.add(vehicle_gen::truck_model(cab, cargo, stripe, false).build()),
            meshes.add(vehicle_gen::truck_model(cab, cargo, stripe, true).build()),
        ));
    }
    commands.insert_resource(VehicleModels { meshes: handles });
}

fn kind_of(world: &World, id: VehicleId) -> Kind {
    match world.traffic.trips.get(id).map(|t| t.purpose) {
        Some(TripPurpose::Goods) => Kind::Truck,
        _ => Kind::Car,
    }
}

fn model_index(seed: u32, kind: Kind) -> usize {
    let h = hash(seed ^ 0x51ED_270B);
    if kind == Kind::Truck {
        return CAR_MODELS + (h / 16) as usize % palette::TRUCK_LIVERIES.len();
    }
    // Mostly sedans and hatchbacks, some vans and pickups.
    let shape = match h % 10 {
        0..=4 => 0,
        5..=7 => 1,
        8 => 2,
        _ => 3,
    };
    let colours = palette::CAR_BODIES.len();
    shape * colours + (h / 16) as usize % colours
}

/// Length and width of a vehicle model, metres.
fn model_size(model: usize) -> (f32, f32) {
    if model >= CAR_MODELS {
        (vehicle_gen::TRUCK_LEN, vehicle_gen::TRUCK_WIDTH)
    } else {
        vehicle_gen::car_size((model / palette::CAR_BODIES.len()) as u32)
    }
}

/// Where the sim says the vehicle is on its straight lane (position and
/// travel direction).
fn lane_sample(lanes: &HashMap<SegmentId, LaneGeom>, v: &Vehicle) -> (Vec2, Vec2) {
    if let Some(g) = v.segment.and_then(|s| lanes.get(&s)) {
        return g.point(v.forward, v.progress, lane_offset(g.kind, v.seed));
    }
    let dir = Vec2::from_angle(v.heading);
    let off = lane_offset(RoadKind::Street, v.seed);
    (Vec2::from(v.pos) + Vec2::new(-dir.y, dir.x) * off, dir)
}

fn bezier(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> (Vec2, Vec2) {
    let u = 1.0 - t;
    let p = p0 * u * u + p1 * 2.0 * u * t + p2 * t * t;
    let d = (p1 - p0) * 2.0 * u + (p2 - p1) * 2.0 * t;
    (p, d)
}

/// Control point for a turn from `p0` heading `d0` into `p2` heading `d2`:
/// where the two lanes meet, or the midpoint when they are (nearly) parallel.
fn turn_control(p0: Vec2, d0: Vec2, p2: Vec2, d2: Vec2) -> Vec2 {
    let denom = d0.perp_dot(d2);
    let mid = (p0 + p2) * 0.5;
    if denom.abs() < 0.05 {
        return mid;
    }
    let t = (p2 - p0).perp_dot(d2) / denom;
    let p1 = p0 + d0 * t;
    if !(0.0..=25.0).contains(&t) || (p1 - p2).dot(d2) > 0.0 {
        mid
    } else {
        p1
    }
}

/// New visual sample for a vehicle (position, heading), updating its state.
fn advance(vis: &mut VehicleVisual, v: &Vehicle, lanes: &HashMap<SegmentId, LaneGeom>) -> (Vec2, f32) {
    let seg = v.segment.map(|s| (s, v.forward));
    let geom = v.segment.and_then(|s| lanes.get(&s));
    if seg != vis.seg {
        vis.crossing = None;
        if let (Some((old, _)), Some(g)) = (vis.seg, geom) {
            // Sim distance from the last sample to the node just crossed.
            let left = lanes.get(&old).map_or(0.0, |o| o.len - vis.progress).max(0.0);
            let lane = g.lane(v.forward);
            let off = lane_offset(g.kind, v.seed);
            // Through a junction or bend: curve to the start of the straight
            // lane. Straight on through a plain node: the lane map runs on,
            // but ease over a few metres if the lane offset changes (e.g.
            // from a highway lane to a street).
            let u1 = if lane.p0 > 0.0 {
                lane.p0
            } else {
                (v.progress + LANE_BLEND_M).min(g.len)
            };
            let (p2, d2) = lane.point(u1, off);
            let p0 = vis.cur.0;
            let d0 = Vec2::from_angle(vis.cur.1);
            let (now, _) = lane.point(v.progress, off);
            let needed = lane.p0 > 0.0 || p0.distance(now) > 0.25;
            if needed && p0.distance(p2) < TELEPORT_M && u1 > -left + 0.1 {
                let from_run = vis.lane.map(|l| l.run);
                let mut c = Crossing::new(p0, turn_control(p0, d0, p2, d2), p2, -left, u1, from_run);
                c.blend = lane.p0 <= 0.0;
                c.to_run = Some(lane.run);
                vis.crossing = Some(c);
            }
        }
        vis.seg = seg;
    }
    vis.progress = v.progress;
    if let Some(c) = vis.crossing {
        if v.progress < c.u1 {
            // Easing sideways within a run still keeps its place in the
            // run's queue.
            vis.lane = c.from_run.filter(|_| c.blend).and_then(|_| run_pos(lanes, v));
            let k = geom.map_or(1.0, |g| g.lane(v.forward).k);
            vis.arc = c.arc_at(v.progress, k);
            return c.at_arc(vis.arc);
        }
        vis.crossing = None;
    }
    vis.lane = run_pos(lanes, v);
    let (p, d) = lane_sample(lanes, v);
    (p, d.y.atan2(d.x))
}

fn run_pos(lanes: &HashMap<SegmentId, LaneGeom>, v: &Vehicle) -> Option<RunPos> {
    let g = lanes.get(&v.segment?)?;
    let lane = g.lane(v.forward);
    Some(RunPos {
        run: lane.run,
        t: lane.t(v.progress.clamp(0.0, g.len)) - lane.t_min,
        seg: v.segment?,
        forward: v.forward,
        offset: lane_offset(g.kind, v.seed),
    })
}

/// Drawn gap kept between the centres of two vehicles one behind the
/// other: half of each length plus a little air.
fn spacing(a: usize, b: usize) -> f32 {
    (model_size(a).0 + model_size(b).0) * 0.5 + 0.35
}

/// Keeps drawn vehicles on a run from touching where the sim lets them
/// come closer than a car length (the head of a queue moves up to the stop
/// line while the car ahead has only just crossed the node): each vehicle
/// is held back behind the one ahead, and the front one behind any vehicle
/// still crossing the junction it left. Runs once per sim tick on the
/// fresh samples.
fn keep_spacing<'a>(all: impl Iterator<Item = &'a mut VehicleVisual>, lanes: &HashMap<SegmentId, LaneGeom>) {
    let mut all: Vec<&mut VehicleVisual> = all.collect();
    // Vehicles leaving each run (still crossing): position and model.
    let mut leaving: HashMap<u32, Vec<(Vec2, usize)>> = HashMap::new();
    let mut on_run: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut crossing: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, v) in all.iter().enumerate() {
        if let Some(l) = v.lane {
            on_run.entry(l.run).or_default().push(i);
        } else if let Some(r) = v.crossing.and_then(|c| c.from_run) {
            crossing.entry(r).or_default().push(i);
        }
    }
    let mut crossing_all: Vec<(u32, usize)> = Vec::new();
    // Vehicles that left one run one after the other (the sim lets a car
    // close up on the one ahead as soon as that one has passed the node)
    // are held back along their own curves.
    for (run, mut idx) in crossing {
        idx.sort_by(|&a, &b| all[b].arc.total_cmp(&all[a].arc));
        for n in 0..idx.len() {
            let v = &*all[idx[n]];
            let Some(c) = v.crossing else { continue };
            let mut arc = v.arc;
            for &j in &idx[..n] {
                let a = &*all[j];
                let gap = spacing(a.model, v.model);
                let d = a.cur.0.distance(v.cur.0);
                if d < gap {
                    arc = arc.min(v.arc - (gap - d)).max(0.0);
                }
            }
            let v = &mut *all[idx[n]];
            if arc < v.arc {
                v.arc = arc;
                v.cur = c.at_arc(arc);
            }
        }
        crossing_all.extend(idx.into_iter().map(|i| (run, i)));
    }
    // Paths crossing inside a junction (the sim does not model conflicts
    // there): the vehicle driving into the other one waits on its curve.
    crossing_all.sort_by(|a, b| all[a.1].cur.0.x.total_cmp(&all[b.1].cur.0.x));
    for _ in 0..2 {
        for x in 0..crossing_all.len() {
            for y in x + 1..crossing_all.len() {
                let (i, j) = (crossing_all[x].1, crossing_all[y].1);
                let (a, b) = (&*all[i], &*all[j]);
                if b.cur.0.x - a.cur.0.x > 6.0 {
                    break;
                }
                if a.cur.0.distance(b.cur.0) > 6.0 || !footprints_overlap(a, b, 0.1) {
                    continue;
                }
                // Who is driving into whom.
                let into = |from: &VehicleVisual, to: &VehicleVisual| {
                    (to.cur.0 - from.cur.0).dot(Vec2::from_angle(from.cur.1))
                };
                let back = if into(a, b) > into(b, a) { i } else { j };
                let other = if back == i { j } else { i };
                let need = spacing(all[other].model, all[back].model) - all[other].cur.0.distance(all[back].cur.0);
                let v = &mut *all[back];
                if let Some(c) = v.crossing {
                    v.arc = (v.arc - need.max(0.3)).max(0.0);
                    v.cur = c.at_arc(v.arc);
                }
            }
        }
    }
    for &(run, i) in &crossing_all {
        leaving.entry(run).or_default().push((all[i].cur.0, all[i].model));
    }
    let mut rear: HashMap<u32, Option<usize>> = HashMap::new();
    for (run, mut idx) in on_run {
        idx.sort_by(|&a, &b| {
            let (ta, tb) = (all[a].lane.map_or(0.0, |l| l.t), all[b].lane.map_or(0.0, |l| l.t));
            tb.total_cmp(&ta)
        });
        let mut ahead: Option<(f32, usize)> = None;
        for (n, &i) in idx.iter().enumerate() {
            let v = &mut *all[i];
            let Some(l) = v.lane else { continue };
            let mut t = l.t;
            if n == 0 {
                for &(p, model) in leaving.get(&run).map(Vec::as_slice).unwrap_or(&[]) {
                    let gap = spacing(model, v.model);
                    let d = p.distance(v.cur.0);
                    if d < gap {
                        t = t.min(l.t - (gap - d));
                    }
                }
            }
            if let Some((ta, model)) = ahead {
                t = t.min(ta - spacing(model, v.model));
            }
            if t < l.t
                && let Some(g) = lanes.get(&l.seg)
            {
                let lane = g.lane(l.forward);
                let (p, d) = lane.at(t + lane.t_min, l.offset);
                v.cur = (p, d.y.atan2(d.x));
                v.lane = Some(RunPos { t, ..l });
            }
            ahead = Some((t, v.model));
        }
        rear.insert(run, idx.last().copied());
    }
    // Vehicles still on their curve into a run stay behind its last one.
    for (_, i) in crossing_all {
        let Some(c) = all[i].crossing else { continue };
        let Some(Some(r)) = c.to_run.and_then(|run| rear.get(&run)) else {
            continue;
        };
        let (rp, rm) = (all[*r].cur.0, all[*r].model);
        let v = &mut *all[i];
        let gap = spacing(rm, v.model);
        let d = rp.distance(v.cur.0);
        if d < gap {
            v.arc = (v.arc - (gap - d)).max(0.0);
            v.cur = c.at_arc(v.arc);
        }
    }
}

/// Oriented footprints `(centre, heading, half length, half width)`
/// overlap (separating axis test), with `slack` metres of tolerance.
fn boxes_overlap(a: (Vec2, f32, f32, f32), b: (Vec2, f32, f32, f32), slack: f32) -> bool {
    let axes = |h: f32| [Vec2::from_angle(h), Vec2::from_angle(h).perp()];
    let extent = |bx: (Vec2, f32, f32, f32), ax: Vec2| {
        let (f, r) = (Vec2::from_angle(bx.1), Vec2::from_angle(bx.1).perp());
        bx.2 * f.dot(ax).abs() + bx.3 * r.dot(ax).abs()
    };
    for ax in axes(a.1).into_iter().chain(axes(b.1)) {
        let d = (a.0 - b.0).dot(ax).abs();
        if d + slack >= extent(a, ax) + extent(b, ax) {
            return false;
        }
    }
    true
}

/// Whether two drawn vehicles' footprints overlap.
fn footprints_overlap(a: &VehicleVisual, b: &VehicleVisual, slack: f32) -> bool {
    let fp = |v: &VehicleVisual| {
        let (l, w) = model_size(v.model);
        (v.cur.0, v.cur.1, l * 0.5, w * 0.5)
    };
    boxes_overlap(fp(a), fp(b), slack)
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    a + d * t
}

#[allow(clippy::too_many_arguments)]
fn sync_vehicles(
    mut commands: Commands,
    sim: Res<SimWorld>,
    fixed: Res<Time<Fixed>>,
    models: Res<VehicleModels>,
    mats: Res<WorldMaterials>,
    mut state: Local<SyncState>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    mut q: Query<(&mut VehicleVisual, &mut Transform, &mut Mesh3d)>,
) {
    let cam = cams.single().map_or(Vec3::ZERO, |c| c.translation());
    let world = &sim.world;
    if state.generation != sim.generation {
        for (_, e) in state.entities.drain() {
            commands.entity(e).despawn();
        }
        state.generation = sim.generation;
    }
    let roads_key = sim.key(world.versions.roads);
    if state.roads_key != Some(roads_key) {
        state.lanes = lane_geoms(world);
        state.roads_key = Some(roads_key);
    }
    let vehicles = &world.vehicles;
    state.entities.retain(|id, e| {
        let keep = vehicles.contains_key(*id);
        if !keep {
            commands.entity(*e).despawn();
        }
        keep
    });
    // Did a fixed update run since last frame, and did the sim advance?
    let fixed_now = fixed.elapsed_secs_f64();
    let stepped = fixed_now != state.last_fixed;
    let tick = world.time.tick;
    let advanced = tick != state.last_tick;
    state.last_fixed = fixed_now;
    state.last_tick = tick;
    if stepped {
        state.moving = advanced;
    }
    let alpha = if state.moving {
        fixed.overstep_fraction().clamp(0.0, 1.0)
    } else {
        1.0
    };
    let state = &mut *state;
    let tick_now = stepped && advanced;
    for (id, v) in vehicles.iter() {
        let kind = kind_of(world, id);
        match state.entities.get(&id) {
            Some(&e) => {
                let Ok((mut vis, _, _)) = q.get_mut(e) else {
                    continue;
                };
                if tick_now {
                    vis.step(v, kind, &state.lanes);
                } else if stepped {
                    vis.prev = vis.cur;
                }
            }
            None => {
                let mut vis = VehicleVisual::new(v, kind, &state.lanes);
                let (p, h) = vis.cur;
                let pos = Vec3::new(p.x, ROAD_Y, p.y);
                vis.lod = cam.distance(pos) > LOD_FAR;
                let mesh = models.mesh(vis.model, vis.lod);
                let e = commands
                    .spawn((
                        vis,
                        Mesh3d(mesh),
                        MeshMaterial3d(mats.lit.clone()),
                        Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(-h)),
                    ))
                    .id();
                state.entities.insert(id, e);
            }
        }
    }
    if tick_now {
        keep_spacing(q.iter_mut().map(|(vis, _, _)| vis.into_inner()), &state.lanes);
    }
    for (mut vis, mut t, mut mesh) in &mut q {
        let p = vis.prev.0.lerp(vis.cur.0, alpha);
        let h = lerp_angle(vis.prev.1, vis.cur.1, alpha);
        let new = Transform::from_xyz(p.x, ROAD_Y, p.y).with_rotation(Quat::from_rotation_y(-h));
        if *t != new {
            *t = new;
        }
        let d = cam.distance(new.translation);
        let lod = if vis.lod { d > LOD_NEAR } else { d > LOD_FAR };
        if lod != vis.lod || vis.mesh_model != Some(vis.model) {
            vis.lod = lod;
            vis.mesh_model = Some(vis.model);
            mesh.0 = models.mesh(vis.model, lod);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use citysim_sim::{Cell, Command};

    #[test]
    fn lane_is_to_the_right_of_travel() {
        // Heading +x in sim (east). Right-hand traffic: the lane is on the
        // driver's right, i.e. (heading x up) in 3D -> +z in Bevy.
        let heading = 0.0f32;
        let (s, c) = heading.sin_cos();
        let right = Vec2::new(-s, c);
        let forward3 = Vec3::new(c, 0.0, s);
        let right3 = forward3.cross(Vec3::Y);
        assert!(right3.distance(Vec3::new(right.x, 0.0, right.y)) < 1e-6);
    }

    #[test]
    fn angle_lerp_takes_the_short_way() {
        let a = lerp_angle(3.0, -3.0, 0.5);
        assert!((a.abs() - std::f32::consts::PI).abs() < 0.3, "{a}");
    }

    fn town() -> World {
        let mut w = World::new(1);
        w.economy.money = 1_000_000;
        for (a, b) in [((11, 128), (40, 128)), ((30, 110), (30, 150)), ((40, 128), (55, 113))] {
            w.apply(Command::BuildRoad {
                from: Cell::new(a.0, a.1),
                to: Cell::new(b.0, b.1),
            })
            .expect("road");
        }
        w
    }

    #[test]
    fn queued_cars_never_overlap_and_stop_before_the_junction() {
        let w = town();
        let lanes = lane_geoms(&w);
        let junction = Vec2::from(Cell::new(30, 128).center());
        for (sid, g) in &lanes {
            let seg = &w.roads.segments[*sid];
            for forward in [true, false] {
                let len = g.len;
                // Vehicles at the sim's minimum queue spacing from the lane end.
                let mut p = len - balance::STOP_LINE_M;
                let mut last: Option<Vec2> = None;
                // Below `p0` a vehicle is still crossing the junction behind.
                while p >= g.lane(forward).p0.max(0.0) {
                    let (pos, _) = g.point(forward, p, 1.35);
                    if let Some(l) = last {
                        let gap = l.distance(pos);
                        assert!(gap >= vehicle_gen::TRUCK_LEN, "overlap on {sid:?}: {gap}");
                    }
                    last = Some(pos);
                    p -= balance::MIN_GAP_M;
                }
                // The head of the queue does not stand inside a crossing.
                let end = if forward { seg.b } else { seg.a };
                if w.roads.nodes[end].segments.len() >= 3 && Vec2::from(w.roads.nodes[end].pos) == junction {
                    let (head, _) = g.point(forward, len - 0.6, 1.35);
                    assert!(head.distance(junction) > 6.0, "head car in the junction");
                }
            }
        }
    }

    #[test]
    fn diagonal_lanes_are_offset_perpendicular_to_travel() {
        let w = town();
        let lanes = lane_geoms(&w);
        for g in lanes.values() {
            for forward in [true, false] {
                // Mid-lane (lane ends may lie on a bend curve).
                let (on, dir) = g.point(forward, g.len * 0.5, 0.0);
                let (off, _) = g.point(forward, g.len * 0.5, 1.35);
                let side = off - on;
                assert!((side.length() - 1.35).abs() < 1e-3);
                assert!(side.dot(dir).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn turn_curve_starts_and_ends_on_the_lanes() {
        let p0 = Vec2::new(0.0, 0.0);
        let d0 = Vec2::X;
        let p2 = Vec2::new(8.0, 8.0);
        let d2 = Vec2::Y;
        let p1 = turn_control(p0, d0, p2, d2);
        assert!(p1.distance(Vec2::new(8.0, 0.0)) < 1e-4);
        let (a, _) = bezier(p0, p1, p2, 0.0);
        let (b, db) = bezier(p0, p1, p2, 1.0);
        assert!(a.distance(p0) < 1e-5 && b.distance(p2) < 1e-5);
        assert!(db.normalize().distance(d2) < 1e-4);
    }

    #[test]
    fn box_overlap_basics() {
        let a = (Vec2::ZERO, 0.0, 2.0, 0.9);
        assert!(boxes_overlap(a, (Vec2::new(3.9, 0.0), 0.0, 2.0, 0.9), 0.0));
        assert!(!boxes_overlap(a, (Vec2::new(4.1, 0.0), 0.0, 2.0, 0.9), 0.0));
        assert!(!boxes_overlap(a, (Vec2::new(0.0, 2.0), 0.0, 2.0, 0.9), 0.0));
        assert!(boxes_overlap(a, (Vec2::new(2.5, 1.5), 1.0, 2.0, 0.9), 0.0));
    }

    /// Runs `world` for `ticks` sim ticks, drawing every vehicle exactly as
    /// `sync_vehicles` does (one sample per tick), and returns the overlap
    /// events (pairs of drawn vehicles whose footprints overlap).
    fn drawn_overlaps(world: &mut World, warmup: u32, ticks: u32, slack: f32) -> (usize, usize, Vec<String>) {
        let mut vis: HashMap<VehicleId, VehicleVisual> = HashMap::new();
        let mut lanes = lane_geoms(world);
        let mut roads_v = world.versions.roads;
        let (mut events, mut samples) = (0, 0);
        let mut examples = Vec::new();
        let mut cats: HashMap<String, usize> = HashMap::new();
        for tick in 0..warmup + ticks {
            world.step();
            if world.versions.roads != roads_v {
                lanes = lane_geoms(world);
                roads_v = world.versions.roads;
            }
            vis.retain(|id, _| world.vehicles.contains_key(*id));
            for (id, v) in world.vehicles.iter() {
                let kind = kind_of(world, id);
                match vis.get_mut(&id) {
                    Some(s) => s.step(v, kind, &lanes),
                    None => {
                        vis.insert(id, VehicleVisual::new(v, kind, &lanes));
                    }
                }
            }
            if std::env::var("NO_SPACING").is_err() {
                keep_spacing(vis.values_mut(), &lanes);
            }
            let boxes: Vec<_> = vis
                .iter()
                .map(|(id, s)| {
                    let (l, w) = model_size(s.model);
                    (*id, (s.cur.0, s.cur.1, l * 0.5, w * 0.5))
                })
                .collect();
            if tick < warmup {
                continue;
            }
            samples += boxes.len();
            for i in 0..boxes.len() {
                for j in i + 1..boxes.len() {
                    let (a, b) = (boxes[i].1, boxes[j].1);
                    if a.0.distance(b.0) < 6.0 && boxes_overlap(a, b, slack) {
                        events += 1;
                        let cat = |id: VehicleId| {
                            let v = &world.vehicles[id];
                            let p0 = v
                                .segment
                                .and_then(|x| lanes.get(&x))
                                .map_or(0.0, |g| g.lane(v.forward).p0);
                            match (vis[&id].crossing.is_some(), v.progress < p0) {
                                (true, _) => "curve",
                                (false, true) => "below-p0",
                                (false, false) => "lane",
                            }
                        };
                        let (ca, cb) = (cat(boxes[i].0), cat(boxes[j].0));
                        let same = world.vehicles[boxes[i].0].segment == world.vehicles[boxes[j].0].segment;
                        let key = format!("{}+{} same-seg {}", ca.min(cb), ca.max(cb), same);
                        *cats.entry(key.clone()).or_default() += 1;
                        let want = std::env::var("OVERLAP_CAT").unwrap_or_default();
                        if examples.len() < 30 && key.starts_with(&want) && events % 5 == 1 {
                            let d = |id: VehicleId| {
                                let v = &world.vehicles[id];
                                let s = &vis[&id];
                                let len = v.segment.and_then(|x| lanes.get(&x)).map_or(0.0, |g| g.len);
                                format!(
                                    "seg {:?} fwd {} p {:.1}/{:.1} v {:.1} cross {} at ({:.1},{:.1}) h {:.2}",
                                    v.segment,
                                    v.forward,
                                    v.progress,
                                    len,
                                    v.speed,
                                    s.crossing.is_some(),
                                    s.cur.0.x,
                                    s.cur.0.y,
                                    s.cur.1
                                )
                            };
                            examples.push(format!(
                                "t{} gap {:.2}: [{}] vs [{}]",
                                world.time.tick,
                                a.0.distance(b.0),
                                d(boxes[i].0),
                                d(boxes[j].0)
                            ));
                        }
                    }
                }
            }
        }
        let mut cats: Vec<_> = cats.into_iter().collect();
        cats.sort();
        for (k, n) in cats {
            examples.push(format!("{k}: {n}"));
        }
        (events, samples, examples)
    }

    #[test]
    #[ignore = "diagnostic: cargo test ... jam_overlaps -- --ignored --nocapture"]
    fn jam_overlaps() {
        let mut w = World::new(1);
        citysim_sim::scenarios::prebuilt(&mut w, |w| citysim_sim::scenarios::two_districts(w, false));
        while w.time.day < 3 || w.time.hour() < 7.5 {
            w.step();
        }
        if std::env::var("DUMP_LANES").is_ok() {
            let lanes = lane_geoms(&w);
            for (id, seg) in w.roads.segments.iter() {
                let (na, nb) = (&w.roads.nodes[seg.a], &w.roads.nodes[seg.b]);
                println!(
                    "{id:?} {:?}(deg {}) -> {:?}(deg {}) len {} lanes {:?}",
                    na.pos,
                    na.segments.len(),
                    nb.pos,
                    nb.segments.len(),
                    seg.length,
                    lanes.get(&id)
                );
            }
        }
        let (events, samples, ex) = drawn_overlaps(&mut w, 100, 2400, 0.15);
        for e in &ex {
            println!("{e}");
        }
        println!("{events} overlap events in {samples} vehicle samples");
    }

    #[test]
    #[ignore = "diagnostic: cargo test ... city_overlaps -- --ignored --nocapture"]
    fn city_overlaps() {
        use citysim_sim::Zone;
        use citysim_sim::scenarios::GridPlayer;
        let mut w = World::new(1);
        let mut player = GridPlayer::new(&[Zone::Residential, Zone::Commercial, Zone::Industrial]);
        while w.stats.population < 5_000 && w.time.day <= 40 {
            player.act(&mut w);
            w.step();
        }
        while w.time.hour() < 7.5 || w.time.hour() > 8.0 {
            w.step();
        }
        let (events, samples, ex) = drawn_overlaps(&mut w, 100, 2400, 0.15);
        for e in &ex {
            println!("{e}");
        }
        println!("{events} overlap events in {samples} vehicle samples");
    }
}
