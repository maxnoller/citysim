//! Roundabouts (`Command::SetRoundabout`): geometry at odd junctions, which
//! sizes fit and why not, the rules of the ring (no overlapping cars, no
//! deadlock when saturated), money, zoning, saves, undo and determinism.

mod common;

use citysim_sim::capacity::{Bench, run_traced};
use citysim_sim::junction::{self, JunctionControl};
use citysim_sim::roundabout::{self, RingProblem, RoundaboutSize as R};
use citysim_sim::*;
use common::{DAY, HOUR};

fn at(o: [f32; 2], dx: f32, dy: f32) -> [f32; 2] {
    [o[0] + dx, o[1] + dy]
}

fn dir(deg: f32) -> [f32; 2] {
    [libm::cosf(deg.to_radians()), libm::sinf(deg.to_radians())]
}

fn new_world() -> World {
    let mut w = World::new(3);
    w.economy.money = 10_000_000;
    w
}

/// Roads from `hub` out along the given angles (degrees), `len` metres.
fn star(w: &mut World, hub: [f32; 2], arms: &[(f32, RoadKind)], len: f32) -> NodeId {
    for &(deg, kind) in arms {
        let d = dir(deg);
        let c = Curve::straight(hub, at(hub, d[0] * len, d[1] * len));
        w.apply(Command::BuildCurve { curve: c, kind }).expect("road");
    }
    w.roads.node_at(hub).expect("hub node")
}

fn set(w: &mut World, n: NodeId, size: R) -> Result<Preview, CommandError> {
    let p = w.preview(&Command::SetRoundabout {
        node: n,
        size: Some(size),
    });
    let a = w.apply(Command::SetRoundabout {
        node: n,
        size: Some(size),
    });
    assert_eq!(
        p.is_ok(),
        a.is_ok(),
        "preview agrees with apply ({size:?}: {p:?} / {a:?})"
    );
    if let (Ok(p), Ok(a)) = (&p, &a) {
        assert_eq!(p.cost, a.cost);
        assert_eq!(p.buildings, a.buildings);
    }
    a
}

/// Every path of a ring: continuous, outside the island, circulating one
/// way while on the ring, from the arm's give-way line to its exit.
fn check_paths(w: &World, n: NodeId) {
    let j = junction::junction(w, n).expect("junction");
    let ring = roundabout::ring(w, n).expect("ring");
    assert_eq!(j.control, JunctionControl::Roundabout);
    let c = ring.center;
    for m in &j.movements {
        let p = m.ring_path().expect("a ring path");
        let pts = p.points(0.25);
        for w2 in pts.windows(2) {
            assert!(geom::dist(w2[0], w2[1]) < 0.4, "path jumps: {:?}", w2);
        }
        for q in &pts {
            assert!(
                geom::dist(*q, c) > ring.dims.island_r + 1.0,
                "path over the island: {q:?} ({:?} arm {} -> {})",
                ring.size,
                m.from_arm,
                m.to_arm
            );
        }
        // On the ring the island is on the left: the cross product of the
        // radius and the direction is negative (sim axes).
        let mut d = p.entry_len + 0.5;
        while d < p.entry_len + p.arc_len - 0.5 {
            let (q, t) = p.pose_at(d);
            let r = geom::sub(q, c);
            assert!(geom::cross(r, t) < 0.0, "runs the wrong way round");
            d += 2.0;
        }
        // Starts at the give-way line of its arm, ends at its exit's cut.
        let arm = &j.arms[m.from_arm as usize];
        let seg = &w.roads.segments[arm.segment];
        let line = seg.pose_dir(seg.a == n, arm.setback, 0.0).pos;
        assert!(geom::dist(pts[0], line) < 6.0);
        let out = &j.arms[m.to_arm as usize];
        let seg = &w.roads.segments[out.segment];
        let cut = seg.pose_dir(seg.a == n, out.setback, 0.0).pos;
        assert!(geom::dist(*pts.last().unwrap(), cut) < 6.0);
        // Every turn and U-turn exists from every arm.
    }
    for a in 0..j.arms.len() {
        for b in 0..j.arms.len() {
            assert!(j.movements_between(a, b).next().is_some(), "no way from arm {a} to {b}");
        }
    }
    // The arms end at the ring.
    for (arm, ra) in j.arms.iter().zip(&ring.arms) {
        assert_eq!(arm.setback, ra.setback);
        assert!(arm.setback > ring.dims.outer_r);
    }
    // The surface outline reaches round the ring.
    let sh = junction::junction_shape(w, n, 0.0).expect("shape");
    let far = sh.outline.iter().map(|p| geom::dist(*p, c)).fold(0.0f32, f32::max);
    assert!(far >= ring.dims.outer_r - 0.1);
}

