//! Junctions: geometry, movements, control (traffic lights or priority) and
//! signal timing. Pure functions of the road graph plus a small serialised
//! state per junction; vehicles (`traffic`) ask it who may enter.
//!
//! # Geometry
//!
//! Every node has one *arm* per attached segment, in `node.segments` order.
//! An arm's stop line sits `setback` metres from the node (0 at dead ends
//! and straight continuations), so a junction of wide roads has a box of
//! real size. Traffic drives on the right: in sim space the right of a
//! travel direction `d` is `(-d.y, d.x)`; lane 0 is the kerb lane.
//!
//! A *movement* is a path through the box from one in-lane to one out-lane
//! (a quadratic curve between the two lane points, control point where the
//! lane lines meet). Two movements *conflict* when their paths cross or
//! merge into the same out-lane; movements from the same in-lane follow
//! each other instead. The crossing test is topological: the in- and
//! out-lane points lie around the node, and two paths cross exactly when
//! their end points interleave around it (U-turns conflict with
//! everything). No trigonometry, so it is the same on every machine.
//!
//! # Control
//!
//! Every node with three or more arms has a [`JunctionControl`]:
//! * `Signal`: traffic lights with a phase plan built from the geometry
//!   (opposite approaches share a phase; a protected left phase on avenues),
//!   actuated timing (phases skip when nobody waits, end early when their
//!   approaches empty, extend while a queue still discharges), and fixed
//!   cycle offsets along avenues so a platoon meets green lights.
//! * `Priority`: the major road (higher class, then the straight-through
//!   pair) has right of way; the others give way and enter on a gap.
//!
//! `JunctionSetting::Auto` picks signals where avenues (or an avenue and
//! the highway) meet, where a street crosses an avenue, and where a
//! junction was busy yesterday; elsewhere priority, so a side street
//! joining an avenue (a T) gives way to it until it gets busy.
//! The player overrides it per junction with `Command::SetJunctionControl`.

use serde::{Deserialize, Serialize};

use crate::curve::{ArcTable, Curve};
use crate::roads::{NodeId, RoadKind, RoadNetwork, SegmentId};
use crate::{Cell, TICKS_PER_SECOND, World, balance, geom};

/// How a junction is controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum JunctionControl {
    /// Traffic lights.
    Signal,
    /// Stop / give-way signs: the minor roads yield to the major road.
    #[default]
    Priority,
}

/// What the player asked for at a junction (`Command::SetJunctionControl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum JunctionSetting {
    /// The sim decides (signals where an avenue meets or the junction is
    /// busy, priority otherwise).
    #[default]
    Auto,
    Signal,
    Priority,
}

/// What a traffic light shows to one approach (or one movement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Aspect {
    Green,
    Yellow,
    Red,
}

/// Kind of a movement through a junction, as seen by the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Turn {
    Straight,
    Right,
    /// Crosses oncoming traffic.
    Left,
    /// Back the way it came (dead ends, or when nothing else works).
    UTurn,
}

/// One arm of a junction: an attached segment seen from the node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub segment: SegmentId,
    /// Unit direction from the node along the segment.
    pub dir: [f32; 2],
    pub kind: RoadKind,
    /// Stop line distance from the node, metres (the box edge).
    pub setback: f32,
    /// Lanes towards the node / away from it.
    pub lanes_in: u8,
    pub lanes_out: u8,
    /// True on the major road of a priority junction (and on every arm of
    /// nodes with fewer than three arms).
    pub major: bool,
    /// Index of the arm straight across, if any.
    pub opposite: Option<u8>,
}

/// A path through a junction box.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Movement {
    /// Arm indices (into the node's `segments`).
    pub from_arm: u8,
    pub to_arm: u8,
    pub from_lane: u8,
    pub to_lane: u8,
    pub turn: Turn,
    /// The path, from the stop line on the in-lane to the box edge on the
    /// out-lane, sim metres (lane offsets included).
    pub curve: Curve,
    /// Curve length, metres.
    pub length: f32,
    #[serde(skip)]
    arc: ArcTable,
}

impl PartialEq for Movement {
    fn eq(&self, o: &Self) -> bool {
        (self.from_arm, self.to_arm, self.from_lane, self.to_lane, self.turn)
            == (o.from_arm, o.to_arm, o.from_lane, o.to_lane, o.turn)
            && self.curve == o.curve
            && self.length == o.length
    }
}

impl Movement {
    /// A movement along the quadratic `p0`, `p1`, `p2` (as a cubic).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn quadratic(
        from_arm: u8,
        to_arm: u8,
        from_lane: u8,
        to_lane: u8,
        turn: Turn,
        p0: [f32; 2],
        p1: [f32; 2],
        p2: [f32; 2],
    ) -> Movement {
        let curve = Curve::new(
            p0,
            [
                p0[0] + (p1[0] - p0[0]) * (2.0 / 3.0),
                p0[1] + (p1[1] - p0[1]) * (2.0 / 3.0),
            ],
            [
                p2[0] + (p1[0] - p2[0]) * (2.0 / 3.0),
                p2[1] + (p1[1] - p2[1]) * (2.0 / 3.0),
            ],
            p2,
        );
        let arc = curve.arc_table();
        let length = curve.length_with(&arc).max(0.5);
        Movement {
            from_arm,
            to_arm,
            from_lane,
            to_lane,
            turn,
            curve,
            length,
            arc,
        }
    }

    /// Point and unit direction `d` metres along the curve (clamped).
    pub fn pose_at(&self, d: f32) -> ([f32; 2], [f32; 2]) {
        let p = self.curve.pose_at_with(&self.arc, d, 0.0);
        (p.pos, p.dir)
    }
}

