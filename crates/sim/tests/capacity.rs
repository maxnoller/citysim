//! Junction capacity, measured on the bench (`citysim_sim::capacity`): one
//! junction loaded from its approaches, cars per minute through it. Each
//! junction type and road kind must pass a plausible number of cars, and
//! the bands are checked both ways (not too few, not implausibly many).

use citysim_sim::capacity::{Bench, BenchResult, run};
use citysim_sim::{JunctionControl, JunctionSetting as S, RoadKind as K};

/// Two minutes to fill the queues, five measured.
fn bench(b: Bench) -> BenchResult {
    run(&b, 7, 120, 300)
}

#[test]
fn a_signalised_lane_passes_about_half_a_car_a_second_on_green() {
    let street = bench(Bench::crossing(K::Street, K::Street, S::Signal));
    let avenue = bench(Bench::crossing(K::Avenue, K::Avenue, S::Signal));
    let s = street.green_flow_per_lane.unwrap();
    let a = avenue.green_flow_per_lane.unwrap();
    eprintln!("per lane on green: street {s:.2}/s, avenue {a:.2}/s");
    assert!((0.28..=0.5).contains(&s), "street {s}");
    assert!((0.45..=0.75).contains(&a), "avenue {a}");
    // Streets (narrow lanes, driveways) are driven with more distance.
    assert!(a > s);
}

#[test]
fn an_all_way_stop_passes_a_car_every_few_seconds_and_serves_every_arm() {
    let straight = bench(Bench::crossing(K::Street, K::Street, S::Priority));
    let turning = bench(Bench::crossing(K::Street, K::Street, S::Priority).turning(0.15, 0.15));
    eprintln!("all-way: {straight:?}\n with turns: {turning:?}");
    assert_eq!(straight.control, Some(JunctionControl::Priority));
    assert!((20.0..=36.0).contains(&straight.per_minute), "{}", straight.per_minute);
    assert!((15.0..=32.0).contains(&turning.per_minute), "{}", turning.per_minute);
    // First come, first served: no arm waits while the others stream.
    for r in [&straight, &turning] {
        let max = r.by_arm.iter().cloned().fold(0.0f32, f32::max);
        let min = r.by_arm.iter().cloned().fold(f32::MAX, f32::min);
        assert!(min > 0.5 * max, "starved arm: {:?}", r.by_arm);
    }
}

#[test]
fn a_four_way_avenue_crossing_passes_well_over_a_hundred_cars_a_minute() {
    let r = bench(Bench::crossing(K::Avenue, K::Avenue, S::Signal));
    eprintln!("{r:?}");
    assert!(r.per_minute > 110.0, "{}", r.per_minute);
    assert!(r.per_minute < 200.0, "implausibly many: {}", r.per_minute);
    // Both avenues are served alike.
    assert!(r.main_per_minute > 0.7 * r.cross_per_minute && r.cross_per_minute > 0.7 * r.main_per_minute);
}

#[test]
fn an_avenue_carries_more_than_twice_a_street_through_a_junction() {
    // Straight through: more than twice; with a tenth of the cars turning
    // each way (left turners wait in turn pockets) clearly more than twice.
    for (left, right, factor) in [(0.0, 0.0, 2.0), (0.1, 0.1, 2.5)] {
        let street = bench(Bench::crossing(K::Street, K::Street, S::Signal).turning(left, right));
        let avenue = bench(Bench::crossing(K::Avenue, K::Avenue, S::Signal).turning(left, right));
        eprintln!(
            "turning {left}/{right}: street crossing {:.1}/min, avenue crossing {:.1}/min",
            street.per_minute, avenue.per_minute
        );
        assert!(
            avenue.per_minute > factor * street.per_minute,
            "{} vs {}",
            avenue.per_minute,
            street.per_minute
        );
    }
    // Along an avenue past a street that gives way, against a street through
    // a street junction with signals.
    let street = bench(Bench::crossing(K::Street, K::Street, S::Signal));
    let avenue = bench(Bench::crossing(K::Avenue, K::Street, S::Priority));
    eprintln!(
        "through a junction: street {:.1}/min, avenue {:.1}/min",
        street.main_per_minute, avenue.main_per_minute
    );
    assert!(avenue.main_per_minute > 2.0 * street.main_per_minute);
}

#[test]
fn a_priority_junction_lets_the_major_road_flow_and_the_side_road_in_on_gaps() {
    // A T: the through street is major, the side street gives way (streets
    // carry about 1,200 cars an hour per lane).
    let r = bench(
        Bench::crossing(K::Street, K::Street, S::Priority)
            .tee()
            .turning(0.15, 0.15),
    );
    eprintln!("{r:?}");
    assert!(r.main_per_minute > 25.0, "{}", r.main_per_minute);
    assert!(r.cross_per_minute > 5.0, "side road starved: {}", r.cross_per_minute);
    assert!(r.main_per_minute > 2.0 * r.cross_per_minute);
}

#[test]
fn turning_traffic_costs_capacity_but_never_blocks_a_junction() {
    for (main, cross) in [(K::Street, K::Street), (K::Avenue, K::Street), (K::Avenue, K::Avenue)] {
        let straight = bench(Bench::crossing(main, cross, S::Signal));
        let turning = bench(Bench::crossing(main, cross, S::Signal).turning(0.15, 0.15));
        eprintln!(
            "{main:?} x {cross:?}: {:.1}/min straight, {:.1}/min with 30 % turning",
            straight.per_minute, turning.per_minute
        );
        assert!(turning.per_minute < straight.per_minute);
        assert!(turning.per_minute > 0.3 * straight.per_minute);
        assert!(
            turning.by_arm.iter().all(|&a| a > 1.0),
            "an arm stands still: {:?}",
            turning.by_arm
        );
    }
}
