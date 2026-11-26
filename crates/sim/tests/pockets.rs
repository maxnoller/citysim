//! Turn pockets: avenue approaches get a short left-turn lane in the median
//! before a junction (`junction::turn_pocket`). Left turners wait there out
//! of the through lanes, signals give them their own phase when the pocket
//! fills, and a full pocket spills back into the inner lane.

use citysim_sim::capacity::{self, Bench, run, run_traced};
use citysim_sim::junction::{self, Turn};
use citysim_sim::{Cell, JunctionSetting as S, RoadKind as K, World, balance, drive, scenarios};

fn c(x: i32, y: i32) -> Cell {
    Cell::new(x, y)
}

fn pockets_of(w: &World) -> Vec<junction::TurnPocket> {
    let mut out = Vec::new();
    for (s, _) in w.roads.segments.iter() {
        for fwd in [true, false] {
            out.extend(junction::turn_pocket(w, s, fwd));
        }
    }
    out
}

#[test]
fn avenue_approaches_get_pockets_streets_and_short_blocks_do_not() {
    let mut w = World::new(1);
    scenarios::prebuilt(&mut w, |w| {
        // A long avenue crossed by an avenue, a street, and a street only
        // 3 cells (24 m) after it: no room for a pocket on that block.
        assert!(scenarios::avenue(w, (11, 128), (90, 128)));
        assert!(scenarios::avenue(w, (40, 110), (40, 146)));
        assert!(scenarios::road(w, (60, 110), (60, 146)));
        assert!(scenarios::road(w, (63, 110), (63, 146)));
        assert!(scenarios::road(w, (30, 140), (50, 140)));
    });
    w.step();
    let pk = pockets_of(&w);
    assert!(!pk.is_empty());
    for p in &pk {
        let seg = &w.roads.segments[p.segment];
        assert_eq!(seg.kind, K::Avenue, "pockets only on avenues");
        let len = p.stop_m - p.start_m;
        assert!(
            (balance::POCKET_MIN_M - 1e-3..=balance::POCKET_MAX_M + 1e-3).contains(&len),
            "{len}"
        );
        // It ends at the stop line of a real junction.
        let (_, stop) = drive::drivable_range(&w, p.segment, p.forward).unwrap();
        assert!((stop - p.stop_m).abs() < 1e-3);
        assert!(junction::junction(&w, p.node).is_some());
        // In the median, clear of the inner lane.
        assert!(p.offset_m.abs() + 0.95 < K::Avenue.lane_offset_m(1) - 0.95 + 0.5);
    }
    // Every avenue approach to the two avenue x road crossings with room.
    let at = |x: i32, y: i32| w.roads.node_at(c(x, y).center()).unwrap();
    let n40 = at(40, 128);
    assert_eq!(pk.iter().filter(|p| p.node == n40).count(), 4, "{pk:?}");
    let n60 = at(60, 128);
    assert_eq!(
        pk.iter().filter(|p| p.node == n60).count(),
        1,
        "the block from 63 is too short"
    );
    // The two pockets of one segment never overlap.
    for (s, seg) in w.roads.segments.iter() {
        if let (Some(f), Some(b)) = (junction::turn_pocket(&w, s, true), junction::turn_pocket(&w, s, false)) {
            let (f0, _) = f.from_a(seg.length);
            let (_, b1) = b.from_a(seg.length);
            assert!(b1 + balance::POCKET_GAP_M - 1e-3 <= f0, "{b1} {f0}");
        }
    }
    // A street crossing has none.
    let n_street = at(50, 140);
    assert!(pk.iter().all(|p| p.node != n_street));
}

