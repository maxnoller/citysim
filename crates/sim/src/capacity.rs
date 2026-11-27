//! Junction capacity bench: one junction on an empty map, loaded from its
//! approaches, and the cars it lets through measured. Used by the capacity
//! tests and the `jbench` example; it drives the real simulation (cars are
//! ordinary trips), nothing is special-cased.
//!
//! The junction sits at cell (128, 64). The main road runs west–east
//! through it, the cross road north–south (or only south: a T). Every arm
//! is [`ARM_CELLS`] cells long. Cars start near the far end of an
//! approach arm and drive to the far end of the arm their turn leads to.
//! Demand per approach is either *saturated* (a standing queue is kept on
//! the approach all the time) or a rate in cars per minute.

use crate::junction::{self, Aspect, JunctionControl, JunctionSetting, Turn};
use crate::roads::{NodeId, RoadKind, SegmentId};
use crate::traffic::{self, Place, VehicleId};
use crate::{Cell, Command, Rng, TICKS_PER_SECOND, World, geom, scenarios};

/// Arm length in cells.
pub const ARM_CELLS: i32 = 40;
const CENTER: (i32, i32) = (128, 64);
/// Cars kept queued on a saturated approach (spawned when fewer stand or
/// drive towards the junction on it).
const SATURATED_QUEUE: usize = 30;
/// Cars start anywhere on the last cells of their arm (one start point
/// alone would cap the flow at what a car pulling out allows).
const SPAWN_CELLS: i32 = 12;

/// Demand on one approach.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Load {
    /// Nobody comes from this arm.
    None,
    /// Always a queue waiting.
    Saturated,
    /// Cars per minute (spread evenly with a little jitter).
    Rate(f32),
}

/// One bench case.
#[derive(Debug, Clone, PartialEq)]
pub struct Bench {
    /// West–east road.
    pub main: RoadKind,
    /// North–south road (`None`: no junction, a plain link).
    pub cross: Option<RoadKind>,
    /// Only the south arm of the cross road (a T junction).
    pub tee: bool,
    pub control: JunctionSetting,
    /// Shares of cars turning left and right (the rest go straight on; at
    /// a T the side road's cars turn left or right in proportion).
    pub left: f32,
    pub right: f32,
    /// Demand on the main road's approaches and on the cross road's.
    pub main_load: Load,
    pub cross_load: Load,
    /// A roundabout of this size at the junction (over `control`).
    pub ring: Option<crate::roundabout::RoundaboutSize>,
}

impl Bench {
    pub fn crossing(main: RoadKind, cross: RoadKind, control: JunctionSetting) -> Bench {
        Bench {
            main,
            cross: Some(cross),
            tee: false,
            control,
            left: 0.0,
            right: 0.0,
            main_load: Load::Saturated,
            cross_load: Load::Saturated,
            ring: None,
        }
    }

    /// The junction as a roundabout of `size`.
    pub fn roundabout(mut self, size: crate::roundabout::RoundaboutSize) -> Bench {
        self.ring = Some(size);
        self
    }

    pub fn turning(mut self, left: f32, right: f32) -> Bench {
        self.left = left;
        self.right = right;
        self
    }

    pub fn tee(mut self) -> Bench {
        self.tee = true;
        self
    }

    pub fn loads(mut self, main: Load, cross: Load) -> Bench {
        self.main_load = main;
        self.cross_load = cross;
        self
    }
}

/// What the bench measured.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchResult {
    /// Cars entering the junction per minute, all approaches.
    pub per_minute: f32,
    /// Per approach (west, east, north, south), cars per minute.
    pub by_arm: [f32; 4],
    /// Main road approaches only, cars per minute.
    pub main_per_minute: f32,
    /// Cross road approaches only.
    pub cross_per_minute: f32,
    /// Cars per second per lane while the approach shows green (signals;
    /// main road approaches, all movements).
    pub green_flow_per_lane: Option<f32>,
    /// Share of the time the main road shows green (signals).
    pub main_green_share: Option<f32>,
    pub gave_up: u64,
    pub control: Option<JunctionControl>,
    /// Main road entries by lane (kerb, inner), cars per minute.
    pub main_by_lane: [f32; 2],
    /// Entries by turn (straight, right, left, U-turn), cars per minute.
    pub by_turn: [f32; 4],
}

struct ArmInfo {
    segment: SegmentId,
    /// Where cars start (spread over the far end) and where they end.
    starts: Vec<[f32; 2]>,
    end: [f32; 2],
    load: Load,
    lanes: u8,
    acc: f32,
}

fn cell(dx: i32, dy: i32) -> Cell {
    Cell::new(CENTER.0 + dx, CENTER.1 + dy)
}

