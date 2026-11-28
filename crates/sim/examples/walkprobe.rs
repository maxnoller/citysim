//! Where people on foot hold up cars: grow the avenue city to a size, then
//! count car ticks held at zebras, by kind, over the next hours.
//!
//! `cargo run -p citysim_sim --release --example walkprobe [pop] [hours]`
use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn main() {
    let args: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let pop = args.first().copied().unwrap_or(10_000);
    let hours = args.get(1).copied().unwrap_or(4);
    let mut w = World::new(std::env::var("SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1));
    let mut p = if std::env::var("STREETS").is_ok() {
        GridPlayer::new(&Zone::ALL)
    } else {
        GridPlayer::hierarchy()
    };
    while w.stats.population < pop {
        p.act(&mut w);
        w.step();
    }
    while w.time.tick_of_day != 7 * balance::TICKS_PER_HOUR {
        p.act(&mut w);
        w.step();
    }
    let h0 = walk::debug_holds(&w);
    let (mut cars, mut walkers, mut waiting) = (0u64, 0u64, 0u64);
    let n = hours * balance::TICKS_PER_HOUR;
    for _ in 0..n {
        p.act(&mut w);
        w.step();
        cars += w.vehicles.len() as u64;
        walkers += w.walking.walkers.len() as u64;
        waiting += w
            .walking
            .walkers
            .values()
            .filter(|x| x.state == walk::WalkState::Waiting)
            .count() as u64;
    }
    let h1 = walk::debug_holds(&w);
    let d: Vec<u64> = (0..8).map(|i| h1[i] - h0[i]).collect();
    println!(
        "pop {} cars {:.0} walkers {:.0} waiting {:.0} | car-ticks held: signal own arm {} priority own arm {} box {} mid people {} mid full {} signal exit {} priority exit {} ({:.1} % of car ticks) | df {:.2}",
        w.stats.population,
        cars as f64 / n as f64,
        walkers as f64 / n as f64,
        waiting as f64 / n as f64,
        d[0],
        d[1],
        d[2],
        d[3],
        d[4],
        d[5],
        d[6],
        100.0 * d.iter().sum::<u64>() as f64 / cars.max(1) as f64,
        w.traffic.delay_factor
    );
}
