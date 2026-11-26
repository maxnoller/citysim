//! Second-by-second trace of an avenue crossing with turning traffic on the
//! junction bench: signal phase, cars per lane on each approach (kerb,
//! inner, pocket), cars in the box by turn, denials at the line.
//!
//! `cargo run -p citysim_sim --release --example pocketdiag -- [left] [right] [secs]`
use citysim_sim::capacity::{Bench, run_traced};
use citysim_sim::junction::Turn;
use citysim_sim::*;

/// Front car of a lane: (turn, metres to the line, speed, last denial, let go).
type FrontInfo = Option<(Turn, f32, f32, u8, bool)>;

fn main() {
    let a: Vec<f32> = std::env::args().skip(1).filter_map(|x| x.parse().ok()).collect();
    let (left, right) = (a.first().copied().unwrap_or(0.1), a.get(1).copied().unwrap_or(0.1));
    let secs = a.get(2).copied().unwrap_or(120.0) as u32;
    let b = Bench::crossing(RoadKind::Avenue, RoadKind::Avenue, JunctionSetting::Signal).turning(left, right);
    let tps = TICKS_PER_SECOND;
    if std::env::var("CONFLICTS").is_ok() {
        let (w, node) = citysim_sim::capacity::build(&b);
        let j = junction::junction(&w, node.unwrap()).unwrap();
        for (i, m) in j.movements.iter().enumerate() {
            if m.turn != Turn::Left {
                continue;
            }
            let c: Vec<String> = j
                .movements
                .iter()
                .enumerate()
                .filter(|(k, _)| j.conflict(i, *k))
                .map(|(_, o)| {
                    format!("{}{}>{}{:?}", o.from_arm, o.from_lane, o.to_arm, o.turn)
                        .chars()
                        .take(9)
                        .collect()
                })
                .collect();
            println!("left {}{}>{} conflicts {:?}", m.from_arm, m.from_lane, m.to_arm, c);
        }
        return;
    }
    let quiet = std::env::var("QUIET").is_ok();
    // Per lane of arm 0 during its through green: [go, signal, box, room, yield, empty, other] ticks,
    // by the front's turn.
    let mut acc = [[[0u32; 7]; 4]; 3];
    let r = run_traced(&b, 7, 120, secs, &mut |w, node, t| {
        let Some(n) = node else { return };
        if t >= 120 * tps {
            let j = junction::junction(w, n).unwrap();
            let s0 = j.arms[0].segment;
            if junction::signal_aspect(w, n, s0) == Some(junction::Aspect::Green) {
                let seg = &w.roads.segments[s0];
                let mut best: [Option<(f32, citysim_sim::VehicleId)>; 3] = [None; 3];
                for (id, v) in w.vehicles.iter() {
                    if v.junction.is_some() || v.segment != Some(s0) || (if v.forward { seg.b } else { seg.a }) != n {
                        continue;
                    }
                    let l = (v.lane as usize).min(2);
                    if best[l].is_none_or(|b| v.progress > b.0) {
                        best[l] = Some((v.progress, id));
                    }
                }
                for l in 0..3 {
                    match best[l] {
                        None => acc[l][0][5] += 1,
                        Some((_, id)) => {
                            let v = &w.vehicles[id];
                            let turn = j
                                .movements
                                .iter()
                                .find(|m| {
                                    m.from_arm == 0
                                        && v.route.first().is_some_and(|&r| j.arms[m.to_arm as usize].segment == r)
                                })
                                .map_or(0, |m| match m.turn {
                                    Turn::Straight => 0,
                                    Turn::Right => 1,
                                    Turn::Left => 2,
                                    Turn::UTurn => 3,
                                });
                            if turn == 2 && l < 2 && std::env::var("MISS").is_ok() && t % 40 == 0 {
                                let (_, stop) = drive::drivable_range(w, s0, v.forward).unwrap();
                                let pk = junction::turn_pocket(w, s0, v.forward);
                                println!(
                                    "t {t} miss lane {l} id {:?} d_line {:.1} shift {:.2} speed {:.1} pocket {:?} cars in pocket {}",
                                    id,
                                    stop - v.progress,
                                    v.lane_shift,
                                    v.speed,
                                    pk.map(|p| (p.start_m, p.stop_m)),
                                    w.vehicles
                                        .values()
                                        .filter(|x| x.segment == Some(s0) && x.lane == 2 && x.junction.is_none())
                                        .count()
                                );
                            }
                            let k = if v.go || v.speed > 3.0 {
                                0
                            } else {
                                match traffic::debug_last_denial(w, id) {
                                    2 => 1,
                                    0 => 2,
                                    1 => 3,
                                    3 => 4,
                                    _ => 6,
                                }
                            };
                            acc[l][turn][k] += 1;
                        }
                    }
                }
            }
        }
        if quiet || t < 120 * tps || t % tps != 0 {
            return;
        }
        let j = junction::junction(w, n).unwrap();
        let mut lanes = vec![[0u16; 3]; j.arms.len()];
        let mut boxed = [0u16; 4];
        for v in w.vehicles.values() {
            if let Some(jm) = v.junction.filter(|jm| jm.node == n) {
                boxed[match jm.turn {
                    Turn::Straight => 0,
                    Turn::Right => 1,
                    Turn::Left => 2,
                    Turn::UTurn => 3,
                }] += 1;
                continue;
            }
            let Some(s) = v.segment else { continue };
            let seg = &w.roads.segments[s];
            if (if v.forward { seg.b } else { seg.a }) != n {
                continue;
            }
            if let Some(a) = j.arm_of(s) {
                lanes[a][(v.lane as usize).min(2)] += 1;
            }
        }
        // Front car of each lane of arm 0: (turn, d_line, speed, last denial).
        let mut front: [FrontInfo; 3] = [None; 3];
        for (id, v) in w.vehicles.iter() {
            if v.junction.is_some() || v.segment != Some(j.arms[0].segment) {
                continue;
            }
            let seg = &w.roads.segments[j.arms[0].segment];
            if (if v.forward { seg.b } else { seg.a }) != n {
                continue;
            }
            let (_, stop) = drive::drivable_range(w, j.arms[0].segment, v.forward).unwrap();
            let d = stop - v.progress;
            let l = (v.lane as usize).min(2);
            if front[l].is_none_or(|f| d < f.1) {
                let turn = j
                    .movements
                    .iter()
                    .find(|m| {
                        v.route.first().is_some_and(|&r| j.arms[m.to_arm as usize].segment == r) && m.from_arm == 0
                    })
                    .map_or(Turn::UTurn, |m| m.turn);
                let den = traffic::debug_last_denial(w, id);
                front[l] = Some((turn, d, v.speed, den, v.go));
            }
        }
        let fs: Vec<String> = front
            .iter()
            .map(|f| {
                f.map_or("-".into(), |f| {
                    format!("{:?}/{:.0}m/{:.1}/{}{}", f.0, f.1, f.2, f.3, if f.4 { "G" } else { "" })
                })
            })
            .collect();
        let plan = j.plan.as_ref().unwrap();
        let ph = &plan.phases[j.signal.phase as usize];
        println!(
            "{:4}s ph {} {:?}{} {:<7} {:3}t lanes {:?} box s/r/l/u {:?} arm0 fronts {:?}",
            t / tps,
            j.signal.phase,
            ph.arms,
            if ph.left_only { "L" } else { " " },
            format!("{:?}", j.signal.stage),
            j.signal.ticks,
            lanes,
            boxed,
            fs
        );
    });
    for (l, a) in acc.iter().enumerate() {
        println!(
            "lane {l} (go, signal, box, room, yield, empty, other) by turn S/R/L/U: {:?}",
            a
        );
    }
    println!(
        "{:.1}/min by arm {:?} lanes {:?} turns {:?} gave up {}",
        r.per_minute, r.by_arm, r.main_by_lane, r.by_turn, r.gave_up
    );
}
