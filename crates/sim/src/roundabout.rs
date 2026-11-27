//! Roundabouts: a junction upgraded to a one-way ring inside its own box.
//!
//! # Representation
//!
//! A roundabout is a setting of a junction node (`JunctionState::ring`,
//! set with `Command::SetRoundabout`), not road pieces: the road graph,
//! the zoning lattices along the roads and the save body stay as they are.
//! The arms are shortened to the ring (their stop lines become give-way
//! lines just outside it) and the junction's movements become ring paths:
//! an entry curve from the give-way line onto one of the ring's lanes, an
//! arc of that lane in the direction of circulation and an exit curve onto
//! the out-lane of the arm it leaves by ([`RingPath`]). The land the ring
//! needs beyond the old junction is a disc round the node
//! ([`RingDims::reserve_r`]): zone cells under it are blocked as under a
//! road, buildings on it are taken down when the ring is built.
//!
//! Traffic drives on the right, so the ring runs with the island on the
//! driver's left: in sim axes (the right of `d` is `(-d.y, d.x)`) the
//! angle round the centre *decreases* along the circulation.
//!
//! # Rules of the ring (see `drive`)
//!
//! * Entering traffic gives way to every car whose path will pass the
//!   entry's merge point (on the ring, or entering upstream): it goes only
//!   when that car reaches the point at least `RING_SAFETY_S` after the
//!   entering car's rear has cleared it, judged from both cars' speed and
//!   distance (and acceleration from a stand).
//! * A car enters only when its exit has room for it and everyone in the
//!   ring heading there (as at any junction) and the ring just beyond its
//!   merge point is not standing: a full ring cannot lock itself up.
//! * Cars on the ring follow whatever is ahead on their own path: a car in
//!   their lane, or a car crossing it. On a two-lane ring a car leaving
//!   from the inner lane across the outer one waits for a gap there (it
//!   never cuts across a car beside it); lanes are chosen by exit.
//! * Speed on the ring follows its radius (`RING_LATERAL_ACCEL`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::curve::{ArcTable, Curve};
use crate::junction::{Arm, Movement, Turn};
use crate::roads::{NodeId, RoadKind, RoadNetwork, SegmentId};
use crate::zoning::CellRef;
use crate::{BuildingId, World, balance, geom};

/// Size of a roundabout, from a mini roundabout that fits where two
/// streets cross to a large two-lane ring for avenues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RoundaboutSize {
    /// About the land of a street crossing; a low island trucks drive over.
    Mini,
    /// One lane round a planted island.
    Small,
    /// Two lanes; for an avenue meeting streets.
    Medium,
    /// Two wide lanes round a big island; avenues crossing.
    Large,
}

/// Dimensions of a ring, metres from its centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingDims {
    /// Outer kerb of the circulating carriageway.
    pub outer_r: f32,
    /// Inner edge of the circulating carriageway (where the apron or the
    /// island starts).
    pub inner_r: f32,
    /// Raised kerb of the central island (inside the apron trucks may use;
    /// equal to `inner_r` without an apron).
    pub island_r: f32,
    /// Circulating lanes (1 or 2).
    pub lanes: u8,
    /// Centre line of each ring lane, outer lane first.
    pub lane_r: [f32; 2],
    /// Width of one ring lane.
    pub lane_w: f32,
    /// The island is a low dome every vehicle may drive over (mini).
    pub mountable: bool,
    /// Pavement round the outside of the ring.
    pub pavement_m: f32,
}

impl RingDims {
    /// Radius of the land the ring takes: carriageway, pavement and a verge.
    pub fn reserve_r(&self) -> f32 {
        self.outer_r + self.pavement_m + balance::RING_VERGE_M
    }
}

impl RoundaboutSize {
    pub const ALL: [RoundaboutSize; 4] = [
        RoundaboutSize::Mini,
        RoundaboutSize::Small,
        RoundaboutSize::Medium,
        RoundaboutSize::Large,
    ];

    /// Lower-case name for the interface ("mini", "small", ...).
    pub fn name(self) -> &'static str {
        match self {
            RoundaboutSize::Mini => "mini",
            RoundaboutSize::Small => "small",
            RoundaboutSize::Medium => "medium",
            RoundaboutSize::Large => "large",
        }
    }

    pub fn dims(self) -> RingDims {
        let (outer_r, lanes, lane_w, apron, mountable) = match self {
            RoundaboutSize::Mini => (8.5, 1, 5.0, 0.0, true),
            RoundaboutSize::Small => (13.0, 1, 5.5, 1.5, false),
            RoundaboutSize::Medium => (19.0, 2, 4.5, 1.5, false),
            RoundaboutSize::Large => (27.0, 2, 4.75, 1.5, false),
        };
        let inner_r = outer_r - lane_w * lanes as f32;
        let lane_r = [outer_r - lane_w * 0.5, outer_r - lane_w * 1.5];
        RingDims {
            outer_r,
            inner_r,
            island_r: inner_r - apron,
            lanes,
            lane_r: if lanes == 1 { [lane_r[0], lane_r[0]] } else { lane_r },
            lane_w,
            mountable,
            pavement_m: balance::RING_PAVEMENT_M,
        }
    }

    /// Price to build (`balance::ROUNDABOUT_COST`).
    pub fn cost(self) -> i64 {
        balance::ROUNDABOUT_COST[self as usize]
    }

    /// Upkeep per day (`balance::ROUNDABOUT_UPKEEP_PER_DAY`), part of road
    /// upkeep.
    pub fn upkeep_per_day(self) -> i64 {
        balance::ROUNDABOUT_UPKEEP_PER_DAY[self as usize]
    }

    /// Speed on ring lane `lane` (0 outer), m/s: what its radius allows.
    pub fn lane_speed(self, lane: u8) -> f32 {
        ring_speed(self.dims().lane_r[(lane as usize).min(1)])
    }
}

/// Speed on a circle of radius `r`, m/s.
fn ring_speed(r: f32) -> f32 {
    (balance::RING_LATERAL_ACCEL * r)
        .sqrt()
        .clamp(balance::RING_MIN_SPEED, balance::STREET_SPEED_LIMIT)
}