/// Stage of the current signal phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SignalStage {
    #[default]
    Green,
    Yellow,
    /// Everyone red while the box clears.
    AllRed,
}

/// One phase of a signal plan: which movements have green.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalPhase {
    /// Arms whose through and right movements are green (left turns too,
    /// permissive: they yield to oncoming traffic).
    pub arms: Vec<u8>,
    /// A protected-left phase: only left turns (and U-turns) of `arms` are
    /// green, and they do not yield.
    pub left_only: bool,
    pub min_green_s: f32,
    pub max_green_s: f32,
}

/// Phase plan of a signalised junction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalPlan {
    pub phases: Vec<SignalPhase>,
    /// Coordinated along an avenue: the first phase (the avenue) starts at
    /// a fixed point of a common cycle, shifted by `offset_s`.
    pub coordinated: bool,
    pub cycle_s: f32,
    pub offset_s: f32,
    /// Index of the main road's through phase (where a coordinated signal
    /// starts its cycle and a new signal starts).
    pub main: u8,
}

/// Live state of a signal.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct SignalState {
    pub phase: u8,
    pub stage: SignalStage,
    /// Ticks spent in the current stage.
    pub ticks: u32,
    /// Phase to switch to after the clearance (set when green ends).
    pub next: u8,
}

/// Everything the sim keeps per junction (node with three or more arms;
/// nodes with fewer arms keep geometry only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JunctionState {
    /// Player override.
    pub setting: JunctionSetting,
    /// Control in force.
    pub control: JunctionControl,
    /// Arms in `node.segments` order.
    pub arms: Vec<Arm>,
    pub movements: Vec<Movement>,
    /// Bit `j` of `conflicts[i]`: movements `i` and `j` cross or merge.
    pub conflicts: Vec<u128>,
    pub plan: Option<SignalPlan>,
    pub signal: SignalState,
    /// Vehicles stopped or crawling on each approach (by arm).
    pub queue: Vec<u16>,
    /// Vehicles that entered the box in the current minute of vehicle
    /// time, and the busiest minute today / yesterday (auto control).
    pub entered_minute: u16,
    pub peak_minute_today: u16,
    pub peak_minute_yesterday: u16,
    /// Auto control decided at the last midnight: was it busy?
    pub busy: bool,
    /// Tick from which the box is clear of the last vehicle let in.
    pub clear_at: u64,
    /// Arm whose straight line was served last, and vehicles let through
    /// since (interim priority rule).
    pub last_served: u8,
    pub phase_count: u16,
}

impl JunctionState {
    /// Index of the arm for segment `s`.
    pub fn arm_of(&self, s: SegmentId) -> Option<usize> {
        self.arms.iter().position(|a| a.segment == s)
    }

    /// Movements from arm `from` (any lane) to arm `to`.
    pub fn movements_between(&self, from: usize, to: usize) -> impl Iterator<Item = (usize, &Movement)> + '_ {
        self.movements
            .iter()
            .enumerate()
            .filter(move |(_, m)| m.from_arm as usize == from && m.to_arm as usize == to)
    }

    /// The movement from `from` lane `lane` to arm `to`, or the nearest
    /// lane's movement when that lane has none.
    pub fn movement(&self, from: usize, lane: u8, to: usize) -> Option<usize> {
        let mut best: Option<(u8, usize)> = None;
        for (i, m) in self.movements_between(from, to) {
            let d = m.from_lane.abs_diff(lane);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, i));
            }
        }
        best.map(|b| b.1)
    }

    pub fn conflict(&self, a: usize, b: usize) -> bool {
        a < 128 && b < 128 && self.conflicts.get(a).is_some_and(|c| c >> b & 1 == 1)
    }

    /// Aspect shown to movement `m` right now (`None` without signals).
    pub fn movement_aspect(&self, m: usize) -> Option<Aspect> {
        let plan = self.plan.as_ref().filter(|_| self.control == JunctionControl::Signal)?;
        let mv = self.movements.get(m)?;
        let phase = plan.phases.get(self.signal.phase as usize)?;
        let in_phase = phase_allows(phase, mv);
        Some(match (in_phase, self.signal.stage) {
            (true, SignalStage::Green) => Aspect::Green,
            (true, SignalStage::Yellow) => Aspect::Yellow,
            _ => Aspect::Red,
        })
    }

    /// Does the current green phase protect movement `m` (no need to yield
    /// to oncoming traffic)? False when permissive or not green.
    pub fn protected(&self, m: usize) -> bool {
        let Some(plan) = self.plan.as_ref() else { return false };
        let Some(mv) = self.movements.get(m) else { return false };
        let Some(phase) = plan.phases.get(self.signal.phase as usize) else {
            return false;
        };
        phase_allows(phase, mv) && (phase.left_only || !matches!(mv.turn, Turn::Left | Turn::UTurn))
    }
}

fn phase_allows(phase: &SignalPhase, m: &Movement) -> bool {
    phase.arms.contains(&m.from_arm) && (!phase.left_only || matches!(m.turn, Turn::Left | Turn::UTurn))
}

