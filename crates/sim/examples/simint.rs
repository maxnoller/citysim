//! Traffic diagnostics for the combined simulation: grow a scripted city to
//! a population once and save it, then replay a day from that save under
//! the current rules and see where trip time goes.
//!
//! ```text
//! cargo run -p citysim_sim --release --example simint -- build hierarchy 7 8000 /tmp/h8k.citysave
//! cargo run -p citysim_sim --release --example simint -- day /tmp/h8k.citysave
//! ```
//! `build <scenario> <seed> <population> <out>`: hierarchy, streets,
//! single_artery, grid or organic; stops at the first midnight after the
//! population is reached. `day <save> [hours]`: runs `hours` (default 24)
//! from the save and prints, per in-game hour, the trips that ended, cars
//! given up, the door-to-door commute, and where the cars on the road spent
//! their time (moving, at a red light, held at a line otherwise, queued
//! behind another car, inside a box), then the junctions where cars gave up.

use std::collections::BTreeMap;

use citysim_sim::junction::Aspect;
use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;

fn player(which: &str, w: &mut World) -> Option<GridPlayer> {
    match which {
        "hierarchy" => Some(GridPlayer::hierarchy()),
        "streets" | "grid" => Some(GridPlayer::new(&Zone::ALL)),
        "single_artery" => Some(GridPlayer::single_artery()),
        "two_grid" | "artery" => {
            let grid = which == "two_grid";
            scenarios::prebuilt(w, |w| scenarios::two_districts(w, grid));
            None
        }
        "organic" => {
            scenarios::prebuilt(w, |w| {
                scenarios::organic(w);
            });
            None
        }
        other => panic!("unknown scenario {other}"),
    }
}

fn build(args: &[String]) {
    let which = args[0].as_str();
    let seed: u64 = args[1].parse().unwrap();
    let pop: u32 = args[2].parse().unwrap();
    let out = &args[3];
    let mut w = World::new(seed);
    let mut p = player(which, &mut w);
    loop {
        if let Some(p) = p.as_mut() {
            p.act(&mut w);
        }
        w.step();
        if w.stats.population >= pop && (w.time.tick_of_day == 0 || std::env::var("SIMINT_NOW").is_ok()) {
            break;
        }
        if w.time.day > 60 {
            println!("did not reach {pop} by day 60");
            break;
        }
    }
    std::fs::write(out, w.save()).unwrap();
    println!(
        "saved {out}: day {} pop {} cars {} given up {}",
        w.time.day,
        w.stats.population,
        w.vehicles.len(),
        w.traffic.stuck_removed
    );
}

/// Time shares of the cars on the road, car-ticks.
#[derive(Default, Clone, Copy)]
struct Where {
    moving: u64,
    red: u64,
    held: u64,
    queued: u64,
    boxed: u64,
    pending: u64,
    highway: u64,
}

fn classify(w: &World, acc: &mut Where, by_node: &mut BTreeMap<NodeId, [u64; 3]>) {
    let mut fronts: BTreeMap<(SegmentId, bool, u8), f32> = BTreeMap::new();
    for v in w.vehicles.values() {
        if v.junction.is_none() {
            let e = fronts
                .entry((v.segment.unwrap(), v.forward, v.lane))
                .or_insert(f32::NEG_INFINITY);
            *e = e.max(v.progress);
        }
    }
    acc.pending += w.traffic.pending.len() as u64;
    for t in w.traffic.trips.values() {
        if t.to == traffic::Place::Entry || t.from == traffic::Place::Entry {
            acc.highway += 1;
        }
    }
    for v in w.vehicles.values() {
        if v.junction.is_some() {
            acc.boxed += 1;
            continue;
        }
        if !v.stopped() {
            acc.moving += 1;
            continue;
        }
        let s = v.segment.unwrap();
        let seg = &w.roads.segments[s];
        let node = if v.forward { seg.b } else { seg.a };
        let e = by_node.entry(node).or_default();
        let front = fronts[&(s, v.forward, v.lane)] <= v.progress;
        if !front {
            acc.queued += 1;
            e[2] += 1;
            continue;
        }
        if junction::signal_aspect(w, node, s) == Some(Aspect::Red) {
            acc.red += 1;
            e[0] += 1;
        } else {
            acc.held += 1;
            e[1] += 1;
        }
    }
}