/// Why a junction cannot take a roundabout (of some size).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RingProblem {
    /// Fewer than three roads meet there (a bend or a road's end).
    TooFewArms,
    /// More roads meet than a ring takes (`balance::RING_MAX_ARMS`).
    TooManyArms,
    /// The highway ends or passes there.
    Highway,
    /// A mini roundabout is for streets only.
    AvenueOnMini,
    /// Two roads meet at too sharp an angle for this size (degrees between
    /// them, where).
    ArmsTooClose { degrees: f32, at: [f32; 2] },
    /// The ring would reach the next junction (or road end) on an arm.
    NeighbourTooClose { at: [f32; 2] },
    /// Another road passes through the ring's land.
    RoadInTheWay { at: [f32; 2] },
    /// The ring would reach off the map.
    OffMap,
    /// It is a roundabout of this size already.
    AlreadyThatSize,
    /// It is no roundabout (nothing to take away).
    NotARoundabout,
}

impl RingProblem {
    pub fn at(&self) -> Option<[f32; 2]> {
        match *self {
            RingProblem::ArmsTooClose { at, .. }
            | RingProblem::NeighbourTooClose { at }
            | RingProblem::RoadInTheWay { at } => Some(at),
            _ => None,
        }
    }
}

impl std::fmt::Display for RingProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RingProblem::TooFewArms => write!(f, "a roundabout needs three or more roads"),
            RingProblem::TooManyArms => write!(f, "too many roads meet here for a roundabout"),
            RingProblem::Highway => write!(f, "not where the highway meets"),
            RingProblem::AvenueOnMini => write!(f, "too small for an avenue"),
            RingProblem::ArmsTooClose { degrees, .. } => {
                write!(f, "roads meet at {degrees:.0}°: too sharp for this size")
            }
            RingProblem::NeighbourTooClose { .. } => write!(f, "too close to the next junction"),
            RingProblem::RoadInTheWay { .. } => write!(f, "another road is in the way"),
            RingProblem::OffMap => write!(f, "too close to the map's edge"),
            RingProblem::AlreadyThatSize => write!(f, "already this roundabout"),
            RingProblem::NotARoundabout => write!(f, "not a roundabout"),
        }
    }
}

// ------------------------------------------------------------- geometry ----

/// Circulation direction at angle `a` round the centre (unit tangent; the
/// island on the left).
fn tangent(a: f32) -> [f32; 2] {
    [libm::sinf(a), -libm::cosf(a)]
}

fn on_circle(c: [f32; 2], r: f32, a: f32) -> [f32; 2] {
    [c[0] + r * libm::cosf(a), c[1] + r * libm::sinf(a)]
}

fn angle_of(c: [f32; 2], p: [f32; 2]) -> f32 {
    libm::atan2f(p[1] - c[1], p[0] - c[0])
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}

/// Circulating sweep from angle `from` to angle `to` (decreasing), in
/// `[0, 2π)`.
fn sweep(from: f32, to: f32) -> f32 {
    (from - to).rem_euclid(std::f32::consts::TAU)
}

/// Spacing of [`RingPath`] samples, metres.
pub(crate) const SAMPLE_M: f32 = 0.5;

/// The way of one movement through a roundabout: entry curve, arc of a
/// ring lane, exit curve. Distances are metres from the give-way line.
#[derive(Debug, Clone)]
pub struct RingPath {
    /// Ring lane (0 outer).
    pub ring_lane: u8,
    pub entry: Curve,
    entry_arc: ArcTable,
    pub entry_len: f32,
    pub center: [f32; 2],
    /// Radius of the ring lane and the angle where the entry joins it.
    pub radius: f32,
    pub start_angle: f32,
    /// Radians round the ring (circulating).
    pub sweep: f32,
    pub arc_len: f32,
    pub exit: Curve,
    exit_arc: ArcTable,
    pub exit_len: f32,
    pub total: f32,
    /// Points every `SAMPLE_M` from 0 to `total` (the last at `total`).
    pub(crate) samples: Vec<[f32; 2]>,
    /// Unit directions at the samples.
    pub(crate) dirs: Vec<[f32; 2]>,
}

impl RingPath {
    /// Point and unit direction `d` metres along (clamped).
    pub fn pose_at(&self, d: f32) -> ([f32; 2], [f32; 2]) {
        let d = d.clamp(0.0, self.total);
        if d < self.entry_len {
            let p = self.entry.pose_at_with(&self.entry_arc, d, 0.0);
            return (p.pos, p.dir);
        }
        let on = d - self.entry_len;
        if on < self.arc_len {
            let a = self.start_angle - on / self.radius;
            return (on_circle(self.center, self.radius, a), tangent(a));
        }
        let p = self
            .exit
            .pose_at_with(&self.exit_arc, (on - self.arc_len).min(self.exit_len), 0.0);
        (p.pos, p.dir)
    }

    /// Sample index nearest to `d` metres along.
    pub(crate) fn sample_at(&self, d: f32) -> usize {
        ((d / SAMPLE_M).round().max(0.0) as usize).min(self.samples.len().saturating_sub(1))
    }

    /// Points along the path every `step` metres (renderers).
    pub fn points(&self, step: f32) -> Vec<[f32; 2]> {
        let mut out = Vec::new();
        let mut d = 0.0;
        while d < self.total {
            out.push(self.pose_at(d).0);
            d += step.max(0.1);
        }
        out.push(self.pose_at(self.total).0);
        out
    }

    fn new(center: [f32; 2], entry: Curve, lane: u8, radius: f32, start: f32, sw: f32, exit: Curve) -> RingPath {
        let entry_arc = entry.arc_table();
        let exit_arc = exit.arc_table();
        let entry_len = entry.length_with(&entry_arc).max(0.1);
        let exit_len = exit.length_with(&exit_arc).max(0.1);
        let arc_len = sw * radius;
        let mut p = RingPath {
            ring_lane: lane,
            entry,
            entry_arc,
            entry_len,
            center,
            radius,
            start_angle: start,
            sweep: sw,
            arc_len,
            exit,
            exit_arc,
            exit_len,
            total: entry_len + arc_len + exit_len,
            samples: Vec::new(),
            dirs: Vec::new(),
        };
        let n = (p.total / SAMPLE_M).ceil() as usize;
        let poses: Vec<([f32; 2], [f32; 2])> = (0..=n).map(|k| p.pose_at((k as f32 * SAMPLE_M).min(p.total))).collect();
        p.samples = poses.iter().map(|x| x.0).collect();
        p.dirs = poses.iter().map(|x| x.1).collect();
        p
    }
}

/// One arm as the ring sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingArm {
    pub segment: SegmentId,
    pub kind: RoadKind,
    /// Give-way line distance from the node along the arm.
    pub setback: f32,
    /// Angle round the centre of the arm's centre line at the give-way line.
    pub angle: f32,
    /// Room for a splitter island between the lanes in and out.
    pub splitter: bool,
}

