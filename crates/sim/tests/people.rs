//! The population model in a running city: trips end in sane time, nights
//! are quiet, rush hours come from people, the read API agrees with the
//! state, happiness responds to the city, saves keep citizens.

mod common;

use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;
use common::{DAY, HOUR, check_consistency};

fn grid_city(seed: u64, days: u64) -> (World, GridPlayer) {
    let mut w = World::new(seed);
    let mut p = GridPlayer::new(&Zone::ALL);
    while w.time.tick < days * DAY {
        p.act(&mut w);
        w.step();
    }
    (w, p)
}

/// No citizen stays on a trip longer than `balance::MAX_TRIP_H` (plus the
/// decision bucket's delay), whatever the traffic; at 03:00 in a healthy
/// town hardly anyone is travelling.
#[test]
fn trips_end_in_sane_time_and_nights_are_quiet() {
    let (mut w, mut p) = grid_city(11, 3);
    let bound = (balance::MAX_TRIP_H * balance::TICKS_PER_HOUR as f32) as u64 + balance::THINK_BUCKETS + 1;
    let mut longest = 0;
    let mut night = Vec::new();
    let mut rush = Vec::new();
    for _ in 0..3 * DAY {
        p.act(&mut w);
        w.step();
        let now = w.time.tick;
        if now.is_multiple_of(20) {
            for (id, c) in w.citizens.iter() {
                if let Location::Travelling(j) = c.location {
                    let age = now - j.departed_tick;
                    longest = longest.max(age);
                    assert!(age <= bound, "{id:?} travelling for {age} ticks: {j:?}");
                }
            }
        }
        let share = inspect::activity_shares(&w)[Activity::Travelling.index()];
        if w.time.tick_of_day == 3 * balance::TICKS_PER_HOUR {
            night.push(share);
        }
        if w.time.tick_of_day == 8 * balance::TICKS_PER_HOUR {
            rush.push(share);
        }
        if now.is_multiple_of(500) {
            check_consistency(&w, true);
        }
    }
    eprintln!("longest trip {longest} ticks; travelling at 03:00 {night:?}, at 08:00 {rush:?}");
    assert!(w.stats.population > 1_000);
    assert!(night.iter().all(|&s| s < 0.01), "night travel {night:?}");
    assert!(
        rush.iter().zip(&night).all(|(r, n)| r > n),
        "more travel at 08:00 than 03:00"
    );
}

/// Rush hours come from people: more commuters set off between 06:00 and
/// 09:00 than in any other three hours, and come home in the afternoon or
/// evening; shopping and leisure fill the day and the evening.
#[test]
fn rush_hours_and_daytime_errands_emerge() {
    let (w, _) = grid_city(12, 6);
    let by_hour = &w.people.trips_by_hour_yesterday;
    let sum =
        |purpose: TripPurpose, h0: usize, h1: usize| -> u32 { (h0..h1).map(|h| by_hour[h][purpose.index()]).sum() };
    let morning = sum(TripPurpose::ToWork, 6, 9);
    for h0 in (0..24).step_by(3).filter(|&h| h != 6) {
        assert!(morning > sum(TripPurpose::ToWork, h0, h0 + 3), "to work {h0}h");
    }
    assert!(sum(TripPurpose::ToHome, 14, 20) > sum(TripPurpose::ToHome, 0, 6) * 3);
    assert!(sum(TripPurpose::Shopping, 9, 21) > 10 * sum(TripPurpose::Shopping, 0, 7).max(1));
    assert!(sum(TripPurpose::Leisure, 14, 22) > sum(TripPurpose::Leisure, 0, 10));
    assert!(sum(TripPurpose::Goods, 0, 24) > 0);
}

/// The read API agrees with the state for citizens, buildings and cars.
#[test]
fn read_api_matches_the_state() {
    let (w, _) = grid_city(13, 3);
    let mut seen_driver = false;
    for (vid, _) in w.vehicles.iter() {
        let v = inspect::vehicle(&w, vid).expect("every car has a trip");
        let occ: Vec<CitizenId> = inspect::occupants(&w, vid).collect();
        assert_eq!(occ.len(), v.occupants as usize);
        if let Some(d) = v.driver {
            seen_driver = true;
            assert_eq!(occ.first(), Some(&d));
            assert_eq!(v.owner, Some(d));
            for c in occ {
                let cv = inspect::citizen(&w, c).unwrap();
                let t = cv.trip.expect("in a car means travelling");
                assert_eq!(t.vehicle, Some(vid));
                assert_eq!(t.purpose, v.purpose);
                assert_eq!(cv.household_id, w.citizens[d].household);
            }
        }
    }
    assert!(seen_driver, "some cars carry people");
    for (bid, b) in w.buildings.iter() {
        let bv = inspect::building(&w, bid).unwrap();
        match b.kind {
            Zone::Residential => {
                assert_eq!(inspect::residents(&w, bid).count() as u32, b.occupied);
                let rent: i64 = bv.households.iter().map(|&h| w.households[h].rent).sum();
                assert_eq!(bv.rent_per_day, rent);
            }
            _ => {
                assert_eq!(bv.workers.len() as u32, b.occupied);
                for &c in bv.workers {
                    assert_eq!(w.citizens[c].workplace(), Some(bid));
                }
            }
        }
    }
    for (cid, c) in w.citizens.iter().take(500) {
        let v = inspect::citizen(&w, cid).unwrap();
        assert!(v.members.contains(&cid));
        assert_eq!(v.home, w.households[c.household].home);
        assert_eq!(v.inside.is_some(), v.trip.is_none());
    }
    let shares = inspect::activity_shares(&w);
    assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    let s = &w.people.stats;
    assert_eq!(s.by_stage.iter().sum::<u32>(), s.population);
    assert_eq!(s.age_pyramid.iter().sum::<u32>(), s.population);
    assert!(s.avg_happiness > 0.0 && s.avg_happiness <= 1.0);
}

