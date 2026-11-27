//! Headless balance runs: scripted cities played through `Command`s only.
//!
//! ```text
//! cargo run -p citysim_sim --release --example headless            # all
//! cargo run -p citysim_sim --release --example headless -- grid artery
//! ```
//!
//! Prints one line per in-game day and writes hourly CSVs to
//! `target/headless/<scenario>.csv` (override with `CITYSIM_HEADLESS_OUT`).
//!
//! Seed sweeps: `-- --seeds 10 hierarchy streets` runs each named scenario
//! for seeds 1..=10 (one run per core, `CITYSIM_HEADLESS_THREADS=n` for fewer;
//! one summary line each) and writes
//! `<scenario>_seed<k>_days.csv` (residents, money and the budget per day)
//! and `<scenario>_seed<k>_hours.csv` (per in-game hour: mean and peak cars
//! on the road, the share of cars with an identified driver, trucks, cars
//! given up, departures waiting). `--days N` overrides the length.
//! "min" is real minutes assuming the player runs at 2.5x on average.
//! Money columns: `income` (taxes), `upkeep` (roads), `svc` (services),
//! `net` (income - upkeep - services, before construction), `d/100c`
//! (days of net income that pay for 100 road cells).

use std::fmt::Write as _;
use std::time::Instant;

use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::{TICKS_PER_SECOND, TripPurpose, World, Zone, balance};

/// Blocks `[R, C, I]` the "upfront" player lays out on day one (~290 road
/// cells, ~60 % of the start money).
const UPFRONT_BLOCKS: [usize; 3] = [8, 3, 3];

/// Average game speed assumed when converting ticks to real minutes.
const ASSUMED_SPEED: f64 = 2.5;

struct Scenario {
    name: &'static str,
    about: &'static str,
    days: u32,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "hierarchy",
        about: "round 2: avenues as arterials, streets inside the blocks, grows to 10,000",
        days: 15,
    },
    Scenario {
        name: "rings",
        about: "the avenue city, every junction on an avenue a roundabout (the biggest that fits) as it is built",
        days: 15,
    },
    Scenario {
        name: "street_rings",
        about: "the streets-only city, every junction a roundabout (the biggest that fits) as it is built",
        days: 15,
    },
    Scenario {
        name: "minis",
        about: "the avenue city, every junction of streets a mini roundabout as it is built",
        days: 15,
    },
    Scenario {
        name: "street_minis",
        about: "the streets-only city, every junction a mini roundabout as it is built",
        days: 15,
    },
    Scenario {
        name: "streets",
        about: "round 2: the same blocks with streets only, grows to 10,000",
        days: 15,
    },
    Scenario {
        name: "rescue",
        about: "the streets-only city, its main streets upgraded to avenues at the start of day 8",
        days: 15,
    },
    Scenario {
        name: "bypass",
        about: "the streets-only city, relief roads around its worst junction (the gateway) at the start of day 8",
        days: 15,
    },
    Scenario {
        name: "single_artery",
        about: "round 2: the same blocks hanging off one spine, grows to 10,000",
        days: 15,
    },
    Scenario {
        name: "grid",
        about: "sensible player: grid blocks following demand, all zone types",
        days: 6,
    },
    Scenario {
        name: "upfront",
        about: "lays out a district of 14 blocks on day one with its own money, then follows demand",
        days: 6,
    },
    Scenario {
        name: "res_only",
        about: "player who zones only residential",
        days: 3,
    },
    Scenario {
        name: "two_grid",
        about: "static two districts, grid streets (good layout)",
        days: 3,
    },
    Scenario {
        name: "artery",
        about: "static two districts, everything off one junction (bad layout)",
        days: 3,
    },
    Scenario {
        name: "artery_fixed",
        about: "bad layout, second connections added at the start of day 2",
        days: 3,
    },
    Scenario {
        name: "short_industry",
        about: "the grid town zoned with a tenth of the industry it needs",
        days: 5,
    },
    Scenario {
        name: "short_industry_fixed",
        about: "the same town, the rest of its industrial land zoned at the start of day 3",
        days: 5,
    },
    Scenario {
        name: "organic",
        about: "free-form roads: curved avenue, turned grid, ring, cul-de-sacs (pre-built)",
        days: 7,
    },
    Scenario {
        name: "overbuilt",
        about: "a road mesh far bigger than its town (pre-built), a little zoning",
        days: 3,
    },
];

