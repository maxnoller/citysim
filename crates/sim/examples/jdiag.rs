//! Junction diagnostics: the most queued junctions at a given time, where
//! cars gave up waiting and why cars were held at the stop line.
//!
//! ```text
//! cargo run -p citysim_sim --release --example jdiag -- hierarchy 10000
//! cargo run -p citysim_sim --release --example jdiag -- organic 0 12
//! ```
//! Second argument: run until this population (0: skip); third: then on to
//! this day (default: the next day). It stops at 08:00.
use std::collections::BTreeMap;

use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;

fn main() {
    let which = std::env::args().nth(1).unwrap_or("hierarchy".into());
    let pop: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let day: Option<u32> = std::env::args().nth(3).and_then(|s| s.parse().ok());
    let mut w = World::new(7);
    let mut p = match which.as_str() {
        "streets" => Some(GridPlayer::new(&Zone::ALL)),
        "single_artery" => Some(GridPlayer::single_artery()),
        "organic" => {
            scenarios::prebuilt(&mut w, |w| {
                scenarios::organic(w);
            });
            None
        }
        _ => Some(GridPlayer::hierarchy()),
    };
    let step = |w: &mut World, p: &mut Option<GridPlayer>| {
        if let Some(p) = p.as_mut() {
            p.act(w);
        }
        w.step();
    };
    while w.stats.population < pop && w.time.day < 60 {
        step(&mut w, &mut p);
    }
    let d = day.unwrap_or(w.time.day + 1);
    let target = d as u64 * balance::TICKS_PER_DAY as u64 + 8 * balance::TICKS_PER_HOUR as u64;
    while w.time.tick < target {
        step(&mut w, &mut p);
    }
    println!(
        "day {} {:.1}h pop {} cars {} jam {:.2} stuck {} dl {} avg speed {:.1} m/s",
        w.time.day,
        w.time.hour(),
        w.stats.population,
        w.vehicles.len(),
        w.stats.congestion_share,
        w.traffic.stuck_removed,
        w.traffic.deadlocks_resolved,
        w.stats.avg_speed
    );
    // Where the cars stand.
    {
        let (mut moving, mut front, mut queued, mut boxed, mut start) = (0, 0, 0, 0, 0);
        let mut v_moving = 0.0;
        let mut lanes: BTreeMap<(SegmentId, bool, u8), Vec<f32>> = BTreeMap::new();
        for v in w.vehicles.values() {
            if v.junction.is_none() {
                lanes
                    .entry((v.segment.unwrap(), v.forward, v.lane))
                    .or_default()
                    .push(v.progress);
            }
        }
        for (id, v) in w.vehicles.iter() {
            if !v.stopped() {
                moving += 1;
                v_moving += v.speed;
                continue;
            }
            if v.junction.is_some() {
                boxed += 1;
                continue;
            }
            let seg = &w.roads.segments[v.segment.unwrap()];
            let (_, stop) = drive::drivable_range(&w, v.segment.unwrap(), v.forward).unwrap();
            let ahead = lanes[&(v.segment.unwrap(), v.forward, v.lane)]
                .iter()
                .any(|&p| p > v.progress);
            let t = traffic::trip_of(&w, id);
            if t.is_some_and(|t| t.wait_ticks < 40) && v.speed < 0.01 && !ahead && stop - v.progress > 8.0 {
                start += 1;
            } else if !ahead && stop - v.progress < 8.0 {
                front += 1;
            } else {
                queued += 1;
            }
            let _ = seg;
        }
        let n = w.vehicles.len().max(1) as f32;
        println!(
            "cars: moving {moving} ({:.1} m/s on average), standing: at a stop line {front}, in a queue {queued}, in a box {boxed}, other (starting / arriving) {start}; mean of all {:.1} m/s",
            v_moving / (moving.max(1) as f32),
            w.vehicles.values().map(|v| v.speed).sum::<f32>() / n
        );
    }
    let mut js: Vec<(u32, NodeId)> = junction::junction_nodes(&w)
        .map(|n| {
            (
                junction::junction(&w, n).unwrap().queue.iter().map(|&q| q as u32).sum(),
                n,
            )
        })
        .collect();
    js.sort_by_key(|j| std::cmp::Reverse(j.0));
    println!("most queued junctions (denials: conflict in box / no room beyond / signal / yielding):");
    for &(q, n) in js.iter().take(10) {
        let j = junction::junction(&w, n).unwrap();
        let pos = w.roads.nodes[n].pos;
        let arms: Vec<String> = j
            .arms
            .iter()
            .zip(&j.queue)
            .map(|(a, q)| format!("{:?}{}{}q{}", a.kind, if a.major { "*" } else { "" }, a.lanes_in, q))
            .collect();
        println!(
            "({:.0},{:.0}) q {q} {:?} {:?} peak/min {} deny {:?} | {}",
            pos[0],
            pos[1],
            j.control,
            j.setting,
            j.peak_minute_today,
            traffic::debug_node_denials(&w, n),
            arms.join(" ")
        );
    }
    // Where cars gave up: the junction ahead of them, and how far back.
    let gave = traffic::debug_gave_up(&w);
    let mut at: BTreeMap<String, (u32, f32)> = BTreeMap::new();
    let mut near_line = 0;
    for &(_, s, fwd, d, _) in gave {
        let Some(seg) = w.roads.segments.get(s) else { continue };
        let n = if fwd { seg.b } else { seg.a };
        let pos = w.roads.nodes.get(n).map_or([0.0; 2], |x| x.pos);
        let ctl = junction::junction_control(&w, n);
        let e = at
            .entry(format!(
                "({:.0},{:.0}) {:?} deg {}",
                pos[0],
                pos[1],
                ctl,
                w.roads.nodes.get(n).map_or(0, |x| x.segments.len())
            ))
            .or_default();
        e.0 += 1;
        e.1 += d;
        if d < 8.0 {
            near_line += 1;
        }
    }
    let mut at: Vec<_> = at.into_iter().collect();
    at.sort_by_key(|x| std::cmp::Reverse(x.1.0));
    println!(
        "gave up: {} cars ({} within 8 m of the stop line); by the junction ahead:",
        gave.len(),
        near_line
    );
    for (k, (n, dsum)) in at.iter().take(12) {
        println!("  {k}: {n} cars, {:.0} m before the line on average", dsum / *n as f32);
    }
    println!("denials total {:?}", traffic::debug_denials(&w));
    println!(
        "gave-up cars' last refusal [conflict in box, no room beyond, signal, yielding, none]: {:?}",
        traffic::debug_gave_up_reasons(&w)
    );
    // The worst give-up nodes in detail.
    let mut per_node: BTreeMap<NodeId, u32> = BTreeMap::new();
    for &(_, s, fwd, _, _) in gave {
        if let Some(seg) = w.roads.segments.get(s) {
            *per_node.entry(if fwd { seg.b } else { seg.a }).or_default() += 1;
        }
    }
    let mut per_node: Vec<_> = per_node.into_iter().collect();
    per_node.sort_by_key(|x| std::cmp::Reverse(x.1));
    for &(n, k) in per_node.iter().take(3) {
        let Some(j) = w.traffic.junction_states.get(n) else {
            continue;
        };
        println!(
            "node {:?} at {:?}: {k} gave up, deny {:?}, control {:?}",
            n,
            w.roads.nodes[n].pos,
            traffic::debug_node_denials(&w, n),
            j.control
        );
        for a in &j.arms {
            println!(
                "   arm {:?} {:?} dir {:?} setback {:.2} lanes {}/{}",
                a.segment, a.kind, a.dir, a.setback, a.lanes_in, a.lanes_out
            );
        }
        for (i, m) in j.movements.iter().enumerate() {
            println!(
                "   mv {i}: {}->{} lane {}->{} {:?} len {:.2} conflicts {:b}",
                m.from_arm,
                m.to_arm,
                m.from_lane,
                m.to_lane,
                m.turn,
                m.length,
                j.conflicts.get(i).copied().unwrap_or(0)
            );
        }
        for (id, v) in w.vehicles.iter() {
            let Some(seg) = v.segment.and_then(|s| w.roads.segments.get(s)) else {
                continue;
            };
            let end = if v.forward { seg.b } else { seg.a };
            if end == n && seg.length - v.progress < 30.0 {
                println!(
                    "   car {id:?} lane {} prog {:.1}/{:.1} speed {:.1} route {:?} jm {:?}",
                    v.lane,
                    v.progress,
                    seg.length,
                    v.speed,
                    v.route.first(),
                    v.junction.map(|j| j.movement)
                );
            }
        }
    }
}
