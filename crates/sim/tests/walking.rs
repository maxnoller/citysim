//! People on foot: nobody is run over on a zebra, people cross signals with
//! the parallel green, cars wait for them, walkers survive save and load in
//! lockstep and a road edit under them, the same seed walks the same way.

mod common;

use citysim_sim::walk::{self, WalkState};
use citysim_sim::*;
use common::{DAY, HOUR, par_map};

/// The free-form town (organic layout, signals on its avenues) grown to
/// `day` at `hour`.
fn organic_at(seed: u64, day: u64, hour: u64) -> World {
    let mut w = World::new(seed);
    scenarios::prebuilt(&mut w, |w| {
        let refused = scenarios::organic(w);
        assert!(refused.is_empty(), "{refused:?}");
    });
    let until = day * DAY + hour * HOUR - balance::START_TICK_OF_DAY as u64;
    while w.time.tick < until {
        w.step();
    }
    w
}

/// Does a circle overlap a convex quad?
fn circle_hits_quad(c: [f32; 2], r: f32, q: &[[f32; 2]; 4]) -> bool {
    // Inside, or within `r` of an edge.
    let mut inside = true;
    let mut sign = 0.0f32;
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let cr = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        if sign == 0.0 {
            sign = cr.signum();
        } else if cr.signum() != sign && cr != 0.0 {
            inside = false;
        }
        let ab = [b[0] - a[0], b[1] - a[1]];
        let l2 = ab[0] * ab[0] + ab[1] * ab[1];
        let t = (((c[0] - a[0]) * ab[0] + (c[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0);
        let p = [a[0] + ab[0] * t, a[1] + ab[1] * t];
        if (p[0] - c[0]).hypot(p[1] - c[1]) < r {
            return true;
        }
    }
    inside
}

#[test]
fn nobody_is_run_over_on_a_zebra() {
    // Two towns at the morning rush and at lunch, side by side.
    let results = par_map([(5u64, 7u64), (6, 12)], |(seed, hour)| {
        let mut w = organic_at(seed, 3, hour);
        let held0 = walk::debug_holds(&w).iter().sum::<u64>();
        let (mut samples, mut hits, mut crossed) = (0u64, Vec::new(), 0u64);
        for _ in 0..2 * HOUR {
            w.step();
            let people = walk::people_on_crossings(&w);
            if people.is_empty() {
                continue;
            }
            samples += people.len() as u64;
            let bodies = traffic::car_bodies(&w);
            for &(p, r) in &people {
                for (id, q) in &bodies {
                    if circle_hits_quad(p, r, q) && hits.len() < 10 {
                        hits.push(format!("tick {} person at {p:?} car {id:?} {q:?}", w.time.tick));
                    }
                }
            }
            crossed += w
                .walking
                .walkers
                .values()
                .filter(|x| x.state == WalkState::Crossing)
                .count() as u64;
        }
        let held = walk::debug_holds(&w).iter().sum::<u64>() - held0;
        w.check_sim_invariants().unwrap();
        (samples, hits, crossed, held)
    });
    for (samples, hits, crossed, held) in results {
        assert!(hits.is_empty(), "cars over people on zebras:\n{}", hits.join("\n"));
        assert!(samples > 1_000, "too few people on zebras to tell: {samples}");
        assert!(
            crossed > 0 && held > 0,
            "people crossed ({crossed}) and cars waited for them ({held})"
        );
    }
}

#[test]
fn people_cross_signals_only_with_the_parallel_green_and_call_it() {
    let mut w = organic_at(7, 3, 11);
    let signal_crossings: Vec<u32> = walk::network(&w)
        .expect("network")
        .crossings
        .iter()
        .enumerate()
        .filter(|(_, c)| c.node.and_then(|n| junction::junction_control(&w, n)) == Some(JunctionControl::Signal))
        .map(|(i, _)| i as u32)
        .collect();
    assert!(!signal_crossings.is_empty(), "the town has signalled crossings");
    // Who waits where (leader -> crossing), last tick.
    let mut was_waiting: std::collections::BTreeMap<CitizenId, walk::CrossingKey> = Default::default();
    let (mut stepped, mut waits) = (0, 0);
    for _ in 0..5 * HOUR {
        w.step();
        for (id, wk) in w.walking.walkers.iter() {
            let Some(k) = wk.crossing() else { continue };
            if wk.state == WalkState::Crossing
                && was_waiting.get(id) == Some(&k)
                && let Some((_, c)) = walk::network(&w).and_then(|n| n.crossing(k))
                && let (Some(n), Some(arm)) = (c.node, c.arm)
                && walk::walk_signal(&w, n, arm).is_some()
            {
                // The light changed this tick at the latest: it shows
                // people the green now or did a tick ago (yellow follows).
                let j = junction::junction(&w, n).unwrap();
                let phase = &j.plan.as_ref().unwrap().phases[j.signal.phase as usize];
                assert!(
                    !phase.arms.contains(&arm) && !phase.left_only,
                    "tick {}: stepped onto a zebra across arm {arm} while it has green",
                    w.time.tick
                );
                stepped += 1;
            }
        }
        was_waiting = w
            .walking
            .walkers
            .iter()
            .filter(|(_, x)| x.state == WalkState::Waiting)
            .filter_map(|(id, x)| Some((*id, x.crossing()?)))
            .collect();
        waits += was_waiting.len();
    }
    assert!(stepped >= 5, "people crossed at signals: {stepped}");
    assert!(waits > 0);
}

#[test]
fn walkers_survive_save_and_load_in_lockstep() {
    let mut a = organic_at(8, 3, 13);
    assert!(
        a.walking.walkers.len() > 15,
        "walkers under way: {}",
        a.walking.walkers.len()
    );
    assert!(
        a.walking.walkers.values().any(|x| x.state != WalkState::Walking),
        "someone at or on a zebra"
    );
    let mut b = World::load(&a.save()).expect("loads");
    assert_eq!(a.walking.walkers, b.walking.walkers);
    assert!(a == b, "{}", common::diff(&a, &b));
    for _ in 0..HOUR {
        a.step();
        b.step();
    }
    assert_eq!(a.walking.walkers, b.walking.walkers);
    assert_eq!(a.save(), b.save(), "lockstep after an hour: {}", common::diff(&a, &b));
    b.check_sim_invariants().unwrap();
}

#[test]
fn walking_is_deterministic() {
    let worlds = par_map([9u64, 9], |s| organic_at(s, 2, 17));
    assert!(!worlds[0].walking.walkers.is_empty());
    assert_eq!(worlds[0].walking.walkers, worlds[1].walking.walkers);
    assert_eq!(worlds[0].save(), worlds[1].save());
}

#[test]
fn a_road_edit_under_walking_people_reroutes_them() {
    let mut w = organic_at(10, 3, 8);
    // A walker with a good way to go: take out a stretch of road ahead of
    // them (with its pavements).
    let (id, ahead) = w
        .walking
        .walkers
        .iter()
        .filter(|(_, x)| x.left_m() > 60.0)
        .map(|(id, x)| (*id, x.ahead()))
        .next()
        .expect("a walker with a way to go");
    let p = ahead[ahead.len() / 2];
    let r = 6.0;
    let quad = [
        [p[0] - r, p[1] - r],
        [p[0] + r, p[1] - r],
        [p[0] + r, p[1] + r],
        [p[0] - r, p[1] + r],
    ];
    let before = w.walking.walkers.len();
    w.apply(Command::BulldozeArea { quad }).expect("bulldoze");
    w.check_sim_invariants().unwrap();
    // On the next step everyone finds a new way (or finishes off screen).
    w.step();
    w.check_sim_invariants().unwrap();
    if let Some(x) = w.walking.walkers.get(&id) {
        let pts = x.ahead();
        assert!(
            pts.iter()
                .all(|q| (q[0] - p[0]).abs() > r - 1.0 || (q[1] - p[1]).abs() > r - 1.0)
                || pts.windows(2).all(|s| s[0] != s[1]),
            "new way"
        );
    }
    assert!(w.walking.walkers.len() + 20 >= before / 2, "most walk on");
    for _ in 0..HOUR {
        w.step();
    }
    w.check_sim_invariants().unwrap();
}
