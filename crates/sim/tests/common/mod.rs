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
    // Households: a car on the road is driven by a member; no more cars
    // out (on the road, waiting to start or parked) than it owns.
    for (hid, h) in w.households.iter() {
        assert!(h.members >= 1, "tick {tick}: {hid:?} is empty");
        if let Some(v) = h.vehicle {
            let driver = w.traffic.trips.get(v).and_then(|t| t.citizen);
            assert!(
                driver.is_some_and(|d| h.citizens.contains(&d)),
                "tick {tick}: {hid:?} car {v:?} not driven by a member"
            );
        }
        let waiting = w.traffic.pending.iter().filter(|d| d.household == Some(hid)).count();
        let cars_out = h.citizens.iter().filter(|&&c| w.citizens[c].has_car).count();
        assert!(
            cars_out <= h.cars as usize,
            "tick {tick}: {hid:?} {cars_out} cars out of {}",
            h.cars
        );
        assert!(
            waiting <= h.cars as usize + 1,
            "tick {tick}: {hid:?} has {waiting} departures for {} cars",
            h.cars
        );
    }
    // Every citizen is in exactly one place: inside an existing building,
    // or on a trip (in a car on the road, waiting for it, or off screen).
    for (cid, c) in w.citizens.iter() {
        match &c.location {
            Location::In(b) => assert!(
                w.buildings.contains_key(*b),
                "tick {tick}: {cid:?} in a missing building"
            ),
            Location::Travelling(j) => {
                let on_road = j.vehicle.is_some_and(|v| w.vehicles.contains_key(v));
                let waiting = w.traffic.pending.iter().any(|d| d.citizen == Some(cid));
                let off_screen = j.arrive_tick.is_some();
                let riding = j.mode == TravelMode::Passenger;
                assert!(
                    on_road || waiting || off_screen || riding,
                    "tick {tick}: {cid:?} travelling nowhere: {j:?}"
                );
                assert!(
                    !(on_road && off_screen),
                    "tick {tick}: {cid:?} both on the road and off screen"
                );
            }
        }
    }
    // Buildings: footprint cells point back and stay zoned for the kind.
    for (bid, b) in w.buildings.iter() {
        assert!(b.level >= 1 && b.capacity > 0, "tick {tick}: {bid:?} {b:?}");
        for cell in b.footprint.cells() {
            let cd = w.zoning.cell(cell).expect("footprint cell exists");
            assert_eq!(cd.building, Some(bid), "tick {tick}: {cell:?}");
            assert_eq!(cd.zone, Some(b.kind), "tick {tick}: {cell:?} zone under {bid:?}");
        }
    }
    // Every building cell belongs to a live building.
    let building_cells: usize = w.buildings.values().map(|b| (b.size().0 * b.size().1) as usize).sum();
    let marked = w.zoning.cells().filter(|(_, cd)| cd.building.is_some()).count();
    assert_eq!(building_cells, marked, "tick {tick}: stray building cells");
    // Stats match the state.
    let s = &w.stats;
    assert_eq!(s.households as usize, w.households.len());
    assert_eq!(s.buildings as usize, w.buildings.len());
    assert_eq!(s.vehicles_on_road as usize, w.vehicles.len());
    let pop: u32 = w.households.values().map(|h| h.members as u32).sum();
    assert_eq!(s.population, pop);
    assert_eq!(s.population as usize, w.citizens.len());
    let wf = w.citizens.values().filter(|c| c.in_workforce()).count();
    assert_eq!(s.workforce as usize, wf);
    assert_eq!(s.employed + s.unemployed, s.workforce);
    let metres: f64 = w.roads.segments.values().map(|s| s.length as f64).sum();
    assert_eq!(
        s.road_cells,
        (metres / CELL_SIZE as f64).round() as u32,
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
        assert!(w.grid_view().is_road(c(x, 128)), "tick {tick}: highway cell {x} lost");
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
            for (d, &have) in seg.load.iter().enumerate() {
                let want = load.get(&(s, d)).copied().unwrap_or(0.0);
                assert_eq!(have, want, "tick {tick}: {s:?} load[{d}]");
            }
        }
    }
}

/// Which top-level parts of two worlds differ (for failure messages).
pub fn diff(a: &World, b: &World) -> String {
    let mut out = Vec::new();
    macro_rules! cmp {
        ($($f:ident),*) => {$(
            if a.$f != b.$f {
                out.push(stringify!($f));
            }
        )*};
    }
    cmp!(
        seed, rng, time, roads, zoning, buildings, households, vehicles, economy, demand, stats, versions, traffic,
        growth, citizens, people
    );
    if a.people.stats != b.people.stats {
        out.push("people.stats");
    }
    if a.people.places != b.people.places {
        out.push("people.places");
    }
    format!("differ: {out:?}")
}

/// Heavier structural checks (road graph, grid vs graph).
pub fn check_full(w: &World) {
    if let Err(e) = w.check_invariants() {
        panic!("invariants at tick {}: {e}", w.time.tick);
    }
}

