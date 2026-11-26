//! The road layout decides how a growing city copes with its traffic: an
//! avenue hierarchy against the same blocks on streets only, and the two
//! fixes a player would try on a jammed streets-only city (upgrading its
//! main streets to avenues; relief roads around its worst junction).
//! Statistics over three seeds each. Long (a few minutes in release, the
//! seeds in parallel): `cargo test -p citysim_sim --release -- --ignored`.

use citysim_sim::scenarios::{self, GridPlayer, Layout};
use citysim_sim::*;

const SEEDS: [u64; 3] = [1, 2, 3];

/// One finished day of a run.
#[derive(Debug, Clone, Copy, Default)]
struct Day {
    population: u32,
    delay: f32,
    gave_up: u32,
    trips: u32,
    income: i64,
    happiness: f32,
}

impl Day {
    fn gave_up_share(&self) -> f32 {
        self.gave_up as f32 / self.trips.max(1) as f32
    }
}

/// Play `p` on `w` until the end of day `until` (1-based count of finished
/// days), recording each finished day.
fn play(w: &mut World, p: &mut GridPlayer, until: u32, days: &mut Vec<Day>) {
    let mut gave_up_before = w.traffic.stuck_removed;
    while (days.len() as u32) < until {
        p.act(w);
        w.step();
        if w.time.tick_of_day == balance::TICKS_PER_DAY - 1 {
            days.push(Day {
                population: w.stats.population,
                delay: traffic::delay_factor(w),
                gave_up: w.traffic.stuck_removed - gave_up_before,
                trips: w.traffic.trips_today,
                income: economy::budget_now(w).income(),
                happiness: w.people.stats.avg_happiness,
            });
            gave_up_before = w.traffic.stuck_removed;
        }
    }
}

fn mean(xs: impl Iterator<Item = f32>) -> f32 {
    let v: Vec<f32> = xs.collect();
    v.iter().sum::<f32>() / v.len().max(1) as f32
}

/// Days `range` (0-based) averaged over the seeds' runs.
fn avg(runs: &[Vec<Day>], range: std::ops::Range<usize>, f: impl Fn(&Day) -> f32) -> f32 {
    mean(runs.iter().flat_map(|r| r[range.clone()].iter().map(&f)))
}

fn run_seeds<F>(f: F) -> Vec<Vec<Day>>
where
    F: Fn(u64) -> Vec<Day> + Sync,
{
    std::thread::scope(|s| {
        let f = &f;
        let handles: Vec<_> = SEEDS.iter().map(|&seed| s.spawn(move || f(seed))).collect();
        handles.into_iter().map(|h| h.join().expect("seed run")).collect()
    })
}

fn city(seed: u64, layout: Layout, days: u32) -> Vec<Day> {
    let mut w = World::new(seed);
    let mut p = if layout == Layout::Hierarchy {
        GridPlayer::hierarchy()
    } else {
        GridPlayer::new(&Zone::ALL)
    };
    let mut out = Vec::new();
    play(&mut w, &mut p, days, &mut out);
    out
}

#[test]
#[ignore = "long: three seeds of two growing cities"]
fn an_avenue_hierarchy_copes_where_streets_alone_jam() {
    let avenues = run_seeds(|s| city(s, Layout::Hierarchy, 12));
    let streets = run_seeds(|s| city(s, Layout::Streets, 12));
    // Days 9 to 12: about 9,000 to 13,000 residents.
    let late = 8..12;
    let pop = |r: &[Vec<Day>]| avg(r, late.clone(), |d| d.population as f32);
    let delay = |r: &[Vec<Day>]| avg(r, late.clone(), |d| d.delay);
    let gave = |r: &[Vec<Day>]| avg(r, late.clone(), |d| d.gave_up_share());
    let income = |r: &[Vec<Day>]| avg(r, late.clone(), |d| d.income as f32);
    eprintln!(
        "avenues: pop {:.0} delay x{:.2} gave up {:.2} % income {:.0}",
        pop(&avenues),
        delay(&avenues),
        100.0 * gave(&avenues),
        income(&avenues)
    );
    eprintln!(
        "streets: pop {:.0} delay x{:.2} gave up {:.2} % income {:.0}",
        pop(&streets),
        delay(&streets),
        100.0 * gave(&streets),
        income(&streets)
    );
    // The avenue city carries more people, closer to free flow, gives up
    // far fewer trips, and earns more.
    assert!(pop(&avenues) > 1.05 * pop(&streets));
    assert!(delay(&avenues) < delay(&streets));
    assert!(gave(&avenues) < 0.8 * gave(&streets));
    assert!(income(&avenues) > 1.05 * income(&streets));
    // It is still busy but moving: give-ups stay rare.
    assert!(gave(&avenues) < 0.025);
}