fn day(args: &[String]) {
    let bytes = std::fs::read(&args[0]).unwrap();
    let hours: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(24);
    let mut w = World::load(&bytes).unwrap();
    // Junction plans under the current rules.
    w.traffic.junctions_version = u64::MAX;
    println!(
        "loaded day {} {:.1}h pop {} cars {}",
        w.time.day,
        w.time.hour(),
        w.stats.population,
        w.vehicles.len()
    );
    w.step();
    let mut storage = 0.0f32;
    let mut lane_m = 0.0f32;
    for (sid, seg) in w.roads.segments.iter() {
        for fwd in [true, false] {
            if let Some((a, b)) = drive::drivable_range(&w, sid, fwd) {
                let l = (b - a).max(0.0) * seg.lane_count() as f32;
                lane_m += l;
                storage += l / balance::MIN_GAP_M;
            }
        }
    }
    println!(
        "network: {:.1} lane-km between stop lines, room for {:.0} standing cars; budget {}",
        lane_m / 1000.0,
        storage,
        traffic::vehicle_budget(&w)
    );
    let gave0 = traffic::debug_gave_up(&w).len();
    let tph = balance::TICKS_PER_HOUR as u64;
    let mut total = Where::default();
    let (mut trips_all, mut gave_all) = (0u32, 0u32);
    let mut step_ns = Vec::new();
    let mut by_node: BTreeMap<NodeId, [u64; 3]> = BTreeMap::new();
    println!(
        "hour  trips gaveup  commute | car-seconds per ended trip: moving red held queued box pending | cars stopped%"
    );
    for _ in 0..hours {
        let mut acc = Where::default();
        let gave_before = w.traffic.stuck_removed;
        let trips_before = w.traffic.trips_today;
        let mut trips = 0u32;
        let mut last_today = trips_before;
        let h = w.time.hour();
        for _ in 0..tph {
            let t0 = std::time::Instant::now();
            w.step();
            step_ns.push(t0.elapsed().as_nanos() as u64);
            if w.traffic.trips_today < last_today {
                // Midnight reset.
                last_today = 0;
            }
            trips += w.traffic.trips_today - last_today;
            last_today = w.traffic.trips_today;
            classify(&w, &mut acc, &mut by_node);
        }
        let gave = w.traffic.stuck_removed - gave_before;
        trips_all += trips;
        gave_all += gave;
        let n = trips.max(1) as f64 * TICKS_PER_SECOND as f64;
        let cars = (acc.moving + acc.red + acc.held + acc.queued + acc.boxed).max(1) as f64;
        println!(
            "{:5.1} {:6} {:6} {:7.1} | {:6.1} {:5.1} {:5.1} {:6.1} {:5.1} {:6.1} | {:4.0}% hw {:4.1}%",
            h,
            trips,
            gave,
            w.traffic.avg_commute_s,
            acc.moving as f64 / n,
            acc.red as f64 / n,
            acc.held as f64 / n,
            acc.queued as f64 / n,
            acc.boxed as f64 / n,
            acc.pending as f64 / n,
            100.0 * (acc.red + acc.held + acc.queued) as f64 / cars,
            100.0 * acc.highway as f64 / cars
        );
        total.moving += acc.moving;
        total.red += acc.red;
        total.held += acc.held;
        total.queued += acc.queued;
        total.boxed += acc.boxed;
        total.pending += acc.pending;
    }
    let n = trips_all.max(1) as f64 * TICKS_PER_SECOND as f64;
    println!(
        "total: {trips_all} trips, {gave_all} given up ({:.2} %); per trip moving {:.1} s, red {:.1}, held {:.1}, queued {:.1}, box {:.1}, pending {:.1}; pop {}",
        100.0 * gave_all as f64 / trips_all.max(1) as f64,
        total.moving as f64 / n,
        total.red as f64 / n,
        total.held as f64 / n,
        total.queued as f64 / n,
        total.boxed as f64 / n,
        total.pending as f64 / n,
        w.stats.population
    );
    step_ns.sort_unstable();
    let mean = step_ns.iter().sum::<u64>() as f64 / step_ns.len() as f64;
    println!(
        "step: mean {:.0} us, p99 {:.0} us, max {:.2} ms",
        mean / 1e3,
        step_ns[step_ns.len() * 99 / 100] as f64 / 1e3,
        *step_ns.last().unwrap() as f64 / 1e6
    );
    println!(
        "gave-up cars' last refusal [conflict in box, no room beyond, signal, yielding, none]: {:?}",
        traffic::debug_gave_up_reasons(&w)
    );
    let mut nodes: Vec<(NodeId, [u64; 3])> = by_node.into_iter().collect();
    nodes.sort_by_key(|x| std::cmp::Reverse(x.1.iter().sum::<u64>()));
    println!("most standing car-seconds by the junction ahead [red, held at the line, queued behind]:");
    for (n, c) in nodes.iter().take(15) {
        let pos = w.roads.nodes.get(*n).map_or([0.0; 2], |x| x.pos);
        let j = w.traffic.junction_states.get(*n);
        let arms: Vec<String> = j.map_or(Vec::new(), |j| {
            j.arms
                .iter()
                .map(|a| format!("{:?}{}{}", a.kind, if a.major { "*" } else { "" }, a.lanes_in))
                .collect()
        });
        let tps = TICKS_PER_SECOND as u64;
        println!(
            "  ({:.0},{:.0}) {:?} deg {} [{}]: {:?} s, deny {:?}",
            pos[0],
            pos[1],
            j.map(|j| j.control),
            arms.len(),
            arms.join(" "),
            [c[0] / tps, c[1] / tps, c[2] / tps],
            traffic::debug_node_denials(&w, *n)
        );
    }
    let mut at: BTreeMap<String, (u32, f32)> = BTreeMap::new();
    for &(_, s, fwd, d, _) in &traffic::debug_gave_up(&w)[gave0..] {
        let Some(seg) = w.roads.segments.get(s) else { continue };
        let n = if fwd { seg.b } else { seg.a };
        let pos = w.roads.nodes.get(n).map_or([0.0; 2], |x| x.pos);
        let arms: Vec<String> = junction::junction(&w, n).map_or(Vec::new(), |j| {
            j.arms
                .iter()
                .map(|a| format!("{:?}{}", a.kind, if a.major { "*" } else { "" }))
                .collect()
        });
        let e = at
            .entry(format!(
                "({:.0},{:.0}) {:?} [{}] from {:?}",
                pos[0],
                pos[1],
                junction::junction_control(&w, n),
                arms.join(" "),
                seg.kind
            ))
            .or_default();
        e.0 += 1;
        e.1 += d;
    }
    let mut at: Vec<_> = at.into_iter().collect();
    at.sort_by_key(|x| std::cmp::Reverse(x.1.0));
    println!("where cars gave up (the junction ahead):");
    for (k, (n, dsum)) in at.iter().take(15) {
        println!("  {k}: {n} cars, {:.0} m before the line on average", dsum / *n as f32);
    }
}

