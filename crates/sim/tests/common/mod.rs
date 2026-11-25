//! Checks and helpers shared by the integration tests.
#![allow(dead_code)]

use std::collections::BTreeSet;

use citysim_sim::*;

pub const DAY: u64 = balance::TICKS_PER_DAY as u64;
pub const HOUR: u64 = balance::TICKS_PER_HOUR as u64;

pub fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

// ------------------------------------------------------------- checks ----

/// Consistency checks beyond `check_sim_invariants`, cheap enough to run
/// after every command. `after_step`: the world was just stepped (segment
/// loads and junction tables are fresh).
pub fn check_consistency(w: &World, after_step: bool) {
    if let Err(e) = w.check_sim_invariants() {
        panic!("sim invariants at tick {}: {e}", w.time.tick);
    }
    let tick = w.time.tick;
    // Every vehicle has a trip, its pose matches its segment and progress.
    for (vid, v) in w.vehicles.iter() {
        let trip = w.traffic.trips.get(vid);
        assert!(trip.is_some(), "tick {tick}: {vid:?} has no trip");
        let t = trip.unwrap();
        if t.needs_route {
            assert!(t.queued, "tick {tick}: {vid:?} needs a route but is not queued");
        }
        if t.queued {
            assert!(
                w.traffic.replan.contains(&vid),
                "tick {tick}: {vid:?} marked queued but not in the queue"
            );
        }
        let s = v.segment.unwrap();
        let seg = &w.roads.segments[s];
        if let Some(jm) = v.junction {
            let j = w.traffic.junction_states.get(jm.node).expect("box junction exists");
            let m = &j.movements[jm.movement as usize];
            assert!(
                jm.distance >= 0.0 && jm.distance <= m.length + 0.01,
                "tick {tick}: {vid:?} {jm:?}"
            );
            assert_eq!(jm.from, s, "tick {tick}: {vid:?} box from");
            assert_eq!(v.route.first(), Some(&jm.to), "tick {tick}: {vid:?} box to");
            continue;
        }
        let pa = w.roads.nodes[seg.a].pos;
        let pb = w.roads.nodes[seg.b].pos;
        let (from, to) = if v.forward { (pa, pb) } else { (pb, pa) };
        let f = (v.progress / seg.length).clamp(0.0, 1.0);
        let want = [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f];
        let d = ((want[0] - v.pos[0]).powi(2) + (want[1] - v.pos[1]).powi(2)).sqrt();
        assert!(d < 0.05, "tick {tick}: {vid:?} pose off by {d} m");
        assert!(
            v.speed.is_finite() && v.speed >= 0.0,
            "tick {tick}: {vid:?} speed {}",
            v.speed
        );
        assert!(v.heading.is_finite());
    }
    // Households: a commuting household has a car or a waiting departure.
    for (hid, h) in w.households.iter() {
        let commuting = matches!(h.state, HouseholdState::CommutingToWork | HouseholdState::CommutingHome);
        if h.vehicle.is_some() {
            assert!(commuting, "tick {tick}: {hid:?} has a car but is {:?}", h.state);
        } else if commuting {
            let waiting = w.traffic.pending.iter().any(|d| d.household == Some(hid));
            assert!(
                waiting,
                "tick {tick}: {hid:?} is {:?} with no car and no departure",
                h.state
            );
        }
        let n = w.traffic.pending.iter().filter(|d| d.household == Some(hid)).count();
        assert!(
            n + usize::from(h.vehicle.is_some()) <= 1,
            "tick {tick}: {hid:?} has {n} departures and car {:?}",
            h.vehicle
        );
        assert!(h.members >= 1);
    }
    // Buildings: footprint cells point back and stay zoned for the kind.
    for (bid, b) in w.buildings.iter() {
        assert!(b.level >= 1 && b.capacity > 0, "tick {tick}: {bid:?} {b:?}");
        for cell in b.footprint.cells() {
            let cd = w.grid.get(cell).expect("footprint in bounds");
            assert_eq!(cd.occupant, Occupant::Building(bid), "tick {tick}: {cell:?}");
            assert_eq!(cd.zone, Some(b.kind), "tick {tick}: {cell:?} zone under {bid:?}");
        }
    }
    // Every building cell belongs to a live building.
    let building_cells: usize = w.buildings.values().map(|b| (b.size().0 * b.size().1) as usize).sum();
    let marked = w
        .grid
        .cells
        .iter()
        .filter(|cd| matches!(cd.occupant, Occupant::Building(_)))
        .count();
    assert_eq!(building_cells, marked, "tick {tick}: stray building cells");
    // Stats match the state.
    let s = &w.stats;
    assert_eq!(s.households as usize, w.households.len());
    assert_eq!(s.buildings as usize, w.buildings.len());
    assert_eq!(s.vehicles_on_road as usize, w.vehicles.len());
    let pop: u32 = w.households.values().map(|h| h.members as u32).sum();
    assert_eq!(s.population, pop);
    let roads = w.grid.cells.iter().filter(|cd| cd.occupant == Occupant::Road).count();
    assert_eq!(
        s.road_cells as usize, roads,
        "tick {tick}: cached road cell count stale"
    );
    for x in [
        w.demand.residential,
        w.demand.commercial,
        w.demand.industrial,
        w.growth.attractiveness,
    ] {
        assert!(x.is_finite() && (-1.0..=1.0).contains(&x), "tick {tick}: demand {x}");
    }
    assert!(w.traffic.avg_commute_s.is_finite() && w.traffic.avg_commute_s >= 0.0);
    // The highway stub is always there.
    let entry = w.roads.entry.expect("entry");
    assert!(w.roads.nodes.contains_key(entry));
    for x in 0..balance::HIGHWAY_STUB_CELLS {
        assert!(w.grid.is_road(c(x, 128)), "tick {tick}: highway cell {x} lost");
    }
    if after_step {
        for (n, node) in w.roads.nodes.iter() {
            let j = w.traffic.junction_states.get(n).expect("junction state for every node");
            assert_eq!(
                j.arms.iter().map(|a| a.segment).collect::<Vec<_>>(),
                node.segments,
                "tick {tick}: stale junction arms at {n:?}"
            );
        }
        let mut load: std::collections::BTreeMap<(SegmentId, usize), f32> = Default::default();
        for v in w.vehicles.values() {
            *load
                .entry((v.segment.unwrap(), RoadSegment::dir(v.forward)))
                .or_default() += 1.0;
        }
        for (s, seg) in w.roads.segments.iter() {
            for d in 0..2 {
                let want = load.get(&(s, d)).copied().unwrap_or(0.0);
                assert_eq!(seg.load[d], want, "tick {tick}: {s:?} load[{d}]");
            }
        }
    }
}

