//! Phase-2 simulation behaviour: demand, growth, households, traffic,
//! determinism, save/load and robustness under road edits.

use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;

const DAY: u64 = balance::TICKS_PER_DAY as u64;
const HOUR: u64 = balance::TICKS_PER_HOUR as u64;

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

/// Step until `tick`, letting the player act.
fn run_to(w: &mut World, p: &mut Option<GridPlayer>, tick: u64) {
    while w.time.tick < tick {
        if let Some(p) = p.as_mut() {
            p.act(w);
        }
        w.step();
    }
}

#[test]
fn demand_responds_in_the_right_direction() {
    let w = World::new(1);
    let (empty, _) = demand::target(&w);
    assert!(empty.residential > 0.2, "an empty city wants residents: {empty:?}");

    // Higher taxes lower every demand.
    let mut taxed = World::new(1);
    taxed.apply(Command::SetTaxRate(balance::MAX_TAX_RATE)).unwrap();
    let (t, _) = demand::target(&taxed);
    assert!(t.residential < empty.residential - 0.5);

    // Only housing: unemployed residents push residential down and
    // commercial / industrial up.
    let mut w = World::new(2);
    let mut p = Some(GridPlayer::new(&[Zone::Residential]));
    run_to(&mut w, &mut p, DAY / 2);
    assert!(w.stats.households > 0 && w.stats.employed == 0);
    let (d, attract) = demand::target(&w);
    assert!(d.residential < empty.residential - 0.3, "{d:?}");
    assert!(d.commercial > 0.5 && d.industrial > 0.5, "{d:?}");
    // The published demand follows (smoothed).
    assert!(
        w.demand.commercial > 0.3 && w.demand.residential < 0.1,
        "{:?}",
        w.demand
    );

    // Adding jobs makes housing attractive again.
    // (A block of its own: the housing player's blocks leave no free land.)
    w.economy.money += 20_000;
    assert!(scenarios::road(&mut w, (30, 128), (75, 128)));
    assert!(scenarios::road(&mut w, (60, 128), (60, 133)));
    assert!(scenarios::road(&mut w, (60, 133), (75, 133)));
    assert!(scenarios::road(&mut w, (75, 128), (75, 133)));
    scenarios::zone(&mut w, (61, 129), (74, 132), Zone::Commercial);
    run_to(&mut w, &mut p, DAY / 2 + 6 * HOUR);
    // (The pull of jobs; the demand bar itself also counts the empty
    // housing built meanwhile.)
    let (d2, attract2) = demand::target(&w);
    assert!(w.stats.jobs > 0);
    assert!(attract2 > attract, "{d:?} -> {d2:?} {:?}", w.stats);
}

#[test]
fn growth_needs_a_road_connection_to_the_highway() {
    let mut w = World::new(3);
    // An island road, not connected to the highway stub.
    scenarios::road(&mut w, (60, 60), (80, 60));
    scenarios::zone(&mut w, (60, 58), (80, 62), Zone::Residential);
    for _ in 0..6 * HOUR {
        w.step();
    }
    assert_eq!(w.buildings.len(), 0, "nothing grows without a connection");
    // Connect it.
    scenarios::road(&mut w, (11, 128), (60, 128));
    scenarios::road(&mut w, (60, 128), (60, 60));
    for _ in 0..6 * HOUR {
        w.step();
    }
    let cells: i32 = w.buildings.values().map(|b| b.size().0 * b.size().1).sum();
    assert!(cells >= 5, "grows once connected: {cells} cells");
    for (_, b) in w.buildings.iter() {
        assert_eq!(b.kind, Zone::Residential);
        assert!(b.level >= 1 && b.capacity > 0);
        // Facing points at a road within reach.
        let (dx, dy) = b.facing.offset();
        let front = (1..=2).any(|k| w.grid_view().is_road(b.footprint.rect().min.offset(dx * k, dy * k)));
        assert!(front, "{:?} faces {:?} without a road", b.footprint.rect(), b.facing);
    }
    let seeds: std::collections::BTreeSet<u32> = w.buildings.values().map(|b| b.seed).collect();
    assert!(seeds.len() > 1);
    w.check_sim_invariants().unwrap();
}