#[test]
fn a_street_crossing_takes_every_size_but_avenues_not_the_mini() {
    let mut w = new_world();
    let hub = [1000.0, 1000.0];
    let n = star(
        &mut w,
        hub,
        &[
            (0.0, RoadKind::Street),
            (90.0, RoadKind::Street),
            (180.0, RoadKind::Street),
            (270.0, RoadKind::Street),
        ],
        150.0,
    );
    for s in R::ALL {
        set(&mut w, n, s).expect("fits");
        check_paths(&w, n);
    }
    let mut w = new_world();
    let n = star(
        &mut w,
        hub,
        &[
            (0.0, RoadKind::Avenue),
            (90.0, RoadKind::Street),
            (180.0, RoadKind::Avenue),
            (270.0, RoadKind::Street),
        ],
        150.0,
    );
    assert_eq!(
        set(&mut w, n, R::Mini).unwrap_err(),
        CommandError::Roundabout(RingProblem::AvenueOnMini)
    );
    for s in [R::Medium, R::Large] {
        set(&mut w, n, s).expect("fits");
        check_paths(&w, n);
    }
}

#[test]
fn odd_junctions_three_and_five_arms_and_a_sharp_meeting() {
    // A T of streets.
    let mut w = new_world();
    let n = star(
        &mut w,
        [600.0, 600.0],
        &[
            (0.0, RoadKind::Street),
            (90.0, RoadKind::Street),
            (180.0, RoadKind::Street),
        ],
        120.0,
    );
    set(&mut w, n, R::Mini).expect("a T takes a mini roundabout");
    check_paths(&w, n);
    // Five streets at uneven angles.
    let mut w = new_world();
    let arms: Vec<(f32, RoadKind)> = [0.0, 80.0, 150.0, 215.0, 290.0]
        .iter()
        .map(|&d| (d, RoadKind::Street))
        .collect();
    let n = star(&mut w, [900.0, 900.0], &arms, 120.0);
    assert!(matches!(
        set(&mut w, n, R::Mini),
        Err(CommandError::Roundabout(RingProblem::ArmsTooClose { .. }))
    ));
    set(&mut w, n, R::Small).expect("five arms on a small ring");
    check_paths(&w, n);
    // Two streets meeting at 30 degrees (and a third road).
    let mut w = new_world();
    let n = star(
        &mut w,
        [1200.0, 700.0],
        &[
            (0.0, RoadKind::Street),
            (30.0, RoadKind::Street),
            (180.0, RoadKind::Street),
        ],
        120.0,
    );
    let sizes = roundabout::sizes(&w, n);
    let ok: Vec<R> = sizes.iter().filter(|s| s.1.is_ok()).map(|s| s.0).collect();
    for (s, r) in &sizes {
        if let Err(p) = r {
            assert!(
                matches!(p, RingProblem::ArmsTooClose { degrees, .. } if (degrees - 30.0).abs() < 8.0),
                "{s:?}: {p:?}"
            );
        }
    }
    assert!(
        !ok.is_empty() && !ok.contains(&R::Mini),
        "only bigger rings fit a 30° meeting: {ok:?}"
    );
    set(&mut w, n, *ok.first().unwrap()).unwrap();
    check_paths(&w, n);
    // Mixed kinds: an avenue ending at a street.
    let mut w = new_world();
    let n = star(
        &mut w,
        [500.0, 1500.0],
        &[
            (0.0, RoadKind::Street),
            (90.0, RoadKind::Avenue),
            (180.0, RoadKind::Street),
        ],
        120.0,
    );
    set(&mut w, n, R::Medium).expect("avenue T");
    check_paths(&w, n);
}