/// Money the `rings` players keep for the city before building roundabouts.
const RING_RESERVE: i64 = 20_000;

/// Day the `rescue` city's main streets become avenues.
const RESCUE_DAY: u32 = 8;

/// The `rescue` scenario's upgrade, at the start of `RESCUE_DAY`; the player
/// builds avenues from then on.
/// (Returns what roundabouts cost this tick.)
fn rescue(sc: &Scenario, w: &mut World, player: &mut Option<GridPlayer>) -> i64 {
    // Roundabouts as the city is built (once an in-game hour, after the
    // player has laid its roads).
    if w.time.tick % balance::TICKS_PER_HOUR as u64 == 1 {
        use citysim_sim::RoundaboutSize as R;
        let (avenue, street): (&[R], &[R]) = match sc.name {
            "rings" => (&[R::Large, R::Medium], &[]),
            "street_rings" => (&[], &[R::Small, R::Mini]),
            "minis" | "street_minis" => (&[], &[R::Mini]),
            _ => (&[], &[]),
        };
        if !avenue.is_empty() || !street.is_empty() {
            return scenarios::add_roundabouts_of(w, avenue, street, RING_RESERVE).1;
        }
    }
    if sc.name == "bypass" && w.time.day == RESCUE_DAY && w.time.tick_of_day == 0 {
        let (built, cost) = scenarios::add_gateway_bypass(w, citysim_sim::RoadKind::Street);
        eprintln!(
            "bypass: day {} pop {}: {built} relief roads, ${cost}",
            w.time.day, w.stats.population
        );
        return 0;
    }
    if sc.name != "rescue" || w.time.day != RESCUE_DAY || w.time.tick_of_day != 0 {
        return 0;
    }
    let (ok, refused, cost) = scenarios::upgrade_to_hierarchy(w);
    if let Some(p) = player.as_mut() {
        p.layout = scenarios::Layout::Hierarchy;
    }
    eprintln!(
        "rescue: day {} pop {}: {ok} pieces upgraded to avenues, {refused} refused, ${cost}",
        w.time.day, w.stats.population
    );
    0
}

fn minutes(tick: u64) -> f64 {
    tick as f64 / TICKS_PER_SECOND as f64 / ASSUMED_SPEED / 60.0
}

/// Per-day traffic and people measurements, taken just before midnight.
#[derive(Default, Clone, Copy)]
struct DayExtra {
    /// Roundabouts standing, money spent on them so far, usable zone cells.
    rings: u32,
    ring_spend: i64,
    usable_cells: u32,
    given_up: u32,
    trips: u32,
    happiness: f32,
    avg_commute_s: f32,
    step_mean_us: f64,
    delay_factor: f32,
    worked_share: f32,
    journeys: citysim_sim::TripTimes,
    unhappy_commute: f32,
    commute_happiness: f32,
    walked: u32,
    other: u32,
}

/// Per-hour vehicle measurements.
#[derive(Default)]
struct HourAcc {
    ticks: u64,
    cars: u64,
    peak: u32,
    occupied: u64,
    trucks: u64,
    pending: u64,
    /// Cars on the road by trip purpose (`TripPurpose::ALL` order).
    by_purpose: [u64; TripPurpose::COUNT],
    /// People walking on the map.
    walkers: u64,
}