#[test]
fn growth_is_gradual() {
    let mut w = World::new(4);
    scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, true));
    let mut max_per_tick = 0;
    for _ in 0..DAY {
        let before = w.buildings.len();
        w.step();
        max_per_tick = max_per_tick.max(w.buildings.len().saturating_sub(before));
    }
    assert!(w.buildings.len() > 30, "{}", w.buildings.len());
    assert!(max_per_tick <= 3, "buildings appear a few at a time: {max_per_tick}");
}

#[test]
fn no_jobs_means_unemployment_and_people_leave() {
    // No jobs -> unemployment -> money runs out -> the household cuts
    // spending, finds no cheaper home and leaves. Pensioners can stay.
    let mut w = World::new(5);
    let mut p = Some(GridPlayer::new(&[Zone::Residential]));
    run_to(&mut w, &mut p, DAY / 2);
    let workers: Vec<HouseholdId> = w
        .households
        .iter()
        .filter(|(_, h)| {
            // Households that need a job: someone in the workforce and no
            // pension coming in.
            h.citizens.iter().any(|&c| w.citizens[c].in_workforce())
                && h.citizens.iter().all(|&c| !w.citizens[c].retired)
        })
        .map(|(id, _)| id)
        .collect();
    assert!(workers.len() > 5, "{}", workers.len());
    assert!(w.stats.unemployment > 0.99);
    let people: Vec<CitizenId> = workers.iter().flat_map(|&h| w.households[h].citizens.clone()).collect();
    // Follow the people, not the household: two jobless singles may move in
    // together (a couple), which dissolves one household into the other.
    let mut tracked: std::collections::BTreeSet<HouseholdId> = workers.iter().copied().collect();
    let mut austerity = std::collections::BTreeSet::new();
    let mut broke = std::collections::BTreeSet::new();
    let mut left = std::collections::BTreeSet::new();
    let mut gone = std::collections::BTreeSet::new();
    let mut last_members: std::collections::BTreeMap<HouseholdId, Vec<CitizenId>> = Default::default();
    // Four midnights: two broke singles who move in together pool their
    // money, and the couple's own count of broke days starts again.
    while w.time.tick < 4 * DAY {
        let t = w.time.tick + 1;
        run_to(&mut w, &mut p, t);
        for &c in &people {
            if let Some(ci) = w.citizens.get(c) {
                tracked.insert(ci.household);
            }
        }
        for &h in &tracked {
            if let Some(hh) = w.households.get(h) {
                last_members.insert(h, hh.citizens.clone());
                if hh.austerity {
                    austerity.insert(h);
                }
                // Money running out: less than two days of rent left.
                if hh.money < 2 * hh.rent.max(1) {
                    broke.insert(h);
                }
            } else if gone.insert(h)
                && last_members
                    .get(&h)
                    .is_none_or(|m| m.iter().all(|&c| !w.citizens.contains_key(c)))
            {
                // Gone with all its members (not merged into a partner's).
                left.insert(h);
            }
        }
    }
    assert!(
        people.iter().all(|c| !w.citizens.contains_key(*c)),
        "jobless households left within four midnights"
    );
    assert!(left.len() * 2 >= workers.len(), "{} of {}", left.len(), workers.len());
    for h in &left {
        assert!(austerity.contains(h), "{h:?} cut spending before it left");
        assert!(broke.contains(h), "{h:?} left when its money ran out");
    }
    w.check_sim_invariants().unwrap();
}