/// Aspect and timing of one approach, for renderers and the interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApproachView {
    pub segment: SegmentId,
    /// Signal head for straight-on and right turns (`None` without
    /// signals).
    pub aspect: Option<Aspect>,
    /// Left-turn arrow (`None` without signals). Green also when the left
    /// turn is permissive (allowed, yielding to oncoming traffic).
    pub left_aspect: Option<Aspect>,
    /// True while the left turn is protected (green arrow, no need to yield).
    pub left_protected: bool,
    /// Estimated seconds until the main aspect changes (`None` while it
    /// rests with nobody else waiting, or without signals).
    pub time_to_change_s: Option<f32>,
    /// Vehicles queued on the approach.
    pub queue: u16,
    /// On the major road (priority junctions; always true for signals).
    pub major: bool,
    /// Stop line distance from the node, metres.
    pub setback: f32,
}

/// One junction as seen by renderers and the interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JunctionView {
    pub node: NodeId,
    pub control: JunctionControl,
    pub setting: JunctionSetting,
    pub approaches: Vec<ApproachView>,
    /// Current phase index and its stage (signals).
    pub phase: Option<(u8, SignalStage)>,
    /// Phases in the plan (signals).
    pub phase_count: u8,
    /// Busiest minute of vehicle time yesterday (vehicles entering).
    pub peak_per_minute: u16,
}

// ------------------------------------------------------------ geometry ----

/// Unit direction from `n` along segment `s`.
pub(crate) fn arm_dir(roads: &RoadNetwork, n: NodeId, s: SegmentId) -> [f32; 2] {
    roads.segments[s].dir_from(n)
}

/// Pose `d` metres from node `n` along segment `s`, direction away from
/// the node.
fn arm_pose(roads: &RoadNetwork, n: NodeId, s: SegmentId, d: f32) -> crate::curve::Pose {
    let seg = &roads.segments[s];
    seg.pose_dir(seg.a == n, d, 0.0)
}

/// Right of travel direction `d`.
pub fn right_of(d: [f32; 2]) -> [f32; 2] {
    [-d[1], d[0]]
}

/// Monotone stand-in for the angle of `v` (0..4 around the circle), using
/// only exact arithmetic.
fn pseudo_angle(v: [f32; 2]) -> f32 {
    let (x, y) = (v[0], v[1]);
    let s = x.abs() + y.abs();
    if s == 0.0 {
        return 0.0;
    }
    if y >= 0.0 {
        if x >= 0.0 { y / s } else { 1.0 - x / s }
    } else if x < 0.0 {
        2.0 - y / s
    } else {
        3.0 + x / s
    }
}

/// Is `a` strictly inside the counter-clockwise arc from `from` to `to`?
fn on_arc(from: f32, to: f32, a: f32) -> bool {
    let span = (to - from).rem_euclid(4.0);
    let off = (a - from).rem_euclid(4.0);
    off > 0.0 && off < span
}

fn classify(din: [f32; 2], dout: [f32; 2]) -> Turn {
    let dt = geom::dot(din, dout);
    if dt > 0.75 {
        Turn::Straight
    } else if geom::cross(din, dout) > 0.0 {
        Turn::Right
    } else {
        Turn::Left
    }
}

/// Where two lines `p + t d` and `q + s e` meet, if not parallel.
fn intersect(p: [f32; 2], d: [f32; 2], q: [f32; 2], e: [f32; 2]) -> Option<[f32; 2]> {
    let den = geom::cross(d, e);
    if den.abs() < 1e-4 {
        return None;
    }
    let t = geom::cross(geom::sub(q, p), e) / den;
    Some([p[0] + d[0] * t, p[1] + d[1] * t])
}

