//! Extreme road layouts: the sim must stay consistent, keep cars moving and
//! survive the player tearing them apart under traffic.

mod common;

use citysim_sim::*;
use common::*;

fn build(w: &mut World, a: (i32, i32), b: (i32, i32)) -> bool {
    w.apply(Command::BuildRoad {
        from: c(a.0, a.1),
        to: c(b.0, b.1),
        kind: citysim_sim::RoadKind::Street,
    })
    .is_ok()
}

fn zone(w: &mut World, a: (i32, i32), b: (i32, i32), z: Zone) {
    w.apply(Command::SetZone {
        min: c(a.0, a.1),
        max: c(b.0, b.1),
        zone: Some(z),
    })
    .unwrap();
}

fn bulldoze(w: &mut World, a: (i32, i32), b: (i32, i32)) {
    w.apply(Command::Bulldoze {
        min: c(a.0, a.1),
        max: c(b.0, b.1),
    })
    .unwrap();
    check_consistency(w, false);
}

/// Step `ticks`, checking consistency every 40 ticks; returns the most
/// cars seen and the car trips completed.
fn run(w: &mut World, ticks: u64) -> (usize, u32) {
    let mut peak = 0;
    let mut done = 0;
    for _ in 0..ticks {
        let before = w.traffic.trips_today;
        w.step();
        done += w.traffic.trips_today.saturating_sub(before);
        peak = peak.max(w.vehicles.len());
        if w.time.tick.is_multiple_of(40) {
            check_consistency(w, true);
        }
    }
    check_consistency(w, true);
    (peak, done)
}

/// Run to `hour` (0..24) of the current or next day.
fn run_to_hour(w: &mut World, hour: u64) -> (usize, u32) {
    let tod = w.time.tick_of_day as u64;
    let want = hour * HOUR;
    run(w, (want + DAY - tod) % DAY)
}

fn rich(w: &mut World) {
    w.economy.money = 10_000_000;
}

/// The highway stub is intact: twelve highway cells, the entry node at its
/// west end with the highway as its first segment.
fn highway_intact(w: &World) {
    let entry = w.roads.entry.unwrap();
    let node = &w.roads.nodes[entry];
    assert!(same(node.pos, c(0, 128).center()));
    assert_eq!(w.roads.segments[node.segments[0]].kind, RoadKind::Highway);
    let p = pieces(w);
    for x in 0..balance::HIGHWAY_STUB_CELLS - 1 {
        assert!(p.contains(&(c(x, 128), c(x + 1, 128), true)), "highway piece {x} lost");
    }
}

fn same(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
}

/// One enormous tree: a trunk with long branches and short twigs, all dead
/// ends (U-turns everywhere). Then the trunk is cut next to the highway at
/// rush hour: everything is cut off, abandoned and emptied.
#[test]
fn enormous_dead_end_tree() {
    let mut w = World::new(31);
    rich(&mut w);
    assert!(build(&mut w, (11, 128), (190, 128)));
    for x in (16..=190).step_by(6) {
        assert!(build(&mut w, (x, 128), (x, 98)));
        assert!(build(&mut w, (x, 128), (x, 158)));
        for y in (100..=156).step_by(6) {
            if y != 128 {
                assert!(build(&mut w, (x, y), (x + 2, y)));
            }
        }
    }
    check_roads(&w);
    let dead_ends = w.roads.nodes.values().filter(|n| n.segments.len() == 1).count();
    assert!(dead_ends > 300, "{dead_ends}");
    zone(&mut w, (12, 96), (192, 127), Zone::Residential);
    zone(&mut w, (12, 129), (192, 142), Zone::Commercial);
    zone(&mut w, (12, 143), (192, 160), Zone::Industrial);
    w.economy.money = 50_000;
    let (peak, done) = run(&mut w, DAY);
    run_to_hour(&mut w, 8);
    assert!(w.households.len() > 100, "{}", w.households.len());
    assert!(peak > 20 && done > 50, "peak {peak}, trips {done}");
    // Cut the trunk under the morning rush.
    bulldoze(&mut w, (13, 127), (13, 129));
    let cut_off = |b: &Building| b.footprint.min.x > 16;
    assert!(w.buildings.values().any(cut_off));
    // Abandoned one in-game day (the grace period) after the cut.
    run(&mut w, DAY + 2 * HOUR);
    // Only the few buildings next to the highway stub are still reachable.
    assert!(!w.buildings.values().any(cut_off), "cut-off buildings are abandoned");
    // Cars already on the tree finish their drives (vehicle time is real
    // time: a cross-tree drive takes several in-game hours).
    run(&mut w, DAY / 2);
    let far = w.vehicles.values().filter(|v| v.pos[0] > 16.0 * CELL_SIZE).count();
    assert_eq!(far, 0, "cars left on the cut-off tree");
    check_roads(&w);
}

