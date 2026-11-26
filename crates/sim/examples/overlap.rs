//! Car bodies never overlap: measure it. Every tick, every car is an
//! oriented rectangle at its exact drawing pose (`drive::vehicle_pose`:
//! lane offsets, lane-change easing, junction turn curves), 4.4 x 1.9 m
//! (trucks 6.5 x 2.3 m); overlapping pairs are counted.
//!
//! ```text
//! cargo run -p citysim_sim --release --example overlap -- organic 6
//! ```
//! Arguments: scenario (`organic`, `hierarchy`), days to grow first. It then
//! measures the next two in-game hours from 07:00.

use std::collections::BTreeMap;

use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;

fn main() {
    let which = std::env::args().nth(1).unwrap_or("organic".into());
    let days: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(6);
    let mut w = World::new(11);
    let mut p = None;
    match which.as_str() {
        "hierarchy" => p = Some(GridPlayer::hierarchy()),
        _ => scenarios::prebuilt(&mut w, |w| {
            scenarios::organic(w);
        }),
    }
    let start = days as u64 * balance::TICKS_PER_DAY as u64 + 7 * balance::TICKS_PER_HOUR as u64
        - balance::START_TICK_OF_DAY as u64;
    while w.time.tick < start {
        if let Some(p) = p.as_mut() {
            p.act(&mut w);
        }
        w.step();
    }
    let (mut pairs, mut worst, mut ticks, mut cars) = (0u64, 0usize, 0u64, 0u64);
    let mut kinds: BTreeMap<&str, u64> = BTreeMap::new();
    let mut deepest = 0.0f32;
    for _ in 0..2 * balance::TICKS_PER_HOUR {
        if let Some(p) = p.as_mut() {
            p.act(&mut w);
        }
        w.step();
        ticks += 1;
        let bodies = traffic::car_bodies(&w);
        cars += bodies.len() as u64;
        let hits = traffic::overlapping_bodies(&bodies, 0.05);
        worst = worst.max(hits.len());
        pairs += hits.len() as u64;
        for &(a, b, depth) in &hits {
            deepest = deepest.max(depth);
            let va = &w.vehicles[bodies[a].0];
            let vb = &w.vehicles[bodies[b].0];
            let k = match (va.junction.is_some(), vb.junction.is_some()) {
                (true, true) => "both in a junction box",
                (true, false) | (false, true) => "box and lane",
                _ if va.segment == vb.segment && va.forward == vb.forward && va.lane == vb.lane => "same lane",
                _ if va.segment == vb.segment && va.forward == vb.forward => "neighbouring lanes",
                _ => "different roads",
            };
            *kinds.entry(k).or_default() += 1;
            if std::env::var("OVERLAP_DEBUG").is_ok() && pairs < 40 {
                let tr = |id| w.traffic.trips.get(id).map(|t| t.purpose);
                println!(
                    "{k}: depth {depth:.2} a {:?} seg {:?} fwd {} lane {} shift {:.2} prog {:.2} v {:.1} jm {:?} {:?} | b {:?} seg {:?} fwd {} lane {} shift {:.2} prog {:.2} v {:.1} jm {:?} {:?}",
                    bodies[a].0,
                    va.segment,
                    va.forward,
                    va.lane,
                    va.lane_shift,
                    va.progress,
                    va.speed,
                    va.junction.map(|j| (j.movement, j.distance)),
                    tr(bodies[a].0),
                    bodies[b].0,
                    vb.segment,
                    vb.forward,
                    vb.lane,
                    vb.lane_shift,
                    vb.progress,
                    vb.speed,
                    vb.junction.map(|j| (j.movement, j.distance)),
                    tr(bodies[b].0)
                );
                for v in [va, vb] {
                    if let Some(jm) = v.junction {
                        let j = w.traffic.junction_states.get(jm.node).unwrap();
                        print!("    deg {} ", j.arms.len());
                        let m = &j.movements[jm.movement as usize];
                        println!(
                            "    box: node {:?} from {:?} to {:?} {:?} arm {}->{} lane {}->{} dist {:.2}/{:.2}",
                            jm.node,
                            jm.from,
                            jm.to,
                            m.turn,
                            m.from_arm,
                            m.to_arm,
                            m.from_lane,
                            m.to_lane,
                            jm.distance,
                            m.length
                        );
                    } else {
                        let s = v.segment.unwrap();
                        let seg = &w.roads.segments[s];
                        let (start, stop) = drive::drivable_range(&w, s, v.forward).unwrap();
                        println!(
                            "    lane: seg {:?} ahead node {:?} behind node {:?} start {:.2} stop {:.2} prog {:.2} len {:.2}",
                            s,
                            if v.forward { seg.b } else { seg.a },
                            if v.forward { seg.a } else { seg.b },
                            start,
                            stop,
                            v.progress,
                            seg.length
                        );
                    }
                }
            }
        }
    }
    println!(
        "{which}: {ticks} ticks, {:.0} cars on average; overlapping car pairs: {pairs} pair-ticks in total, at most {worst} at once, deepest {deepest:.2} m; {kinds:?}",
        cars as f64 / ticks as f64
    );
}
