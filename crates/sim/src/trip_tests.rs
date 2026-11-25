//! Rule tests for trips and running costs, each in both directions: short
//! trips are walked, the vehicle budget, people stay in their car until it
//! arrives, a car taken off the road leaves its people on their way, a
//! driver heading for a shop that has closed turns back, services cost more
//! per resident in a bigger city.

use crate::citizens::{Activity, CitizenId, Location, TravelMode};
use crate::traffic::{self, Departure, Place, TripEnd, TripPurpose};
use crate::{BuildingId, Cell, Command, RoadKind, World, Zone, balance, day, economy, geom, people, scenarios};

const DAY: u64 = balance::TICKS_PER_DAY as u64;

/// A long street with housing, shops and industry, grown for half a day.
fn town(seed: u64) -> World {
    let mut w = World::new(seed);
    w.apply(Command::BuildRoad {
        from: Cell::new(11, 128),
        to: Cell::new(90, 128),
        kind: RoadKind::Street,
    })
    .unwrap();
    scenarios::zone(&mut w, (12, 126), (90, 127), Zone::Residential);
    scenarios::zone(&mut w, (12, 129), (50, 130), Zone::Commercial);
    scenarios::zone(&mut w, (51, 129), (90, 130), Zone::Industrial);
    for _ in 0..DAY / 2 {
        w.step();
    }
    w.check_sim_invariants().unwrap();
    w
}

fn centre(w: &World, b: BuildingId) -> [f32; 2] {
    crate::derived::rect_center(w.buildings[b].footprint)
}