#[test]
fn path_exists_and_vehicles_arrive() {
    let mut w = World::new(6);
    scenarios::road(&mut w, (11, 128), (60, 128));
    scenarios::road(&mut w, (40, 128), (40, 100));
    let from = w.roads.node_at(c(11, 128).center()).unwrap();
    let to = w.roads.node_at(c(40, 100).center()).unwrap();
    let path = traffic::shortest_path(&w.roads, from, to).expect("connected");
    assert_eq!(path.len(), 2);
    let island_a = {
        scenarios::road(&mut w, (100, 30), (110, 30));
        w.roads.node_at(c(100, 30).center()).unwrap()
    };
    assert!(traffic::shortest_path(&w.roads, from, island_a).is_none());

    traffic::debug_request(&mut w, c(20, 129).center(), c(41, 105).center());
    let mut seen = 0;
    let mut positions = Vec::new();
    for _ in 0..2000 {
        w.step();
        if let Some((id, v)) = w.vehicles.iter().next() {
            seen += 1;
            positions.push(v.pos);
            assert!(v.segment.is_some());
            assert!(traffic::destination_pos(&w, id).is_some());
        } else if seen > 0 {
            break;
        }
    }
    assert!(seen > 20, "the car drove for a while: {seen}");
    assert!(w.vehicles.is_empty(), "and arrived");
    assert!(w.traffic.trips_today >= 1);
    let last = positions.last().unwrap();
    assert!(
        (last[0] - c(40, 105).center()[0]).abs() < 12.0,
        "ends near the destination: {last:?}"
    );
}

/// Mean speed of all cars (moving or standing in queues) over the ticks
/// with cars, the final average commute, the mean delay factor (commutes
/// over free-flow time) over the same stretch, and the population.
fn run_layout_delay(grid: bool, fix_on_day: Option<u32>, days: u32) -> (f32, f32, f32, u32) {
    let mut w = World::new(8);
    scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, grid));
    let (mut speed, mut n) = (0.0f64, 0u32);
    let mut commute = Vec::new();
    let mut delay = Vec::new();
    for _ in 0..days as u64 * DAY {
        if Some(w.time.day) == fix_on_day && w.time.tick_of_day == 0 {
            scenarios::add_connections(&mut w);
        }
        w.step();
        if w.time.day + 2 > days && w.stats.vehicles_on_road > 20 {
            // `avg_speed` averages the moving cars only.
            speed += (w.stats.avg_speed * (1.0 - w.traffic.stopped_share)) as f64;
            n += 1;
            commute.push(w.stats.avg_commute_s);
            delay.push(traffic::delay_factor(&w));
        }
    }
    w.check_sim_invariants().unwrap();
    let c = commute.iter().sum::<f32>() / commute.len().max(1) as f32;
    let d = delay.iter().sum::<f32>() / delay.len().max(1) as f32;
    ((speed / n.max(1) as f64) as f32, c, d, w.stats.population)
}

#[test]
fn bad_layout_jams_and_a_second_connection_fixes_it() {
    // Three independent cities: grown side by side (the test's run time).
    let [grid, bad, fix] = std::thread::scope(|s| {
        [(true, None), (false, None), (false, Some(3))]
            .map(|(grid, fix)| s.spawn(move || run_layout_delay(grid, fix, 6)))
            .map(|h| h.join().unwrap())
    });
    let (grid_speed, grid_commute, grid_delay, grid_pop) = grid;
    let (bad_speed, bad_commute, bad_delay, bad_pop) = bad;
    let (fix_speed, fix_commute, fix_delay, fix_pop) = fix;
    println!("grid {grid_speed:.1} m/s {grid_commute:.0} s x{grid_delay:.2} pop {grid_pop}");
    println!("bad  {bad_speed:.1} m/s {bad_commute:.0} s x{bad_delay:.2} pop {bad_pop}");
    println!("fix  {fix_speed:.1} m/s {fix_commute:.0} s x{fix_delay:.2} pop {fix_pop}");
    // Same zoning: the one-junction city has much longer commutes although
    // it carries fewer than half the people (every car on the road stands
    // in some queue in both, so their mean speeds are alike).
    assert!(bad_commute > 1.5 * grid_commute, "{bad_commute} vs {grid_commute}");
    assert!(bad_pop * 2 < grid_pop, "{bad_pop} vs {grid_pop}");
    // Adding connections measurably fixes it: the town grows again, to
    // about twice the people (it grows into the new capacity until
    // traffic holds it back again), and although it then carries several
    // times the cars, commutes are no longer and no further from free flow.
    // (About twice: the run is chaotic, and six-row zoning blocks give
    // the district's rectangle a few more cells than two-row ones did.)
    assert!(fix_pop * 10 > 19 * bad_pop, "{fix_pop} vs {bad_pop}");
    assert!(fix_commute < bad_commute, "{fix_commute} vs {bad_commute}");
    assert!(fix_delay <= bad_delay, "x{fix_delay} vs x{bad_delay}");
}

