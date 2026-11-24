//! Regression tests for defects found by review and fuzzing. Each test
//! failed before its fix.

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use citysim_sim::*;
use common::*;

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

/// What a bulldoze must leave: every piece without an end in the
/// rectangle, plus all highway pieces.
fn expected_after_bulldoze(before: &BTreeSet<(Cell, Cell, bool)>, rect: CellRect) -> BTreeSet<(Cell, Cell, bool)> {
    before
        .iter()
        .copied()
        .filter(|&(a, b, hw)| hw || !(rect.contains(a) || rect.contains(b)))
        .collect()
}

/// Bulldoze planning looked up every road piece with a scan over all
/// segments, per road cell and direction, and execution did the same per
/// piece. On a dense map a full-map drag took seconds per preview (the UI
/// previews every frame) and seconds to apply.
#[test]
fn bulldoze_preview_and_apply_scale_on_a_dense_map() {
    let mut w = World::new(4);
    mesh(&mut w, 12, 250, 4, 250, 6);
    // Diagonals through the mesh as well.
    w.economy.money = i64::MAX / 4;
    for k in (0..200).step_by(12) {
        let _ = w.apply(Command::BuildRoad {
            from: c(12 + k, 4),
            to: c(12 + k + 40, 44),
        });
    }
    check_roads(&w);
    let before = pieces(&w);
    assert!(w.roads.segments.len() > 1500, "{}", w.roads.segments.len());
    let all = Command::Bulldoze {
        min: c(0, 0),
        max: c(255, 255),
    };
    let t = Instant::now();
    let p = w.preview(&all).unwrap();
    let preview_time = t.elapsed();
    assert!(p.cells.len() > 10_000, "{}", p.cells.len());

    // Partial bulldoze first: exact result, then the rest.
    let rect = CellRect::from_corners(c(30, 30), c(130, 140));
    let t = Instant::now();
    w.apply(Command::Bulldoze {
        min: rect.min,
        max: rect.max,
    })
    .unwrap();
    let partial_time = t.elapsed();
    check_roads(&w);
    assert_eq!(pieces(&w), expected_after_bulldoze(&before, rect));

    let t = Instant::now();
    w.apply(all).unwrap();
    let apply_time = t.elapsed();
    w.check_invariants().unwrap();
    assert_eq!(w.roads.segments.len(), 1, "only the highway is left");
    let limit = Duration::from_millis(400);
    assert!(
        preview_time < limit && partial_time < limit && apply_time < limit,
        "preview {preview_time:?}, partial apply {partial_time:?}, apply {apply_time:?}"
    );
}

/// Roads not lying on cell centres are never built by commands, but the
/// graph allows them: bulldozing one still removes exactly the pieces in
/// the rectangle (slow path).
#[test]
fn bulldoze_handles_a_segment_off_the_cell_centres() {
    let mut w = World::new(6);
    let y = 60;
    let a = w.roads.add_node([20.0 * CELL_SIZE + 1.0, (y as f32 + 0.5) * CELL_SIZE]);
    let b = w.roads.add_node([31.0 * CELL_SIZE - 1.0, (y as f32 + 0.5) * CELL_SIZE]);
    w.roads.add_segment(a, b, RoadKind::Street);
    for x in 20..=30 {
        w.grid.get_mut(c(x, y)).unwrap().occupant = Occupant::Road;
    }
    w.versions.roads += 1;
    w.versions.grid += 1;
    w.check_invariants().unwrap();
    let out = w
        .apply(Command::Bulldoze {
            min: c(24, y - 3),
            max: c(26, y + 3),
        })
        .unwrap();
    assert_eq!(out.cells, vec![c(24, y), c(25, y), c(26, y)]);
    w.check_invariants().unwrap();
    for x in 20..=30 {
        assert_eq!(w.grid.is_road(c(x, y)), !(24..=26).contains(&x), "{x}");
    }
    assert_eq!(w.roads.segments.len(), 3, "highway and the two remaining ends");
}

/// A ring road with one link to the highway and some trips on it.
fn ring_with_trips() -> World {
    let mut w = World::new(8);
    road(&mut w, (11, 128), (40, 128));
    for (a, b) in [
        ((40, 120), (60, 120)),
        ((60, 120), (60, 136)),
        ((60, 136), (40, 136)),
        ((40, 136), (40, 120)),
    ] {
        road(&mut w, a, b);
    }
    for k in 0..20 {
        let from = c(41 + k % 18, 120).center();
        let to = c(41 + (k * 7) % 18, 136).center();
        traffic::debug_request(&mut w, from, to);
    }
    w
}

/// A segment with a negative speed limit (only from a corrupt save, or a
/// front end writing the public field) gives negative edge costs: the
/// search's back-pointers formed a cycle and rebuilding the route pushed
/// segments forever until the process ran out of memory.
#[test]
fn negative_edge_costs_do_not_hang_the_path_search() {
    let mut w = ring_with_trips();
    for seg in w.roads.segments.values_mut() {
        if seg.kind == RoadKind::Street {
            seg.speed_limit = -seg.speed_limit;
        }
    }
    for _ in 0..200 {
        w.step();
    }
    w.check_sim_invariants().unwrap();
}