/// A streets-only city to the end of day 7, then two copies: one goes on
/// as it is, the other gets `fix` at the start of day 8; both play to the
/// end of day 11.
fn fixed_and_not(seed: u64, fix: fn(&mut World, &mut GridPlayer)) -> (Vec<Day>, Vec<Day>) {
    let mut w = World::new(seed);
    let mut p = GridPlayer::new(&Zone::ALL);
    let mut days = Vec::new();
    play(&mut w, &mut p, 7, &mut days);
    // `play` stops one tick before midnight; step to it.
    p.act(&mut w);
    w.step();
    let (mut w2, mut p2, mut days2) = (w.clone(), p.clone(), days.clone());
    fix(&mut w2, &mut p2);
    play(&mut w, &mut p, 11, &mut days);
    play(&mut w2, &mut p2, 11, &mut days2);
    (days, days2)
}

fn upgrade(w: &mut World, p: &mut GridPlayer) {
    let (ok, _, cost) = scenarios::upgrade_to_hierarchy(w);
    assert!(ok > 20 && cost > 0, "{ok} pieces for ${cost}");
    p.layout = Layout::Hierarchy;
}

fn bypass(w: &mut World, _: &mut GridPlayer) {
    let (built, cost) = scenarios::add_gateway_bypass(w, RoadKind::Street);
    assert!(built == 2 && cost > 0, "{built} relief roads for ${cost}");
}

#[test]
#[ignore = "long: three seeds of a jammed city, fixed and not"]
fn upgrading_main_streets_to_avenues_rescues_a_jammed_city() {
    let runs: Vec<(Vec<Day>, Vec<Day>)> = std::thread::scope(|s| {
        let h: Vec<_> = SEEDS
            .iter()
            .map(|&seed| s.spawn(move || fixed_and_not(seed, upgrade)))
            .collect();
        h.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let (plain, fixed): (Vec<Vec<Day>>, Vec<Vec<Day>>) = runs.into_iter().unzip();
    // Days 9 to 11: from the day after the fix on.
    let after = 8..11;
    for (name, f) in [
        ("delay", (|d: &Day| d.delay) as fn(&Day) -> f32),
        ("gave up", |d: &Day| d.gave_up_share()),
        ("pop", |d: &Day| d.population as f32),
        ("income", |d: &Day| d.income as f32),
        ("happy", |d: &Day| d.happiness),
    ] {
        eprintln!(
            "{name}: as it was {:.3}, upgraded {:.3}",
            avg(&plain, after.clone(), f),
            avg(&fixed, after.clone(), f)
        );
    }
    // Commutes drop markedly, give-ups fall and stay down, the city grows
    // and earns more.
    assert!(avg(&fixed, after.clone(), |d| d.delay) < 0.85 * avg(&plain, after.clone(), |d| d.delay));
    assert!(
        avg(&fixed, after.clone(), |d| d.gave_up_share()) < 0.5 * avg(&plain, after.clone(), |d| d.gave_up_share())
    );
    assert!(avg(&fixed, 10..11, |d| d.gave_up_share()) < avg(&plain, 10..11, |d| d.gave_up_share()));
    assert!(avg(&fixed, after.clone(), |d| d.population as f32) > avg(&plain, after.clone(), |d| d.population as f32));
    assert!(avg(&fixed, after.clone(), |d| d.income as f32) > avg(&plain, after, |d| d.income as f32));
}

#[test]
#[ignore = "long: three seeds of a jammed city, fixed and not"]
fn relief_roads_around_the_worst_junction_help_a_jammed_city() {
    let runs: Vec<(Vec<Day>, Vec<Day>)> = std::thread::scope(|s| {
        let h: Vec<_> = SEEDS
            .iter()
            .map(|&seed| s.spawn(move || fixed_and_not(seed, bypass)))
            .collect();
        h.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let (plain, fixed): (Vec<Vec<Day>>, Vec<Vec<Day>>) = runs.into_iter().unzip();
    let after = 8..11;
    let gave = |r: &[Vec<Day>]| avg(r, after.clone(), |d| d.gave_up_share());
    let delay = |r: &[Vec<Day>]| avg(r, after.clone(), |d| d.delay);
    eprintln!(
        "as it was: gave up {:.2} % delay x{:.2}; relief roads: gave up {:.2} % delay x{:.2}",
        100.0 * gave(&plain),
        delay(&plain),
        100.0 * gave(&fixed),
        delay(&fixed)
    );
    // Fewer cars stuck behind the gateway; commutes about the same (the
    // city's own junctions still limit it).
    assert!(gave(&fixed) < 0.8 * gave(&plain));
    assert!(delay(&fixed) < 1.05 * delay(&plain));
}