#[test]
fn same_seed_and_commands_give_identical_worlds() {
    let run = |seed: u64| {
        let mut w = World::new(seed);
        let mut p = Some(GridPlayer::new(&Zone::ALL));
        run_to(&mut w, &mut p, DAY + 9 * HOUR);
        w
    };
    let a = run(11);
    let b = run(11);
    assert!(a.vehicles.len() > 10, "rush hour has cars: {}", a.vehicles.len());
    assert_eq!(a, b);
    assert_eq!(a.save(), b.save());
    let c = run(12);
    assert_ne!(a.save(), c.save(), "a different seed gives a different city");
}

#[test]
fn save_load_mid_rush_hour_stays_in_lockstep() {
    let mut w = World::new(13);
    let mut p = Some(GridPlayer::new(&Zone::ALL));
    run_to(&mut w, &mut p, 2 * DAY + 2 * HOUR + HOUR / 3); // 08:20 on day 3
    assert!(w.vehicles.len() > 20, "{}", w.vehicles.len());
    assert!(w.traffic.trips.get(w.vehicles.keys().next().unwrap()).is_some());
    let bytes = w.save();
    let loaded = World::load(&bytes).unwrap();
    assert_eq!(loaded, w);
    assert_eq!(loaded.save(), bytes);
    let (mut a, mut b) = (w, loaded);
    let (mut pa, mut pb) = (p.clone(), p);
    let end = a.time.tick + 2 * HOUR;
    run_to(&mut a, &mut pa, end);
    run_to(&mut b, &mut pb, end);
    assert_eq!(a, b);
    assert_eq!(a.save(), b.save());
    a.check_sim_invariants().unwrap();
}

#[test]
fn bulldozing_under_traffic_never_panics() {
    let mut w = World::new(14);
    let mut p = Some(GridPlayer::new(&Zone::ALL));
    run_to(&mut w, &mut p, 2 * DAY + 2 * HOUR);
    assert!(w.vehicles.len() > 20);
    let occupied = w.buildings.values().filter(|b| b.occupied > 0).count();
    assert!(occupied > 10);
    w.economy.money = 1_000_000;
    let mut rng = Rng::new(99);
    for round in 0..12 {
        // Rip out a chunk of town (roads under moving cars, occupied
        // buildings), sometimes rebuild a road through it.
        let x = 18 + rng.below(50) as i32;
        let y = 110 + rng.below(30) as i32;
        let r = 1 + rng.below(6) as i32;
        w.apply(Command::Bulldoze {
            min: c(x, y),
            max: c(x + r, y + r),
        })
        .unwrap();
        w.check_sim_invariants().unwrap();
        if round % 3 == 0 {
            let _ = w.apply(Command::BuildRoad {
                from: c(x, y),
                to: c(x + 2 * r, y),
                kind: citysim_sim::RoadKind::Street,
            });
        }
        for _ in 0..150 {
            w.step();
        }
        w.check_sim_invariants().unwrap();
    }
    w.check_invariants().unwrap();
    // Everything bulldozed: the city empties without panicking.
    w.apply(Command::Bulldoze {
        min: c(12, 0),
        max: c(255, 255),
    })
    .unwrap();
    w.check_sim_invariants().unwrap();
    assert_eq!(w.buildings.len(), 0);
    assert_eq!(w.households.len(), 0);
    for _ in 0..HOUR {
        w.step();
    }
    w.check_sim_invariants().unwrap();
    assert!(
        w.vehicles
            .values()
            .all(|v| w.roads.segments.contains_key(v.segment.unwrap()))
    );
}

