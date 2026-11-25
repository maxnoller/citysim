//! Junction diagnostics: the most queued junctions at a given time.
use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn main() {
    let which = std::env::args().nth(1).unwrap_or("hierarchy".into());
    let pop: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let mut w = World::new(7);
    let mut p = match which.as_str() {
        "streets" => GridPlayer::new(&Zone::ALL),
        "single_artery" => GridPlayer::single_artery(),
        _ => GridPlayer::hierarchy(),
    };
    while w.stats.population < pop && w.time.day < 60 {
        p.act(&mut w);
        w.step();
    }
    let target = w.time.day as u64 * balance::TICKS_PER_DAY as u64 + 2 * balance::TICKS_PER_HOUR as u64;
    while w.time.tick < target {
        p.act(&mut w);
        w.step();
    }
    println!(
        "day {} {:.1}h pop {} cars {} jam {:.2} stuck {} dl {}",
        w.time.day,
        w.time.hour(),
        w.stats.population,
        w.vehicles.len(),
        w.stats.congestion_share,
        w.traffic.stuck_removed,
        w.traffic.deadlocks_resolved
    );
    let mut js: Vec<(u32, NodeId)> = junction::junction_nodes(&w)
        .map(|n| {
            (
                junction::junction(&w, n).unwrap().queue.iter().map(|&q| q as u32).sum(),
                n,
            )
        })
        .collect();
    js.sort_by_key(|j| std::cmp::Reverse(j.0));
    for &(q, n) in js.iter().take(12) {
        let j = junction::junction(&w, n).unwrap();
        let c = Cell::from_pos(w.roads.nodes[n].pos);
        let arms: Vec<String> = j
            .arms
            .iter()
            .zip(&j.queue)
            .map(|(a, q)| format!("{:?}{}{}q{}", a.kind, if a.major { "*" } else { "" }, a.lanes_in, q))
            .collect();
        println!(
            "({},{}) q {q} {:?} {:?} sig {:?} peak/min {} | {}",
            c.x,
            c.y,
            j.control,
            j.setting,
            j.signal.phase,
            j.peak_minute_today,
            arms.join(" ")
        );
    }
    // Where are the stopped cars: on which kind, how far from the line.
    let mut by_kind = [0u32; 3];
    for v in w.vehicles.values() {
        if v.stopped() {
            let k = w.roads.segments[v.segment.unwrap()].kind;
            by_kind[k.rank() as usize] += 1;
        }
    }
    let mut purposes = std::collections::BTreeMap::new();
    for (id, _) in w.vehicles.iter() {
        *purposes
            .entry(format!("{:?}", w.traffic.trips.get(id).map(|t| t.purpose)))
            .or_insert(0) += 1;
    }
    println!("purposes {purposes:?} pending {}", w.traffic.pending.len());
    println!(
        "stopped on street/avenue/highway {:?} turns {:?}",
        by_kind,
        traffic::debug_turns(&w)
    );
}