/// A point where a movement's path enters the band of a ring lane: entering
/// cars give way there, a car leaving across the outer lane waits for a gap
/// there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ConflictPoint {
    /// Metres along the path.
    pub s: f32,
    /// Before the path is on the ring (an entry), or on the way out.
    pub entry: bool,
}

/// Derived geometry and tables of a roundabout (rebuilt with the junction,
/// not saved).
#[derive(Debug, Clone)]
pub struct Ring {
    pub size: RoundaboutSize,
    pub center: [f32; 2],
    pub dims: RingDims,
    /// In the junction's arm order.
    pub arms: Vec<RingArm>,
    /// Per movement: its conflict points.
    pub(crate) points: Vec<Vec<ConflictPoint>>,
    /// `passes[(mx * MAX_POINTS + c) * n + my]`: metres along movement
    /// `my` where its path passes conflict point `c` of `mx` (infinite if
    /// it never does).
    pub(crate) passes: Vec<f32>,
    /// Movements.
    pub(crate) n: usize,
    /// What the geometry was built from (reused while unchanged).
    sig: Vec<(SegmentId, Curve, RoadKind)>,
    /// The movements built with it (reused with it).
    movements: Vec<Movement>,
}

impl PartialEq for Ring {
    fn eq(&self, o: &Self) -> bool {
        self.size == o.size && self.center == o.center && self.arms == o.arms
    }
}

pub(crate) const MAX_POINTS: usize = 3;

impl Ring {
    pub(crate) fn passes(&self, mx: usize, c: usize, my: usize) -> f32 {
        self.passes
            .get((mx * MAX_POINTS + c) * self.n + my)
            .copied()
            .unwrap_or(f32::INFINITY)
    }
}

/// Where the arm along `s` reaches radius `r` from the node `n` (metres
/// along from the node), if it does.
fn reach_radius(roads: &RoadNetwork, n: NodeId, s: SegmentId, r: f32) -> Option<f32> {
    let seg = &roads.segments[s];
    let c = roads.nodes[n].pos;
    let forward = seg.a == n;
    let len = seg.length;
    let dist = |d: f32| geom::dist(seg.pose_dir(forward, d, 0.0).pos, c);
    let step = 0.5;
    let mut d0 = 0.0;
    while d0 < len {
        let d1 = (d0 + step).min(len);
        if dist(d1) >= r {
            let (mut lo, mut hi) = (d0, d1);
            for _ in 0..20 {
                let m = 0.5 * (lo + hi);
                if dist(m) >= r {
                    hi = m;
                } else {
                    lo = m;
                }
            }
            return Some(hi);
        }
        d0 = d1;
    }
    None
}

/// The road on the arm in the order of circulation: index of each arm in
/// the order a car meets them going round from arm `i` (1 = first exit; `i`
/// itself last).
fn exit_number(angles: &[f32], i: usize, j: usize) -> usize {
    if i == j {
        return angles.len();
    }
    let sw = |k: usize| sweep(angles[i], angles[k]);
    let mine = sw(j);
    1 + (0..angles.len()).filter(|&k| k != i && k != j && sw(k) < mine).count()
}

/// Does a roundabout of `size` fit at node `n`? The arms' give-way lines on
/// success.
pub(crate) fn fit(
    roads: &RoadNetwork,
    states: &crate::side::SideTable<NodeId, crate::junction::JunctionState>,
    n: NodeId,
    size: RoundaboutSize,
) -> Result<Vec<RingArm>, RingProblem> {
    let node = roads.nodes.get(n).ok_or(RingProblem::TooFewArms)?;
    let deg = node.segments.len();
    if deg < 3 {
        return Err(RingProblem::TooFewArms);
    }
    if deg > balance::RING_MAX_ARMS {
        return Err(RingProblem::TooManyArms);
    }
    let dims = size.dims();
    let c = node.pos;
    let reserve = dims.reserve_r();
    let map = crate::MAP_CELLS as f32 * crate::CELL_SIZE;
    if c[0] - reserve < 0.0 || c[1] - reserve < 0.0 || c[0] + reserve > map || c[1] + reserve > map {
        return Err(RingProblem::OffMap);
    }
    let mut arms = Vec::with_capacity(deg);
    for &s in &node.segments {
        let seg = &roads.segments[s];
        if seg.kind == RoadKind::Highway {
            return Err(RingProblem::Highway);
        }
        if size == RoundaboutSize::Mini && seg.kind != RoadKind::Street {
            return Err(RingProblem::AvenueOnMini);
        }
        let other = if seg.a == n { seg.b } else { seg.a };
        let far_at = roads.nodes[other].pos;
        let Some(setback) = reach_radius(roads, n, s, dims.outer_r + balance::RING_GIVE_WAY_OUTSIDE_M) else {
            return Err(RingProblem::NeighbourTooClose { at: far_at });
        };
        // Room before the next junction: its own box (as a ring too, if
        // it is one), and a stretch of road with room for a car or two.
        // (A junction not rebuilt yet after a road edit: its geometry now.)
        let far_degree_now = roads.nodes[other].segments.len();
        let far_setback = states
            .get(other)
            .filter(|j| j.arms.len() == far_degree_now)
            .and_then(|j| j.arm_of(s).map(|a| j.arms[a].setback))
            .unwrap_or_else(|| {
                let (arms, _) = crate::junction::build_geometry(roads, other);
                arms.iter().find(|a| a.segment == s).map_or(0.0, |a| a.setback)
            });
        let far_degree = roads.nodes[other].segments.len();
        let need = balance::RING_MIN_ARM_M + if far_degree >= 3 { far_setback } else { 0.0 };
        if seg.length - setback < need {
            return Err(RingProblem::NeighbourTooClose { at: far_at });
        }
        let pose = seg.pose_dir(seg.a == n, setback, 0.0);
        arms.push(RingArm {
            segment: s,
            kind: seg.kind,
            setback,
            angle: angle_of(c, pose.pos),
            splitter: size != RoundaboutSize::Mini,
        });
    }
    // Roads that are not arms must keep off the ring's land.
    for (s, seg) in roads.segments.iter() {
        if node.segments.contains(&s) {
            continue;
        }
        let (lo, hi) = seg.curve.bounds();
        let r = reserve + seg.kind.half_width_m();
        if lo[0] > c[0] + r || hi[0] < c[0] - r || lo[1] > c[1] + r || hi[1] < c[1] - r {
            continue;
        }
        let (_, d, q) = seg.nearest(c);
        if d < r {
            return Err(RingProblem::RoadInTheWay { at: q });
        }
    }
    // Neighbouring arms: room on the ring between one arm's entry and the
    // next one's exit (the paths of the first exit).
    let angles: Vec<f32> = arms.iter().map(|a| a.angle).collect();
    for i in 0..deg {
        let next = (0..deg)
            .filter(|&j| j != i)
            .min_by(|&x, &y| sweep(angles[i], angles[x]).total_cmp(&sweep(angles[i], angles[y])))
            .expect("three or more arms");
        let (a, b) = (&arms[i], &arms[next]);
        let gap = sweep(a.angle, b.angle);
        let need = entry_angle(&dims, a, 0, 0) + exit_angle(&dims, b, 0, 0) + balance::RING_MIN_SWEEP;
        let kerbs = (a.kind.half_width_m() + b.kind.half_width_m() + 1.5) / dims.outer_r;
        if gap < need.max(kerbs) {
            let degrees = gap.to_degrees();
            let mid = on_circle(c, dims.outer_r, a.angle - gap * 0.5);
            return Err(RingProblem::ArmsTooClose { degrees, at: mid });
        }
    }
    Ok(arms)
}

