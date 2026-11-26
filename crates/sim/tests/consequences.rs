//! What a city's roads and zoning do to its people and its money: a jammed
//! town is unhappy about its commutes (and says so), people give up on
//! their commute and move or leave, traffic costs business tax in the
//! budget, nobody's trip takes most of a day, and a town short of work
//! land shows what is missing and why people are jobless. Three towns per
//! case (seeds), compared both ways.

mod common;

use citysim_sim::demand::{JoblessReason, jobs_view};
use citysim_sim::*;
use common::{DAY, par_map};

/// The two-district town on a street grid (`good`) or with everything
/// through one junction, run for `days`.
fn district_town(seed: u64, good: bool, days: u64) -> World {
    let mut w = World::new(seed);
    scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, good));
    while w.time.tick < days * DAY {
        w.step();
    }
    w
}

/// Workers' mean commute happiness and the share of them whose commute is
/// their first complaint "stuck in traffic" or "hopeless".
fn commute_mood(w: &World) -> (f32, f32) {
    let (mut sum, mut n, mut stuck) = (0.0f32, 0u32, 0u32);
    for (id, c) in w.citizens.iter() {
        if c.job.is_none() || c.last_commute_s <= 0.0 {
            continue;
        }
        n += 1;
        sum += c.happiness.commute;
        let r = inspect::happiness_reasons(w, id).unwrap();
        if r.iter()
            .any(|r| !r.good && matches!(r.text, "Stuck in traffic every day" | "Hopeless commute"))
        {
            stuck += 1;
        }
    }
    (sum / n.max(1) as f32, stuck as f32 / n.max(1) as f32)
}

#[test]
fn a_jammed_town_is_unhappy_about_its_commutes_and_says_so() {
    let (mut good, mut bad) = (Vec::new(), Vec::new());
    let towns = [1, 2, 3].into_iter().flat_map(|seed| [(seed, true), (seed, false)]);
    for (layout, run) in par_map(towns, |(seed, layout)| {
        let w = district_town(seed, layout, 3);
        let (commute, stuck) = commute_mood(&w);
        let gave_up = w.people.commute_moves_yesterday.left
            + w.people.commute_moves_yesterday.moved_nearer
            + w.people.commute_moves_today.left
            + w.people.commute_moves_today.moved_nearer;
        let run = (
            commute,
            stuck,
            w.people.stats.avg_happiness,
            traffic::delay_factor(&w),
            gave_up,
        );
        (layout, run)
    }) {
        if layout { good.push(run) } else { bad.push(run) }
    }
    let mean = |v: &[(f32, f32, f32, f32, u32)], f: fn(&(f32, f32, f32, f32, u32)) -> f32| {
        v.iter().map(f).sum::<f32>() / v.len() as f32
    };
    eprintln!("grid (commute, stuck share, happiness, delay, gave up): {good:?}");
    eprintln!("one junction: {bad:?}");
    let (gc, bc) = (mean(&good, |x| x.0), mean(&bad, |x| x.0));
    let (gs, bs) = (mean(&good, |x| x.1), mean(&bad, |x| x.1));
    let (gh, bh) = (mean(&good, |x| x.2), mean(&bad, |x| x.2));
    // The jammed town is far worse off on its commutes, visibly so overall,
    // and its workers name the traffic.
    assert!(mean(&bad, |x| x.3) > mean(&good, |x| x.3));
    assert!(bc < gc - 0.2, "commute happiness {bc} vs {gc}");
    assert!(bh < gh - 0.03, "happiness {bh} vs {gh}");
    assert!(bs > 0.25 && bs > 2.0 * gs, "stuck in traffic {bs} vs {gs}");
    // Some give up on it (move nearer work or leave).
    assert!(bad.iter().map(|x| x.4).sum::<u32>() > good.iter().map(|x| x.4).sum::<u32>());
}

#[test]
fn traffic_costs_business_tax_and_the_budget_shows_it() {
    let (mut good, mut bad) = (Vec::new(), Vec::new());
    let towns = [4, 5, 6].into_iter().flat_map(|seed| [(seed, true), (seed, false)]);
    for (layout, run) in par_map(towns, |(seed, layout)| {
        let w = district_town(seed, layout, 2);
        let t = w.economy.last_day_traffic;
        let b = w.economy.last_day;
        // Business tax is booked net of the loss, never below zero.
        assert!(t.business_tax >= 0 && b.commercial_tax >= 0 && b.industrial_tax >= 0);
        let business = (b.commercial_tax + b.industrial_tax + t.business_tax) as f32;
        (layout, (t.per_commute_s(), t.business_tax as f32 / business.max(1.0)))
    }) {
        if layout { good.push(run) } else { bad.push(run) }
    }
    eprintln!("lost s per commute, share of business tax lost: grid {good:?}, one junction {bad:?}");
    let lost = |v: &[(f32, f32)]| v.iter().map(|x| x.1).sum::<f32>() / v.len() as f32;
    let secs = |v: &[(f32, f32)]| v.iter().map(|x| x.0).sum::<f32>() / v.len() as f32;
    // A jam costs a visible part of the business tax; a working grid much
    // less.
    assert!(secs(&bad) > 2.0 * secs(&good), "{} vs {}", secs(&bad), secs(&good));
    assert!(lost(&bad) > 0.1, "{}", lost(&bad));
    assert!(lost(&good) < lost(&bad));
    assert!(lost(&bad) <= balance::TRAFFIC_LOSS_MAX_SHARE + 1e-3);
}

