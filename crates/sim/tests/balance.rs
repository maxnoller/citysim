//! Game-feel rules: how fast a new town comes alive, the grace period for
//! buildings cut off from the highway, and the HUD numbers (budget by
//! category, unemployment) and the money targets of `balance`.

mod common;

use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::*;
use common::{DAY, HOUR, c};

const SECOND: u64 = TICKS_PER_SECOND as u64;

fn road(w: &mut World, a: (i32, i32), b: (i32, i32)) {
    w.apply(Command::BuildRoad {
        from: c(a.0, a.1),
        to: c(b.0, b.1),
        kind: citysim_sim::RoadKind::Street,
    })
    .unwrap();
}

fn zone(w: &mut World, a: (i32, i32), b: (i32, i32), z: Zone) {
    w.apply(Command::SetZone {
        min: c(a.0, a.1),
        max: c(b.0, b.1),
        zone: Some(z),
    })
    .unwrap();
}

/// A player's first move: one street off the highway, housing on one side,
/// shops and industry on the other.
fn first_street(w: &mut World) {
    road(w, (11, 128), (31, 128));
    zone(w, (12, 126), (31, 127), Zone::Residential);
    zone(w, (12, 129), (21, 130), Zone::Commercial);
    zone(w, (22, 129), (31, 130), Zone::Industrial);
}

/// Real seconds at 1x (= ticks / 20) until each milestone, from now.
#[derive(Debug, Default)]
struct Start {
    building: Option<u64>,
    resident: Option<u64>,
    home: Option<u64>,
    employed: Option<u64>,
}

fn time_start(w: &mut World, limit_s: u64) -> Start {
    let t0 = w.time.tick;
    let mut s = Start::default();
    while w.time.tick < t0 + limit_s * SECOND {
        w.step();
        let t = Some((w.time.tick - t0) / SECOND);
        let at_home = || {
            w.households
                .values()
                .any(|h| h.state == HouseholdState::AtHome && !h.citizens.is_empty())
        };
        if s.building.is_none() && w.stats.buildings > 0 {
            s.building = t;
        }
        if s.resident.is_none() && w.stats.population > 0 {
            s.resident = t;
        }
        if s.home.is_none() && at_home() {
            s.home = t;
        }
        if s.employed.is_none() && w.stats.employed > 0 {
            s.employed = t;
        }
    }
    s
}

/// After the first street and zoning, the first building appears within a
/// minute at 1x and the first residents shortly after, whether the player
/// zones at once or after looking around for a few minutes.
#[test]
fn a_new_town_comes_alive_within_a_minute() {
    for seed in [1, 2, 3, 7] {
        for idle_s in [0, 180] {
            let mut w = World::new(seed);
            for _ in 0..idle_s * SECOND {
                w.step();
            }
            first_street(&mut w);
            let s = time_start(&mut w, 150);
            let ok = |x: Option<u64>, max: u64| x.is_some_and(|x| x <= max);
            assert!(ok(s.building, 30), "seed {seed}, idle {idle_s}: {s:?}");
            assert!(ok(s.resident, 45), "seed {seed}, idle {idle_s}: {s:?}");
            assert!(ok(s.home, 60), "seed {seed}, idle {idle_s}: {s:?}");
            assert!(ok(s.employed, 60), "seed {seed}, idle {idle_s}: {s:?}");
        }
    }
}

/// A small town on one street; returns once it has a few dozen buildings,
/// at `hour` of the day.
fn small_town(seed: u64, hour_of_day: u64) -> World {
    let mut w = World::new(seed);
    road(&mut w, (11, 128), (45, 128));
    zone(&mut w, (12, 126), (45, 127), Zone::Residential);
    zone(&mut w, (12, 129), (28, 130), Zone::Commercial);
    zone(&mut w, (29, 129), (45, 130), Zone::Industrial);
    for _ in 0..DAY {
        w.step();
    }
    let want = hour_of_day * HOUR - 1;
    while w.time.tick_of_day as u64 != want {
        w.step();
    }
    assert!(w.buildings.len() > 30, "{}", w.buildings.len());
    w
}

/// Cut the street just past the highway stub; everything east of x = 18
/// loses its connection.
fn cut(w: &mut World) -> Vec<BuildingId> {
    w.apply(Command::Bulldoze {
        min: c(14, 128),
        max: c(14, 128),
    })
    .unwrap();
    let ids: Vec<BuildingId> = w
        .buildings
        .iter()
        .filter(|(_, b)| b.footprint.rect().min.x >= 18)
        .map(|(id, _)| id)
        .collect();
    assert!(ids.len() > 20, "{}", ids.len());
    ids
}

fn alive(w: &World, ids: &[BuildingId]) -> usize {
    ids.iter().filter(|&&id| w.buildings.contains_key(id)).count()
}