/// Radius of the give-way line's lane point of `lane` (in or out) on arm
/// `a`: the stop line is across the road, so lanes off the centre line sit a
/// little further out.
fn lane_point_r(dims: &RingDims, a: &RingArm, lane: u8) -> f32 {
    let off = a.kind.lane_offset_m(lane);
    let r = dims.outer_r + balance::RING_GIVE_WAY_OUTSIDE_M;
    (r * r + off * off).sqrt()
}

/// Angle from an arm's centre line to where its in-lane `lane` joins ring
/// lane `ring_lane` (downstream).
fn entry_angle(dims: &RingDims, a: &RingArm, lane: u8, ring_lane: u8) -> f32 {
    let r = lane_point_r(dims, a, lane);
    let rl = dims.lane_r[ring_lane as usize];
    let lateral = libm::atan2f(a.kind.lane_offset_m(lane), r);
    lateral + libm::atan2f((r - rl).max(0.5), rl)
}

/// Angle from an arm's centre line to where ring lane `ring_lane` leaves for
/// its out-lane `lane` (upstream).
fn exit_angle(dims: &RingDims, a: &RingArm, lane: u8, ring_lane: u8) -> f32 {
    let r = lane_point_r(dims, a, lane);
    let rl = dims.lane_r[ring_lane as usize];
    let lateral = libm::atan2f(a.kind.lane_offset_m(lane), r);
    lateral + libm::atan2f((r - rl).max(0.5), rl) * balance::RING_EXIT_SPREAD
}

/// Lanes of a movement round the ring: `(ring lane, to lane, preferred)`
/// for each in-lane of arm `i` going to arm `j`. Lane by exit: the first
/// exit from the outer lane, later exits from the inner lane where the
/// road in has two lanes (straight on from both), and only from the outer
/// lane into a road with one lane out (nobody crosses the outer lane where
/// it can be helped); an avenue's exit takes both ring lanes side by side.
fn lane_plan(lanes: u8, arms: &[Arm], i: usize, j: usize, k: usize, turn: Turn) -> Vec<(u8, u8, u8, bool)> {
    let (ni, no) = (arms[i].lanes_in, arms[j].lanes_out);
    let mut out = Vec::new();
    for l in 0..ni {
        let (ring, pref) = if lanes == 1 {
            let pref = ni == 1
                || match turn {
                    Turn::Right => l == 0,
                    Turn::Straight => true,
                    Turn::Left | Turn::UTurn => l + 1 == ni,
                };
            (0u8, pref)
        } else if ni == 1 {
            let inner = matches!(turn, Turn::Left | Turn::UTurn) && no >= 2 && k >= 2;
            (u8::from(inner), true)
        } else if l == 0 {
            // Kerb lane: the outer ring lane; right turns, and straight on.
            (0, k == 1 || turn == Turn::Right || (turn == Turn::Straight))
        } else {
            // Inner lane: the inner ring lane, for anything past the first
            // exit (straight on only where the exit has two lanes).
            let pref = k >= 2
                && turn != Turn::Right
                && (turn != Turn::Straight || no >= 2)
                && (matches!(turn, Turn::Left | Turn::UTurn) || no >= 2);
            (1, pref)
        };
        // The first exit, and any road with one lane out, from the outer
        // lane: nobody crosses the outer lane to squeeze into one lane.
        let ring = if k == 1 || no < 2 { 0 } else { ring };
        let to = if ring == 1 { 1 } else { 0 };
        out.push((l, ring, to, pref));
    }
    // Every turn has a preferred lane.
    if !out.iter().any(|x| x.3) {
        let l = if matches!(turn, Turn::Left | Turn::UTurn) {
            ni - 1
        } else {
            0
        };
        if let Some(x) = out.iter_mut().find(|x| x.0 == l) {
            x.3 = true;
        }
    }
    out
}