/// One hourly row: `day,hour,population,money,cars_mean,cars_peak,occupied_share,trucks_mean,given_up,pending_mean`,
/// then the mean cars on the road by purpose (`HOUR_PURPOSES`).
fn hour_row(w: &World, a: &HourAcc, out: &mut String) {
    let n = a.ticks.max(1) as f64;
    let people_cars = a.cars.saturating_sub(a.trucks).max(1) as f64;
    let _ = writeln!(
        out,
        "{},{},{},{},{:.2},{},{:.4},{:.2},{},{:.2}{}",
        w.time.day,
        w.time.hour().floor() as u32,
        w.stats.population,
        w.economy.money,
        a.cars as f64 / n,
        a.peak,
        a.occupied as f64 / people_cars,
        a.trucks as f64 / n,
        w.traffic.stuck_removed,
        a.pending as f64 / n,
        a.by_purpose.iter().fold(String::new(), |mut s, &c| {
            let _ = write!(s, ",{:.2}", c as f64 / n);
            s
        }) + &{
            // People on foot, and the trips started in the hour that just
            // ended on foot and otherwise.
            let h = (w.time.tick_of_day / balance::TICKS_PER_HOUR + 23) as usize % 24;
            let st = &w.walking.stats;
            let (wk, ot) = if h == 23 {
                (st.walked_by_hour_yesterday[h], st.other_by_hour_yesterday[h])
            } else {
                (st.walked_by_hour[h], st.other_by_hour[h])
            };
            format!(",{:.2},{},{}", a.walkers as f64 / n, wk, ot)
        }
    );
}

/// Column names of the per-purpose car counts in the hourly CSV.
const HOUR_PURPOSES: &str = ",to_work,to_home,move_in,move_out,shopping,goods,leisure,walkers_mean,walked,other";

fn sample(w: &World, a: &mut HourAcc) {
    a.ticks += 1;
    let n = w.vehicles.len() as u32;
    a.cars += n as u64;
    a.walkers += w.walking.walkers.len() as u64;
    a.peak = a.peak.max(n);
    a.pending += w.traffic.pending.len() as u64;
    for t in w.traffic.trips.values() {
        a.by_purpose[t.purpose.index()] += 1;
        if t.purpose == TripPurpose::Goods {
            a.trucks += 1;
        } else if t.citizen.is_some_and(|c| w.citizens.contains_key(c)) {
            a.occupied += 1;
        }
    }
}

fn start(sc: &Scenario, w: &mut World) -> Option<GridPlayer> {
    match sc.name {
        "grid" | "streets" | "rescue" | "bypass" => Some(GridPlayer::new(&Zone::ALL)),
        "hierarchy" | "rings" | "minis" => Some(GridPlayer::hierarchy()),
        "street_rings" | "street_minis" => Some(GridPlayer::new(&Zone::ALL)),
        "single_artery" => Some(GridPlayer::single_artery()),
        "upfront" => {
            let mut p = GridPlayer::new(&Zone::ALL);
            p.build_ahead(w, UPFRONT_BLOCKS);
            Some(p)
        }
        "res_only" => Some(GridPlayer::new(&[Zone::Residential])),
        "two_grid" => {
            scenarios::prebuilt(w, |w| scenarios::two_districts(w, true));
            None
        }
        "artery" | "artery_fixed" => {
            scenarios::prebuilt(w, |w| scenarios::two_districts(w, false));
            None
        }
        "short_industry" | "short_industry_fixed" => {
            scenarios::prebuilt(w, scenarios::short_of_industry);
            None
        }
        "overbuilt" => {
            scenarios::prebuilt(w, scenarios::overbuilt);
            None
        }
        "organic" => {
            scenarios::prebuilt(w, |w| {
                let refused = scenarios::organic(w);
                assert!(refused.is_empty(), "organic layout refused: {refused:?}");
            });
            None
        }
        _ => unreachable!(),
    }
}