/// Build arms and movements of node `n` (any degree).
pub(crate) fn build_geometry(roads: &RoadNetwork, n: NodeId) -> (Vec<Arm>, Vec<Movement>) {
    let node = &roads.nodes[n];
    let deg = node.segments.len();
    let mut arms: Vec<Arm> = node
        .segments
        .iter()
        .map(|&s| {
            let seg = &roads.segments[s];
            Arm {
                segment: s,
                dir: arm_dir(roads, n, s),
                kind: seg.kind,
                setback: 0.0,
                lanes_in: seg.lane_count(),
                lanes_out: seg.lane_count(),
                major: true,
                opposite: None,
            }
        })
        .collect();
    // Opposite pairs, greedily by straightness.
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for i in 0..deg {
        for j in i + 1..deg {
            let d = geom::dot(arms[i].dir, arms[j].dir);
            if d < -0.85 {
                pairs.push((d, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for (_, i, j) in pairs {
        if arms[i].opposite.is_none() && arms[j].opposite.is_none() {
            arms[i].opposite = Some(j as u8);
            arms[j].opposite = Some(i as u8);
        }
    }
    // Setbacks: clear the carriageways of the other arms.
    for i in 0..deg {
        let mut sb: f32 = 0.0;
        for j in 0..deg {
            if i == j || arms[i].opposite == Some(j as u8) {
                continue;
            }
            let (di, dj) = (arms[i].dir, arms[j].dir);
            let sin = geom::cross(di, dj).abs().max(0.3);
            let cos = geom::dot(di, dj).abs();
            let need = (arms[j].kind.half_width_m() + arms[i].kind.half_width_m() * cos) / sin;
            // Junctions leave room for a pedestrian crossing; a bend only
            // needs its kerb radius.
            let margin = if deg >= 3 {
                balance::JUNCTION_MARGIN_M
            } else {
                balance::BEND_MARGIN_M
            };
            sb = sb.max(need + margin);
        }
        let len = roads.segments[arms[i].segment].length;
        let cap = 3.0 * arms.iter().map(|a| a.kind.half_width_m()).fold(0.0, f32::max) + 2.0;
        arms[i].setback = sb.min(cap).min(len * 0.4);
    }
    // A road that changes width without a junction (a lane ends, e.g. the
    // highway or an avenue continuing as a street): a short merge box, so
    // cars waiting to merge stop clear of the stream they join and the
    // merge runs along a curve instead of a sideways jump.
    if deg == 2 && arms[0].lanes_in != arms[1].lanes_in {
        for a in arms.iter_mut() {
            let len = roads.segments[a.segment].length;
            a.setback = (balance::LANE_MERGE_TAPER_M * 0.5).min(len * 0.4);
        }
    }
    // Movements.
    let mut movements = Vec::new();
    for i in 0..deg {
        for j in 0..deg {
            let (a, b) = (&arms[i], &arms[j]);
            let din = [-a.dir[0], -a.dir[1]];
            let dout = b.dir;
            let turn = if i == j { Turn::UTurn } else { classify(din, dout) };
            let (ni, no) = (a.lanes_in, b.lanes_out);
            let lanes: Vec<(u8, u8)> = match turn {
                Turn::Right => vec![(0, 0)],
                Turn::Left | Turn::UTurn => vec![(ni - 1, no - 1)],
                Turn::Straight => (0..ni).map(|l| (l, l.min(no - 1))).collect(),
            };
            // Stop line and box edge on the arms' centre lines, and the
            // travel directions there (the arms may be curved).
            let line_a = arm_pose(roads, n, a.segment, a.setback);
            let line_b = arm_pose(roads, n, b.segment, b.setback);
            let din = [-line_a.dir[0], -line_a.dir[1]];
            let dout = line_b.dir;
            for (fl, tl) in lanes {
                let p0 = geom_add(line_a.pos, scale(right_of(din), a.kind.lane_offset_m(fl)));
                let p2 = geom_add(line_b.pos, scale(right_of(dout), b.kind.lane_offset_m(tl)));
                let p1 = match turn {
                    Turn::UTurn => {
                        // Loop around the node.
                        let mid = [(p0[0] + p2[0]) * 0.5, (p0[1] + p2[1]) * 0.5];
                        geom_add(mid, scale(din, a.kind.half_width_m() + 2.0))
                    }
                    _ => intersect(p0, din, p2, dout).unwrap_or([(p0[0] + p2[0]) * 0.5, (p0[1] + p2[1]) * 0.5]),
                };
                // The quadratic through the lane lines' meeting point, as a
                // cubic.
                let curve = Curve::new(
                    p0,
                    [
                        p0[0] + (p1[0] - p0[0]) * (2.0 / 3.0),
                        p0[1] + (p1[1] - p0[1]) * (2.0 / 3.0),
                    ],
                    [
                        p2[0] + (p1[0] - p2[0]) * (2.0 / 3.0),
                        p2[1] + (p1[1] - p2[1]) * (2.0 / 3.0),
                    ],
                    p2,
                );
                let arc = curve.arc_table();
                let length = curve.length_with(&arc).max(0.5);
                movements.push(Movement {
                    from_arm: i as u8,
                    to_arm: j as u8,
                    from_lane: fl,
                    to_lane: tl,
                    turn,
                    curve,
                    length,
                    arc,
                });
            }
        }
    }
    (arms, movements)
}

fn geom_add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] * k, a[1] * k]
}

/// Conflict bit sets for `movements` around `node_pos`.
pub(crate) fn build_conflicts(node_pos: [f32; 2], movements: &[Movement]) -> Vec<u128> {
    let n = movements.len().min(128);
    let ang = |p: [f32; 2]| pseudo_angle(geom::sub(p, node_pos));
    let ends: Vec<(f32, f32)> = movements
        .iter()
        .take(n)
        .map(|m| (ang(m.curve.start()), ang(m.curve.end())))
        .collect();
    let mut out = vec![0u128; n];
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (&movements[i], &movements[j]);
            let same_in = a.from_arm == b.from_arm && a.from_lane == b.from_lane;
            let same_out = a.to_arm == b.to_arm && a.to_lane == b.to_lane;
            let c = if same_in {
                false
            } else if same_out || a.turn == Turn::UTurn || b.turn == Turn::UTurn {
                true
            } else {
                let (a0, a1) = ends[i];
                let (b0, b1) = ends[j];
                on_arc(a0, a1, b0) != on_arc(a0, a1, b1) || paths_brush(a, b)
            };
            if c {
                out[i] |= 1 << j;
                out[j] |= 1 << i;
            }
        }
    }
    out
}

/// Do the paths of two movements come closer than a car's width (plus a
/// margin) anywhere, so two cars on them would touch? Catches movements
/// that do not cross topologically but pass side by side in a small box
/// (opposite left turns, a turn sweeping past a neighbouring lane). Only
/// products and sums of the curve points: the same on every machine.
fn paths_brush(a: &Movement, b: &Movement) -> bool {
    const SAMPLES: usize = 16;
    let clear2 = balance::MOVEMENT_CLEARANCE_M * balance::MOVEMENT_CLEARANCE_M;
    let pts = |m: &Movement| -> [[f32; 2]; SAMPLES + 1] {
        let mut out = [[0.0; 2]; SAMPLES + 1];
        for (k, p) in out.iter_mut().enumerate() {
            *p = m.curve.point_t(k as f32 / SAMPLES as f32);
        }
        out
    };
    let (pa, pb) = (pts(a), pts(b));
    pa.iter().any(|p| {
        pb.iter().any(|q| {
            let d = geom::sub(*p, *q);
            geom::dot(d, d) < clear2
        })
    })
}