/// The player cuts a road at 23:59 to rebuild it: midnight passes without
/// any building being abandoned, and once the road is back nothing is.
#[test]
fn cutting_a_road_just_before_midnight_and_rebuilding_it_abandons_nothing() {
    let mut w = small_town(41, 24);
    assert_eq!(w.time.tick_of_day as u64, DAY - 1, "23:59");
    let ids = cut(&mut w);
    let n = ids.len();
    // Past midnight and a few hours into the night: all still standing,
    // and flagged as cut off.
    for _ in 0..4 * HOUR {
        w.step();
    }
    assert_eq!(alive(&w, &ids), n, "abandoned within hours of the cut");
    let flagged = ids
        .iter()
        .filter(|&&id| w.growth.meta.get(id).is_some_and(|m| m.unreachable_since.is_some()))
        .count();
    assert_eq!(flagged, n);
    // Rebuilt: the clocks reset at the next check and nothing is abandoned
    // for lack of access afterwards.
    road(&mut w, (13, 128), (15, 128));
    for _ in 0..HOUR + 1 {
        w.step();
    }
    assert!(
        ids.iter()
            .all(|&id| w.growth.meta.get(id).unwrap().unreachable_since.is_none())
    );
    for _ in 0..2 * DAY {
        w.step();
    }
    // A building may still empty out and go the usual way; allow a couple.
    assert!(alive(&w, &ids) + 2 >= n, "{} of {n} left", alive(&w, &ids));
}

/// A building cut off for good is abandoned after one in-game day (the
/// grace period, checked hourly), not before and not much later.
#[test]
fn buildings_cut_off_for_good_are_abandoned_after_the_grace_period() {
    for hour in [24, 9] {
        let mut w = small_town(42, hour);
        let ids = cut(&mut w);
        let n = ids.len();
        let grace = balance::ABANDON_UNREACHABLE_GRACE_TICKS;
        for _ in 0..grace - HOUR {
            w.step();
        }
        assert_eq!(alive(&w, &ids), n, "cut at {hour}:00, abandoned too early");
        for _ in 0..2 * HOUR + 1 {
            w.step();
        }
        assert_eq!(
            alive(&w, &ids),
            0,
            "cut at {hour}:00, not abandoned after the grace period"
        );
        // The zoning stays; the buildings near the stub are untouched.
        assert!(!w.buildings.is_empty());
        assert_eq!(w.grid_view().get(c(30, 127)).unwrap().zone, Some(Zone::Residential));
    }
}

/// The grace clock survives save / load.
#[test]
fn grace_clock_is_saved() {
    let mut w = small_town(43, 12);
    let ids = cut(&mut w);
    for _ in 0..3 * HOUR {
        w.step();
    }
    let since = w.growth.meta.get(ids[0]).unwrap().unreachable_since;
    assert!(since.is_some());
    let mut loaded = World::load(&w.save()).unwrap();
    assert_eq!(loaded.growth.meta.get(ids[0]).unwrap().unreachable_since, since);
    assert_eq!(loaded, w);
    for _ in 0..DAY {
        w.step();
        loaded.step();
    }
    assert_eq!(loaded, w);
    assert_eq!(alive(&w, &ids), 0);
}

/// The budget by category, for yesterday and for every past day, adds up
/// and survives save / load; unemployment has a stated workforce.
#[test]
fn budget_by_category_and_unemployment_for_the_hud() {
    let mut w = World::new(44);
    let mut p = GridPlayer::new(&Zone::ALL);
    while w.time.tick < 3 * DAY + 2 * HOUR {
        p.act(&mut w);
        w.step();
    }
    let e = &w.economy;
    assert_eq!(e.history.len(), 3);
    let b = e.last_day;
    assert!(
        b.residential_tax > 0 && b.commercial_tax > 0 && b.industrial_tax > 0,
        "{b:?}"
    );
    assert!(b.road_upkeep > 0 && b.services > 0, "{b:?}");
    assert_eq!(e.last_day_income, b.income());
    assert_eq!(e.last_day_expenses, b.expenses());
    assert_eq!(b.net(), b.income() - b.road_upkeep - b.services);
    assert_eq!(e.history.last().unwrap().budget, b);
    let mut money = balance::STARTING_MONEY;
    for d in &e.history {
        assert_eq!(d.income, d.budget.income());
        assert_eq!(d.expenses, d.budget.expenses());
        assert_eq!(
            d.budget.road_upkeep,
            d.road_cells as i64 * balance::ROAD_UPKEEP_PER_CELL_PER_DAY
        );
        let services = (d.population as f64 * balance::SERVICES_COST_PER_RESIDENT_PER_DAY as f64).round() as i64;
        assert_eq!(d.budget.services, services);
        money += d.budget.net();
        assert!(d.money < money, "construction comes on top of the budget");
    }
    let s = &w.stats;
    // The workforce: adults (and teens with a part-time job) who want work.
    let wf = w.citizens.values().filter(|c| c.in_workforce()).count();
    assert_eq!(s.workforce as usize, wf);
    assert!(s.workforce > s.households, "two-earner households");
    assert_eq!(s.employed + s.unemployed, s.workforce);
    assert_eq!(s.vacant_jobs, s.jobs - s.employed);
    assert!((s.unemployment - s.unemployed as f32 / s.workforce as f32).abs() < 1e-6);
    let loaded = World::load(&w.save()).unwrap();
    assert_eq!(loaded.economy, w.economy);
    assert_eq!(loaded.stats, w.stats);
    assert_eq!(loaded, w);
}