#[test]
fn a_ring_never_swallows_its_neighbour_and_refuses_with_a_reason() {
    let mut w = new_world();
    // A crossing with another junction 30 m along one arm.
    let n = star(
        &mut w,
        [800.0, 800.0],
        &[
            (0.0, RoadKind::Street),
            (90.0, RoadKind::Street),
            (180.0, RoadKind::Street),
            (270.0, RoadKind::Street),
        ],
        150.0,
    );
    w.apply(Command::BuildCurve {
        curve: Curve::straight([830.0, 700.0], [830.0, 900.0]),
        kind: RoadKind::Street,
    })
    .unwrap();
    let sizes = roundabout::sizes(&w, n);
    assert!(sizes[R::Mini as usize].1.is_ok(), "{sizes:?}");
    assert!(
        matches!(sizes[R::Large as usize].1, Err(RingProblem::NeighbourTooClose { .. })),
        "{sizes:?}"
    );
    // Two arms: no roundabout; and nothing to take away.
    let mut w = new_world();
    w.apply(Command::BuildCurve {
        curve: Curve::straight([400.0, 400.0], [500.0, 400.0]),
        kind: RoadKind::Street,
    })
    .unwrap();
    w.apply(Command::BuildCurve {
        curve: Curve::straight([500.0, 400.0], [500.0, 500.0]),
        kind: RoadKind::Street,
    })
    .unwrap();
    let bend = w.roads.node_at([500.0, 400.0]).unwrap();
    assert_eq!(
        w.preview(&Command::SetRoundabout {
            node: bend,
            size: Some(R::Small)
        })
        .unwrap_err(),
        CommandError::Roundabout(RingProblem::TooFewArms)
    );
    assert_eq!(
        w.preview(&Command::SetRoundabout { node: bend, size: None })
            .unwrap_err(),
        CommandError::Roundabout(RingProblem::NotARoundabout)
    );
    // The highway's end.
    let w = new_world();
    let entry_end = w
        .roads
        .nodes
        .keys()
        .find(|&n| Some(n) != w.roads.entry)
        .expect("the stub's inner end");
    assert!(
        w.preview(&Command::SetRoundabout {
            node: entry_end,
            size: Some(R::Large)
        })
        .is_err()
    );
}

#[test]
fn money_upkeep_and_removal() {
    let mut w = new_world();
    let n = star(
        &mut w,
        [700.0, 1300.0],
        &[
            (0.0, RoadKind::Street),
            (120.0, RoadKind::Street),
            (240.0, RoadKind::Street),
        ],
        120.0,
    );
    let before = w.economy.money;
    let p = set(&mut w, n, R::Small).unwrap();
    assert_eq!(p.cost, R::Small.cost());
    assert_eq!(w.economy.money, before - R::Small.cost());
    // Undo gives the ring's price back once (`Refund`, as for roads).
    assert_eq!(p.road_cost, R::Small.cost());
    let tick = w.time.tick;
    w.apply(Command::Refund {
        tick,
        amount: p.road_cost,
    })
    .expect("refund");
    assert_eq!(w.economy.money, before);
    assert!(
        w.preview(&Command::Refund {
            tick,
            amount: p.road_cost
        })
        .is_err()
    );
    w.economy.money -= R::Small.cost();
    let costs = economy::running_costs(&w);
    assert_eq!(costs.roundabouts, R::Small.upkeep_per_day());
    assert_eq!(costs.roundabout_count, 1);
    assert!(R::Large.upkeep_per_day() < balance::SIGNAL_UPKEEP_PER_DAY);
    // Removal refunds nothing and is never refused for money.
    w.economy.money = -5_000;
    w.apply(Command::SetRoundabout { node: n, size: None }).unwrap();
    assert_eq!(w.economy.money, -5_000);
    assert!(roundabout::roundabout(&w, n).is_none());
    assert_ne!(junction::junction_control(&w, n), Some(JunctionControl::Roundabout));
    // Not affordable: refused.
    assert!(matches!(
        w.preview(&Command::SetRoundabout {
            node: n,
            size: Some(R::Large)
        }),
        Err(CommandError::InsufficientFunds { .. })
    ));
}