/// Major arms of a priority junction: the highest-class opposite pair (the
/// straight-through road), or the two highest-class arms. When two pairs
/// are equally good (a crossing of two equal streets) no arm is major and
/// the junction works first come, first served (all-way give-way).
fn mark_major(arms: &mut [Arm]) {
    let deg = arms.len();
    if deg < 3 {
        return;
    }
    let mut best: Option<((u8, u8), usize, usize)> = None;
    let mut tie = false;
    for i in 0..deg {
        for j in i + 1..deg {
            let straight = arms[i].opposite == Some(j as u8);
            let key = (arms[i].kind.rank() + arms[j].kind.rank(), u8::from(straight));
            match &best {
                Some(b) if key == b.0 => tie = true,
                Some(b) if key < b.0 => {}
                _ => {
                    best = Some((key, i, j));
                    tie = false;
                }
            }
        }
    }
    for a in arms.iter_mut() {
        a.major = false;
    }
    // Two equally good through roads (a crossing of equal streets): no
    // major road, everyone gives way first come, first served.
    if let Some((_, i, j)) = best.filter(|_| !tie) {
        arms[i].major = true;
        arms[j].major = true;
    }
}

/// Phase plan from the arms: opposite approaches share a phase, the main
/// road first; avenues get a protected left phase before their through
/// phase.
fn build_plan(roads: &RoadNetwork, n: NodeId, arms: &[Arm]) -> SignalPlan {
    let deg = arms.len();
    let mut groups: Vec<Vec<u8>> = Vec::new();
    let mut used = vec![false; deg];
    for i in 0..deg {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut g = vec![i as u8];
        if let Some(j) = arms[i].opposite {
            used[j as usize] = true;
            g.push(j);
        }
        groups.push(g);
    }
    let score = |g: &Vec<u8>| -> (u32, u32) {
        let rank: u32 = g.iter().map(|&a| arms[a as usize].kind.rank() as u32 * 10 + 1).sum();
        let lanes: u32 = g.iter().map(|&a| arms[a as usize].lanes_in as u32).sum();
        (rank, lanes)
    };
    // Stable: main group first, the rest in arm order.
    let main = (0..groups.len())
        .max_by(|&a, &b| score(&groups[a]).cmp(&score(&groups[b])).then(b.cmp(&a)))
        .unwrap_or(0);
    let main_group = groups.remove(main);
    groups.insert(0, main_group);
    let mut phases = Vec::new();
    let mut main = 0u8;
    for (k, g) in groups.iter().enumerate() {
        let wide = g.len() == 2 && g.iter().any(|&a| arms[a as usize].lanes_in >= 2);
        if wide {
            phases.push(SignalPhase {
                arms: g.clone(),
                left_only: true,
                min_green_s: balance::SIGNAL_LEFT_MIN_GREEN_S,
                max_green_s: balance::SIGNAL_LEFT_MAX_GREEN_S,
            });
        }
        if k == 0 {
            main = phases.len() as u8;
        }
        phases.push(SignalPhase {
            arms: g.clone(),
            left_only: false,
            min_green_s: balance::SIGNAL_MIN_GREEN_S,
            max_green_s: if k == 0 {
                balance::SIGNAL_MAIN_MAX_GREEN_S
            } else {
                balance::SIGNAL_SIDE_MAX_GREEN_S
            },
        });
    }
    // Coordination along avenues: the main group is an avenue pair.
    let g0 = &groups[0];
    // Only where arterials meet: a street crossing is served on demand.
    let coordinated = g0.len() == 2
        && g0.iter().all(|&a| arms[a as usize].kind == RoadKind::Avenue)
        && arms.iter().all(|a| a.kind.rank() >= RoadKind::Avenue.rank());
    let cycle_s = balance::SIGNAL_COORD_CYCLE_S;
    let offset_s = if coordinated {
        // Canonical axis direction (pointing to +x, or +y when vertical).
        let mut d = arms[g0[0] as usize].dir;
        if d[0] < -1e-3 || (d[0].abs() <= 1e-3 && d[1] < 0.0) {
            d = [-d[0], -d[1]];
        }
        let p = roads.nodes[n].pos;
        let along = geom::dot(p, d);
        (along / balance::SIGNAL_PROGRESSION_SPEED).rem_euclid(cycle_s)
    } else {
        0.0
    };
    SignalPlan {
        phases,
        coordinated,
        cycle_s,
        offset_s,
        main,
    }
}

/// Control `Auto` resolves to.
pub(crate) fn auto_control(arms: &[Arm], busy: bool) -> JunctionControl {
    // Arterials meeting (avenues, the highway), or a street crossing an
    // avenue: signals. A side street joining an avenue (a T) gives way
    // until it gets busy.
    let arterial = arms.iter().filter(|a| a.kind.rank() >= RoadKind::Avenue.rank()).count();
    let through_pair = arterial == 2
        && arms.iter().enumerate().any(|(i, a)| {
            a.kind.rank() >= RoadKind::Avenue.rank()
                && a.opposite
                    .is_some_and(|o| arms[o as usize].kind.rank() >= RoadKind::Avenue.rank() && o as usize != i)
        });
    let crossing = arterial >= 1 && arms.len() >= 4;
    if busy || arterial >= 3 || (arterial == 2 && !through_pair) || crossing {
        JunctionControl::Signal
    } else {
        JunctionControl::Priority
    }
}