/// Movements and ring geometry of node `n` as a roundabout of `size` (the
/// arms as `junction::build_geometry` made them, setbacks replaced).
pub(crate) fn build(
    roads: &RoadNetwork,
    n: NodeId,
    size: RoundaboutSize,
    ring_arms: Vec<RingArm>,
    arms: &mut [Arm],
    old: Option<&Arc<Ring>>,
    tables: bool,
) -> (Arc<Ring>, Vec<Movement>) {
    let dims = size.dims();
    let c = roads.nodes[n].pos;
    let deg = arms.len();
    for (a, r) in arms.iter_mut().zip(&ring_arms) {
        a.setback = r.setback;
        a.major = false;
    }
    let sig: Vec<(SegmentId, Curve, RoadKind)> = ring_arms
        .iter()
        .map(|a| (a.segment, roads.segments[a.segment].curve, a.kind))
        .collect();
    // Nothing changed: the same paths and tables.
    if let Some(o) = old.filter(|o| o.size == size && o.sig == sig && (!tables || !o.passes.is_empty())) {
        return (Arc::clone(o), o.movements.clone());
    }
    let angles: Vec<f32> = ring_arms.iter().map(|a| a.angle).collect();
    let mut movements = Vec::new();
    for i in 0..deg {
        for j in 0..deg {
            let (a, b) = (&arms[i], &arms[j]);
            let turn = if i == j {
                Turn::UTurn
            } else {
                crate::junction::classify_turn([-a.dir[0], -a.dir[1]], b.dir)
            };
            let k = exit_number(&angles, i, j);
            let pa = roads.segments[a.segment].pose_dir(roads.segments[a.segment].a == n, a.setback, 0.0);
            let pb = roads.segments[b.segment].pose_dir(roads.segments[b.segment].a == n, b.setback, 0.0);
            let din = [-pa.dir[0], -pa.dir[1]];
            let dout = pb.dir;
            for (fl, ring, tl, preferred) in lane_plan(dims.lanes, arms, i, j, k, turn) {
                let rl = dims.lane_r[ring as usize];
                let p0 = add(pa.pos, scale(crate::junction::right_of(din), a.kind.lane_offset_m(fl)));
                let p3 = add(pb.pos, scale(crate::junction::right_of(dout), b.kind.lane_offset_m(tl)));
                let ea = ring_arms[i].angle - entry_angle(&dims, &ring_arms[i], fl, ring);
                let xa = ring_arms[j].angle + exit_angle(&dims, &ring_arms[j], tl, ring);
                let q_in = on_circle(c, rl, ea);
                let q_out = on_circle(c, rl, xa);
                let h_in = 0.55 * geom::dist(p0, q_in);
                let h_out = 0.55 * geom::dist(q_out, p3);
                let entry = Curve::new(
                    p0,
                    add(p0, scale(din, h_in)),
                    add(q_in, scale(tangent(ea), -h_in)),
                    q_in,
                );
                let exit = Curve::new(
                    q_out,
                    add(q_out, scale(tangent(xa), h_out)),
                    add(p3, scale(dout, -h_out)),
                    p3,
                );
                let mut sw = sweep(ea, xa);
                if sw < balance::RING_MIN_SWEEP * 0.5 {
                    sw += std::f32::consts::TAU;
                }
                let path = RingPath::new(c, entry, ring, rl, ea, sw, exit);
                movements.push(Movement::ring(
                    i as u8,
                    j as u8,
                    fl,
                    tl,
                    turn,
                    preferred,
                    Arc::new(path),
                ));
            }
        }
    }
    let points: Vec<Vec<ConflictPoint>> = movements.iter().map(|m| conflict_points(&dims, c, m)).collect();
    let nm = movements.len();
    let mut passes = vec![f32::INFINITY; if tables { nm * MAX_POINTS * nm } else { 0 }];
    let w2 = balance::RING_CONFLICT_M * balance::RING_CONFLICT_M;
    for (mx, pts) in points.iter().enumerate().filter(|_| tables) {
        let px = movements[mx].ring_path().expect("ring movement");
        for (ci, cp) in pts.iter().enumerate().take(MAX_POINTS) {
            let q = px.samples[px.sample_at(cp.s)];
            for (my, m) in movements.iter().enumerate() {
                if my == mx {
                    continue;
                }
                let py = m.ring_path().expect("ring movement");
                // The first sample near the point.
                if let Some(k) = py.samples.iter().position(|p| {
                    let d = geom::sub(*p, q);
                    geom::dot(d, d) < w2
                }) {
                    // A car on the same way before the point follows (or
                    // leads): the same entry lane for a merge, the same ring
                    // lane to the same exit lane for a crossing on the way out.
                    let (a, b) = (&movements[mx], m);
                    let shared = if cp.entry {
                        (a.from_arm, a.from_lane) == (b.from_arm, b.from_lane)
                    } else {
                        (a.to_arm, a.to_lane) == (b.to_arm, b.to_lane) && py.ring_lane == px.ring_lane
                    };
                    if !shared {
                        passes[(mx * MAX_POINTS + ci) * nm + my] = k as f32 * SAMPLE_M;
                    }
                }
            }
        }
    }
    let ring = Ring {
        size,
        center: c,
        dims,
        arms: ring_arms,
        points,
        passes,
        n: nm,
        sig,
        movements: movements.clone(),
    };
    (Arc::new(ring), movements)
}

/// Where a path enters the band of each ring lane (entering cars give way
/// there; a car leaving across the outer lane waits for a gap there).
fn conflict_points(dims: &RingDims, c: [f32; 2], m: &Movement) -> Vec<ConflictPoint> {
    let p = m.ring_path().expect("ring movement");
    let half = dims.lane_w * 0.5;
    let band = |q: [f32; 2]| -> Option<u8> {
        let r = geom::dist(q, c);
        (0..dims.lanes).find(|&l| (r - dims.lane_r[l as usize]).abs() < half)
    };
    let mut out = Vec::new();
    let mut prev: Option<u8> = None;
    for (k, &q) in p.samples.iter().enumerate() {
        let b = band(q);
        if b.is_some() && b != prev {
            let s = k as f32 * SAMPLE_M;
            // On the way in, the point is a little into the band (the
            // centre of the lane it merges with); on the way out, where the
            // car starts across.
            let entry = s < p.entry_len + 0.5;
            if out.len() < MAX_POINTS && (entry || b != Some(p.ring_lane)) {
                out.push(ConflictPoint {
                    s: if entry { (s + half).min(p.entry_len) } else { s },
                    entry,
                });
            }
        }
        prev = b;
    }
    if out.is_empty() {
        out.push(ConflictPoint {
            s: p.entry_len,
            entry: true,
        });
    }
    out
}

/// The size of the roundabout at node `n`, if it is one.
pub fn roundabout(world: &World, n: NodeId) -> Option<RoundaboutSize> {
    world.traffic.junction_states.get(n).and_then(|j| j.ring)
}

/// Ring geometry of the roundabout at node `n` (renderers, the interface).
pub fn ring(world: &World, n: NodeId) -> Option<&Ring> {
    world
        .traffic
        .junction_states
        .get(n)
        .and_then(|j| j.ring_geom.0.as_deref())
}

/// Which sizes fit at node `n` (in size order), with the reason for each
/// that does not.
pub fn sizes(world: &World, n: NodeId) -> Vec<(RoundaboutSize, Result<(), RingProblem>)> {
    RoundaboutSize::ALL
        .iter()
        .map(|&s| (s, fit(&world.roads, &world.traffic.junction_states, n, s).map(|_| ())))
        .collect()
}

/// Every roundabout as `(node, centre, reserve radius)`, in node order
/// (zoning keeps its land clear).
pub(crate) fn reserves(world: &World) -> Vec<(NodeId, [f32; 2], f32)> {
    world
        .traffic
        .junction_states
        .iter()
        .filter_map(|(n, j)| {
            let s = j.ring?;
            Some((n, world.roads.nodes.get(n)?.pos, s.dims().reserve_r()))
        })
        .collect()
}

/// The ring of `size` at node `n` as it would be built, for the tool's
/// preview: geometry, or why it does not fit.
pub fn plan_ring(world: &World, n: NodeId, size: RoundaboutSize) -> Result<Ring, RingProblem> {
    let ring_arms = fit(&world.roads, &world.traffic.junction_states, n, size)?;
    let (mut arms, _) = crate::junction::build_geometry(&world.roads, n);
    let (ring, _) = build(&world.roads, n, size, ring_arms, &mut arms, None, false);
    Ok(Arc::unwrap_or_clone(ring))
}

