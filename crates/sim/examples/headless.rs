//! Headless balance runs: scripted cities played through `Command`s only.
//!
//! ```text
//! cargo run -p citysim_sim --release --example headless            # all
//! cargo run -p citysim_sim --release --example headless -- grid artery
//! ```
//!
//! Prints one line per in-game day and writes hourly CSVs to
//! `target/headless/<scenario>.csv` (override with `CITYSIM_HEADLESS_OUT`).
//! "min" is real minutes assuming the player runs at 2.5x on average.

use std::fmt::Write as _;
use std::time::Instant;

use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::{TICKS_PER_SECOND, World, Zone, balance};

/// Average game speed assumed when converting ticks to real minutes.
const ASSUMED_SPEED: f64 = 2.5;

struct Scenario {
    name: &'static str,
    about: &'static str,
    days: u32,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "grid",
        about: "sensible player: grid blocks following demand, all zone types",
        days: 20,
    },
    Scenario {
        name: "res_only",
        about: "player who zones only residential",
        days: 10,
    },
    Scenario {
        name: "two_grid",
        about: "static two districts, grid streets (good layout)",
        days: 8,
    },
    Scenario {
        name: "artery",
        about: "static two districts, everything off one junction (bad layout)",
        days: 8,
    },
    Scenario {
        name: "artery_fixed",
        about: "bad layout, second connections added at the start of day 4",
        days: 8,
    },
    Scenario {
        name: "overbuilt",
        about: "builds far more road than it needs",
        days: 10,
    },
];

fn minutes(tick: u64) -> f64 {
    tick as f64 / TICKS_PER_SECOND as f64 / ASSUMED_SPEED / 60.0
}

