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

/// People stay with their car from departure to arrival: every car on the
/// road (trucks aside) carries its driver, who is in it, or a household
/// moving out; nobody waits for a car that is not queued; an off-screen
/// trip ends when it is due (plus the decision bucket's delay). At 03:00
/// few are travelling (early-shift workers setting off for a long drive,
/// the last cars home from a night out, newcomers still driving in), far
/// fewer than at 08:00: measured over three towns and two nights each.
#[test]
fn people_stay_with_their_car_and_nights_are_quiet() {
    let (mut night, mut rush) = (Vec::new(), Vec::new());
    for seed in [11, 12, 13] {
        let (n, r) = nights_and_mornings(seed);
        night.extend(n);
        rush.extend(r);
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let (n, r) = (mean(&night), mean(&rush));
    eprintln!("travelling at 03:00 {night:?} (mean {n:.3}), at 08:00 {rush:?} (mean {r:.3})");
    // Most of those still on the way at 03:00 are newcomers driving in
    // through the town's one highway connection.
    assert!(n < 0.10, "night travel {night:?}");
    assert!(n * 1.6 < r, "nights quieter than mornings: {n} vs {r}");
}

/// Two days of a grid town from its first day on, checking the people /
/// car bookkeeping; the travelling share at 03:00 and 08:00 each day.
fn nights_and_mornings(seed: u64) -> (Vec<f32>, Vec<f32>) {
    let (mut w, mut p) = grid_city(seed, 1);
    let mut night = Vec::new();
    let mut rush = Vec::new();
    let (mut cars, mut with_driver) = (0u64, 0u64);
    for _ in 0..2 * DAY {
        p.act(&mut w);
        w.step();
        let now = w.time.tick;
        if now.is_multiple_of(20) {
            for (id, c) in w.citizens.iter() {
                if let Location::Travelling(j) = c.location
                    && let Some(t) = j.arrive_tick
                {
                    assert!(now <= t + balance::THINK_BUCKETS, "{id:?} past its arrival: {j:?}");
                }
            }
            for (vid, t) in w.traffic.trips.iter() {
                if t.purpose == TripPurpose::Goods {
                    continue;
                }
                cars += 1;
                match t.citizen {
                    Some(d) => {
                        let j = w.citizens[d].location.journey().expect("the driver is travelling");
                        assert_eq!(j.vehicle, Some(vid), "{d:?} drives {vid:?}");
                        with_driver += 1;
                    }
                    None => assert!(
                        t.purpose == TripPurpose::MoveOut && t.party > 0,
                        "{vid:?} on the road with nobody in it: {t:?}"
                    ),
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
    assert!(w.stats.population > 1_000);
    assert!(with_driver * 100 >= cars * 98, "{with_driver} of {cars}");
    (night, rush)
}

/// A commuter learns how long the trip to work takes and leaves early
/// enough: most workers who went to work from home arrive before their
/// shift starts. Newcomers whose move-in trip ended after they should have
/// left are not counted (they go straight on to work, late).
#[test]
fn commuters_leave_early_enough() {
    // Three towns, so one unlucky jam does not decide it.
    let (mut arrived, mut on_time) = (0u32, 0u32);
    for seed in [14, 15, 16] {
        let (a, o) = commuters_on_time(seed);
        eprintln!("seed {seed}: {o} of {a} commuters on time");
        arrived += a;
        on_time += o;
    }
    assert!(arrived > 600, "{arrived}");
    assert!(on_time * 100 >= arrived * 85, "{on_time} of {arrived} on time");
}

/// Commuters on day 2 who were at home when they planned to leave, and how
/// many of them arrived before their shift started.
fn commuters_on_time(seed: u64) -> (u32, u32) {
    let (mut w, mut p) = grid_city(seed, 1);
    let (mut arrived, mut on_time) = (0u32, 0u32);
    // At home since (tick), and the leave time planned while there.
    let mut home_since: std::collections::BTreeMap<CitizenId, (u64, Option<u64>)> = Default::default();
    // Commuters on their way (counted ones only).
    let mut was_travelling: std::collections::BTreeSet<CitizenId> = Default::default();
    for _ in 0..DAY {
        p.act(&mut w);
        w.step();
        for (id, c) in w.citizens.iter() {
            let home = w.households.get(c.household).map(|h| h.home);
            match c.location {
                Location::In(b) if Some(b) == home => {
                    let e = home_since.entry(id).or_insert((w.time.tick, None));
                    if e.1.is_none() {
                        e.1 = inspect::plan(&w, id).and_then(|p| p.next_shift).map(|s| s.0);
                    }
                }
                Location::Travelling(j) if j.purpose == TripPurpose::ToWork && c.workplace() == Some(j.to) => {
                    if let Some((since, Some(leave))) = home_since.get(&id).copied()
                        && since <= leave
                    {
                        was_travelling.insert(id);
                    }
                    home_since.remove(&id);
                }
                Location::In(b)
                    if c.workplace() == Some(b) && c.activity == Activity::Working && was_travelling.remove(&id) =>
                {
                    arrived += 1;
                    // Arrived before the shift's start (plus the daily
                    // wobble): the shift has at least its length left.
                    let job = c.job.unwrap();
                    let left = c.until_tick - w.time.tick;
                    let wobble = (balance::SHIFT_DAILY_WOBBLE_H * balance::TICKS_PER_HOUR as f32) as u64;
                    if left + wobble >= job.shift.length as u64 {
                        on_time += 1;
                    }
                    assert!(c.commute_ticks > 0, "{id:?} learnt its commute time");
                }
                _ => {
                    home_since.remove(&id);
                }
            }
        }
    }
    (arrived, on_time)
}

/// Rush hours come from people: more commuters set off between 06:00 and
/// 09:00 than in any other three hours, and come home in the afternoon or
/// evening; shopping and leisure fill the day and the evening.
#[test]
fn rush_hours_and_daytime_errands_emerge() {
    let (w, _) = grid_city(12, 2);
    let by_hour = &w.people.trips_by_hour_yesterday;
    eprintln!("{by_hour:?}");
    let sum =
        |purpose: TripPurpose, h0: usize, h1: usize| -> u32 { (h0..h1).map(|h| by_hour[h][purpose.index()]).sum() };
    // Workers leave early enough for the trip (an hour or two by car on
    // the compressed calendar), so the commute starts before dawn.
    let morning = sum(TripPurpose::ToWork, 4, 8);
    for h0 in (0..24).step_by(4).filter(|&h| h != 4) {
        assert!(morning > sum(TripPurpose::ToWork, h0, h0 + 4), "to work {h0}h");
    }
    assert!(sum(TripPurpose::ToHome, 14, 20) > sum(TripPurpose::ToHome, 0, 6) * 3);
    assert!(sum(TripPurpose::Shopping, 9, 21) > 10 * sum(TripPurpose::Shopping, 0, 7).max(1));
    assert!(sum(TripPurpose::Leisure, 14, 22) > sum(TripPurpose::Leisure, 0, 10));
    assert!(sum(TripPurpose::Goods, 0, 24) > 0);
}

/// The read API agrees with the state for citizens, buildings and cars.
#[test]
fn read_api_matches_the_state() {
    let (w, _) = grid_city(13, 1);
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
    // Who is inside each building right now: exactly the citizens there.
    let mut inside: std::collections::BTreeMap<BuildingId, Vec<CitizenId>> = Default::default();
    for (cid, c) in w.citizens.iter() {
        if let Location::In(b) = c.location {
            inside.entry(b).or_default().push(cid);
        }
    }
    for (bid, _) in w.buildings.iter() {
        let mut got: Vec<CitizenId> = inspect::inside(&w, bid).collect();
        got.sort();
        let mut want = inside.remove(&bid).unwrap_or_default();
        want.sort();
        assert_eq!(got, want, "{bid:?}");
    }
    // A plan for everyone; working means a trip home next, at the shift's
    // end; travelling means arriving where the trip goes.
    for (cid, c) in w.citizens.iter() {
        let plan = inspect::plan(&w, cid).unwrap();
        assert_eq!(plan.activity, c.activity);
        if c.activity == Activity::Working {
            let n = plan.next.unwrap();
            assert_eq!(n.purpose, Some(TripPurpose::ToHome));
            assert_eq!(n.at_tick, Some(c.until_tick.max(w.time.tick)));
        }
        if let Some(t) = plan.trip {
            assert_eq!(plan.next.unwrap().place, Some(t.to));
        }
        let reasons = inspect::happiness_reasons(&w, cid).unwrap();
        assert!(reasons.windows(2).all(|r| r[0].loss >= r[1].loss));
        assert!(reasons.iter().all(|r| !r.text.is_empty()));
        assert!(inspect::citizen_pos(&w, cid).is_some_and(|p| p[0].is_finite() && p[1].is_finite()));
    }
    // A car says who is in it, where from and to, and since when.
    for (vid, _) in w.vehicles.iter() {
        let v = inspect::vehicle(&w, vid).unwrap();
        assert!(v.started_tick >= v.requested_tick && v.started_tick <= w.time.tick);
        if v.purpose == TripPurpose::Goods {
            assert!(v.cargo > 0.0);
        } else {
            assert!(v.occupants > 0, "{vid:?} {v:?}");
        }
        if let Some(d) = v.driver
            && let Some(t) = inspect::citizen(&w, d).unwrap().trip
        {
            assert_eq!(v.destination_building, Some(t.to));
            assert_eq!(v.origin_building, t.from);
        }
    }
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
    let (w, _) = grid_city(14, 2);
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
    let (mut w, _) = grid_city(15, 2);
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
/// save in the current format; damaged current saves never panic.
#[test]
fn round2_saves_load_with_citizens() {
    let (w, _) = grid_city(17, 1);
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
