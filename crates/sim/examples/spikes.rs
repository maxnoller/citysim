//! Find slow steps in the hierarchy city and time road edits.
use std::time::Instant;

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn main() {
    let days: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut w = World::new(7);
    let mut p = GridPlayer::hierarchy();
    let mut slow = Vec::new();
    let mut edits = Vec::new();
    while w.time.day < days && w.stats.population < 10_000 {
        let v = w.versions.roads;
        let t = Instant::now();
        p.act(&mut w);
        if w.versions.roads != v {
            edits.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let t = Instant::now();
        w.step();
        let dt = t.elapsed().as_secs_f64() * 1e3;
        if dt > 5.0 {
            slow.push((w.time.tick, dt, w.versions.roads != v));
        }
    }
    println!(
        "pop {} roads {} blocks {}",
        w.stats.population,
        w.roads.segments.len(),
        w.zoning.blocks.len()
    );
    println!("slow steps (> 5 ms): {}", slow.len());
    for s in slow.iter().take(10) {
        println!("  tick {} {:.1} ms (after a road edit: {})", s.0, s.1, s.2);
    }
    edits.sort_by(f64::total_cmp);
    println!(
        "player road-building calls: {} (each builds a block of 4 roads + zoning), median {:.1} ms, max {:.1} ms",
        edits.len(),
        edits.get(edits.len() / 2).copied().unwrap_or(0.0),
        edits.last().copied().unwrap_or(0.0)
    );
    // One small edit in the grown city: a short street and its undo.
    let mut times = Vec::new();
    let mut blocks_ms = Vec::new();
    for k in 0..10 {
        let a = Cell::new(30 + k * 10, 60);
        let b = a.offset(0, 2);
        let t = Instant::now();
        let r = w.apply(Command::BuildRoad {
            from: a,
            to: b,
            kind: RoadKind::Street,
        });
        times.push((t.elapsed().as_secs_f64() * 1e3, r.is_ok()));
        blocks_ms.push(traffic::debug_last_block_sync_ns(&w) as f64 / 1e6);
        let t = Instant::now();
        w.step();
        times.push((t.elapsed().as_secs_f64() * 1e3, true));
    }
    println!("single edits (apply, then the next step): {times:.2?}");
    println!("zone-block regeneration per edit, ms: {blocks_ms:.2?}");
}
