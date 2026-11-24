//! Regression tests for defects found by review and fuzzing. Each test
//! failed before its fix.

use citysim_sim::*;

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

fn road(w: &mut World, a: (i32, i32), b: (i32, i32)) {
    w.apply(Command::BuildRoad {
        from: c(a.0, a.1),
        to: c(b.0, b.1),
    })
    .unwrap();
}

/// A square mesh of streets every `step` cells over `x0..=x1`, `y0..=y1`,
/// joined to the highway stub.
fn mesh(w: &mut World, x0: i32, x1: i32, y0: i32, y1: i32, step: usize) {
    let money = w.economy.money;
    w.economy.money = i64::MAX / 4;
    for x in (x0..=x1).step_by(step) {
        road(w, (x, y0), (x, y1));
    }
    for y in (y0..=y1).step_by(step) {
        road(w, (x0, y), (x1, y));
    }
    road(w, (11, 128), (x0, 128));
    w.economy.money = money;
}

/// Every road edit makes all cars reconsider their routes. Those re-plans
/// used to get the whole per-tick path budget first, so while a player kept
/// editing roads in a busy city no waiting trip ever got a car: departures
/// piled up and were all cancelled (counting as hopeless commutes).
#[test]
fn road_edits_do_not_starve_departures() {
    let mut w = World::new(3);
    mesh(&mut w, 12, 87, 98, 158, 3);
    let mut r = Rng::new(5);
    let mut far = || {
        let x = 12 + 3 * r.below(26) as i32;
        let y = 98 + 3 * r.below(21) as i32;
        c(x, y).center()
    };
    for _ in 0..300 {
        let (a, b) = (far(), far());
        traffic::debug_request(&mut w, a, b);
    }
    for _ in 0..60 {
        w.step();
    }
    assert!(w.vehicles.len() > 150, "cars on the road: {}", w.vehicles.len());
    for _ in 0..100 {
        let (a, b) = (far(), far());
        traffic::debug_request(&mut w, a, b);
    }
    // The player keeps editing a spur far away: every other tick a road
    // edit (build, then bulldoze it again).
    for t in 0..240 {
        if t % 2 == 0 {
            road(&mut w, (200, 20), (200, 21));
        } else {
            w.apply(Command::Bulldoze {
                min: c(200, 20),
                max: c(200, 21),
            })
            .unwrap();
        }
        w.step();
    }
    assert!(
        w.traffic.pending.len() < 10,
        "{} trips still waiting for a car after 12 s of edits",
        w.traffic.pending.len()
    );
}