#[test]
fn economy_books_taxes_and_upkeep_daily() {
    let mut w = World::new(15);
    let mut p = Some(GridPlayer::new(&Zone::ALL));
    run_to(&mut w, &mut p, 2 * DAY + 1);
    let e = &w.economy;
    assert_eq!(e.history.len(), 2);
    let b = e.last_day;
    assert!(b.residential_tax > 0 && b.commercial_tax + b.industrial_tax > 0);
    let day = e.history.last().unwrap();
    // Road upkeep by road class and traffic lights, services by city size
    // and density: the categories add up to the budget's lines.
    let costs = e.last_day_costs;
    assert_eq!(b.road_upkeep, costs.road_upkeep());
    assert_eq!(b.services, costs.services());
    assert_eq!(
        costs.streets + costs.highway,
        (day.road_cells - w.stats.avenue_cells) as i64 * balance::ROAD_UPKEEP_PER_CELL_PER_DAY
    );
    assert_eq!(
        costs.signals,
        costs.signal_count as i64 * balance::SIGNAL_UPKEEP_PER_DAY
    );
    assert!(costs.services_city_size > 0 && costs.services_base > costs.services_city_size);
    assert_eq!(day.income - day.expenses, b.income() - b.expenses());
    assert_eq!(e.last_day_income, b.income());
    assert_eq!(e.last_day_expenses, b.expenses());
    // Higher taxes, more income from the same city.
    let base = economy::budget_now(&w).income();
    w.apply(Command::SetTaxRate(0.2)).unwrap();
    assert!(economy::budget_now(&w).income() > base);
}

#[test]
fn overbuilding_goes_broke_but_a_sensible_layout_stays_solvent() {
    // The whole over-eager mesh, built for free: its upkeep sinks the town.
    let mut over = World::new(16);
    scenarios::prebuilt(&mut over, scenarios::overbuilt);
    // The static districts, built for free, run on the normal economy.
    let mut good = World::new(16);
    scenarios::prebuilt(&mut good, |w| scenarios::two_districts(w, true));
    // The scripted sensible player pays for everything it builds and always
    // keeps the price of 100 road cells in hand.
    let mut player = World::new(16);
    let mut p = Some(GridPlayer::new(&Zone::ALL));
    let hundred_cells = 100 * balance::ROAD_COST_PER_CELL;
    for _ in 0..DAY {
        over.step();
        good.step();
        let next = player.time.tick + 1;
        run_to(&mut player, &mut p, next);
        assert!(player.economy.money >= hundred_cells, "{}", player.economy.money);
    }
    assert!(over.economy.money < 0, "{}", over.economy.money);
    assert!(good.economy.money > 0, "{}", good.economy.money);
    // Its roads cost more to keep than its town pays (the young player city
    // is about break-even on day 1 and in profit later).
    let (over_net, player_net) = (over.economy.last_day.net(), player.economy.last_day.net());
    assert!(
        over_net < 0 && over_net < player_net - 1000,
        "{over_net} vs {player_net}"
    );
    // Broke: building fails with the usual error.
    let err = over.apply(Command::BuildRoad {
        from: c(30, 30),
        to: c(30, 35),
        kind: citysim_sim::RoadKind::Street,
    });
    assert!(matches!(err, Err(CommandError::InsufficientFunds { .. })));
}

/// Release-mode timing check: `cargo test -p citysim_sim --release -- --ignored`.
#[test]
#[ignore]
fn step_time_at_5000_residents() {
    let mut w = World::new(7);
    let mut p = Some(GridPlayer::new(&Zone::ALL));
    while w.stats.population < 5000 {
        let t = w.time.tick + 100;
        run_to(&mut w, &mut p, t);
    }
    let start = w.time.tick;
    let mut max = 0u128;
    let t0 = std::time::Instant::now();
    while w.time.tick < start + DAY {
        if let Some(p) = p.as_mut() {
            p.act(&mut w);
        }
        let t = std::time::Instant::now();
        w.step();
        max = max.max(t.elapsed().as_nanos());
    }
    let mean = t0.elapsed().as_nanos() as f64 / DAY as f64;
    println!("mean {:.1} us, max {:.2} ms", mean / 1e3, max as f64 / 1e6);
    assert!(mean < 1e6, "mean step {mean} ns");
    assert!(max < 5_000_000 * 2, "max step {max} ns (allowing OS noise)");
}