/// Car bodies overlapping by more than 5 cm, counted every half second.
fn bench_overlaps(b: &Bench, secs: u32) -> (u32, f32, u32, f32) {
    let mut overlaps = 0;
    let mut gave_up = 0;
    let r = run_traced(b, 5, 30, secs, &mut |w, _, t| {
        if t % 10 == 0 {
            let bodies = traffic::car_bodies(w);
            overlaps += traffic::overlapping_bodies(&bodies, 0.05).len() as u32;
        }
        gave_up = w.traffic.deadlocks_resolved;
    });
    (overlaps, r.per_minute, gave_up, r.gave_up as f32)
}

#[test]
fn saturated_rings_neither_overlap_nor_lock_up() {
    let cases = vec![
        Bench::crossing(RoadKind::Street, RoadKind::Street, JunctionSetting::Priority)
            .turning(0.25, 0.25)
            .roundabout(R::Mini),
        Bench::crossing(RoadKind::Street, RoadKind::Street, JunctionSetting::Priority)
            .turning(0.15, 0.15)
            .roundabout(R::Small),
        Bench::crossing(RoadKind::Avenue, RoadKind::Street, JunctionSetting::Priority)
            .turning(0.15, 0.15)
            .roundabout(R::Medium),
        Bench::crossing(RoadKind::Avenue, RoadKind::Avenue, JunctionSetting::Priority)
            .turning(0.25, 0.25)
            .roundabout(R::Large),
    ];
    let res = common::par_map(cases, |b| (b.ring, b.main, bench_overlaps(&b, 240)));
    for (size, main, (overlaps, per_min, deadlocks, _)) in res {
        assert_eq!(overlaps, 0, "{size:?} on {main:?}: cars overlap");
        assert_eq!(deadlocks, 0, "{size:?} on {main:?}: a car stuck on the ring");
        assert!(per_min > 25.0, "{size:?} on {main:?}: only {per_min} cars a minute");
    }
}

#[test]
fn rings_carry_more_than_give_way_and_signals_at_a_street_crossing() {
    let sat = |s: JunctionSetting, r: Option<R>| {
        let mut b = Bench::crossing(RoadKind::Street, RoadKind::Street, s).turning(0.15, 0.15);
        if let Some(r) = r {
            b = b.roundabout(r);
        }
        b
    };
    let res = common::par_map(
        vec![
            sat(JunctionSetting::Priority, None),
            sat(JunctionSetting::Signal, None),
            sat(JunctionSetting::Priority, Some(R::Small)),
        ],
        |b| citysim_sim::capacity::run(&b, 7, 60, 300).per_minute,
    );
    assert!(res[2] > res[0] * 1.2 && res[2] > res[1] * 1.2, "{res:?}");
}