/// Unit road pieces `(p, q, highway)` with `p < q`. A segment ending
/// between two cell centres (where crossing diagonals meet) counts as the
/// whole piece it is part of.
pub fn pieces(w: &World) -> BTreeSet<(Cell, Cell, bool)> {
    let mut out = BTreeSet::new();
    for (s, seg) in w.roads.segments.iter() {
        let (pa, pb) = w.roads.segment_ends(s);
        let d = [pb[0] - pa[0], pb[1] - pa[1]];
        let step = d[0].abs().max(d[1].abs());
        let u = [d[0] / step, d[1] / step];
        let (sx, sy) = (u[0].round() as i32, u[1].round() as i32);
        // Cell centres on the line from the one at or before `pa` to the
        // one at or after `pb`.
        let first = |p: [f32; 2], back: bool| {
            let c = Cell::from_pos(p);
            if (c.center()[0] - p[0]).abs() < 1e-3 && (c.center()[1] - p[1]).abs() < 1e-3 {
                return c;
            }
            // Half way along a piece: the centre half a piece back / ahead.
            let k = if back { -0.5 } else { 0.5 };
            Cell::from_pos([
                p[0] + k * u[0] * CELL_SIZE + 1e-3 * u[0],
                p[1] + k * u[1] * CELL_SIZE + 1e-3 * u[1],
            ])
        };
        let (ca, cb) = (first(pa, true), first(pb, false));
        let _ = (sx, sy);
        let line = ca.straight_line_to(cb).expect("straight");
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
    assert!(
        got == want,
        "bulldoze {rect:?} left the wrong pieces: extra {:?}, missing {:?}",
        got.difference(&want).collect::<Vec<_>>(),
        want.difference(&got).collect::<Vec<_>>()
    );
    let mut reported: BTreeSet<Cell> = old
        .difference(&want)
        .flat_map(|&(a, b, _)| [a, b])
        .filter(|&x| rect.contains(x))
        .collect();
    let rect_quad = {
        let (lo, hi) = (rect.min.center(), rect.max.center());
        let h = CELL_SIZE / 2.0;
        [
            [lo[0] - h, lo[1] - h],
            [hi[0] + h, lo[1] - h],
            [hi[0] + h, hi[1] + h],
            [lo[0] - h, hi[1] + h],
        ]
    };
    for b in before.buildings.values() {
        // A building on the world lattice reports its cells, one on another
        // lattice the cell under its centre.
        if b.footprint.lattice == citysim_sim::LatticeId::WORLD {
            if !b.footprint.rect().intersects(&rect) {
                continue;
            }
            reported.extend(b.footprint.rect().cells());
        } else {
            if !convex_quads_overlap(&b.corners(), &rect_quad) {
                continue;
            }
            reported.insert(Cell::from_pos(b.center()));
        }
        assert!(!after.buildings.values().any(|a| a == b), "{b:?} survived");
    }
    assert_eq!(cells, reported.into_iter().collect::<Vec<_>>().as_slice());
    after.roads.validate().unwrap();
    let covered: BTreeSet<Cell> = got.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    let view = after.grid_view();
    for i in 0..MAP_CELLS * MAP_CELLS {
        let cell = c(i % MAP_CELLS, i / MAP_CELLS);
        assert_eq!(view.is_road(cell), covered.contains(&cell), "{cell:?} after {rect:?}");
    }
}

/// Two convex quads overlap by more than touching (separating axes, as
/// the bulldozer decides).
fn convex_quads_overlap(a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]) -> bool {
    for q in [a, b] {
        for i in 0..4 {
            let (p, r) = (q[i], q[(i + 1) % 4]);
            let l = ((r[0] - p[0]).powi(2) + (r[1] - p[1]).powi(2)).sqrt().max(1e-6);
            let n = [(p[1] - r[1]) / l, (r[0] - p[0]) / l];
            let proj = |x: &[[f32; 2]; 4]| {
                x.iter()
                    .map(|v| v[0] * n[0] + v[1] * n[1])
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), d| (lo.min(d), hi.max(d)))
            };
            let ((a0, a1), (b0, b1)) = (proj(a), proj(b));
            if a1 <= b0 + 0.04 || b1 <= a0 + 0.04 {
                return false;
            }
        }
    }
    true
}

/// Road graph valid and the grid's road cells exactly the cells roads pass
/// (`check_invariants` checks this too, but slowly on a dense map).
pub fn check_roads(w: &World) {
    w.roads.validate().unwrap();
    let covered: BTreeSet<Cell> = pieces(w).into_iter().flat_map(|(a, b, _)| [a, b]).collect();
    let view = w.grid_view();
    for i in 0..MAP_CELLS * MAP_CELLS {
        let cell = c(i % MAP_CELLS, i / MAP_CELLS);
        assert_eq!(view.is_road(cell), covered.contains(&cell), "{cell:?}");
    }
}