/// The land a ring of `size` at node `n` takes beyond today's: the zone
/// cells under it and the buildings on it (in id order). One function for
/// the preview and the command (the rule roads use for buildings in their
/// way will meet it here).
pub(crate) fn land_taken(world: &World, n: NodeId, size: RoundaboutSize) -> (Vec<CellRef>, Vec<BuildingId>) {
    let Some(node) = world.roads.nodes.get(n) else {
        return (Vec::new(), Vec::new());
    };
    let c = node.pos;
    let r = size.dims().reserve_r();
    let cells = crate::blocks::cells_under_ring(world, c, r);
    let mut buildings: Vec<BuildingId> = cells
        .iter()
        .filter_map(|&cr| world.zoning.cell(cr).and_then(|x| x.building))
        .collect();
    for (id, b) in world.buildings.iter() {
        let p = b.center();
        let reach = r + (b.footprint.size[0].max(b.footprint.size[1]) as f32) * crate::CELL_SIZE;
        if geom::dist(p, c) > reach {
            continue;
        }
        let q = world.zoning.lot_quad(&b.footprint).unwrap_or_else(|| b.corners());
        if crate::build::point_polygon_distance(c, &q) < r {
            buildings.push(id);
        }
    }
    buildings.sort();
    buildings.dedup();
    let cells = cells
        .into_iter()
        .filter(|&cr| {
            world
                .zoning
                .cell(cr)
                .is_some_and(|x| x.blocked != Some(crate::Blocked::Road))
        })
        .collect();
    (cells, buildings)
}

/// Make node `n` a roundabout of `size` (or an ordinary junction again):
/// the junction is rebuilt with the roads (`versions.roads`), its land
/// taken or given back to zoning.
pub(crate) fn set(world: &mut World, n: NodeId, size: Option<RoundaboutSize>) {
    let Some(pos) = world.roads.nodes.get(n).map(|x| x.pos) else {
        return;
    };
    let before = world.traffic.junction_states.get(n).and_then(|j| j.ring);
    if let Some(j) = world.traffic.junction_states.get_mut(n) {
        j.ring = size;
        j.ring_geom = crate::junction::RingCache::default();
    }
    world.versions.roads += 1;
    world.traffic.control_version += 1;
    // Rebuild the junctions now (zoning and the drawn roads follow them).
    crate::traffic::sync_junctions(world);
    let r = [before, size]
        .iter()
        .flatten()
        .map(|s| s.dims().reserve_r())
        .fold(0.0f32, f32::max);
    let boxes = [([pos[0] - r, pos[1] - r], [pos[0] + r, pos[1] + r])];
    let doomed = crate::blocks::refresh_boxes(world, &boxes);
    crate::growth::remove_buildings(world, &doomed);
    // Cars on an arm that now stand past the ring's give-way line (the ring
    // reaches further out than the junction did) go back to the line, one
    // behind another (lane by lane, front first).
    let arms: Vec<SegmentId> = world.roads.nodes[n].segments.clone();
    let mut on_arms: Vec<(SegmentId, bool, u8, f32, crate::VehicleId)> = world
        .vehicles
        .iter()
        .filter_map(|(id, v)| {
            let s = v.segment?;
            let seg = world.roads.segments.get(s)?;
            // (A turn pocket's cars join the inner lane: a ring has none.)
            (v.junction.is_none() && arms.contains(&s) && (if v.forward { seg.b } else { seg.a }) == n).then_some((
                s,
                v.forward,
                v.lane.min(seg.lane_count().saturating_sub(1)),
                v.progress,
                id,
            ))
        })
        .collect();
    on_arms.sort_by(|x, y| {
        (x.0, x.1, x.2)
            .cmp(&(y.0, y.1, y.2))
            .then(y.3.total_cmp(&x.3))
            .then(x.4.cmp(&y.4))
    });
    let mut lane: Option<(SegmentId, bool, u8)> = None;
    let mut room = 0.0f32;
    let mut lifted = Vec::new();
    for (s, fwd, l, progress, id) in on_arms {
        if lane != Some((s, fwd, l)) {
            lane = Some((s, fwd, l));
            room = crate::drive::drivable_range(world, s, fwd).map_or(f32::INFINITY, |r| r.1);
        }
        if progress > room && room < balance::VEHICLE_SPACING_M * 0.5 {
            // No room left on the arm: off the road (as when a road goes).
            lifted.push(id);
            continue;
        }
        let to = progress.min(room).max(0.0);
        if world.vehicles[id].lane != l {
            let v = &mut world.vehicles[id];
            v.lane = l;
            v.lane_shift = 1.0;
        }
        if to < progress {
            let v = &mut world.vehicles[id];
            v.progress = to;
            v.speed = 0.0;
            crate::traffic::set_pose(&world.roads, v);
        }
        room = to - balance::VEHICLE_SPACING_M;
    }
    let mut ended = Vec::new();
    for id in lifted {
        crate::traffic::remove_vehicle(world, id, crate::traffic::TripEnd::Vanished, &mut ended);
    }
    for e in ended {
        crate::households::trip_ended(world, e);
    }
}

// -------------------------------------------------------------- driving ----

/// A car in a roundabout (or let in at its give-way line) for the ring's
/// rules: its movement, the distance of its centre along the path
/// (negative before the line), speed, half its length and its pose.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RingCar {
    pub movement: usize,
    pub d: f32,
    pub speed: f32,
    pub half: f32,
    pub pos: [f32; 2],
    pub dir: [f32; 2],
}

impl RingCar {
    pub(crate) fn new(j: &crate::junction::JunctionState, movement: usize, d: f32, speed: f32, half: f32) -> RingCar {
        let (pos, dir) = j.movements.get(movement).map_or(([0.0; 2], [1.0, 0.0]), |m| {
            if d >= 0.0 {
                m.pose_at(d)
            } else {
                let (p, dir) = m.pose_at(0.0);
                ([p[0] + dir[0] * d, p[1] + dir[1] * d], dir)
            }
        });
        RingCar {
            movement,
            d,
            speed,
            half,
            pos,
            dir,
        }
    }
}

/// Speed on the ring for movement `m` of junction `j` (its ring lane's).
pub(crate) fn path_speed(ring: &Ring, j: &crate::junction::JunctionState, m: usize) -> f32 {
    let lane = j
        .movements
        .get(m)
        .and_then(|m| m.ring_path())
        .map_or(0, |p| p.ring_lane);
    ring.size.lane_speed(lane)
}