/// One scenario with one seed, quietly: CSVs and a summary line.
fn run_seed(sc: &Scenario, seed: u64, days: u32, out_dir: &str) -> String {
    let mut w = World::new(seed);
    let mut player = start(sc, &mut w);
    let mut hours =
        String::from("day,hour,population,money,cars_mean,cars_peak,occupied_share,trucks_mean,given_up,pending_mean");
    hours.push_str(HOUR_PURPOSES);
    hours.push('\n');
    let mut acc = HourAcc::default();
    let total = days as u64 * balance::TICKS_PER_DAY as u64;
    let (mut sum_ns, mut max_ns, mut n) = (0u128, 0u128, 0u64);
    let mut reached = [None::<u64>; 3];
    let mut costs: Vec<citysim_sim::RunningCosts> = Vec::new();
    let mut losses: Vec<citysim_sim::TrafficLoss> = Vec::new();
    let mut extra: Vec<DayExtra> = Vec::new();
    let (mut given_up_before, mut day_ns_before) = (0u32, 0u128);
    let mut ring_spend = 0i64;
    while w.time.tick < total {
        if let Some(p) = player.as_mut() {
            p.act(&mut w);
        }
        if sc.name == "artery_fixed" && w.time.day == 2 && w.time.tick_of_day == 0 {
            scenarios::add_connections(&mut w);
        }
        if sc.name == "short_industry_fixed" && w.time.day == 3 && w.time.tick_of_day == 0 {
            scenarios::add_industry_land(&mut w);
        }
        ring_spend += rescue(sc, &mut w, &mut player);
        let t0 = Instant::now();
        w.step();
        let dt = t0.elapsed().as_nanos();
        sum_ns += dt;
        max_ns = max_ns.max(dt);
        n += 1;
        sample(&w, &mut acc);
        if w.time.tick_of_day == 0 {
            costs.push(w.economy.last_day_costs);
            losses.push(w.economy.last_day_traffic);
        }
        if w.time.tick_of_day == balance::TICKS_PER_DAY - 1 {
            // Just before midnight: the day's trips and give-ups.
            extra.push(DayExtra {
                rings: citysim_sim::junction::junction_nodes(&w)
                    .filter(|&n| citysim_sim::roundabout::roundabout(&w, n).is_some())
                    .count() as u32,
                ring_spend,
                usable_cells: w.zoning.cells().filter(|(_, c)| c.usable()).count() as u32,
                given_up: w.traffic.stuck_removed - given_up_before,
                trips: w.traffic.trips_today,
                happiness: w.people.stats.avg_happiness,
                avg_commute_s: w.traffic.avg_commute_s,
                step_mean_us: (sum_ns - day_ns_before) as f64 / balance::TICKS_PER_DAY as f64 / 1e3,
                delay_factor: w.traffic.delay_factor,
                worked_share: w.economy.attendance.share(),
                journeys: w.people.trip_times_today,
                unhappy_commute: {
                    let r = w.people.stats.unhappy_reasons;
                    r[citysim_sim::HappinessPart::Commute.index()] as f32 / w.stats.population.max(1) as f32
                },
                commute_happiness: {
                    let (mut sum, mut n) = (0.0f64, 0u32);
                    for c in w.citizens.values() {
                        if c.job.is_some() {
                            sum += c.happiness.commute as f64;
                            n += 1;
                        }
                    }
                    (sum / n.max(1) as f64) as f32
                },
                walked: w.walking.stats.walked_today,
                other: w.walking.stats.other_today,
            });
            given_up_before = w.traffic.stuck_removed;
            day_ns_before = sum_ns;
        }
        if w.time.tick_of_day.is_multiple_of(balance::TICKS_PER_HOUR) {
            hour_row(&w, &acc, &mut hours);
            acc = HourAcc::default();
        }
        for (k, goal) in [5_000, 10_000, 20_000].into_iter().enumerate() {
            if reached[k].is_none() && w.stats.population >= goal {
                reached[k] = Some(w.time.tick);
            }
        }
    }
    let mut daily = String::from(
        "day,minutes,population,peak_vehicles,money,income,expenses,residential_tax,commercial_tax,industrial_tax,road_upkeep,services,road_cells,\
         streets,avenues,highway,signals,signal_count,services_base,services_city_size,services_density,\
         given_up,trips,happiness,avg_commute_s,step_mean_us,delay_factor,worked_share,\
         journeys,journey_mean_h,journey_over3h,journey_over4h,journey_over8h,journey_max_h,journey_max_purpose,journey_max_offscreen,commute_over4h,\
         unhappy_commute_share,commute_happiness,traffic_lost_tax,traffic_lost_per_commute_s,walked,other_trips,roundabouts,roundabout_spend,usable_cells\n",
    );
    for (i, h) in w.economy.history.iter().enumerate() {
        let b = h.budget;
        let c = costs.get(i).copied().unwrap_or_default();
        let _ = writeln!(
            daily,
            "{},{:.3},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            h.day,
            minutes(h.day as u64 * balance::TICKS_PER_DAY as u64),
            h.population,
            h.peak_vehicles,
            h.money,
            h.income,
            h.expenses,
            b.residential_tax,
            b.commercial_tax,
            b.industrial_tax,
            b.road_upkeep,
            b.services,
            h.road_cells,
            c.streets,
            c.avenues,
            c.highway,
            c.signals,
            c.signal_count,
            c.services_base,
            c.services_city_size,
            c.services_density
        );
        daily.pop();
        let e = extra.get(i).copied().unwrap_or_default();
        let tt = e.journeys;
        let th = balance::TICKS_PER_HOUR as f64;
        let commutes: u32 = tt.commutes.iter().sum();
        let _ = writeln!(
            daily,
            ",{},{},{:.4},{:.1},{:.1},{:.3},{:.3},{},{:.3},{:.4},{:.4},{:.5},{:.2},{},{},{:.4},{:.4},{:.3}",
            e.given_up,
            e.trips,
            e.happiness,
            e.avg_commute_s,
            e.step_mean_us,
            e.delay_factor,
            e.worked_share,
            tt.total(),
            tt.total_ticks as f64 / tt.total().max(1) as f64 / th,
            tt.share_over(3.0),
            tt.share_over(4.0),
            tt.share_over(8.0),
            tt.max_ticks as f64 / th,
            tt.max_purpose,
            tt.max_off_screen,
            tt.commutes[4..].iter().sum::<u32>() as f32 / commutes.max(1) as f32,
            e.unhappy_commute,
            e.commute_happiness
        );
        daily.pop();
        let l = losses.get(i).copied().unwrap_or_default();
        let _ = writeln!(
            daily,
            ",{},{:.1},{},{},{},{},{}",
            l.business_tax,
            l.per_commute_s(),
            e.walked,
            e.other,
            e.rings,
            e.ring_spend,
            e.usable_cells
        );
    }
    std::fs::write(format!("{out_dir}/{}_seed{seed}_days.csv", sc.name), daily).expect("write csv");
    std::fs::write(format!("{out_dir}/{}_seed{seed}_hours.csv", sc.name), hours).expect("write csv");
    let day_of = |t: Option<u64>| {
        t.map_or("-".to_string(), |t| {
            format!("{}", t / balance::TICKS_PER_DAY as u64 + 1)
        })
    };
    let money_max = w.economy.history.iter().map(|h| h.money).max().unwrap_or(0);
    let wait = player.as_ref().map_or(String::new(), |p| {
        format!(
            " waited {:.0} s (longest {:.0} s)",
            minutes(p.money_wait_ticks) * 60.0,
            minutes(p.longest_money_wait_ticks) * 60.0
        )
    });
    format!(
        "{:<14} seed {seed:>2}: pop {:>6} on day {}, 5k day {} 10k day {} 20k day {}, money final ${} max ${money_max}, given up {}{wait}; step mean {:.0} us max {:.1} ms",
        sc.name,
        w.stats.population,
        w.time.day,
        day_of(reached[0]),
        day_of(reached[1]),
        day_of(reached[2]),
        w.economy.money,
        w.traffic.stuck_removed,
        sum_ns as f64 / n.max(1) as f64 / 1e3,
        max_ns as f64 / 1e6
    )
}

