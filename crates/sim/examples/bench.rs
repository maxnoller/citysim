//! Step timing over a full day in a big city (default 20,000 citizens, the
//! hierarchy player's blocks all laid out and grown with demand held high),
//! with extra trips through the rush if fewer than 2,500 cars are on the
//! road at the peak (every car trip is drawn now, up to the vehicle budget,
//! so the city's own traffic usually fills it). Each slow step is re-timed on clones (best of 5) to separate
//! real cost from OS noise (other processes on the machine).
//!
//! cargo run -p citysim_sim --release --example bench [population] [cars]
use std::time::Instant;

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn main() {
    let args: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let pop = args.first().copied().unwrap_or(20_000);
    let cars = args.get(1).copied().unwrap_or(2_500) as usize;
    // `CITYSIM_BENCH_CITY=file`: reuse a grown city (written on the first
    // run).
    let cache = std::env::var("CITYSIM_BENCH_CITY").ok();
    let mut w = match cache.as_ref().and_then(|f| std::fs::read(f).ok()) {
        Some(bytes) => World::load(&bytes).expect("bench city loads"),
        None => World::new(7),
    };
    let mut p = GridPlayer::hierarchy();
    // Lay out every block at once, then grow the city with demand held
    // high (a benchmark shortcut: written to public fields, not played).
    if w.time.tick == 0 {
        scenarios::prebuilt(&mut w, |w| {
            p.build_ahead(w, [100, 100, 100]);
        });
    }
    let t0 = Instant::now();
    while w.stats.population < pop && w.time.day < 120 {
        w.demand = Demand {
            residential: 1.0,
            commercial: 1.0,
            industrial: 1.0,
        };
        w.growth.attractiveness = 1.0;
        w.economy.money = w.economy.money.max(100_000);
        w.step();
    }
    println!(
        "grew to {} citizens in {} days ({:.0} s)",
        w.stats.population,
        w.time.day,
        t0.elapsed().as_secs_f64()
    );
    // Advance to 06:00 (demand no longer forced: the city stays near its
    // size), then time a full day.
    while w.time.tick_of_day != 6 * balance::TICKS_PER_HOUR {
        w.step();
    }
    if let Some(f) = &cache {
        let _ = std::fs::write(f, w.save());
    }
    println!(
        "pop {} ({} households, {} jobs filled) at day {}",
        w.stats.population, w.stats.households, w.stats.employed, w.time.day
    );
    let mut times = Vec::new();
    let mut worst: Vec<(u128, World)> = Vec::new();
    let mut peak = 0;
    let homes: Vec<[f32; 2]> = w
        .buildings
        .values()
        .map(|b| derived::rect_center(b.footprint))
        .collect();
    let mut r = Rng::new(9);
    for _ in 0..balance::TICKS_PER_DAY {
        // Top up the rush (06:30-10:00 and 16:00-19:30) towards `cars`.
        let h = w.time.hour();
        if ((6.5..10.0).contains(&h) || (16.0..19.5).contains(&h)) && w.vehicles.len() + w.traffic.pending.len() < cars
        {
            for _ in 0..2 {
                let a = homes[r.below(homes.len() as u32) as usize];
                let b = homes[r.below(homes.len() as u32) as usize];
                traffic::debug_request(&mut w, a, b);
            }
        }
        let keep = w.time.tick_of_day + 1 == balance::TICKS_PER_DAY || w.time.tick.is_multiple_of(37);
        let before = keep.then(|| w.clone());
        let t = Instant::now();
        w.step();
        let dt = t.elapsed().as_nanos();
        peak = peak.max(w.vehicles.len());
        if w.time.tick_of_day.is_multiple_of(2 * balance::TICKS_PER_HOUR) {
            println!(
                "  {:02}:00 cars {} waiting to start {}, on foot {} ({} at kerbs)",
                w.time.tick_of_day / balance::TICKS_PER_HOUR,
                w.vehicles.len(),
                w.traffic.pending.len(),
                w.walking.walkers.len(),
                w.walking
                    .walkers
                    .values()
                    .filter(|x| x.state == walk::WalkState::Waiting)
                    .count()
            );
        }
        times.push(dt);
        if let Some(before) = before
            && (worst.len() < 5 || dt > worst.last().unwrap().0)
        {
            worst.push((dt, before));
            worst.sort_by_key(|w| std::cmp::Reverse(w.0));
            worst.truncate(5);
        }
    }
    // Frames at top speed: 16 steps each, in order.
    let frames: Vec<u128> = times.chunks(16).map(|c| c.iter().sum()).collect();
    let fmean = frames.iter().sum::<u128>() as f64 / frames.len() as f64;
    let mut fs = frames.clone();
    fs.sort();
    println!(
        "16 steps (one frame at top speed): mean {:.2} ms, p99 {:.2} ms, max {:.2} ms",
        fmean / 1e6,
        fs[fs.len() * 99 / 100] as f64 / 1e6,
        fs[fs.len() - 1] as f64 / 1e6
    );
    if let Ok(f) = std::env::var("CITYSIM_BENCH_TIMES") {
        let txt: String = times.iter().map(|t| format!("{}\n", t / 1000)).collect();
        std::fs::write(f, txt).expect("write step times");
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