/// `node <save> <x m> <y m>`: the junction nearest a point, its arms and
/// movements with their conflicts.
fn node(args: &[String]) {
    let mut w = World::load(&std::fs::read(&args[0]).unwrap()).unwrap();
    w.traffic.junctions_version = u64::MAX;
    w.step();
    let p = [args[1].parse::<f32>().unwrap(), args[2].parse::<f32>().unwrap()];
    let (n, _) = w
        .roads
        .nodes
        .iter()
        .map(|(n, x)| (n, geom::dist(x.pos, p)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    let j = w.traffic.junction_states.get(n).unwrap();
    println!(
        "node {n:?} at {:?} control {:?} plan {:?}",
        w.roads.nodes[n].pos, j.control, j.plan
    );
    for (i, a) in j.arms.iter().enumerate() {
        println!(
            "  arm {i}: {:?} {:?} dir {:?} setback {:.2} lanes {}/{} major {}",
            a.segment, a.kind, a.dir, a.setback, a.lanes_in, a.lanes_out, a.major
        );
    }
    for (i, m) in j.movements.iter().enumerate() {
        let c: Vec<usize> = (0..j.movements.len()).filter(|&k| j.conflict(i, k)).collect();
        println!(
            "  mv {i}: {}->{} lane {}->{} {:?} len {:.2} pref {} conflicts {:?}",
            m.from_arm, m.to_arm, m.from_lane, m.to_lane, m.turn, m.length, m.preferred, c
        );
    }
}

/// `flow <street|avenue> <none|signal|priority|minor>`: saturation flow of
/// one road eastbound, with a crossing road at x = 50 (none: no crossing;
/// minor: the crossing gives way to the main road; signal: lights).
/// Cars start along x = 14..40 heading for x = 95, as many as the road
/// takes; prints arrivals per minute of vehicle time.
fn flow(args: &[String]) {
    let kind = if args[0] == "avenue" {
        RoadKind::Avenue
    } else {
        RoadKind::Street
    };
    let mut w = World::new(1);
    w.economy.money = 10_000_000;
    scenarios::road_of(&mut w, (11, 128), (100, 128), kind);
    let ctl = args.get(1).map(String::as_str).unwrap_or("none");
    if ctl != "none" {
        scenarios::road(&mut w, (50, 100), (50, 156));
        let c = match ctl {
            "signal" => JunctionSetting::Signal,
            _ => JunctionSetting::Priority,
        };
        w.apply(Command::SetJunctionControl {
            cell: Cell::new(50, 128),
            control: c,
        })
        .unwrap();
        // Cross traffic too: north-south.
    }
    let mut k = 0u32;
    let mut arrived_before = 0u32;
    for minute in 0..4 {
        for t in 0..60 * TICKS_PER_SECOND {
            if t % 4 == 0 && w.traffic.pending.len() < 40 {
                let x = 14 + (k % 27) as i32;
                k += 1;
                traffic::debug_request(&mut w, Cell::new(x, 128).center(), Cell::new(95, 128).center());
                if ctl != "none" && k.is_multiple_of(3) {
                    traffic::debug_request(&mut w, Cell::new(50, 102).center(), Cell::new(50, 154).center());
                }
            }
            w.step();
        }
        let arrived = w.traffic.trips_today;
        println!(
            "{:?} {ctl}: minute {minute}: {} trips ended, cars {}, stopped {}",
            kind,
            arrived - arrived_before,
            w.vehicles.len(),
            w.vehicles.values().filter(|v| v.stopped()).count()
        );
        arrived_before = arrived;
    }
    // One minute second by second: entries into the box, the main road's
    // aspect, cars queued westward of the junction.
    let node = junction::junction_at(&w, Cell::new(50, 128));
    let mut line = String::new();
    for sec in 0..60 {
        let before = traffic::debug_turns(&w).iter().sum::<u64>();
        for t in 0..TICKS_PER_SECOND {
            if t % 4 == 0 && w.traffic.pending.len() < 40 {
                let x = 14 + (k % 27) as i32;
                k += 1;
                traffic::debug_request(&mut w, Cell::new(x, 128).center(), Cell::new(95, 128).center());
                if ctl != "none" && k.is_multiple_of(3) {
                    traffic::debug_request(&mut w, Cell::new(50, 102).center(), Cell::new(50, 154).center());
                }
            }
            w.step();
        }
        let n = traffic::debug_turns(&w).iter().sum::<u64>() - before;
        let asp = node.and_then(|n| {
            let j = junction::junction(&w, n)?;
            let east = j.arms.iter().position(|a| a.dir[0] < -0.5)?;
            junction::signal_aspect(&w, n, j.arms[east].segment)
        });
        let c = match asp {
            Some(junction::Aspect::Green) => 'G',
            Some(junction::Aspect::Yellow) => 'Y',
            Some(junction::Aspect::Red) => 'R',
            None => '-',
        };
        line.push_str(&format!("{sec}:{c}{n} "));
    }
    println!("{line}");
}

/// `cross <street|avenue> <auto|signal|priority> [turn share]`: one crossing
/// of two equal roads (x = 50, y = 128) loaded from all four sides (a
/// share of the cars turn); total cars through per minute.
fn cross(args: &[String]) {
    let kind = if args[0] == "avenue" {
        RoadKind::Avenue
    } else {
        RoadKind::Street
    };
    let turn: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.25);
    let mut w = World::new(1);
    w.economy.money = 10_000_000;
    scenarios::road_of(&mut w, (11, 128), (90, 128), kind);
    scenarios::road_of(&mut w, (50, 90), (50, 166), kind);
    let c = match args.get(1).map(String::as_str) {
        Some("signal") => JunctionSetting::Signal,
        Some("priority") => JunctionSetting::Priority,
        _ => JunctionSetting::Auto,
    };
    w.apply(Command::SetJunctionControl {
        cell: Cell::new(50, 128),
        control: c,
    })
    .unwrap();
    let ends = [(20, 128), (85, 128), (50, 95), (50, 161)];
    let mut r = Rng::new(3);
    for minute in 0..5 {
        let before = traffic::debug_turns(&w).iter().sum::<u64>();
        for t in 0..60 * TICKS_PER_SECOND {
            if t % 2 == 0 && w.traffic.pending.len() < 60 {
                let a = r.below(4) as usize;
                let b = if r.next_f32() < turn {
                    [2, 3, 0, 1][a] ^ (r.below(2) as usize)
                } else {
                    [1, 0, 3, 2][a]
                };
                let b = if b == a { [1, 0, 3, 2][a] } else { b };
                let (fx, fy) = ends[a];
                // Start a little way in from the end, spread along the arm.
                let k = 2 + r.below(10) as i32;
                let from = match a {
                    0 => (fx + k, fy),
                    1 => (fx - k, fy),
                    2 => (fx, fy + k),
                    _ => (fx, fy - k),
                };
                traffic::debug_request(
                    &mut w,
                    Cell::new(from.0, from.1).center(),
                    Cell::new(ends[b].0, ends[b].1).center(),
                );
            }
            w.step();
        }
        let n = traffic::debug_turns(&w).iter().sum::<u64>() - before;
        let node = junction::junction_at(&w, Cell::new(50, 128)).unwrap();
        if minute == 4 && std::env::var("TRACE").is_ok() {
            let mut line = String::new();
            for sec in 0..40 {
                let b0 = traffic::debug_turns(&w).iter().sum::<u64>();
                let d0 = traffic::debug_denials(&w);
                for _ in 0..TICKS_PER_SECOND {
                    w.step();
                }
                let d1 = traffic::debug_denials(&w);
                let j = junction::junction(&w, node).unwrap();
                line.push_str(&format!(
                    "{sec}:p{}{:?} in{} deny{:?} | ",
                    j.signal.phase,
                    j.signal.stage,
                    traffic::debug_turns(&w).iter().sum::<u64>() - b0,
                    [d1[0] - d0[0], d1[1] - d0[1], d1[2] - d0[2], d1[3] - d0[3]]
                ));
                if sec % 4 == 3 {
                    line.push('\n');
                }
                if sec == 9 || sec == 22 {
                    for (id, v) in w.vehicles.iter() {
                        if let Some(jm) = v.junction.filter(|jm| jm.node == node) {
                            let m = &j.movements[jm.movement as usize];
                            line.push_str(&format!(
                                "   BOX {id:?} mv {} {}->{} lane {}->{} {:?} d {:.1}/{:.1} v {:.1} wait {}\n",
                                jm.movement,
                                m.from_arm,
                                m.to_arm,
                                m.from_lane,
                                m.to_lane,
                                m.turn,
                                jm.distance,
                                m.length,
                                v.speed,
                                traffic::trip_of(&w, id).map_or(0, |t| t.wait_ticks)
                            ));
                        } else if let Some(s) = v.segment
                            && let Some(a) = j.arm_of(s)
                        {
                            let (_, stop) = drive::drivable_range(&w, s, v.forward).unwrap();
                            let seg = &w.roads.segments[s];
                            let toward = if v.forward { seg.b } else { seg.a };
                            if toward == node && stop - v.progress < 4.0 {
                                let nxt = v.route.first().and_then(|t| j.arm_of(*t));
                                line.push_str(&format!(
                                    "   LINE {id:?} arm {a} lane {} to arm {nxt:?} d_line {:.1} v {:.1}\n",
                                    v.lane,
                                    stop - v.progress,
                                    v.speed
                                ));
                            }
                        }
                    }
                }
            }
            println!("{line}");
            let j = junction::junction(&w, node).unwrap();
            let plan: Vec<(Vec<u8>, bool)> = j.plan.as_ref().map_or(Vec::new(), |p| {
                p.phases.iter().map(|ph| (ph.arms.clone(), ph.left_only)).collect()
            });
            println!("plan {plan:?}");
        }
        println!(
            "{kind:?} {:?}: minute {minute}: {n} through the crossing (all junctions), cars {}, stopped {}, given up {}",
            junction::junction_control(&w, node),
            w.vehicles.len(),
            w.vehicles.values().filter(|v| v.stopped()).count(),
            w.traffic.stuck_removed
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build") => build(&args[1..]),
        Some("day") => day(&args[1..]),
        Some("node") => node(&args[1..]),
        Some("flow") => flow(&args[1..]),
        Some("cross") => cross(&args[1..]),
        _ => eprintln!("usage: simint build <scenario> <seed> <pop> <out> | simint day <save> [hours]"),
    }
}
