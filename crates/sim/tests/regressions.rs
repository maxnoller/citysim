//! Regression tests for defects found by review and fuzzing. Each test
//! failed before its fix.

use citysim_sim::*;

const DAY: u64 = balance::TICKS_PER_DAY as u64;
const HOUR: u64 = balance::TICKS_PER_HOUR as u64;

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

fn zone(w: &mut World, a: (i32, i32), b: (i32, i32), z: Zone) {
    w.apply(Command::SetZone {
        min: c(a.0, a.1),
        max: c(b.0, b.1),
        zone: Some(z),
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

/// `avg_commute_s` only moves when a car commute finishes. When a city
/// with long commutes emptied out, the stale average stayed forever: with
/// no households nobody could commute to correct it, residential demand was
/// pinned at the bottom and a freshly built town never grew again.
#[test]
fn stale_commute_average_does_not_freeze_an_empty_city() {
    let mut w = World::new(9);
    // Homes and jobs at opposite ends of a very long loop.
    road(&mut w, (11, 128), (250, 128));
    road(&mut w, (250, 128), (250, 140));
    road(&mut w, (250, 140), (12, 140));
    zone(&mut w, (12, 126), (40, 127), Zone::Residential);
    zone(&mut w, (12, 141), (40, 142), Zone::Commercial);
    zone(&mut w, (12, 138), (40, 139), Zone::Industrial);
    while w.traffic.avg_commute_s < 200.0 {
        assert!(w.time.tick < 5 * DAY, "commutes never got long");
        w.step();
    }
    // Wipe the town and build a compact one next to the highway.
    w.economy.money = 50_000;
    w.apply(Command::Bulldoze {
        min: c(0, 0),
        max: c(255, 255),
    })
    .unwrap();
    w.apply(Command::SetZone {
        min: c(0, 0),
        max: c(255, 255),
        zone: None,
    })
    .unwrap();
    for _ in 0..HOUR {
        w.step();
    }
    assert_eq!(w.households.len(), 0);
    road(&mut w, (11, 128), (40, 128));
    road(&mut w, (11, 133), (40, 133));
    road(&mut w, (20, 120), (20, 136));
    zone(&mut w, (12, 120), (40, 127), Zone::Residential);
    zone(&mut w, (12, 129), (40, 130), Zone::Commercial);
    zone(&mut w, (12, 131), (40, 136), Zone::Industrial);
    let end = w.time.tick + DAY;
    while w.time.tick < end {
        w.step();
    }
    assert!(
        w.households.len() > 20 && w.buildings.len() > 10,
        "the new town never grew: {} households, {} buildings, avg commute {}",
        w.households.len(),
        w.buildings.len(),
        w.traffic.avg_commute_s
    );
}