/// Newcomers to a young town are mostly of working age: seniors are a
/// small minority of households, people in their twenties and thirties the
/// biggest groups.
#[test]
fn a_new_town_is_young() {
    let (w, _) = grid_city(14, 8);
    let s = &w.people.stats;
    let hh: u32 = s.by_kind.iter().sum();
    assert!(
        s.by_kind[HouseholdKind::Seniors.index()] * 100 < hh * 18,
        "{:?}",
        s.by_kind
    );
    let over_60: u32 = s.age_pyramid[6..].iter().sum();
    assert!(over_60 * 100 < s.population * 15, "{:?}", s.age_pyramid);
    let twenties_thirties = s.age_pyramid[2] + s.age_pyramid[3];
    assert!(
        s.age_pyramid[4..].iter().all(|&n| n < twenties_thirties / 2),
        "{:?}",
        s.age_pyramid
    );
}

/// Happiness explains itself and responds: high taxes make taxes the top
/// complaint and lower the average; back to normal, it recovers.
#[test]
fn happiness_responds_to_taxes() {
    let (mut w, _) = grid_city(15, 3);
    let base = w.people.stats.avg_parts[HappinessPart::Taxes.index()];
    assert!(base > 0.95, "default tax is fine: {base}");
    w.apply(Command::SetTaxRate(balance::MAX_TAX_RATE)).unwrap();
    for _ in 0..DAY + 2 * HOUR {
        w.step();
    }
    let s = &w.people.stats;
    assert!(s.avg_parts[HappinessPart::Taxes.index()] < 0.05);
    assert!(s.avg_happiness < 0.9);
    w.apply(Command::SetTaxRate(balance::DEFAULT_TAX_RATE)).unwrap();
    for _ in 0..DAY + 2 * HOUR {
        w.step();
    }
    assert!(w.people.stats.avg_parts[HappinessPart::Taxes.index()] > 0.95);
}

/// A town without jobs: people are unhappy about having no job (the top
/// reason), the city publishes it.
#[test]
fn unhappiness_has_reasons() {
    let mut w = World::new(16);
    let mut p = GridPlayer::new(&[Zone::Residential]);
    while w.time.tick < DAY + 6 * HOUR {
        p.act(&mut w);
        w.step();
    }
    let s = &w.people.stats;
    assert!(s.unhappy > 0, "{s:?}");
    let top: Vec<_> = s.top_reasons(3).collect();
    assert!(!top.is_empty());
    assert!(top[0].1 >= top.last().unwrap().1);
}

/// Round-2 saves (format 4) load with plausible citizens, keep running and
/// save in format 5; damaged format-5 saves never panic.
#[test]
fn round2_saves_load_with_citizens() {
    let (w, _) = grid_city(17, 2);
    let old = save::v4::save_as_v4(&w);
    assert_eq!(&old[4..8], &4u32.to_le_bytes());
    let mut l = World::load(&old).expect("round-2 save loads");
    assert_eq!(l.stats.population, w.stats.population, "same residents");
    assert_eq!(l.households.len(), w.households.len());
    assert_eq!(l.citizens.len() as u32, l.stats.population);
    l.check_sim_invariants().unwrap();
    for h in l.households.values() {
        assert!(h.money > 0 && h.cars <= 2 && !h.citizens.is_empty());
        if let Some(j) = h.job {
            assert!(h.citizens.iter().any(|&c| l.citizens[c].workplace() == Some(j)));
        }
    }
    for _ in 0..HOUR * 3 {
        l.step();
    }
    l.check_sim_invariants().unwrap();
    let new = l.save();
    assert_eq!(&new[4..8], &save::FORMAT_VERSION.to_le_bytes());
    assert!(World::load(&new).unwrap() == l);
    // Damage: truncations and flipped bytes are errors, never panics.
    let mut r = Rng::new(5);
    for k in 0..200 {
        let mut b = new.clone();
        if k % 2 == 0 {
            b.truncate(8 + r.below((b.len() - 8) as u32) as usize);
        } else {
            for _ in 0..4 {
                let i = 8 + r.below((b.len() - 8) as u32) as usize;
                b[i] ^= 1 << r.below(8);
            }
        }
        if let Ok(mut x) = World::load(&b) {
            // Loaded despite the damage: must still run.
            for _ in 0..20 {
                x.step();
            }
        }
    }
}
