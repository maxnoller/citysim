//! Entries into one junction by approach, lane and turn over a few hours of
//! a saved city: `lanes <save> <x m> <y m> [hour] [hours]`.

use std::collections::BTreeMap;

use citysim_sim::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut w = World::load(&std::fs::read(&args[0]).unwrap()).unwrap();
    w.traffic.junctions_version = u64::MAX;
    let p = [args[1].parse::<f32>().unwrap(), args[2].parse::<f32>().unwrap()];
    let hour: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let hours: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
    let target = (hour * balance::TICKS_PER_HOUR as f32) as u32;
    let day = w.time.day;
    while w.time.day == day && w.time.tick_of_day < target {
        w.step();
    }
    let (n, _) = w
        .roads
        .nodes
        .iter()
        .map(|(n, x)| (n, geom::dist(x.pos, p)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    if std::env::var("LANES_USE").is_ok() {
        // Cars per lane on two-lane roads (all, standing), sampled each
        // second, and why the ones in the inner lane are there.
        let mut acc = [[0u64; 2]; 2];
        let mut turn_by_lane = [[0u64; 4]; 2];
        for _ in 0..hours * balance::TICKS_PER_HOUR / TICKS_PER_SECOND {
            for _ in 0..TICKS_PER_SECOND {
                w.step();
            }
            for v in w.vehicles.values() {
                if v.junction.is_some() {
                    continue;
                }
                let Some(seg) = v.segment.and_then(|s| w.roads.segments.get(s)) else {
                    continue;
                };
                if seg.lane_count() < 2 {
                    continue;
                }
                let l = (v.lane as usize).min(1);
                acc[l][0] += 1;
                if v.stopped() {
                    acc[l][1] += 1;
                }
                let node = if v.forward { seg.b } else { seg.a };
                let t = v.route.first().and_then(|&to| {
                    let j = junction::junction(&w, node)?;
                    let (a, b) = (j.arm_of(v.segment?)?, j.arm_of(to)?);
                    j.movements_between(a, b).next().map(|(_, m)| m.turn)
                });
                let k = match t {
                    Some(Turn::Straight) => 0,
                    Some(Turn::Right) => 1,
                    Some(Turn::Left) => 2,
                    _ => 3,
                };
                turn_by_lane[l][k] += 1;
            }
        }
        println!(
            "two-lane roads, car-seconds [all, standing]: kerb {:?} inner {:?}",
            acc[0], acc[1]
        );
        println!(
            "  next turn [straight, right, left, other] kerb {:?} inner {:?}",
            turn_by_lane[0], turn_by_lane[1]
        );
        return;
    }
    if std::env::var("LANES_TRACE").is_ok() {
        for sec in 0..30 {
            let d0 = traffic::debug_node_denials(&w, n);
            for _ in 0..TICKS_PER_SECOND {
                w.step();
            }
            let d1 = traffic::debug_node_denials(&w, n);
            let j = junction::junction(&w, n).unwrap();
            let mut line = format!(
                "{sec:2}s ph {} {:?} deny {:?} |",
                j.signal.phase,
                j.signal.stage,
                [d1[0] - d0[0], d1[1] - d0[1], d1[2] - d0[2], d1[3] - d0[3]]
            );
            for (id, v) in w.vehicles.iter() {
                if let Some(jm) = v.junction.filter(|jm| jm.node == n) {
                    line.push_str(&format!(
                        " BOX{}{:?}{:.1}",
                        j.arm_of(jm.from).unwrap_or(9),
                        jm.turn,
                        v.speed
                    ));
                    continue;
                }
                let Some(s) = v.segment else { continue };
                let Some(a) = j.arm_of(s) else { continue };
                let seg = &w.roads.segments[s];
                let toward = if v.forward { seg.b } else { seg.a };
                if toward != n {
                    continue;
                }
                let (_, stop) = drive::drivable_range(&w, s, v.forward).unwrap();
                if stop - v.progress < 3.5 {
                    let to = v.route.first().and_then(|t| j.arm_of(*t));
                    let turn = to.and_then(|b| j.movements_between(a, b).next().map(|(_, m)| m.turn));
                    let _ = id;
                    line.push_str(&format!(" a{a}l{} {:?} v{:.1} go{}", v.lane, turn, v.speed, v.go as u8));
                }
            }
            println!("{line}");
        }
        return;
    }
    let mut inside: Vec<VehicleId> = Vec::new();
    let mut count: BTreeMap<(u8, u8, String), u32> = BTreeMap::new();
    let mut green: BTreeMap<u8, u32> = BTreeMap::new();
    let ticks = hours * balance::TICKS_PER_HOUR;
    for _ in 0..ticks {
        w.step();
        let j = junction::junction(&w, n).unwrap();
        for (i, a) in j.arms.iter().enumerate() {
            if junction::signal_aspect(&w, n, a.segment) == Some(Aspect::Green) {
                *green.entry(i as u8).or_default() += 1;
            }
        }
        let mut now: Vec<VehicleId> = Vec::new();
        for (id, v) in w.vehicles.iter() {
            if let Some(jm) = v.junction.filter(|jm| jm.node == n) {
                now.push(id);
                if !inside.contains(&id) {
                    let arm = j.arm_of(jm.from).unwrap_or(9) as u8;
                    *count.entry((arm, jm.from_lane, format!("{:?}", jm.turn))).or_default() += 1;
                }
            }
        }
        inside = now;
    }
    let j = junction::junction(&w, n).unwrap();
    let mins = ticks as f32 / TICKS_PER_SECOND as f32 / 60.0;
    println!(
        "node at {:?}, {:?}, {:.0} vehicle-minutes",
        w.roads.nodes[n].pos, j.control, mins
    );
    for (i, a) in j.arms.iter().enumerate() {
        println!(
            "  arm {i} {:?} dir {:?} green {:.0}%",
            a.kind,
            a.dir.map(|x| (x * 10.0).round() / 10.0),
            100.0 * *green.get(&(i as u8)).unwrap_or(&0) as f32 / ticks as f32
        );
    }
    for ((arm, lane, turn), c) in count {
        println!("  from arm {arm} lane {lane} {turn:8} {:5.1}/min", c as f32 / mins);
    }
}