/// Entering cars give way: when a car enters the ring, any car on the
/// ring heading through its merge is more than a second away from it (at
/// its speed).
#[test]
fn entering_cars_give_way_to_the_ring() {
    let b = Bench::crossing(RoadKind::Street, RoadKind::Street, JunctionSetting::Priority)
        .turning(0.2, 0.2)
        .roundabout(R::Small);
    let mut seen: std::collections::BTreeSet<VehicleId> = Default::default();
    let mut checked = 0;
    let mut worst = f32::INFINITY;
    run_traced(&b, 9, 0, 240, &mut |w, n, _| {
        let Some(n) = n else { return };
        let Some(j) = junction::junction(w, n) else { return };
        for (id, v) in w.vehicles.iter() {
            let Some(jm) = v.junction else { continue };
            if !seen.insert(id) {
                continue;
            }
            // Just entered: the merge point on its path.
            let m = &j.movements[jm.movement as usize];
            let p = m.ring_path().unwrap();
            let merge = p.pose_at(p.entry_len).0;
            checked += 1;
            for (oid, o) in w.vehicles.iter() {
                let Some(ojm) = o.junction else { continue };
                if oid == id || ojm.from == jm.from {
                    continue;
                }
                let om = &j.movements[ojm.movement as usize];
                let op = om.ring_path().unwrap();
                // Distance along its way to the merge point, if it passes it.
                let mut d = ojm.distance;
                let mut found = None;
                while d < op.total {
                    if geom::dist(op.pose_at(d).0, merge) < 1.5 {
                        found = Some(d - ojm.distance);
                        break;
                    }
                    d += 0.5;
                }
                if let Some(ahead) = found.filter(|&a| a > 2.5)
                    && op.ring_lane == p.ring_lane
                {
                    worst = worst.min((ahead - 2.2) / o.speed.max(0.5));
                }
            }
        }
    });
    assert!(checked > 50);
    assert!(worst > 1.0, "a car entered {worst:.2} s ahead of a car on the ring");
}

#[test]
fn same_seed_same_ring_traffic() {
    let b = Bench::crossing(RoadKind::Avenue, RoadKind::Street, JunctionSetting::Priority)
        .turning(0.2, 0.2)
        .roundabout(R::Medium);
    let sums = common::par_map([0, 1], |_| {
        let mut sum = 0u64;
        run_traced(&b, 11, 0, 120, &mut |w, _, t| {
            if t % 200 == 199 {
                for x in w.save() {
                    sum = sum.wrapping_mul(0x100_0000_01b3) ^ x as u64;
                }
            }
        });
        sum
    });
    assert_eq!(sums[0], sums[1]);
}

/// The two-district town, grown a day and a half.
fn town() -> World {
    let mut w = World::new(5);
    scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, true));
    while w.time.tick < DAY + 6 * HOUR {
        w.step();
    }
    w
}

/// A junction of the town and the biggest ring that fits it and takes
/// buildings down.
fn ring_with_buildings(w: &World) -> Option<(NodeId, R, Preview)> {
    let mut best: Option<(NodeId, R, Preview)> = None;
    for n in junction::junction_nodes(w).collect::<Vec<_>>() {
        for s in R::ALL {
            if let Ok(p) = w.preview(&Command::SetRoundabout { node: n, size: Some(s) })
                && !p.buildings.is_empty()
                && best.as_ref().is_none_or(|b| p.buildings.len() > b.2.buildings.len())
            {
                best = Some((n, s, p));
            }
        }
    }
    best
}