/// A ring road with one link to the highway; then a side of the ring is
/// removed at rush hour and later rebuilt.
#[test]
fn ring_road() {
    let mut w = World::new(32);
    rich(&mut w);
    assert!(build(&mut w, (11, 128), (40, 128)));
    for (a, b) in [
        ((40, 100), (110, 100)),
        ((110, 100), (110, 156)),
        ((110, 156), (40, 156)),
        ((40, 156), (40, 100)),
    ] {
        assert!(build(&mut w, a, b));
    }
    zone(&mut w, (38, 98), (112, 127), Zone::Residential);
    zone(&mut w, (38, 129), (112, 140), Zone::Commercial);
    zone(&mut w, (38, 141), (112, 158), Zone::Industrial);
    w.economy.money = 50_000;
    let (_, done) = run(&mut w, DAY);
    run_to_hour(&mut w, 8);
    assert!(
        w.households.len() > 50 && done > 20,
        "{} households, {done} trips",
        w.households.len()
    );
    rich(&mut w);
    bulldoze(&mut w, (105, 110), (115, 146));
    run(&mut w, 2 * HOUR);
    assert!(build(&mut w, (110, 100), (110, 156)));
    let (_, done) = run(&mut w, DAY);
    assert!(
        w.households.len() > 50 && done > 50,
        "{} households, {done} trips",
        w.households.len()
    );
    check_full(&w);
}

/// Islands of road with no link to the highway: zoning works, nothing
/// grows. Linking one makes it grow; cutting it again empties it.
#[test]
fn disconnected_islands() {
    let mut w = World::new(33);
    rich(&mut w);
    for (x0, y0) in [(40, 40), (120, 40), (40, 180), (180, 180), (60, 120)] {
        for k in 0..=3 {
            assert!(build(&mut w, (x0, y0 + 6 * k), (x0 + 24, y0 + 6 * k)));
            assert!(build(&mut w, (x0 + 8 * k, y0), (x0 + 8 * k, y0 + 18)));
        }
        zone(&mut w, (x0 - 2, y0 - 2), (x0 + 26, y0 + 8), Zone::Residential);
        zone(&mut w, (x0 - 2, y0 + 9), (x0 + 26, y0 + 13), Zone::Commercial);
        zone(&mut w, (x0 - 2, y0 + 14), (x0 + 26, y0 + 20), Zone::Industrial);
    }
    w.economy.money = 50_000;
    run(&mut w, DAY);
    assert_eq!(w.buildings.len(), 0, "nothing grows without a highway link");
    assert_eq!(w.households.len(), 0);
    assert!(w.vehicles.is_empty());
    // Link the island at (60, 120).
    assert!(build(&mut w, (11, 128), (60, 128)));
    run(&mut w, DAY);
    run_to_hour(&mut w, 8);
    assert!(w.households.len() > 30, "{}", w.households.len());
    assert!(
        w.buildings
            .values()
            .all(|b| b.footprint.min.x >= 30 && (100..=150).contains(&b.footprint.min.y))
    );
    // Cut it again under traffic.
    bulldoze(&mut w, (20, 127), (21, 129));
    // Abandoned one in-game day (the grace period) after the cut; cars
    // already on the island then finish their drives.
    run(&mut w, DAY + 2 * HOUR);
    assert_eq!(w.buildings.len(), 0);
    assert_eq!(w.households.len(), 0);
    run(&mut w, DAY / 2);
    assert!(w.vehicles.is_empty() && w.traffic.pending.is_empty());
}