/// Build the bench world (money is no object).
pub fn build(b: &Bench) -> (World, Option<NodeId>) {
    let mut w = World::new(1);
    w.economy.money = i64::MAX / 4;
    let r = ARM_CELLS;
    match b.cross {
        None => {
            scenarios::road_of(&mut w, (CENTER.0 - r, CENTER.1), (CENTER.0 + r, CENTER.1), b.main);
        }
        Some(k) => {
            scenarios::road_of(&mut w, (CENTER.0 - r, CENTER.1), (CENTER.0 + r, CENTER.1), b.main);
            let top = if b.tee { CENTER.1 } else { CENTER.1 - r };
            scenarios::road_of(&mut w, (CENTER.0, top), (CENTER.0, CENTER.1 + r), k);
        }
    }
    let node = w.roads.node_at(cell(0, 0).center());
    if let Some(n) = node {
        w.apply(Command::SetNodeControl {
            node: n,
            control: b.control,
        })
        .expect("bench junction control");
        if let Some(size) = b.ring {
            w.apply(Command::SetRoundabout {
                node: n,
                size: Some(size),
            })
            .expect("bench roundabout");
        }
    }
    w.economy.money = i64::MAX / 4;
    // Settle the junction tables.
    w.step();
    (w, node)
}

/// Run a bench: `warmup_s` seconds to fill the queues, then `measure_s`
/// seconds counted.
pub fn run(b: &Bench, seed: u64, warmup_s: u32, measure_s: u32) -> BenchResult {
    run_traced(b, seed, warmup_s, measure_s, &mut |_, _, _| {})
}

