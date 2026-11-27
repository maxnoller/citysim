//! Cost of the midnight step in a big city: `midnight <save> [repeats]`
//! (a save from `bench` with `CITYSIM_BENCH_CITY`). Steps to the tick
//! before midnight, then times the midnight step on clones.
use std::time::Instant;

use citysim_sim::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut w = World::load(&std::fs::read(&a[0]).unwrap()).unwrap();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(50);
    while w.time.tick_of_day + 1 != balance::TICKS_PER_DAY {
        w.step();
    }
    println!(
        "day {} pop {} cars {}",
        w.time.day,
        w.stats.population,
        w.vehicles.len()
    );
    let mut best = f64::MAX;
    let mut sum = 0.0;
    for _ in 0..n {
        let mut c = w.clone();
        let t = Instant::now();
        c.step();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        best = best.min(ms);
        sum += ms;
    }
    // An ordinary step for comparison.
    let mut ord = 0.0;
    for _ in 0..n {
        let mut c = w.clone();
        c.step();
        let t = Instant::now();
        c.step();
        ord += t.elapsed().as_secs_f64() * 1e3;
    }
    println!(
        "midnight step: best {best:.2} ms, mean {:.2} ms; the step after: mean {:.2} ms",
        sum / n as f64,
        ord / n as f64
    );
    // Steps around midnight, one by one.
    let mut c = w.clone();
    for k in 0..12 {
        let t = Instant::now();
        c.step();
        println!("  tick +{k}: {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
    }
    if std::env::var("PROFILE").is_ok() {
        for _ in 0..200 {
            let mut c = w.clone();
            for _ in 0..3 {
                c.step();
            }
        }
    }
}
