//! Goods traffic. Shops sell goods to households (`day`); a shop whose
//! shelves run low orders a delivery, which a truck brings from a nearby
//! factory with workers (or from outside the city through the highway,
//! `balance::GOODS_IMPORT_SHARE`, and always when there is no industry).
//! The stock arrives with the truck. Industry also ships part of its output
//! out of the city (`balance::EXPORT_TRIPS_PER_INDUSTRIAL_JOB_PER_DAY`).
//! Trucks belong to no household.

use crate::derived::rect_center;
use crate::traffic::{self, Departure, Place, TripPurpose};
use crate::{Building, BuildingId, Cell, World, Zone, balance, geom, households, people};

/// Relative goods traffic by hour of day.
fn goods_profile(h: f32) -> f32 {
    match h as u32 {
        0..=4 => 0.15,
        5..=18 => 1.0,
        _ => 0.3,
    }
}

/// Sum of a profile over a day, in hours.
fn day_sum(f: fn(f32) -> f32) -> f32 {
    (0..24).map(|h| f(h as f32)).sum()
}

/// Shelf space of a shop, units.
pub fn stock_cap(b: &Building) -> f32 {
    if b.kind == Zone::Commercial {
        b.capacity as f32 * balance::SHOP_STOCK_PER_JOB
    } else {
        0.0
    }
}

/// A new (or newly loaded) shop starts with full shelves.
pub(crate) fn stock_up(world: &mut World, b: BuildingId) {
    let Some(cap) = world.buildings.get(b).map(stock_cap) else {
        return;
    };
    if cap > 0.0 {
        people::place(world, b).stock = cap;
    }
}

pub(crate) fn tick(world: &mut World) {
    if !world.time.tick.is_multiple_of(balance::ERRAND_INTERVAL_TICKS) {
        return;
    }
    deliveries(world);
    exports(world);
}

/// Send trucks to shops waiting for goods.
fn deliveries(world: &mut World) {
    let mut n = world.people.restock.len().min(balance::DELIVERIES_PER_ROUND);
    if n == 0 {
        return;
    }
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    while n > 0 {
        n -= 1;
        let Some(shop) = world.people.restock.pop_front() else {
            break;
        };
        let Some(sb) = world.buildings.get(shop) else { continue };
        let to = rect_center(sb.footprint);
        // Nearest of a few random factories with workers.
        let f = world.cache.factories.len();
        let mut best: Option<(f32, [f32; 2], BuildingId)> = None;
        if f > 0 && world.rng.next_f32() >= balance::GOODS_IMPORT_SHARE {
            for _ in 0..balance::DELIVERY_SAMPLES {
                let b = world.cache.factories[world.rng.below(f as u32) as usize];
                let Some(fb) = world.buildings.get(b) else { continue };
                if fb.occupied == 0 || !households::connected(world, fb.footprint) {
                    continue;
                }
                let p = rect_center(fb.footprint);
                let d = geom::dist(p, to);
                if best.is_none_or(|x| d < x.0) {
                    best = Some((d, p, b));
                }
            }
        }
        let from = best.map_or(Place::Entry, |b| Place::At(b.1));
        let mut d = Departure::new(world, TripPurpose::Goods, from, Place::At(to));
        d.from_building = best.map(|b| b.2);
        d.to_building = Some(shop);
        d.cargo = balance::TRUCK_LOAD;
        traffic::request_trip(world, d);
        people::count_trip(world, TripPurpose::Goods);
    }
}

/// Industry ships out of the city through the highway.
fn exports(world: &mut World) {
    let h = world.time.hour();
    let hours = balance::ERRAND_INTERVAL_TICKS as f32 / balance::TICKS_PER_HOUR as f32;
    let factory_jobs: u32 = world
        .buildings
        .values()
        .filter(|b| b.kind == Zone::Industrial)
        .map(|b| b.occupied)
        .sum();
    let acc = &mut world.traffic.errand_acc;
    acc[0] = 0.0;
    acc[1] += factory_jobs as f32 * balance::EXPORT_TRIPS_PER_INDUSTRIAL_JOB_PER_DAY * goods_profile(h)
        / day_sum(goods_profile)
        * hours;
    acc[1] = acc[1].min(20.0);
    if acc[1] < 1.0 {
        return;
    }
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let f = world.cache.factories.len();
    while world.traffic.errand_acc[1] >= 1.0 {
        world.traffic.errand_acc[1] -= 1.0;
        if f == 0 {
            world.traffic.errand_acc[1] = 0.0;
            break;
        }
        let b = world.cache.factories[world.rng.below(f as u32) as usize];
        let Some(fb) = world.buildings.get(b) else { continue };
        if fb.occupied == 0 || !households::connected(world, fb.footprint) {
            continue;
        }
        let from = Place::At(rect_center(fb.footprint));
        let mut d = Departure::new(world, TripPurpose::Goods, from, Place::Entry);
        d.from_building = Some(b);
        d.cargo = balance::TRUCK_LOAD;
        traffic::request_trip(world, d);
        people::count_trip(world, TripPurpose::Goods);
    }
}

/// A goods trip ended (arrived, or completed off screen): a shop at its
/// destination gets a truckload.
pub(crate) fn delivered(world: &mut World, to: Place, to_building: Option<BuildingId>) {
    let b = match (to_building, to) {
        (Some(b), _) => b,
        // Trucks from saves before trips named their buildings.
        (None, Place::At(p)) => match world.grid.get(Cell::from_pos(p)).copied() {
            Some(crate::grid::CellData {
                occupant: crate::grid::Occupant::Building(b),
                ..
            }) => b,
            _ => return,
        },
        (None, Place::Entry) => return,
    };
    let Some(cap) = world.buildings.get(b).map(stock_cap) else {
        return;
    };
    if cap <= 0.0 {
        return;
    }
    let pl = people::place(world, b);
    pl.stock = (pl.stock + balance::TRUCK_LOAD).min(cap);
    pl.delivery_pending = false;
    if pl.stock < cap * balance::RESTOCK_BELOW {
        // Still low: order another.
        pl.delivery_pending = true;
        world.people.restock.push_back(b);
    }
}
