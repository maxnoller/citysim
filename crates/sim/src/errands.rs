//! Non-commute traffic: shopping runs between homes and shops, and goods
//! trips from industry to shops or to and from the highway. These cars
//! belong to no household; they appear, drive and vanish, and keep the
//! roads busy outside the rush hours (with a thin trickle at night).

use crate::derived::rect_center;
use crate::traffic::{self, Place, TripPurpose};
use crate::{World, Zone, balance, geom, households};

/// Relative shopping activity by hour of day.
fn shop_profile(h: f32) -> f32 {
    match h as u32 {
        0..=5 => 0.05,
        6..=9 => 0.4,
        10..=19 => 1.0,
        20..=21 => 0.5,
        _ => 0.15,
    }
}

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

pub(crate) fn tick(world: &mut World) {
    if !world.time.tick.is_multiple_of(balance::ERRAND_INTERVAL_TICKS) {
        return;
    }
    let h = world.time.hour();
    let hours = balance::ERRAND_INTERVAL_TICKS as f32 / balance::TICKS_PER_HOUR as f32;
    let households = world.households.len() as f32;
    let factory_jobs: u32 = world
        .buildings
        .values()
        .filter(|b| b.kind == Zone::Industrial)
        .map(|b| b.occupied)
        .sum();
    let acc = &mut world.traffic.errand_acc;
    acc[0] += households * balance::SHOP_TRIPS_PER_HOUSEHOLD_PER_DAY * shop_profile(h) / day_sum(shop_profile) * hours;
    acc[1] += factory_jobs as f32 * balance::GOODS_TRIPS_PER_INDUSTRIAL_JOB_PER_DAY * goods_profile(h)
        / day_sum(goods_profile)
        * hours;
    acc[0] = acc[0].min(20.0);
    acc[1] = acc[1].min(20.0);
    if acc[0] < 1.0 && acc[1] < 1.0 {
        return;
    }
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    let mut homes = Vec::new();
    let mut shops = Vec::new();
    let mut factories = Vec::new();
    for b in world.buildings.values() {
        if !households::connected(world, b.footprint) {
            continue;
        }
        let p = rect_center(b.footprint);
        match b.kind {
            Zone::Residential if b.occupied > 0 => homes.push(p),
            Zone::Commercial => shops.push(p),
            Zone::Industrial if b.occupied > 0 => factories.push(p),
            _ => {}
        }
    }
    // Nearest of a few random shops.
    let pick_shop = |world: &mut World, from: [f32; 2]| -> Option<[f32; 2]> {
        let mut best: Option<(f32, [f32; 2])> = None;
        for _ in 0..balance::ERRAND_SAMPLES {
            if shops.is_empty() {
                return None;
            }
            let s = shops[world.rng.below(shops.len() as u32) as usize];
            let d = geom::dist(from, s);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, s));
            }
        }
        best.map(|b| b.1)
    };
    while world.traffic.errand_acc[0] >= 1.0 {
        world.traffic.errand_acc[0] -= 1.0;
        if homes.is_empty() {
            world.traffic.errand_acc[0] = 0.0;
            break;
        }
        let home = homes[world.rng.below(homes.len() as u32) as usize];
        let Some(shop) = pick_shop(world, home) else {
            world.traffic.errand_acc[0] = 0.0;
            break;
        };
        // Half the runs are the way back.
        let (from, to) = if world.rng.below(2) == 0 {
            (home, shop)
        } else {
            (shop, home)
        };
        traffic::request(world, None, TripPurpose::Shopping, Place::At(from), Place::At(to));
    }
    while world.traffic.errand_acc[1] >= 1.0 {
        world.traffic.errand_acc[1] -= 1.0;
        if factories.is_empty() {
            world.traffic.errand_acc[1] = 0.0;
            break;
        }
        let factory = factories[world.rng.below(factories.len() as u32) as usize];
        let via_highway = world.rng.next_f32() < balance::GOODS_VIA_HIGHWAY_SHARE || shops.is_empty();
        let (from, to) = if via_highway {
            if world.rng.below(2) == 0 || shops.is_empty() {
                (Place::At(factory), Place::Entry)
            } else {
                let shop = shops[world.rng.below(shops.len() as u32) as usize];
                (Place::Entry, Place::At(shop))
            }
        } else {
            let shop = pick_shop(world, factory).expect("shops not empty");
            (Place::At(factory), Place::At(shop))
        };
        traffic::request(world, None, TripPurpose::Goods, from, to);
    }
}
