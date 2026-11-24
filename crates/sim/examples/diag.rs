//! Traffic diagnostics: why are stopped cars stopped?
use std::collections::BTreeMap;

use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;

fn main() {
    let which = std::env::args().nth(1).unwrap_or("overbuilt".into());
    let days: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let hour: u64 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let mut w = World::new(7);
    let mut p = None;
    match which.as_str() {
        "overbuilt" => scenarios::overbuilt(&mut w),
        "two_grid" => scenarios::two_districts(&mut w, true),
        "artery" => scenarios::two_districts(&mut w, false),
        _ => p = Some(GridPlayer::new(&Zone::ALL)),
    }
    let target = days * balance::TICKS_PER_DAY as u64 + hour * balance::TICKS_PER_HOUR as u64;
    while w.time.tick < target {
        if let Some(p) = p.as_mut() {
            p.act(&mut w);
        }
        w.step();
    }
    println!(
        "pop {} vehicles {} avg speed {:.1} pending {} jam {:.2}",
        w.stats.population,
        w.vehicles.len(),
        w.stats.avg_speed,
        w.traffic.pending.len(),
        w.stats.congestion_share
    );
    let lane = |v: &Vehicle| (v.segment.unwrap(), v.forward);
    let mut lanes: BTreeMap<(SegmentId, bool), Vec<(f32, VehicleId)>> = BTreeMap::new();
    for (id, v) in w.vehicles.iter() {
        lanes.entry(lane(v)).or_default().push((v.progress, id));
    }
    for l in lanes.values_mut() {
        l.sort_by(|a, b| b.0.total_cmp(&a.0));
    }
    let mut reasons: BTreeMap<String, u32> = BTreeMap::new();
    for (l, vs) in &lanes {
        let seg = &w.roads.segments[l.0];
        for (k, &(p, id)) in vs.iter().enumerate() {
            let v = &w.vehicles[id];
            if v.speed > 0.5 {
                *reasons.entry("moving".into()).or_default() += 1;
                continue;
            }
            let r = if k > 0 {
                "behind leader".to_string()
            } else if v.route.is_empty() {
                format!("front, no route, d_end {:.0}", seg.length - p)
            } else {
                let end = if v.forward { seg.b } else { seg.a };
                let deg = w.roads.nodes[end].segments.len();
                let next = v.route[0];
                let ns = &w.roads.segments[next];
                let nl = (next, ns.a == end);
                let rear = lanes.get(&nl).and_then(|x| x.last()).map(|x| x.0);
                let rear_speed = lanes.get(&nl).and_then(|x| x.last()).map(|x| w.vehicles[x.1].speed);
                if seg.length - p > 1.5 {
                    format!("front, braking far from node (d {:.0})", seg.length - p)
                } else if rear.is_some_and(|r| r < balance::MIN_GAP_M) {
                    format!(
                        "front, next lane full (deg {deg}, rear moving {})",
                        rear_speed.unwrap() > 0.5
                    )
                } else {
                    format!("front, junction wait (deg {deg})")
                }
            };
            *reasons.entry(r).or_default() += 1;
        }
    }
    for (r, n) in reasons {
        println!("{n:5} {r}");
    }
    // Map: road cells by congestion, buildings by kind.
    let (x0, x1, y0, y1) = (8, 115, 80, 175);
    for y in y0..y1 {
        let mut line = String::new();
        for x in x0..x1 {
            let c = Cell::new(x, y);
            let cd = w.grid.get(c).unwrap();
            let ch = match cd.occupant {
                Occupant::Road => {
                    let cong = w
                        .roads
                        .segment_through(c.center())
                        .map(|s| w.roads.segments[s].congestion())
                        .unwrap_or(0.0);
                    if cong > 0.8 {
                        '#'
                    } else if cong > 0.4 {
                        '+'
                    } else {
                        '.'
                    }
                }
                Occupant::Building(id) => match w.buildings[id].kind {
                    Zone::Residential => 'r',
                    Zone::Commercial => 'c',
                    Zone::Industrial => 'i',
                },
                Occupant::Empty => ' ',
            };
            line.push(ch);
        }
        if !line.trim().is_empty() {
            println!("{y:3} {line}");
        }
    }
}