fn resolve(setting: JunctionSetting, arms: &[Arm], busy: bool) -> JunctionControl {
    match setting {
        JunctionSetting::Auto => auto_control(arms, busy),
        JunctionSetting::Signal => JunctionControl::Signal,
        JunctionSetting::Priority => JunctionControl::Priority,
    }
}

/// Fresh state for node `n`, keeping the setting, busy flag and (when the
/// geometry did not change) the signal state of `old`.
pub(crate) fn build_state(roads: &RoadNetwork, n: NodeId, old: Option<&JunctionState>) -> JunctionState {
    let (mut arms, movements) = build_geometry(roads, n);
    mark_major(&mut arms);
    let conflicts = build_conflicts(roads.nodes[n].pos, &movements);
    let deg = arms.len();
    let setting = old.map_or(JunctionSetting::Auto, |o| o.setting);
    let busy = old.is_some_and(|o| o.busy);
    let control = if deg >= 3 {
        resolve(setting, &arms, busy)
    } else {
        JunctionControl::Priority
    };
    let plan = (deg >= 3).then(|| build_plan(roads, n, &arms));
    let same_shape = old
        .is_some_and(|o| o.arms.len() == arms.len() && o.arms.iter().zip(&arms).all(|(a, b)| a.segment == b.segment));
    let signal = match old {
        Some(o) if same_shape && o.plan == plan => o.signal,
        _ => fresh_signal(plan.as_ref()),
    };
    JunctionState {
        setting,
        control,
        queue: vec![0; deg],
        arms,
        movements,
        conflicts,
        plan,
        signal,
        entered_minute: old.map_or(0, |o| o.entered_minute),
        peak_minute_today: old.map_or(0, |o| o.peak_minute_today),
        peak_minute_yesterday: old.map_or(0, |o| o.peak_minute_yesterday),
        busy,
        clear_at: old.map_or(0, |o| o.clear_at),
        last_served: old.filter(|_| same_shape).map_or(0, |o| o.last_served),
        phase_count: old.filter(|_| same_shape).map_or(0, |o| o.phase_count),
    }
}

/// A signal starting on its main through phase.
fn fresh_signal(plan: Option<&SignalPlan>) -> SignalState {
    let main = plan.map_or(0, |p| p.main);
    SignalState {
        phase: main,
        next: main,
        ..SignalState::default()
    }
}

/// Change the setting of a junction state and re-resolve its control.
pub(crate) fn set_setting(j: &mut JunctionState, setting: JunctionSetting) {
    j.setting = setting;
    let c = resolve(setting, &j.arms, j.busy);
    if c != j.control {
        j.control = c;
        j.signal = fresh_signal(j.plan.as_ref());
    }
}

/// Control the junction would have under `setting`.
pub(crate) fn control_under(j: &JunctionState, setting: JunctionSetting) -> JunctionControl {
    resolve(setting, &j.arms, j.busy)
}

// -------------------------------------------------------------- signals ----

fn ticks(s: f32) -> u32 {
    (s * TICKS_PER_SECOND as f32).round() as u32
}

/// Per-arm demand seen by a signal this tick.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ArmDemand {
    /// A vehicle within detection range wants a non-left movement.
    pub through: bool,
    /// ... wants a left turn (or U-turn).
    pub left: bool,
}

fn phase_demand(phase: &SignalPhase, demand: &[ArmDemand]) -> bool {
    phase.arms.iter().any(|&a| {
        let d = demand.get(a as usize).copied().unwrap_or_default();
        if phase.left_only { d.left } else { d.through || d.left }
    })
}

/// Advance a signal by one tick. `box_clear`: nobody is inside the box
/// (all-red may end early once it is).
pub(crate) fn tick_signal(j: &mut JunctionState, demand: &[ArmDemand], now: u64, box_clear: bool) {
    let Some(plan) = j.plan.as_ref() else { return };
    if j.control != JunctionControl::Signal || plan.phases.is_empty() {
        return;
    }
    let np = plan.phases.len();
    let s = &mut j.signal;
    if s.phase as usize >= np {
        *s = fresh_signal(Some(plan));
    }
    let main = plan.main;
    s.ticks += 1;
    match s.stage {
        SignalStage::Green => {
            let phase = &plan.phases[s.phase as usize];
            let elapsed = s.ticks;
            if elapsed < ticks(phase.min_green_s) {
                return;
            }
            // Next phase with demand, in plan order.
            let next = (1..np)
                .map(|k| (s.phase as usize + k) % np)
                .find(|&p| phase_demand(&plan.phases[p], demand));
            let Some(mut next) = next else {
                // Nobody else waits: rest in this phase.
                return;
            };
            let mine = phase_demand(phase, demand);
            let mut end = !mine || elapsed >= ticks(phase.max_green_s);
            if plan.coordinated {
                let c = ticks(plan.cycle_s).max(1) as u64;
                let t = (now + c - (ticks(plan.offset_s) as u64 % c)) % c;
                let clear = ticks(balance::SIGNAL_YELLOW_S + balance::SIGNAL_ALL_RED_S) as u64;
                if s.phase == main {
                    // The avenue keeps green until its share of the cycle
                    // is used (a platoon is on its way).
                    let hold = (c as f32 * balance::SIGNAL_COORD_MAIN_SHARE) as u64;
                    // An empty avenue gaps out early (the phase returns
                    // at the next cycle start anyway).
                    end = end && (!mine || t >= hold) && t + clear < c;
                } else if t + clear >= c {
                    // Force-off so the avenue turns green on time.
                    end = true;
                    next = main as usize;
                }
            }
            if end {
                s.stage = SignalStage::Yellow;
                s.ticks = 0;
                s.next = next as u8;
            }
        }
        SignalStage::Yellow => {
            if s.ticks >= ticks(balance::SIGNAL_YELLOW_S) {
                s.stage = SignalStage::AllRed;
                s.ticks = 0;
            }
        }
        SignalStage::AllRed => {
            let min = ticks(balance::SIGNAL_ALL_RED_S);
            if s.ticks >= min && (box_clear || s.ticks >= 3 * min) {
                s.stage = SignalStage::Green;
                s.ticks = 0;
                s.phase = s.next.min(np as u8 - 1);
            }
        }
    }
}

