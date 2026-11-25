//! Step timing at ~5,000 residents during the morning rush, with each
//! slow step re-timed on clones to separate real cost from OS noise.
//!
//! cargo run -p citysim_sim --release --example bench
use std::time::Instant;

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn main() {
    let mut w = World::new(7);
    let mut p = GridPlayer::new(&Zone::ALL);
    while w.stats.population < 5000 {
        p.act(&mut w);
        w.step();
    }
    // Advance to 06:00 of the next day, then time a full day.
    while w.time.tick_of_day != 6 * balance::TICKS_PER_HOUR {
        p.act(&mut w);
        w.step();
    }
    println!("pop {} at day {}", w.stats.population, w.time.day);
    let mut times = Vec::new();
    let mut worst: Vec<(u128, World)> = Vec::new();
    let mut peak = 0;
    for _ in 0..balance::TICKS_PER_DAY {
        p.act(&mut w);
        let before = w.clone();
        let t = Instant::now();
        w.step();
        let dt = t.elapsed().as_nanos();
        peak = peak.max(w.vehicles.len());
        times.push(dt);
        if worst.len() < 5 || dt > worst.last().unwrap().0 {
            worst.push((dt, before));
            worst.sort_by_key(|w| std::cmp::Reverse(w.0));
            worst.truncate(5);
        }
    }
    times.sort();
    let n = times.len();
    let mean = times.iter().sum::<u128>() as f64 / n as f64;
    println!(
        "steps {n}, peak cars {peak}: mean {:.1} us, median {:.1} us, p99 {:.1} us, max {:.2} ms",
        mean / 1e3,
        times[n / 2] as f64 / 1e3,
        times[n * 99 / 100] as f64 / 1e3,
        times[n - 1] as f64 / 1e6
    );
    for (dt, before) in worst {
        let mut best = u128::MAX;
        for _ in 0..5 {
            let mut c = before.clone();
            let t = Instant::now();
            c.step();
            best = best.min(t.elapsed().as_nanos());
        }
        println!(
            "slow step at tick {}: {:.2} ms, re-timed best of 5: {:.2} ms ({} cars)",
            before.time.tick,
            dt as f64 / 1e6,
            best as f64 / 1e6,
            before.vehicles.len()
        );
    }
}