/// Heavier structural checks (road graph, grid vs graph).
pub fn check_full(w: &World) {
    if let Err(e) = w.check_invariants() {
        panic!("invariants at tick {}: {e}", w.time.tick);
    }
}

/// Unit road pieces `(p, q, highway)` with `p < q`.
pub fn pieces(w: &World) -> BTreeSet<(Cell, Cell, bool)> {
    let mut out = BTreeSet::new();
    for (s, seg) in w.roads.segments.iter() {
        let (pa, pb) = w.roads.segment_ends(s);
        let line = Cell::from_pos(pa)
            .straight_line_to(Cell::from_pos(pb))
            .expect("straight");
        for p in line.windows(2) {
            let (a, b) = if p[0] <= p[1] { (p[0], p[1]) } else { (p[1], p[0]) };
            out.insert((a, b, seg.kind == RoadKind::Highway));
        }
    }
    out
}

/// A bulldoze removed exactly the street pieces with an end in the
/// rectangle and the buildings overlapping it, reported exactly the cells it
/// cleared, and left grid road cells matching the remaining pieces.
pub fn check_bulldoze(before: &World, after: &World, rect: CellRect, cells: &[Cell]) {
    let Some(rect) = rect.clamp_to_map() else { return };
    let old = pieces(before);
    let want: BTreeSet<_> = old
        .iter()
        .copied()
        .filter(|&(a, b, hw)| hw || !(rect.contains(a) || rect.contains(b)))
        .collect();
    let got = pieces(after);
    assert_eq!(got, want, "bulldoze {rect:?} left the wrong pieces");
    let mut reported: BTreeSet<Cell> = old
        .difference(&want)
        .flat_map(|&(a, b, _)| [a, b])
        .filter(|&x| rect.contains(x))
        .collect();
    for b in before.buildings.values().filter(|b| b.footprint.intersects(&rect)) {
        reported.extend(b.footprint.cells());
        assert!(!after.buildings.values().any(|a| a == b), "{b:?} survived");
    }
    assert_eq!(cells, reported.into_iter().collect::<Vec<_>>().as_slice());
    after.roads.validate().unwrap();
    let covered: BTreeSet<Cell> = got.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    for (i, cd) in after.grid.cells.iter().enumerate() {
        let cell = c(i as i32 % MAP_CELLS, i as i32 / MAP_CELLS);
        assert_eq!(
            cd.occupant == Occupant::Road,
            covered.contains(&cell),
            "{cell:?} after {rect:?}"
        );
    }
}

/// Road graph valid and the grid's road cells exactly the cells roads pass
/// (`check_invariants` checks this too, but slowly on a dense map).
pub fn check_roads(w: &World) {
    w.roads.validate().unwrap();
    let covered: BTreeSet<Cell> = pieces(w).into_iter().flat_map(|(a, b, _)| [a, b]).collect();
    for (i, cd) in w.grid.cells.iter().enumerate() {
        let cell = c(i as i32 % MAP_CELLS, i as i32 / MAP_CELLS);
        assert_eq!(cd.occupant == Occupant::Road, covered.contains(&cell), "{cell:?}");
    }
}