// ------------------------------------------------- money targets ----
// See the comment above the money section of `balance`.

fn real_s(ticks: u64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND as f64 / 2.5
}

/// Plays `p` for 20 days; returns the most money held before reaching
/// 5,000 residents, the lowest money at any tick and the tick it reached
/// 5,000 residents.
fn play(w: &mut World, p: &mut GridPlayer) -> (i64, i64, u64) {
    let (mut most, mut lowest, mut reached) = (i64::MIN, i64::MAX, None);
    while w.time.tick < 20 * DAY {
        p.act(w);
        w.step();
        lowest = lowest.min(w.economy.money);
        if reached.is_none() {
            most = most.max(w.economy.money);
            if w.stats.population >= 5000 {
                reached = Some(w.time.tick);
            }
        }
    }
    (most, lowest, reached.expect("reaches 5,000 residents"))
}

/// The block-by-block player reaches 5,000 residents in about 30 minutes,
/// never waits for money and never runs low, but cannot bank much either.
#[test]
fn money_stays_a_constraint_for_a_player_who_keeps_expanding() {
    let hundred_cells = 100 * balance::ROAD_COST_PER_CELL;
    let mut w = World::new(7);
    let mut p = GridPlayer::new(&Zone::ALL);
    let (most, lowest, reached) = play(&mut w, &mut p);
    let minutes = real_s(reached) / 60.0;
    assert!(
        (24.0..=34.0).contains(&minutes),
        "5,000 residents after {minutes:.1} min"
    );
    assert!(
        (20_000..=40_000).contains(&most),
        "most cash before 5,000 residents: ${most}"
    );
    assert!(lowest >= hundred_cells, "lowest cash ${lowest}");
    let waited = real_s(p.longest_money_wait_ticks);
    assert!(waited <= 60.0, "waited {waited:.0} s for money");
    let at_5k = w.economy.history.iter().find(|d| d.population >= 5000).unwrap().budget;
    let days_per_100 = hundred_cells as f64 / at_5k.net() as f64;
    assert!(
        (0.7..=2.0).contains(&days_per_100),
        "100 cells = {days_per_100:.2} days of net"
    );
    assert!(w.economy.history.iter().skip(1).all(|d| d.budget.net() > 0));
}

/// A player who spends most of the start money on a district on day one
/// and then follows demand is just as viable.
#[test]
fn building_a_district_up_front_is_viable() {
    let mut w = World::new(7);
    let mut p = GridPlayer::new(&Zone::ALL);
    assert_eq!(p.build_ahead(&mut w, [8, 3, 3]), 14);
    assert!(w.economy.money < balance::STARTING_MONEY / 2, "${}", w.economy.money);
    let (most, lowest, reached) = play(&mut w, &mut p);
    assert!(real_s(reached) / 60.0 <= 34.0);
    assert!(lowest > 0, "lowest cash ${lowest}");
    assert!(most <= 40_000, "${most}");
    assert!(real_s(p.longest_money_wait_ticks) <= 60.0);
}

/// Every road of a growing two-district town laid on day one: a deficit on
/// the first day or two while it is empty, then a clear, growing surplus;
/// money never goes below zero. The same for the one-junction layout fixed
/// on day 4 (which also pays for the fix).
#[test]
fn a_sensible_layout_built_all_at_once_stays_solvent() {
    for (grid, fix) in [(true, false), (false, true)] {
        let mut w = World::new(7);
        scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, grid));
        let mut lowest = i64::MAX;
        for _ in 0..10 * DAY {
            if fix && w.time.day == 4 && w.time.tick_of_day == 0 {
                let before = w.economy.money;
                scenarios::add_connections(&mut w);
                assert!(w.economy.money < before, "the fix was built and paid");
            }
            w.step();
            lowest = lowest.min(w.economy.money);
        }
        let h = &w.economy.history;
        assert!(lowest > 0, "grid {grid}: lowest ${lowest}");
        assert!(
            h.iter().skip(2).all(|d| d.budget.net() > 0),
            "grid {grid}: deficit after day 2"
        );
        // (The one-junction layout fixed on day 4 recovers more slowly.)
        let factor = if grid { 3.0 } else { 1.8 };
        assert!(
            (h[2].budget.net() as f64) < h[9].budget.net() as f64 / factor,
            "grid {grid}: surplus grows {} -> {}",
            h[2].budget.net(),
            h[9].budget.net()
        );
        assert!(h[9].money > h[5].money + 3_000, "grid {grid}: money trends up");
    }
}

/// A road mesh far bigger than its town loses money every day.
#[test]
fn overbuilding_goes_broke() {
    let mut over = World::new(7);
    scenarios::prebuilt(&mut over, scenarios::overbuilt);
    assert!(over.stats.road_cells > 5_000);
    for _ in 0..6 * DAY {
        over.step();
    }
    assert!(over.economy.history.iter().all(|d| d.budget.net() < 0));
    assert!(over.economy.money < 0, "${}", over.economy.money);
}