/// Roads crossing, touching and running along the highway stub, a town on
/// it, then everything around the stub bulldozed in every shape. The stub
/// survives untouched and a new town grows from it.
#[test]
fn highway_stub_with_everything_bulldozed_around_it() {
    let mut w = World::new(34);
    rich(&mut w);
    assert!(build(&mut w, (0, 118), (0, 138)), "through the entry cell");
    for x in [2, 5, 8, 11] {
        assert!(build(&mut w, (x, 118), (x, 138)));
    }
    assert!(build(&mut w, (0, 127), (30, 127)));
    assert!(build(&mut w, (0, 129), (30, 129)));
    let _ = build(&mut w, (1, 125), (7, 131));
    let _ = build(&mut w, (7, 125), (1, 131));
    assert!(build(&mut w, (11, 128), (40, 128)));
    assert!(build(&mut w, (40, 110), (40, 146)));
    check_roads(&w);
    highway_intact(&w);
    zone(&mut w, (0, 110), (42, 126), Zone::Residential);
    zone(&mut w, (0, 130), (42, 138), Zone::Commercial);
    zone(&mut w, (0, 139), (42, 146), Zone::Industrial);
    w.economy.money = 50_000;
    run(&mut w, DAY);
    run_to_hour(&mut w, 8);
    assert!(w.households.len() > 30, "{}", w.households.len());
    rich(&mut w);
    let rects = [
        ((0, 128), (0, 128)),
        ((5, 128), (5, 128)),
        ((11, 128), (12, 128)),
        ((0, 126), (3, 130)),
        ((-5, 127), (11, 129)),
        ((4, 120), (9, 136)),
        ((0, 0), (11, 255)),
        ((0, 128), (255, 128)),
    ];
    for (a, b) in rects {
        bulldoze(&mut w, a, b);
        check_roads(&w);
        highway_intact(&w);
        run(&mut w, 60);
    }
    bulldoze(&mut w, (-10, -10), (300, 300));
    assert_eq!(w.roads.segments.len(), 1);
    assert_eq!(w.roads.nodes.len(), 2);
    assert!(w.buildings.is_empty() && w.households.is_empty());
    highway_intact(&w);
    // Zoning survives bulldozing (and would regrow along the stub).
    w.apply(Command::SetZone {
        min: c(0, 0),
        max: c(255, 255),
        zone: None,
    })
    .unwrap();
    run(&mut w, 2 * HOUR);
    assert!(w.vehicles.is_empty());
    check_full(&w);
    // A new town grows from the bare stub.
    assert!(build(&mut w, (11, 128), (40, 128)));
    assert!(build(&mut w, (25, 118), (25, 140)));
    zone(&mut w, (12, 118), (40, 126), Zone::Residential);
    zone(&mut w, (12, 130), (40, 134), Zone::Commercial);
    zone(&mut w, (12, 135), (40, 140), Zone::Industrial);
    w.economy.money = 50_000;
    run(&mut w, DAY);
    assert!(w.households.len() > 30, "{}", w.households.len());
    highway_intact(&w);
}

/// A map-spanning mesh of 45° roads in both directions (crossing at cell
/// centres), a town on part of it, then big chunks torn out under traffic.
#[test]
fn map_spanning_diagonal_mesh() {
    let mut w = World::new(35);
    w.economy.money = i64::MAX / 4;
    let last = MAP_CELLS - 1;
    for k in (-240..=240).step_by(16) {
        // x - y = k
        let (x0, y0) = if k >= 0 { (k, 0) } else { (0, -k) };
        let n = last - x0.max(y0);
        assert!(build(&mut w, (x0, y0), (x0 + n, y0 + n)), "x - y = {k}");
    }
    for k in (16..=2 * last - 16).step_by(16) {
        // x + y = k
        let (x0, y0) = if k <= last { (0, k) } else { (k - last, last) };
        let n = y0.min(last - x0);
        assert!(build(&mut w, (x0, y0), (x0 + n, y0 - n)), "x + y = {k}");
    }
    assert!(build(&mut w, (11, 128), (60, 128)));
    check_roads(&w);
    assert!(w.roads.segments.len() > 800, "{}", w.roads.segments.len());
    zone(&mut w, (12, 100), (60, 126), Zone::Residential);
    zone(&mut w, (12, 130), (60, 140), Zone::Commercial);
    zone(&mut w, (12, 141), (60, 156), Zone::Industrial);
    w.economy.money = 50_000;
    let (peak, done) = run(&mut w, DAY);
    run_to_hour(&mut w, 8);
    assert!(w.households.len() > 30, "{}", w.households.len());
    assert!(peak > 5 && done > 10, "peak {peak}, trips {done}");
    rich(&mut w);
    bulldoze(&mut w, (30, 110), (45, 150));
    run(&mut w, 2 * HOUR);
    bulldoze(&mut w, (100, 0), (255, 255));
    check_roads(&w);
    run(&mut w, DAY);
    bulldoze(&mut w, (0, 0), (255, 255));
    check_roads(&w);
    w.apply(Command::SetZone {
        min: c(0, 0),
        max: c(255, 255),
        zone: None,
    })
    .unwrap();
    run(&mut w, 2 * HOUR);
    assert!(w.vehicles.is_empty() && w.households.is_empty());
}