fn run(sc: &Scenario, out_dir: &str) {
    let mut w = World::new(7);
    let mut player = None;
    match sc.name {
        "grid" => player = Some(GridPlayer::new(&Zone::ALL)),
        "res_only" => player = Some(GridPlayer::new(&[Zone::Residential])),
        "two_grid" => scenarios::two_districts(&mut w, true),
        "artery" | "artery_fixed" => scenarios::two_districts(&mut w, false),
        "overbuilt" => scenarios::overbuilt(&mut w),
        _ => unreachable!(),
    }
    println!("\n=== {} — {}", sc.name, sc.about);
    println!(
        "{:>3} {:>5} {:>6} {:>5} {:>5} {:>5} {:>5} {:>5} {:>6} {:>6} {:>5} {:>9} {:>7} {:>7}  demand R/C/I",
        "day",
        "min",
        "pop",
        "hh",
        "jobs",
        "unemp",
        "bldg",
        "peakV",
        "comm_s",
        "spd",
        "jam",
        "money",
        "income",
        "upkeep"
    );
    let mut csv = String::from(
        "tick,day,hour,minutes,population,households,jobs,employed,unemployment,buildings,vehicles,avg_commute_s,avg_speed,congested,money,dem_r,dem_c,dem_i,waiting\n",
    );
    let total = sc.days as u64 * balance::TICKS_PER_DAY as u64;
    let (mut sum_ns, mut max_ns, mut n, mut max_at) = (0u128, 0u128, 0u64, 0u64);
    let (mut rush_sum, mut rush_n, mut rush_max) = (0u128, 0u64, 0u128);
    let mut reached_5k: Option<u64> = None;
    let mut speed_acc = (0.0f64, 0u32);
    let mut jam_max = 0.0f32;
    for _ in 0..total {
        if let Some(p) = player.as_mut() {
            p.act(&mut w);
        }
        if sc.name == "artery_fixed" && w.time.day == 4 && w.time.tick_of_day == 0 {
            scenarios::add_connections(&mut w);
        }
        let t0 = Instant::now();
        w.step();
        let dt = t0.elapsed().as_nanos();
        sum_ns += dt;
        if dt > max_ns {
            max_ns = dt;
            max_at = w.time.tick;
        }
        n += 1;
        if w.stats.population >= 4500 && w.stats.vehicles_on_road >= 100 {
            rush_sum += dt;
            rush_n += 1;
            rush_max = rush_max.max(dt);
        }
        if reached_5k.is_none() && w.stats.population >= 5000 {
            reached_5k = Some(w.time.tick);
        }
        if w.stats.vehicles_on_road > 0 {
            speed_acc.0 += w.stats.avg_speed as f64;
            speed_acc.1 += 1;
        }
        jam_max = jam_max.max(w.stats.congestion_share * 100.0);
        if w.time.tick.is_multiple_of(balance::TICKS_PER_HOUR as u64 / 2) {
            let s = &w.stats;
            let d = &w.demand;
            let _ = writeln!(
                csv,
                "{},{},{:.2},{:.3},{},{},{},{},{:.3},{},{},{:.1},{:.2},{},{},{:.3},{:.3},{:.3},{}",
                w.time.tick,
                w.time.day,
                w.time.hour(),
                minutes(w.time.tick),
                s.population,
                s.households,
                s.jobs,
                s.employed,
                s.unemployment,
                s.buildings,
                s.vehicles_on_road,
                s.avg_commute_s,
                s.avg_speed,
                s.congested_segments,
                w.economy.money,
                d.residential,
                d.commercial,
                d.industrial,
                s.waiting_departures
            );
        }
        if w.time.tick_of_day == 0 {
            let h = w.economy.history.last().copied().unwrap_or_default();
            let spd = if speed_acc.1 > 0 {
                speed_acc.0 / speed_acc.1 as f64 * 3.6
            } else {
                0.0
            };
            println!(
                "{:>3} {:>5.1} {:>6} {:>5} {:>5} {:>5.2} {:>5} {:>5} {:>6.1} {:>6.1} {:>5.1} {:>9} {:>7} {:>7}  {:+.2} {:+.2} {:+.2}",
                h.day,
                minutes(w.time.tick),
                h.population,
                h.households,
                h.jobs,
                w.stats.unemployment,
                h.buildings,
                h.peak_vehicles,
                h.avg_commute_s,
                spd,
                jam_max,
                h.money,
                h.income,
                h.expenses,
                h.demand[0],
                h.demand[1],
                h.demand[2],
            );
            speed_acc = (0.0, 0);
            jam_max = 0.0;
        }
    }
    w.check_invariants().expect("invariants");
    println!(
        "step: mean {:.1} us, max {:.2} ms (tick {max_at}) over {} steps; at >=4500 pop & >=100 cars: mean {:.1} us, max {:.2} ms ({} steps)",
        sum_ns as f64 / n as f64 / 1e3,
        max_ns as f64 / 1e6,
        n,
        if rush_n > 0 {
            rush_sum as f64 / rush_n as f64 / 1e3
        } else {
            0.0
        },
        rush_max as f64 / 1e6,
        rush_n
    );
    match reached_5k {
        Some(t) => println!(
            "reached 5,000 residents on day {} ({:.1} real min at {ASSUMED_SPEED}x)",
            t / balance::TICKS_PER_DAY as u64 + 1,
            minutes(t)
        ),
        None => println!("did not reach 5,000 residents (final {})", w.stats.population),
    }
    if let Some(p) = &player {
        println!("blocks built: {}", p.blocks_built);
    }
    let path = format!("{out_dir}/{}.csv", sc.name);
    std::fs::write(&path, csv).expect("write csv");
    let mut days =
        String::from("day,minutes,population,peak_vehicles,peak_congestion,avg_commute_s,money,income,expenses\n");
    for h in &w.economy.history {
        let _ = writeln!(
            days,
            "{},{:.3},{},{},{:.4},{:.1},{},{},{}",
            h.day,
            minutes(h.day as u64 * balance::TICKS_PER_DAY as u64),
            h.population,
            h.peak_vehicles,
            h.peak_congestion,
            h.avg_commute_s,
            h.money,
            h.income,
            h.expenses
        );
    }
    std::fs::write(format!("{out_dir}/{}_days.csv", sc.name), days).expect("write csv");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = std::env::var("CITYSIM_HEADLESS_OUT").unwrap_or_else(|_| "target/headless".into());
    std::fs::create_dir_all(&out_dir).expect("output dir");
    for sc in SCENARIOS {
        if args.is_empty() || args.iter().any(|a| a == sc.name) {
            run(sc, &out_dir);
        }
    }
}