/// A driver at home with a free car (everyone else in the household too),
/// and two shops: the nearest one and one far away.
fn driver_at_home(w: &mut World) -> (CitizenId, BuildingId, BuildingId) {
    let now = w.time.tick;
    let shop_pos: Vec<[f32; 2]> = w
        .buildings
        .iter()
        .filter(|(_, b)| b.kind == Zone::Commercial)
        .map(|(b, _)| centre(w, b))
        .collect();
    // The household with a car living nearest to a shop.
    let (_, hid, id) = w
        .households
        .iter()
        .filter_map(|(hid, h)| {
            let d = h.citizens.iter().copied().find(|&c| w.citizens[c].drives())?;
            let hp = centre(w, h.home);
            let near = shop_pos.iter().map(|&p| geom::dist(hp, p)).fold(f32::MAX, f32::min);
            (h.cars > 0).then_some((near, hid, d))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("a household with a car");
    let home = w.households[hid].home;
    for m in w.households[hid].citizens.clone() {
        w.traffic.pending.retain(|d| d.citizen != Some(m));
        w.citizens[m].has_car = false;
        people::set_place(w, m, Activity::Home, Location::In(home), now + DAY);
    }
    let hp = centre(w, home);
    let mut shops: Vec<(f32, BuildingId)> = w
        .buildings
        .iter()
        .filter(|(_, b)| b.kind == Zone::Commercial)
        .map(|(b, _)| (geom::dist(hp, centre(w, b)), b))
        .collect();
    shops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let near = shops.first().expect("shops").1;
    let far = shops.last().unwrap().1;
    (id, near, far)
}

fn journey(w: &World, id: CitizenId) -> crate::Journey {
    *w.citizens[id].location.journey().expect("travelling")
}

#[test]
fn short_trips_are_walked_longer_ones_driven() {
    let mut w = town(31);
    let (id, near, far) = driver_at_home(&mut w);
    let home = w.households[w.citizens[id].household].home;
    let d_near = geom::dist(centre(&w, home), centre(&w, near));
    let d_far = geom::dist(centre(&w, home), centre(&w, far));
    assert!(d_far > balance::WALK_MAX_M, "{d_far}");
    assert!(d_near <= balance::WALK_MAX_M, "a shop across the street: {d_near}");
    {
        let before = w.traffic.pending.len();
        day::go(&mut w, id, near, TripPurpose::Shopping, &[]);
        let j = journey(&w, id);
        assert_eq!(j.mode, TravelMode::Walking);
        assert!(j.arrive_tick.is_some(), "on foot, off screen");
        assert_eq!(w.traffic.pending.len(), before, "no car");
        assert!(!w.citizens[id].has_car);
        let home = w.households[w.citizens[id].household].home;
        let now = w.time.tick;
        people::set_place(&mut w, id, Activity::Home, Location::In(home), now + DAY);
    }
    // Far away: the car, on the road.
    let before = w.traffic.pending.len();
    day::go(&mut w, id, far, TripPurpose::Shopping, &[]);
    let j = journey(&w, id);
    assert_eq!(j.mode, TravelMode::Driving);
    assert!(j.arrive_tick.is_none(), "a car on the road ends the trip");
    assert_eq!(w.traffic.pending.len(), before + 1);
    assert!(w.citizens[id].has_car);
    w.check_sim_invariants().unwrap();
}

#[test]
fn over_the_vehicle_budget_a_car_trip_happens_off_screen() {
    let mut w = town(32);
    let (id, _, far) = driver_at_home(&mut w);
    // Fill the budget with waiting trucks.
    let budget = traffic::vehicle_budget(&w);
    while traffic::has_room(&w) {
        let d = Departure::new(&w, TripPurpose::Goods, Place::Entry, Place::Entry);
        traffic::request_trip(&mut w, d);
    }
    assert!(w.vehicles.len() + w.traffic.pending.len() >= budget);
    let before = w.traffic.pending.len();
    day::go(&mut w, id, far, TripPurpose::Shopping, &[]);
    let j = journey(&w, id);
    assert_eq!(j.mode, TravelMode::Driving, "they still take the car");
    assert!(j.arrive_tick.is_some(), "off screen");
    assert_eq!(w.traffic.pending.len(), before, "no car on the road");
    // Room again: the next trip is a car on the road.
    let mut w = town(32);
    let (id, _, far) = driver_at_home(&mut w);
    assert!(traffic::has_room(&w));
    day::go(&mut w, id, far, TripPurpose::Shopping, &[]);
    assert!(journey(&w, id).arrive_tick.is_none());
}

/// Start a car trip of `id` to `to` and step until the car is on the road.
fn drive_off(w: &mut World, id: CitizenId, to: BuildingId) -> crate::VehicleId {
    day::go(w, id, to, TripPurpose::Shopping, &[]);
    for _ in 0..2000 {
        w.step();
        if let Some(v) = w.citizens[id].location.journey().and_then(|j| j.vehicle) {
            return v;
        }
    }
    panic!("the car never started");
}

#[test]
fn people_stay_in_the_car_until_it_arrives() {
    let mut w = town(33);
    let (id, _, far) = driver_at_home(&mut w);
    let v = drive_off(&mut w, id, far);
    let due = journey(&w, id).due_tick;
    let mut ticks = 0;
    while w.vehicles.contains_key(v) {
        // In the car the whole way, past the expected arrival too.
        let j = journey(&w, id);
        assert_eq!(j.vehicle, Some(v), "tick {}", w.time.tick);
        w.step();
        ticks += 1;
        assert!(ticks < 20 * 60 * 5, "the car never arrived");
    }
    // The car arrived: they are in the shop (or on the way home from it).
    let c = &w.citizens[id];
    assert!(
        c.location == Location::In(far) || c.location.journey().is_some_and(|j| j.from == Some(far)),
        "{:?} {:?}, due {due}",
        c.activity,
        c.location
    );
}

#[test]
fn a_car_taken_off_the_road_leaves_its_people_on_their_way() {
    for end in [TripEnd::GaveUp(90.0), TripEnd::Arrived(30.0)] {
        let mut w = town(34);
        let (id, _, far) = driver_at_home(&mut w);
        let v = drive_off(&mut w, id, far);
        let mut ended = Vec::new();
        traffic::remove_vehicle(&mut w, v, end, &mut ended);
        for e in ended {
            crate::households::trip_ended(&mut w, e);
        }
        let now = w.time.tick;
        match end {
            TripEnd::GaveUp(_) => {
                // Not at the shop yet: they go on off screen.
                let j = journey(&w, id);
                assert!(j.vehicle.is_none());
                assert!(j.arrive_tick.is_some_and(|t| t > now), "{j:?}");
                assert_eq!(j.to, far);
            }
            _ => assert_eq!(w.citizens[id].location, Location::In(far), "arrived"),
        }
        w.check_sim_invariants().unwrap();
    }
}

#[test]
fn a_driver_heading_for_a_shop_that_has_closed_turns_back() {
    for closed in [true, false] {
        let mut w = town(35);
        let (id, _, far) = driver_at_home(&mut w);
        let home = w.households[w.citizens[id].household].home;
        let v = drive_off(&mut w, id, far);
        // Pretend the clock has moved on (only the time of day matters).
        let h = if closed { balance::SHOP_CLOSE_H + 0.5 } else { 14.0 };
        w.time.tick_of_day = (h * balance::TICKS_PER_HOUR as f32) as u32;
        day::decide(&mut w, id);
        let j = journey(&w, id);
        let trip = traffic::trip_of(&w, v).unwrap();
        if closed {
            assert_eq!(j.to, home, "back home");
            assert_eq!(trip.to_building, Some(home));
            assert!(trip.needs_route);
        } else {
            assert_eq!(j.to, far, "carries on");
            assert_eq!(trip.to_building, Some(far));
        }
        assert_eq!(j.vehicle, Some(v), "still in the car");
    }
}

#[test]
fn services_cost_more_per_resident_in_a_bigger_denser_city() {
    let w = town(36);
    let small = economy::running_costs(&w);
    let pop: u32 = w
        .buildings
        .values()
        .filter(|b| b.kind == Zone::Residential)
        .map(|b| b.occupied)
        .sum();
    assert!(pop > 0);
    // The same homes five times as full, one level denser.
    let mut big = w.clone();
    for b in big.buildings.values_mut() {
        if b.kind == Zone::Residential {
            b.occupied *= 5;
            b.level = (b.level + 1).min(3);
        }
    }
    let large = economy::running_costs(&big);
    let per = |c: &economy::RunningCosts, n: u32| c.services() as f64 / n as f64;
    assert!(per(&large, pop * 5) > per(&small, pop) * 1.1, "{small:?} {large:?}");
    assert!(large.services_city_size > small.services_city_size * 5);
    assert!(large.services_density > small.services_density);
    // The base cost per resident is the same.
    let base = |c: &economy::RunningCosts, n: u32| c.services_base as f64 / n as f64;
    assert!((base(&large, pop * 5) - base(&small, pop)).abs() < 0.01);
    // Traffic lights cost upkeep: more lights, more upkeep.
    assert_eq!(
        small.signals,
        small.signal_count as i64 * balance::SIGNAL_UPKEEP_PER_DAY
    );
    assert_eq!(small.road_upkeep(), economy::budget_now(&w).road_upkeep);
}