/// Estimated seconds until arm `arm`'s main aspect changes.
fn time_to_change(j: &JunctionState, arm: usize, any_demand_elsewhere: bool) -> Option<f32> {
    let plan = j.plan.as_ref()?;
    let s = j.signal;
    let tps = TICKS_PER_SECOND as f32;
    let el = s.ticks as f32 / tps;
    let phase = plan.phases.get(s.phase as usize)?;
    let green_here = phase.arms.contains(&(arm as u8)) && !phase.left_only;
    let clear = balance::SIGNAL_YELLOW_S + balance::SIGNAL_ALL_RED_S;
    match s.stage {
        SignalStage::Yellow if green_here => Some((balance::SIGNAL_YELLOW_S - el).max(0.0)),
        SignalStage::Green if green_here => {
            if !any_demand_elsewhere {
                None
            } else {
                Some((phase.min_green_s - el).max(0.0))
            }
        }
        _ => {
            // Red: remaining clearance plus the greens of the phases before
            // this arm's next through phase (at their minimum).
            let mut t = match s.stage {
                SignalStage::Green => (phase.min_green_s - el).max(0.0) + clear,
                SignalStage::Yellow => (balance::SIGNAL_YELLOW_S - el).max(0.0) + balance::SIGNAL_ALL_RED_S,
                SignalStage::AllRed => (balance::SIGNAL_ALL_RED_S - el).max(0.0),
            };
            let np = plan.phases.len();
            let start = if s.stage == SignalStage::Green {
                s.phase as usize + 1
            } else {
                s.next as usize
            };
            for k in 0..np {
                let p = &plan.phases[(start + k) % np];
                if p.arms.contains(&(arm as u8)) && !p.left_only {
                    return Some(t);
                }
                t += p.min_green_s + clear;
            }
            None
        }
    }
}

// -------------------------------------------------------------- queries ----

/// State of the junction at node `n`, if it has one (three or more arms).
pub fn junction(world: &World, n: NodeId) -> Option<&JunctionState> {
    world.traffic.junction_states.get(n).filter(|j| j.arms.len() >= 3)
}

/// Control of the junction at `n` (`None` for nodes with fewer than three
/// arms).
pub fn junction_control(world: &World, n: NodeId) -> Option<JunctionControl> {
    junction(world, n).map(|j| j.control)
}

/// The node at a cell centre, if it is a junction (three or more arms).
pub fn junction_at(world: &World, cell: Cell) -> Option<NodeId> {
    let n = world.roads.node_at(cell.center())?;
    (world.roads.nodes[n].segments.len() >= 3).then_some(n)
}

/// Aspect shown to the straight-on / right-turn traffic arriving at node
/// `n` along segment `approach` (`None` without signals there).
pub fn signal_aspect(world: &World, n: NodeId, approach: SegmentId) -> Option<Aspect> {
    let j = junction(world, n)?;
    let arm = j.arm_of(approach)?;
    approach_aspects(j, arm).0
}

/// Aspect shown to the movement from `from` into `to` at node `n`.
pub fn movement_aspect(world: &World, n: NodeId, from: SegmentId, to: SegmentId) -> Option<Aspect> {
    let j = junction(world, n)?;
    let (a, b) = (j.arm_of(from)?, j.arm_of(to)?);
    let m = j.movement(a, 0, b)?;
    j.movement_aspect(m)
}

/// (main aspect, left aspect, left protected) of an arm.
fn approach_aspects(j: &JunctionState, arm: usize) -> (Option<Aspect>, Option<Aspect>, bool) {
    if j.control != JunctionControl::Signal {
        return (None, None, false);
    }
    let mut main: Option<Aspect> = None;
    let mut left: Option<Aspect> = None;
    let mut prot = false;
    let rank = |a: Aspect| match a {
        Aspect::Green => 2,
        Aspect::Yellow => 1,
        Aspect::Red => 0,
    };
    for (i, m) in j.movements.iter().enumerate() {
        if m.from_arm as usize != arm {
            continue;
        }
        let Some(a) = j.movement_aspect(i) else { continue };
        let slot = if matches!(m.turn, Turn::Left | Turn::UTurn) {
            if a != Aspect::Red && j.protected(i) {
                prot = true;
            }
            &mut left
        } else {
            &mut main
        };
        if slot.is_none_or(|s| rank(a) > rank(s)) {
            *slot = Some(a);
        }
    }
    if main.is_none() {
        main = left;
    }
    if left.is_none() {
        left = main;
    }
    (main, left, prot)
}