#[test]
fn a_ring_takes_its_land_and_undo_brings_the_buildings_back() {
    let mut w = town();
    let (n, size, p) = ring_with_buildings(&w).expect("a junction where a ring takes buildings");
    let records: Vec<BuildingRecord> = p.buildings.iter().filter_map(|&b| w.building_record(b)).collect();
    let pop = w.stats.population;
    let money = w.economy.money;
    let a = w
        .apply(Command::SetRoundabout {
            node: n,
            size: Some(size),
        })
        .unwrap();
    assert_eq!(a.buildings, p.buildings);
    for b in &p.buildings {
        assert!(
            !w.buildings.contains_key(*b),
            "building on the ring's land still stands"
        );
    }
    assert!(w.stats.population < pop);
    assert!(w.economy.money < money);
    // Nothing stands or is zoned on the ring's land any more.
    let ring = roundabout::ring(&w, n).unwrap();
    let r = ring.dims.reserve_r();
    for (c, z) in w.zoning.cells() {
        let ctr = w.zoning.cell_center(c).unwrap();
        if geom::dist(ctr, ring.center) < r - 4.0 {
            assert!(!z.usable(), "usable cell under the ring at {ctr:?}");
            assert!(z.building.is_none());
        }
    }
    for b in w.buildings.values() {
        assert!(geom::dist(b.center(), ring.center) > r - 2.0);
    }
    common::check_consistency(&w, false);
    // Undo: the junction back, the lots zoned again (land under a ring loses
    // its zone, as under a road), then the buildings.
    w.apply(Command::SetRoundabout { node: n, size: None }).unwrap();
    for rec in &records {
        let cells: Vec<CellRef> = rec.cells.iter().filter_map(|&(_, p)| w.zoning.cell_at(p)).collect();
        w.apply(Command::ZoneCells {
            cells,
            zone: Some(rec.building.kind),
        })
        .unwrap();
    }
    for rec in records {
        w.apply(Command::RestoreBuilding(Box::new(rec)))
            .expect("building comes back");
    }
    assert!(w.stats.population >= pop - 4, "{} of {pop}", w.stats.population);
    common::check_consistency(&w, false);
}

#[test]
fn saves_keep_the_rings_and_continue_in_lockstep() {
    let b = Bench::crossing(RoadKind::Avenue, RoadKind::Street, JunctionSetting::Signal)
        .turning(0.2, 0.2)
        .roundabout(R::Large);
    let mut saved = None;
    run_traced(&b, 4, 0, 60, &mut |w, _, t| {
        if t == 60 * 20 - 1 {
            saved = Some(w.save());
        }
    });
    let bytes = saved.unwrap();
    let mut a = World::load(&bytes).expect("loads");
    let mut b2 = World::load(&bytes).expect("loads");
    let n = junction::junction_nodes(&a).find(|&n| roundabout::roundabout(&a, n).is_some());
    let n = n.expect("the ring survives a save");
    assert_eq!(roundabout::roundabout(&a, n), Some(R::Large));
    assert_eq!(junction::junction(&a, n).unwrap().setting, JunctionSetting::Signal);
    assert!(
        a.vehicles.values().any(|v| v.junction.is_some_and(|j| j.node == n)),
        "cars on the ring"
    );
    // A save of the loaded world is the same save.
    assert_eq!(a.save(), bytes);
    for _ in 0..400 {
        a.step();
        b2.step();
    }
    assert_eq!(a.save(), b2.save());
    // And the ring keeps carrying cars without overlaps.
    let bodies = traffic::car_bodies(&a);
    assert!(traffic::overlapping_bodies(&bodies, 0.05).is_empty());
}

#[test]
fn a_road_edit_that_leaves_no_room_makes_it_an_ordinary_junction() {
    let mut w = new_world();
    let n = star(
        &mut w,
        [1000.0, 1000.0],
        &[
            (0.0, RoadKind::Street),
            (90.0, RoadKind::Street),
            (180.0, RoadKind::Street),
            (270.0, RoadKind::Street),
        ],
        150.0,
    );
    set(&mut w, n, R::Large).unwrap();
    // A new street across an arm right by the ring.
    w.apply(Command::BuildCurve {
        curve: Curve::straight([1040.0, 900.0], [1040.0, 1100.0]),
        kind: RoadKind::Street,
    })
    .unwrap();
    assert!(roundabout::roundabout(&w, n).is_none());
    assert_ne!(junction::junction_control(&w, n), Some(JunctionControl::Roundabout));
}