/// [`run`], calling `trace(world, junction node, tick)` after every step
/// (diagnostics).
pub fn run_traced(
    b: &Bench,
    seed: u64,
    warmup_s: u32,
    measure_s: u32,
    trace: &mut dyn FnMut(&World, Option<NodeId>, u32),
) -> BenchResult {
    let (mut w, node) = build(b);
    let r = ARM_CELLS - 1;
    // Arms: west, east, north, south (the direction they lie in).
    let dirs: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    let mut arms: Vec<Option<ArmInfo>> = Vec::new();
    for (i, &(dx, dy)) in dirs.iter().enumerate() {
        let exists = match (i, b.cross) {
            (0 | 1, _) => true,
            (_, None) => false,
            (2, Some(_)) => !b.tee,
            _ => true,
        };
        if !exists {
            arms.push(None);
            continue;
        }
        let starts: Vec<[f32; 2]> = (1..=SPAWN_CELLS)
            .map(|k| cell(dx * (r - k), dy * (r - k)).center())
            .collect();
        let end = cell(dx * r, dy * r).center();
        let probe = cell(dx * 3, dy * 3).center();
        let Some((segment, _, _)) = w.roads.nearest_segment(probe, 2.0) else {
            arms.push(None);
            continue;
        };
        let load = if i < 2 { b.main_load } else { b.cross_load };
        arms.push(Some(ArmInfo {
            segment,
            starts,
            end,
            load,
            lanes: w.roads.segments[segment].lane_count(),
            acc: 0.0,
        }));
    }
    // Turn from each approach to each exit.
    let turn_of = |w: &World, from: usize, to: usize| -> Option<Turn> {
        if from == to {
            return None;
        }
        let (Some(fa), Some(ta)) = (arms[from].as_ref(), arms[to].as_ref()) else {
            return None;
        };
        match node.and_then(|n| junction::junction(w, n)) {
            Some(j) => {
                let (a, bb) = (j.arm_of(fa.segment)?, j.arm_of(ta.segment)?);
                j.movements_between(a, bb).next().map(|(_, m)| m.turn)
            }
            // A plain link: only straight on.
            None => (dirs[from].0 == -dirs[to].0 && dirs[from].1 == -dirs[to].1).then_some(Turn::Straight),
        }
    };
    let mut exits: [[Option<Turn>; 4]; 4] = [[None; 4]; 4];
    for (from, row) in exits.iter_mut().enumerate() {
        for (to, e) in row.iter_mut().enumerate() {
            *e = turn_of(&w, from, to);
        }
    }
    let mut rng = Rng::new(seed);
    let total_s = warmup_s + measure_s;
    let tps = TICKS_PER_SECOND;
    let mut in_box: Vec<VehicleId> = Vec::new();
    let mut counted = [0u64; 4];
    let mut lane_count = [0u64; 2];
    let mut turn_count = [0u64; 4];
    let mut green_ticks = [0u64; 4];
    let mut green_entries = [0u64; 4];
    let mut gave_up_base = w.traffic.stuck_removed;
    for t in 0..total_s * tps {
        let measuring = t >= warmup_s * tps;
        if t == warmup_s * tps {
            gave_up_base = w.traffic.stuck_removed;
        }
        // Demand.
        if t % 2 == 0 {
            for from in 0..4 {
                let Some(a) = arms[from].as_ref() else { continue };
                let want = match a.load {
                    Load::None => 0,
                    Load::Saturated => {
                        let on = w
                            .vehicles
                            .values()
                            .filter(|v| v.segment == Some(a.segment) && v.junction.is_none() && heads_in(&w, v, node))
                            .count();
                        let pend = w
                            .traffic
                            .pending
                            .iter()
                            .filter(
                                |d| matches!(d.from, Place::At(p) if a.starts.iter().any(|&q| geom::dist(p, q) < 1.0)),
                            )
                            .count();
                        usize::from(on + pend < SATURATED_QUEUE * a.lanes as usize)
                    }
                    Load::Rate(per_min) => {
                        let acc = arms[from].as_ref().map_or(0.0, |a| a.acc) + per_min / 60.0 * 2.0 / tps as f32;
                        let n = acc.floor();
                        if let Some(a) = arms[from].as_mut() {
                            a.acc = acc - n;
                        }
                        n as usize
                    }
                };
                for _ in 0..want {
                    let to = pick_exit(&exits[from], b, &mut rng);
                    if let (Some(to), Some(a)) = (to, arms[from].as_ref())
                        && let Some(ta) = arms[to].as_ref()
                    {
                        let start = a.starts[rng.below(a.starts.len() as u32) as usize];
                        traffic::debug_request(&mut w, start, ta.end);
                    }
                }
            }
        }
        w.step();
        trace(&w, node, t);
        // Entries into the box.
        if let Some(n) = node {
            let mut now_in: Vec<VehicleId> = Vec::new();
            for (id, v) in w.vehicles.iter() {
                if let Some(jm) = v.junction.filter(|jm| jm.node == n) {
                    now_in.push(id);
                    if measuring && !in_box.contains(&id) {
                        let from = arms
                            .iter()
                            .position(|a| a.as_ref().is_some_and(|a| a.segment == jm.from));
                        if let Some(f) = from {
                            counted[f] += 1;
                            if f < 2 {
                                lane_count[(jm.from_lane as usize).min(1)] += 1;
                            }
                            turn_count[match jm.turn {
                                Turn::Straight => 0,
                                Turn::Right => 1,
                                Turn::Left => 2,
                                Turn::UTurn => 3,
                            }] += 1;
                            if junction::signal_aspect(&w, n, jm.from).is_some() {
                                green_entries[f] += 1;
                            }
                        }
                    }
                }
            }
            in_box = now_in;
            if measuring {
                for (i, a) in arms.iter().enumerate() {
                    if let Some(a) = a
                        && matches!(
                            junction::signal_aspect(&w, n, a.segment),
                            Some(Aspect::Green | Aspect::Yellow)
                        )
                    {
                        green_ticks[i] += 1;
                    }
                }
            }
        }
    }
    let mins = measure_s as f32 / 60.0;
    let by_arm = counted.map(|c| c as f32 / mins);
    let main_per_minute = by_arm[0] + by_arm[1];
    let cross_per_minute = by_arm[2] + by_arm[3];
    let control = node.and_then(|n| junction::junction_control(&w, n));
    let signal = control == Some(JunctionControl::Signal);
    let main_lanes: u64 = arms[..2].iter().flatten().map(|a| a.lanes as u64).sum::<u64>() / 2;
    let green_flow_per_lane = (signal && green_ticks[0] + green_ticks[1] > 0).then(|| {
        let g = (green_ticks[0] + green_ticks[1]) as f32 / tps as f32;
        (green_entries[0] + green_entries[1]) as f32 / g / main_lanes.max(1) as f32
    });
    let main_green_share = signal.then(|| (green_ticks[0] as f32 / tps as f32) / measure_s as f32);
    BenchResult {
        per_minute: main_per_minute + cross_per_minute,
        by_arm,
        main_per_minute,
        cross_per_minute,
        green_flow_per_lane,
        main_green_share,
        gave_up: (w.traffic.stuck_removed - gave_up_base) as u64,
        control,
        main_by_lane: lane_count.map(|c| c as f32 / mins),
        by_turn: turn_count.map(|c| c as f32 / mins),
    }
}

/// Is `v` driving towards the bench junction?
fn heads_in(w: &World, v: &crate::Vehicle, node: Option<NodeId>) -> bool {
    let Some(n) = node else { return true };
    let Some(seg) = v.segment.and_then(|s| w.roads.segments.get(s)) else {
        return false;
    };
    (if v.forward { seg.b } else { seg.a }) == n
}

fn pick_exit(row: &[Option<Turn>; 4], b: &Bench, rng: &mut Rng) -> Option<usize> {
    let find = |t: Turn| row.iter().position(|&x| x == Some(t));
    let (l, r, s) = (find(Turn::Left), find(Turn::Right), find(Turn::Straight));
    let x = rng.next_f32();
    match s {
        Some(s) => {
            if x < b.left && l.is_some() {
                l
            } else if x < b.left + b.right && r.is_some() {
                r
            } else {
                Some(s)
            }
        }
        // The side road of a T: left or right in proportion.
        None => {
            let tot = (b.left + b.right).max(1e-6);
            if x < b.left / tot { l.or(r) } else { r.or(l) }
        }
    }
}