/// Everything about one junction for renderers and the interface.
pub fn junction_view(world: &World, n: NodeId) -> Option<JunctionView> {
    let j = junction(world, n)?;
    let signal = j.control == JunctionControl::Signal;
    let approaches = j
        .arms
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let (aspect, left_aspect, left_protected) = approach_aspects(j, i);
            let elsewhere = j.queue.iter().enumerate().any(|(k, &q)| k != i && q > 0);
            ApproachView {
                segment: a.segment,
                aspect,
                left_aspect,
                left_protected,
                time_to_change_s: if signal { time_to_change(j, i, elsewhere) } else { None },
                queue: j.queue.get(i).copied().unwrap_or(0),
                major: signal || a.major,
                setback: a.setback,
            }
        })
        .collect();
    Some(JunctionView {
        node: n,
        control: j.control,
        setting: j.setting,
        approaches,
        phase: signal.then_some((j.signal.phase, j.signal.stage)),
        phase_count: j.plan.as_ref().map_or(0, |p| p.phases.len() as u8),
        peak_per_minute: j.peak_minute_yesterday,
    })
}

/// All junction nodes (three or more arms), in slot order.
pub fn junction_nodes(world: &World) -> impl Iterator<Item = NodeId> + '_ {
    world
        .roads
        .nodes
        .iter()
        .filter(|(_, n)| n.segments.len() >= 3)
        .map(|(id, _)| id)
}

/// Stop line distance from node `n` on segment `s`, metres (0 when the
/// node has no box).
pub fn setback(world: &World, n: NodeId, s: SegmentId) -> f32 {
    world
        .traffic
        .junction_states
        .get(n)
        .and_then(|j| j.arm_of(s).map(|a| j.arms[a].setback))
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------- shape ----

/// Where an arm's road surface meets the junction surface: the line across
/// the road `setback` metres from the node.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ArmCut {
    pub segment: SegmentId,
    /// Metres along the segment's centre line from the node.
    pub setback: f32,
    /// Centre-line point of the cut.
    pub center: [f32; 2],
    /// Unit direction along the road away from the node at the cut.
    pub dir: [f32; 2],
    /// Kerb ends of the cut: left and right seen looking away from the node
    /// (`right` = `center + right_of(dir) * half_width`).
    pub left: [f32; 2],
    pub right: [f32; 2],
    /// Half the width of the cut (carriageway plus the requested extra).
    pub half_width: f32,
}

/// A junction's surface for renderers: the outline polygon and the cut
/// line of every arm, built from the same geometry vehicles use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JunctionShape {
    pub node: NodeId,
    pub pos: [f32; 2],
    /// Arms in counter-clockwise order (sim axes: from `+x` towards `+y`).
    pub arms: Vec<ArmCut>,
    /// Closed polygon, counter-clockwise, no repeated first point: for each
    /// arm its `left` then `right` cut point, then kerb-fillet points round
    /// to the next arm's `left`.
    pub outline: Vec<[f32; 2]>,
}

/// The surface of node `n` with its kerbs `extra` metres outside the
/// carriageway (0 for the asphalt, the sidewalk width for the pavement).
/// `None` for dead ends and for nodes without a box (straight or smooth
/// continuations).
pub fn junction_shape(world: &World, n: NodeId, extra: f32) -> Option<JunctionShape> {
    let roads = &world.roads;
    let node = roads.nodes.get(n)?;
    if node.segments.len() < 2 {
        return None;
    }
    let mut arms: Vec<(f32, ArmCut)> = Vec::new();
    for &s in &node.segments {
        let seg = roads.segments.get(s)?;
        let sb = setback(world, n, s).min(seg.length);
        let forward = seg.a == n;
        let pose = seg.pose_dir(forward, sb, 0.0);
        let hw = seg.kind.half_width_m() + extra;
        let r = right_of(pose.dir);
        let cut = ArmCut {
            segment: s,
            setback: sb,
            center: pose.pos,
            dir: pose.dir,
            left: [pose.pos[0] - r[0] * hw, pose.pos[1] - r[1] * hw],
            right: [pose.pos[0] + r[0] * hw, pose.pos[1] + r[1] * hw],
            half_width: hw,
        };
        arms.push((pseudo_angle(seg.dir_from(n)), cut));
    }
    if arms.iter().all(|(_, a)| a.setback <= 1e-3) {
        return None;
    }
    arms.sort_by(|x, y| x.0.total_cmp(&y.0));
    let arms: Vec<ArmCut> = arms.into_iter().map(|(_, a)| a).collect();
    let mut outline = Vec::new();
    let k = arms.len();
    for i in 0..k {
        let (a, b) = (&arms[i], &arms[(i + 1) % k]);
        outline.push(a.left);
        outline.push(a.right);
        // Kerb fillet from this arm's right kerb to the next arm's left one:
        // a quadratic whose control point is where the two kerb lines meet
        // (towards the node), else a straight edge.
        let ctrl = intersect(a.right, a.dir, b.left, b.dir)
            .filter(|c| geom::dist(*c, node.pos) < 3.0 * (a.half_width + b.half_width) + a.setback + b.setback);
        if let Some(c) = ctrl {
            for t in [0.25f32, 0.5, 0.75] {
                let u = 1.0 - t;
                outline.push([
                    u * u * a.right[0] + 2.0 * u * t * c[0] + t * t * b.left[0],
                    u * u * a.right[1] + 2.0 * u * t * c[1] + t * t * b.left[1],
                ]);
            }
        }
    }
    Some(JunctionShape {
        node: n,
        pos: node.pos,
        arms,
        outline,
    })
}
