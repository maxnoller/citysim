//! Follow cars through a saved city: `follow <save> <hour> <cars>`. Prints
//! each car's path segment by segment with time spent and standing time.

use citysim_sim::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut w = World::load(&std::fs::read(&args[0]).unwrap()).unwrap();
    w.traffic.junctions_version = u64::MAX;
    let hour: f32 = args[1].parse().unwrap();
    let n: usize = args[2].parse().unwrap();
    let target = (hour * balance::TICKS_PER_HOUR as f32) as u32;
    let day = w.time.day;
    while w.time.day == day && w.time.tick_of_day < target {
        w.step();
    }
    // Cars that start now.
    let before: Vec<VehicleId> = w.vehicles.keys().collect();
    let mut picked: Vec<VehicleId> = Vec::new();
    while picked.len() < n {
        w.step();
        for (id, v) in w.vehicles.iter() {
            if picked.len() < n && !before.contains(&id) && !picked.contains(&id) && v.progress >= 0.0 {
                let commute = traffic::trip_of(&w, id).is_some_and(|t| t.purpose == TripPurpose::ToWork);
                if commute {
                    picked.push(id);
                }
            }
        }
    }
    picked.retain(|&id| w.vehicles.contains_key(id));
    struct Log {
        segs: Vec<(String, u32, u32)>,
        start: u64,
        from: [f32; 2],
        to: [f32; 2],
        dist: f32,
    }
    let mut logs: Vec<Log> = picked
        .iter()
        .map(|&id| {
            let v = &w.vehicles[id];
            Log {
                segs: Vec::new(),
                start: w.time.tick,
                from: v.pos,
                to: traffic::destination_pos(&w, id).unwrap_or([0.0; 2]),
                dist: 0.0,
            }
        })
        .collect();
    let mut done = vec![false; picked.len()];
    let mut sums = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut last_pos: Vec<[f32; 2]> = picked.iter().map(|&id| w.vehicles[id].pos).collect();
    for _ in 0..20_000 {
        w.step();
        for (i, &id) in picked.iter().enumerate() {
            if done[i] {
                continue;
            }
            let Some(v) = w.vehicles.get(id) else {
                done[i] = true;
                let secs = (w.time.tick - logs[i].start) as f32 / TICKS_PER_SECOND as f32;
                let straight = geom::dist(logs[i].from, logs[i].to);
                let standing: u32 = logs[i].segs.iter().map(|x| x.2).sum();
                sums.0 += 1.0;
                sums.1 += logs[i].dist / straight.max(1.0);
                sums.2 += secs;
                sums.3 += standing as f32 / 20.0;
                sums.4 += secs / (straight * 1.3 / 10.0 + 6.0);
                if std::env::var("FOLLOW_QUIET").is_ok() {
                    continue;
                }
                println!(
                    "car {i}: {:.0} s, {:.0} m driven, straight line {:.0} m, estimate {:.0} s",
                    secs,
                    logs[i].dist,
                    geom::dist(logs[i].from, logs[i].to),
                    geom::dist(logs[i].from, logs[i].to) * 1.3 / 10.0 + 6.0
                );
                for (name, t, st) in &logs[i].segs {
                    println!(
                        "    {name:60} {:5.1} s  standing {:5.1} s",
                        *t as f32 / 20.0,
                        *st as f32 / 20.0
                    );
                }
                continue;
            };
            logs[i].dist += geom::dist(v.pos, last_pos[i]);
            last_pos[i] = v.pos;
            let name = match v.junction {
                Some(jm) => format!("  box {:?} {:?}", jm.turn, w.roads.nodes.get(jm.node).map(|n| n.pos)),
                None => {
                    let s = v.segment.unwrap();
                    let seg = &w.roads.segments[s];
                    let node = if v.forward { seg.b } else { seg.a };
                    let ctl = junction::junction_control(&w, node);
                    format!(
                        "{:?} {:.0} m to {:?} {:?} lane {}",
                        seg.kind,
                        seg.length,
                        w.roads.nodes[node].pos.map(|x| x.round()),
                        ctl,
                        v.lane
                    )
                }
            };
            let stopped = v.stopped() as u32;
            match logs[i].segs.last_mut() {
                Some(l) if l.0 == name => {
                    l.1 += 1;
                    l.2 += stopped;
                }
                _ => logs[i].segs.push((name, 1, stopped)),
            }
        }
        if done.iter().all(|&d| d) {
            break;
        }
    }
    let n = sums.0.max(1.0);
    println!(
        "{} cars: driven/straight {:.2}, trip {:.0} s, standing {:.0} s, trip/estimate {:.2}",
        sums.0,
        sums.1 / n,
        sums.2 / n,
        sums.3 / n,
        sums.4 / n
    );
}