/// Seconds until a car `dist` metres from a point gets there, starting at
/// `v` and accelerating up to `vmax`.
fn arrival(dist: f32, v: f32, vmax: f32) -> f32 {
    crate::drive::arrival_time(dist, v, vmax)
}

/// May the front car on movement `m`, `d_line` metres before its give-way
/// line at `speed`, enter now? It gives way to every car whose path passes
/// its merge point(s) and would get there less than `RING_SAFETY_S` after
/// its rear has cleared them, and does not enter onto a ring standing just
/// beyond its merge.
pub(crate) fn may_enter(
    ring: &Ring,
    j: &crate::junction::JunctionState,
    m: usize,
    d_line: f32,
    speed: f32,
    half: f32,
    cars: &[RingCar],
) -> bool {
    let Some(points) = ring.points.get(m) else {
        return true;
    };
    // The start of its own way is free (a car ahead on the same path, or let
    // in this tick from another lane onto it).
    if cars
        .iter()
        .any(|y| y.movement == m && y.d + d_line < half + y.half + balance::RING_FRONT_GAP_M + 0.5)
    {
        return false;
    }
    let vmax = path_speed(ring, j, m);
    let mut last_entry = 0.0f32;
    for (ci, cp) in points.iter().enumerate() {
        if !cp.entry {
            continue;
        }
        last_entry = last_entry.max(cp.s);
        let t_me = arrival(d_line + cp.s + half + balance::RING_CLEAR_M, speed, vmax);
        for y in cars.iter().filter(|y| y.movement != usize::MAX) {
            let s_y = ring.passes(m, ci, y.movement);
            if !s_y.is_finite() {
                continue;
            }
            let ahead = s_y - y.d;
            if ahead < -(y.half + balance::RING_CLEAR_M) {
                // Past it already.
                continue;
            }
            if ahead <= y.half + balance::RING_CLEAR_M {
                return false;
            }
            let t_y = arrival(ahead - y.half, y.speed, path_speed(ring, j, y.movement));
            if t_y < t_me + balance::RING_SAFETY_S {
                return false;
            }
        }
    }
    // Room on the ring beyond the merge: nobody standing there.
    let Some(path) = j.movements.get(m).and_then(|x| x.ring_path()) else {
        return true;
    };
    let lo = path.sample_at(last_entry);
    let hi = path.sample_at(last_entry + 2.0 * half + balance::MIN_GAP_M);
    let w2 = balance::RING_CONFLICT_M * balance::RING_CONFLICT_M;
    for y in cars {
        if y.speed >= balance::WAIT_SPEED * 2.0 || y.d < 0.0 {
            continue;
        }
        if path.samples[lo..=hi].iter().any(|p| {
            let d = geom::sub(*p, y.pos);
            geom::dot(d, d) < w2
        }) {
            return false;
        }
    }
    true
}

/// Free distance ahead of each car in the ring (how far its centre may
/// travel along its path before its body, with a margin, would touch
/// another car), infinite when nothing is within `RING_LOOKAHEAD_M`. When
/// two cars are each in the other's way (side by side where paths cross or
/// merge), the one with more room to move goes on as far as it can without
/// touching the other at all; the other waits.
pub(crate) fn free_ahead(ring: &Ring, j: &crate::junction::JunctionState, cars: &[RingCar]) -> Vec<f32> {
    let n = cars.len();
    let look = balance::RING_LOOKAHEAD_M.min(0.45 * std::f32::consts::TAU * ring.dims.lane_r[1]);
    // How far car `xi` may go before touching car `yi` with margins
    // `ahead` / `side`.
    let hit = |xi: usize, yi: usize, ahead: f32, side: f32| -> f32 {
        let (x, y) = (&cars[xi], &cars[yi]);
        let Some(px) = j.movements.get(x.movement).and_then(|m| m.ring_path()) else {
            return f32::INFINITY;
        };
        // From just ahead of the car to `look` beyond, straight on past
        // the end of its path (onto its exit lane).
        let last = px.samples.len() - 1;
        let start = (x.d.max(0.0) / SAMPLE_M).round() as usize + 1;
        let steps = (look / SAMPLE_M) as usize;
        (start..start + steps)
            .find(|&k| {
                let (c, d) = if k <= last {
                    (px.samples[k], px.dirs[k])
                } else {
                    let (e, d) = (px.samples[last], px.dirs[last]);
                    let over = (k - last) as f32 * SAMPLE_M;
                    ([e[0] + d[0] * over, e[1] + d[1] * over], d)
                };
                // A car behind this one (its centre behind this one's rear
                // at that point) is not in the way.
                if geom::dot(geom::sub(y.pos, c), d) < -x.half && geom::dot(y.dir, d) > 0.0 {
                    return false;
                }
                // Oncoming: a car leaving by the arm this one enters by (or
                // the other way round) keeps to its own lane.
                if geom::dot(y.dir, d) < -0.5 {
                    return false;
                }
                let c = [c[0] + d[0] * ahead * 0.5, c[1] + d[1] * ahead * 0.5];
                boxes_overlap(
                    (c, d, x.half + ahead * 0.5, CAR_HALF_W + side),
                    (y.pos, y.dir, y.half, CAR_HALF_W),
                )
            })
            .map_or(f32::INFINITY, |k| ((k - 1) as f32 * SAMPLE_M - x.d).max(0.0))
    };
    // Every car in the way of every car: (free distance, which).
    let mut ways: Vec<Vec<(f32, usize)>> = vec![Vec::new(); n];
    for xi in 0..n {
        for yi in 0..n {
            if yi == xi || geom::dist(cars[xi].pos, cars[yi].pos) > look + 8.0 {
                continue;
            }
            let f = hit(xi, yi, balance::RING_FRONT_GAP_M, balance::RING_SIDE_GAP_M);
            if f.is_finite() {
                ways[xi].push((f, yi));
            }
        }
    }
    let mut free: Vec<f32> = ways
        .iter()
        .map(|w| w.iter().map(|x| x.0).fold(f32::INFINITY, f32::min))
        .collect();
    for xi in 0..n {
        for &(_, yi) in &ways[xi] {
            if yi < xi || !ways[yi].iter().any(|w| w.1 == xi) {
                continue;
            }
            // Each in the other's way: the one whose front is further
            // ahead goes, as far as it can without touching the other
            // (real widths, no margin); if it cannot move at all, the other.
            let (x, y) = (&cars[xi], &cars[yi]);
            let along = geom::add2(x.dir, y.dir);
            let front = |c: &RingCar| geom::dot(geom::add2(c.pos, geom::scale2(c.dir, c.half)), along);
            let (tx, ty) = (hit(xi, yi, 0.1, -0.1), hit(yi, xi, 0.1, -0.1));
            let x_first = match front(x).total_cmp(&front(y)) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => xi < yi,
            };
            let x_first = if x_first {
                tx > 0.05 || ty <= 0.05
            } else {
                ty <= 0.05 && tx > 0.05
            };
            let (goes, other, room) = if x_first { (xi, yi, tx) } else { (yi, xi, ty) };
            let rest = ways[goes]
                .iter()
                .filter(|w| w.1 != other)
                .map(|w| w.0)
                .fold(f32::INFINITY, f32::min);
            free[goes] = rest.min(room);
        }
    }
    free
}