fn run(sc: &Scenario, out_dir: &str) {
    let mut w = World::new(7);
    // The static layouts lay every road on day one; they get that
    // construction for free and then run on the normal economy.
    let mut player = start(sc, &mut w);
    if sc.name == "upfront" {
        println!(
            "upfront: built {} road cells, ${} left",
            w.stats.road_cells, w.economy.money
        );
    }
    println!("\n=== {} — {}", sc.name, sc.about);
    println!(
        "{:>3} {:>5} {:>6} {:>5} {:>5} {:>5} {:>5} {:>5} {:>6} {:>6} {:>5} {:>9} {:>7} {:>7} {:>6} {:>6} {:>6}  demand R/C/I",
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
        "upkeep",
        "svc",
        "net",
        "d/100c"
    );
    let mut csv = String::from(
        "tick,day,hour,minutes,population,households,jobs,employed,unemployment,buildings,vehicles,avg_commute_s,avg_speed,congested,money,dem_r,dem_c,dem_i,waiting,\
         v_towork,v_tohome,v_movein,v_moveout,v_shopping,v_goods,v_leisure,\
         a_sleeping,a_home,a_working,a_shopping,a_leisure,a_travelling,happiness\n",
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
        if sc.name == "artery_fixed" && w.time.day == 2 && w.time.tick_of_day == 0 {
            scenarios::add_connections(&mut w);
        }
        if sc.name == "short_industry_fixed" && w.time.day == 3 && w.time.tick_of_day == 0 {
            scenarios::add_industry_land(&mut w);
        }
        let _ = rescue(sc, &mut w, &mut player);
        let t0 = Instant::now();
        w.step();
        let dt = t0.elapsed().as_nanos();
        sum_ns += dt;
        if dt > max_ns {
            max_ns = dt;
            max_at = w.time.tick;
        }
        n += 1;
        if sc.days > 10 && w.stats.population >= 10_500 {
            // The 10,000 runs stop a little past the goal.
            break;
        }
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
            let mut vp = [0u32; TripPurpose::COUNT];
            for t in w.traffic.trips.values() {
                vp[t.purpose.index()] += 1;
            }
            let a = citysim_sim::inspect::activity_shares(&w);
            csv.pop();
            let _ = writeln!(
                csv,
                ",{},{},{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.3}",
                vp[0],
                vp[1],
                vp[2],
                vp[3],
                vp[4],
                vp[5],
                vp[6],
                a[0],
                a[1],
                a[2],
                a[3],
                a[4],
                a[5],
                w.people.stats.avg_happiness
            );
        }
        if w.time.tick_of_day == 0 {
            let h = w.economy.history.last().copied().unwrap_or_default();
            let spd = if speed_acc.1 > 0 {
                speed_acc.0 / speed_acc.1 as f64 * 3.6
            } else {
                0.0
            };
            let b = h.budget;
            let per_100 = if b.net() > 0 {
                format!("{:.2}", 100.0 * balance::ROAD_COST_PER_CELL as f64 / b.net() as f64)
            } else {
                "-".into()
            };
            print!(
                "{:>3} {:>5.1} {:>6} {:>5} {:>5} {:>5.2} {:>5} {:>5} {:>6.1} {:>6.1} {:>5.1} {:>9} {:>7} {:>7} {:>6} {:>6} {:>6}  {:+.2} {:+.2} {:+.2}",
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
                b.road_upkeep,
                b.services,
                b.net(),
                per_100,
                h.demand[0],
                h.demand[1],
                h.demand[2],
            );
            let ps = &w.people.stats;
            let ev = &w.people.events_yesterday;
            let rc = w.economy.last_day_costs;
            print!(
                "  costs streets {} avenues {} signals {} ({}) | svc base {} size {} density {} |",
                rc.streets,
                rc.avenues,
                rc.signals,
                rc.signal_count,
                rc.services_base,
                rc.services_city_size,
                rc.services_density
            );
            println!(
                "  wf {} hap {:.2} kids/ad/sen {}/{}/{} hhk {:?} $hh {:.0} burd {:.2} broke {} left {} +{} b{} d{} trips {:?} {:?}",
                w.stats.workforce,
                ps.avg_happiness,
                ps.by_stage[0] + ps.by_stage[1],
                ps.by_stage[2],
                ps.by_stage[3],
                ps.by_kind,
                ps.avg_money,
                ps.avg_rent_burden,
                ps.broke,
                ev.households_left,
                ev.households_arrived,
                ev.births,
                ev.deaths,
                w.people.trips_yesterday,
                w.economy.fiscal.state
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
        println!(
            "blocks built: {}; waited for money {:.0} s in total, longest {:.0} s (real time at {ASSUMED_SPEED}x)",
            p.blocks_built,
            minutes(p.money_wait_ticks) * 60.0,
            minutes(p.longest_money_wait_ticks) * 60.0
        );
    }
    let money_max = w.economy.history.iter().map(|h| h.money).max().unwrap_or(0);
    let money_min = w.economy.history.iter().map(|h| h.money).min().unwrap_or(0);
    println!(
        "money: lowest ${money_min}, highest ${money_max}, final ${}",
        w.economy.money
    );
    let path = format!("{out_dir}/{}.csv", sc.name);
    std::fs::write(&path, csv).expect("write csv");
    let mut days = String::from(
        "day,minutes,population,peak_vehicles,peak_congestion,avg_commute_s,money,income,expenses,\
         residential_tax,commercial_tax,industrial_tax,road_upkeep,services,road_cells\n",
    );
    for h in &w.economy.history {
        let b = h.budget;
        let _ = writeln!(
            days,
            "{},{:.3},{},{},{:.4},{:.1},{},{},{},{},{},{},{},{},{}",
            h.day,
            minutes(h.day as u64 * balance::TICKS_PER_DAY as u64),
            h.population,
            h.peak_vehicles,
            h.peak_congestion,
            h.avg_commute_s,
            h.money,
            h.income,
            h.expenses,
            b.residential_tax,
            b.commercial_tax,
            b.industrial_tax,
            b.road_upkeep,
            b.services,
            h.road_cells
        );
    }
    std::fs::write(format!("{out_dir}/{}_days.csv", sc.name), days).expect("write csv");
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = std::env::var("CITYSIM_HEADLESS_OUT").unwrap_or_else(|_| "target/headless".into());
    std::fs::create_dir_all(&out_dir).expect("output dir");
    let mut flag = |name: &str| -> Option<u32> {
        let i = args.iter().position(|a| a == name)?;
        let v = args.get(i + 1).and_then(|v| v.parse().ok());
        args.drain(i..(i + 2).min(args.len()));
        v
    };
    let seeds = flag("--seeds");
    let days = flag("--days");
    if let Some(seeds) = seeds {
        let jobs: Vec<(&Scenario, u64)> = SCENARIOS
            .iter()
            .filter(|sc| args.is_empty() || args.iter().any(|a| a == sc.name))
            .flat_map(|sc| (1..=seeds as u64).map(move |k| (sc, k)))
            .collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        // One run per core (`CITYSIM_HEADLESS_THREADS=n` caps it).
        let threads: usize = std::env::var("CITYSIM_HEADLESS_THREADS")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
            .min(jobs.len().max(1));
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&(sc, seed)) = jobs.get(i) else { break };
                        println!("{}", run_seed(sc, seed, days.unwrap_or(sc.days), &out_dir));
                    }
                });
            }
        });
        return;
    }
    for sc in SCENARIOS {
        if args.is_empty() || args.iter().any(|a| a == sc.name) {
            run(sc, &out_dir);
        }
    }
}