#[test]
fn turning_traffic_no_longer_chokes_an_avenue_crossing() {
    let straight = run(&Bench::crossing(K::Avenue, K::Avenue, S::Signal), 7, 120, 300);
    let turning = run(
        &Bench::crossing(K::Avenue, K::Avenue, S::Signal).turning(0.1, 0.1),
        7,
        120,
        300,
    );
    let street = run(
        &Bench::crossing(K::Street, K::Street, S::Signal).turning(0.1, 0.1),
        7,
        120,
        300,
    );
    eprintln!(
        "avenue crossing straight {:.1}/min, 20 % turning {:.1}/min; street crossing 20 % turning {:.1}/min",
        straight.per_minute, turning.per_minute, street.per_minute
    );
    assert!(turning.per_minute >= 105.0, "{}", turning.per_minute);
    assert!(turning.per_minute > 2.5 * street.per_minute);
    // Turning still costs something (not free), and every arm moves.
    assert!(turning.per_minute < straight.per_minute);
    assert!(
        turning.by_arm.iter().all(|&a| a > 0.15 * turning.per_minute),
        "{:?}",
        turning.by_arm
    );
    assert!(turning.by_turn[2] > 5.0, "lefts get through: {:?}", turning.by_turn);
}

#[test]
fn left_turners_use_the_pocket_and_a_full_one_spills_back() {
    // Heavy left turns: pockets fill, the next left turner waits in the
    // inner lane at the pocket's entry.
    let b = Bench::crossing(K::Avenue, K::Avenue, S::Signal).turning(0.3, 0.0);
    let (lane_of_lefts, mut waiting_at_entry, mut pocket_full) = (std::cell::RefCell::new([0u32; 3]), 0u32, 0u32);
    let mut seen: Vec<citysim_sim::VehicleId> = Vec::new();
    let r = run_traced(&b, 3, 120, 240, &mut |w, node, t| {
        let Some(n) = node else { return };
        if t < 120 * citysim_sim::TICKS_PER_SECOND {
            return;
        }
        for (id, v) in w.vehicles.iter() {
            if let Some(jm) = v.junction.filter(|jm| jm.node == n && jm.turn == Turn::Left)
                && jm.distance < 1.0
                && !seen.contains(&id)
            {
                seen.push(id);
                lane_of_lefts.borrow_mut()[(jm.from_lane as usize).min(2)] += 1;
            }
        }
        if t % 20 != 0 {
            return;
        }
        for (s, _) in w.roads.segments.iter() {
            for fwd in [true, false] {
                let Some(p) = junction::turn_pocket(w, s, fwd).filter(|p| p.node == n) else {
                    continue;
                };
                let inside = w
                    .vehicles
                    .values()
                    .filter(|v| v.segment == Some(s) && v.forward == fwd && v.lane == p.lane && v.junction.is_none())
                    .count();
                if inside as f32 >= ((p.stop_m - p.start_m) / balance::MIN_GAP_M).floor() - 1.0 {
                    pocket_full += 1;
                }
                let entry = p.start_m + balance::POCKET_ENTRY_M;
                waiting_at_entry += w
                    .vehicles
                    .values()
                    .filter(|v| {
                        v.segment == Some(s)
                            && v.forward == fwd
                            && v.lane + 1 == p.lane
                            && v.speed < 0.5
                            && (v.progress - entry).abs() < 1.0
                    })
                    .count() as u32;
            }
        }
    });
    let lanes = *lane_of_lefts.borrow();
    eprintln!("left entries by lane {lanes:?}, pocket full {pocket_full}, waiting at entry {waiting_at_entry}, {r:?}");
    let total: u32 = lanes.iter().sum();
    assert!(total > 20);
    assert!(
        lanes[2] as f32 > 0.8 * total as f32,
        "lefts turn from the pocket: {lanes:?}"
    );
    assert!(pocket_full > 0 && waiting_at_entry > 0, "a full pocket spills back");
    assert!(r.gave_up < 5, "{}", r.gave_up);
}

#[test]
fn straight_traffic_and_streets_are_unaffected() {
    // Pockets cost the through lanes nothing; streets have no pockets.
    let ave = run(&Bench::crossing(K::Avenue, K::Avenue, S::Signal), 7, 120, 300);
    assert!(ave.per_minute > 140.0, "{}", ave.per_minute);
    let (w, _) = capacity::build(&Bench::crossing(K::Street, K::Street, S::Signal));
    assert!(pockets_of(&w).is_empty());
    let st = run(&Bench::crossing(K::Street, K::Street, S::Signal), 7, 120, 300);
    assert!((50.0..=62.0).contains(&st.per_minute), "{}", st.per_minute);
}