/// Half a car's width (trucks a little more), metres.
const CAR_HALF_W: f32 = 1.05;

/// Do two oriented rectangles `(centre, unit dir, half length, half
/// width)` overlap? (Separating axes: only products and sums.)
fn boxes_overlap(a: ([f32; 2], [f32; 2], f32, f32), b: ([f32; 2], [f32; 2], f32, f32)) -> bool {
    let t = geom::sub(b.0, a.0);
    for &(axis, owner) in &[(a.1, 0), ([-a.1[1], a.1[0]], 0), (b.1, 1), ([-b.1[1], b.1[0]], 1)] {
        let _ = owner;
        let proj = |bx: &([f32; 2], [f32; 2], f32, f32)| {
            let r = [-bx.1[1], bx.1[0]];
            bx.2 * geom::dot(bx.1, axis).abs() + bx.3 * geom::dot(r, axis).abs()
        };
        if geom::dot(t, axis).abs() > proj(&a) + proj(&b) {
            return false;
        }
    }
    true
}

/// A car on the ring about to leave across the outer lane (or merge into
/// it): the distance it must stop within, if a car in that lane would
/// reach the crossing too soon after it (it never cuts across a car beside
/// it). `None`: free to go (or committed: too close to stop).
pub(crate) fn crossing_hold(
    ring: &Ring,
    j: &crate::junction::JunctionState,
    x: &RingCar,
    cars: &[RingCar],
) -> Option<f32> {
    let points = ring.points.get(x.movement)?;
    let vmax = path_speed(ring, j, x.movement);
    for (ci, cp) in points.iter().enumerate() {
        if cp.entry || cp.s <= x.d {
            continue;
        }
        let to_point = cp.s - x.d - x.half - 0.5;
        if to_point < 0.0 {
            // Its front is in the lane already: it goes on.
            continue;
        }
        if x.speed > 1.0 && to_point < x.speed * x.speed / (2.0 * balance::BRAKING) {
            // Moving and too close to stop: committed.
            continue;
        }
        let t_me = arrival(cp.s - x.d + x.half + balance::RING_CLEAR_M, x.speed, vmax);
        for y in cars {
            let s_y = ring.passes(x.movement, ci, y.movement);
            if !s_y.is_finite() {
                continue;
            }
            let ahead = s_y - y.d;
            if ahead < -(y.half + balance::RING_CLEAR_M) {
                continue;
            }
            let t_y = if ahead <= y.half {
                0.0
            } else {
                arrival(ahead - y.half, y.speed, path_speed(ring, j, y.movement))
            };
            if t_y < t_me + balance::RING_CROSS_SAFETY_S {
                return Some(to_point.max(0.0));
            }
        }
    }
    None
}

/// After loading a save: the roundabouts of the extension block, their
/// junctions rebuilt (counters, setting and the cars on the ring kept).
pub(crate) fn restore(world: &mut World, rings: &[(NodeId, RoundaboutSize)]) {
    let mut any = false;
    for &(n, size) in rings {
        if let Some(j) = world.traffic.junction_states.get_mut(n) {
            j.ring = Some(size);
            any = true;
        }
    }
    if !any {
        return;
    }
    for &(n, _) in rings {
        let Some(old) = world.traffic.junction_states.remove(n) else {
            continue;
        };
        let mut fresh = crate::junction::build_state(&world.roads, &world.traffic.junction_states, n, Some(&old));
        fresh.queue = old.queue;
        fresh.signal = old.signal;
        world.traffic.junction_states.insert(n, fresh);
    }
}

/// The surface outline of a roundabout for `junction_shape`: each arm's
/// cut, a kerb fillet onto the ring's outer kerb (`extra` metres out), the
/// kerb round to the next arm. Counter-clockwise (arms sorted so).
pub(crate) fn outline(ring: &Ring, arms: &[crate::junction::ArmCut], extra: f32) -> Vec<[f32; 2]> {
    let c = ring.center;
    let r = ring.dims.outer_r + extra;
    let k = arms.len();
    let mut out = Vec::new();
    let tau = std::f32::consts::TAU;
    for i in 0..k {
        let (a, b) = (&arms[i], &arms[(i + 1) % k]);
        out.push(a.left);
        out.push(a.right);
        // Fillet from the cut's right kerb onto the circle a little round.
        let fillet = (1.5 + extra).min(r * 0.3);
        let a0 = angle_of(c, a.right) + fillet / r;
        let mut a1 = angle_of(c, b.left) - fillet / r;
        let start = angle_of(c, a.right);
        let ctrl_a = on_circle(c, r, start);
        let p_a = on_circle(c, r, a0);
        for t in [0.33f32, 0.66] {
            let u = 1.0 - t;
            out.push([
                u * u * a.right[0] + 2.0 * u * t * ctrl_a[0] + t * t * p_a[0],
                u * u * a.right[1] + 2.0 * u * t * ctrl_a[1] + t * t * p_a[1],
            ]);
        }
        while a1 < a0 {
            a1 += tau;
        }
        let steps = ((a1 - a0) * r / 1.5).ceil().max(1.0) as usize;
        for s in 0..=steps {
            out.push(on_circle(c, r, a0 + (a1 - a0) * s as f32 / steps as f32));
        }
        let end = angle_of(c, b.left);
        let ctrl_b = on_circle(c, r, end);
        let p_b = on_circle(c, r, a1);
        for t in [0.33f32, 0.66] {
            let u = 1.0 - t;
            out.push([
                u * u * p_b[0] + 2.0 * u * t * ctrl_b[0] + t * t * b.left[0],
                u * u * p_b[1] + 2.0 * u * t * ctrl_b[1] + t * t * b.left[1],
            ]);
        }
    }
    out
}