#[test]
fn nobody_spends_most_of_a_day_on_a_trip() {
    // A sensible player's growing town: no commute over eight in-game
    // hours, few over four, hardly any journey that long at all.
    let towns = par_map([7, 8, 9], |seed| {
        let mut w = World::new(seed);
        let mut p = scenarios::GridPlayer::new(&Zone::ALL);
        while w.time.tick < 4 * DAY {
            p.act(&mut w);
            w.step();
        }
        (seed, w)
    });
    for (seed, w) in towns {
        let t = w.people.trip_times_yesterday;
        let h = t.max_ticks as f32 / balance::TICKS_PER_HOUR as f32;
        let commutes: u32 = t.commutes.iter().sum();
        // Bins: <1, <2, <3, <4, <6, <8, more (hours).
        let over4 = t.commutes[4..].iter().sum::<u32>() as f32 / commutes.max(1) as f32;
        let over8 = t.commutes[6] as f32 / commutes.max(1) as f32;
        eprintln!(
            "seed {seed}: {} journeys ({commutes} commutes), commutes over 4 h {over4:.4}, over 8 h {over8:.4}; \
             all journeys over 8 h {:.4}, longest {h:.1} h; over 4 h by purpose [on, off] {:?}",
            t.total(),
            t.share_over(8.0),
            t.over_4h
        );
        assert!(commutes > 1000);
        assert!(over8 < 0.002, "{t:?}");
        assert!(over4 < 0.03, "{t:?}");
        assert!(t.share_over(8.0) < 0.03, "{t:?}");
    }
}

/// The town short of industrial land, run for `days`; with the land zoned
/// at the start of day `fix` when given.
fn short_town(seed: u64, days: u64, fix: Option<u32>) -> World {
    let mut w = World::new(seed);
    scenarios::prebuilt(&mut w, scenarios::short_of_industry);
    while w.time.tick < days * DAY {
        if fix.is_some_and(|d| w.time.day == d && w.time.tick_of_day == 0) {
            scenarios::add_industry_land(&mut w);
        }
        w.step();
    }
    w
}

#[test]
fn a_town_short_of_work_land_says_what_is_missing_and_recovers_once_zoned() {
    // Each seed's town short of land and its fixed copy, all six side by side.
    let towns = [1, 2, 3]
        .into_iter()
        .flat_map(|seed| [(seed, 4, None), (seed, 5, Some(3))]);
    let mut towns = par_map(towns, |(seed, days, fix)| short_town(seed, days, fix)).into_iter();
    for seed in [1, 2, 3] {
        let (w, fixed) = (towns.next().unwrap(), towns.next().unwrap());
        let v = jobs_view(&w);
        eprintln!("seed {seed}: jobless {:.3}, demand {:?}, {v:?}", v.rate(), w.demand);
        // People are out of work, industry is what is short, it has no free
        // land, and the demand bar says so loudly.
        assert!(v.rate() > 0.05, "{v:?}");
        assert_eq!(v.short_of, Some(Zone::Industrial));
        assert_eq!(v.reason, JoblessReason::NoLandZoned);
        assert!(w.demand.industrial > 0.75, "{:?}", w.demand);
        assert!(w.demand.industrial > w.demand.commercial + 0.4);
        // A jobless citizen's reason names it.
        let jobless = w
            .citizens
            .iter()
            .find(|(_, c)| c.in_workforce() && c.job.is_none())
            .map(|(id, _)| id)
            .unwrap();
        let r = inspect::happiness_reasons(&w, jobless).unwrap();
        assert!(
            r.iter().any(|r| r.text == "Out of work: not enough jobs in the city"),
            "{r:?}"
        );
        // Zoned on day 3: within two days nearly everyone has work again.
        let w = fixed;
        let v = jobs_view(&w);
        eprintln!("seed {seed} fixed: jobless {:.3}, {v:?}", v.rate());
        assert!(v.rate() < 0.03, "{v:?}");
        assert!(matches!(v.reason, JoblessReason::None | JoblessReason::Searching));
    }
}

#[test]
fn a_town_with_enough_work_has_no_jobless_problem() {
    let w = district_town(4, true, 3);
    let v = jobs_view(&w);
    eprintln!("{v:?} demand {:?}", w.demand);
    assert!(v.rate() < 0.05, "{v:?}");
    assert!(w.demand.industrial < 0.75);
}
