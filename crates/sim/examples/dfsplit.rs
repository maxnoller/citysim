//! What the delay factor is made of, from a saved city's next day:
//! `dfsplit <save> [hours]`. For every commute that arrives: door-to-door
//! time over the free-flow estimate (what `delay_factor` averages), split
//! into waiting to pull out, the route at the speed limits (detour and road
//! speeds against the estimate) and time lost on the road beyond that.
use std::collections::BTreeMap;

use citysim_sim::*;

#[derive(Default, Clone, Copy)]
struct Acc {
    n: f64,
    w: f64,
    s: f64,
    est: f64,
    pending: f64,
    ideal: f64,
    dist: f64,
}

/// Per car: (purpose, from, to, requested, started, metres, ideal seconds).
type Live = (TripPurpose, Place, Place, u64, u64, f64, f64);

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut w = World::load(&std::fs::read(&a[0]).unwrap()).unwrap();
    let hours: u32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(24);
    let mut live: BTreeMap<VehicleId, Live> = BTreeMap::new();
    let mut by_hour: BTreeMap<u32, Acc> = BTreeMap::new();
    let dt = 1.0 / TICKS_PER_SECOND as f64;
    let end = w.time.tick + hours as u64 * balance::TICKS_PER_HOUR as u64;
    let pos = |w: &World, p: Place| match p {
        Place::At(x) => x,
        Place::Entry => w.roads.entry.map(|e| w.roads.nodes[e].pos).unwrap_or([0.0, 1024.0]),
    };
    while w.time.tick < end {
        w.step();
        let now = w.time.tick;
        let mut seen = Vec::new();
        for (id, v) in w.vehicles.iter() {
            let t = traffic::trip_of(&w, id).unwrap();
            let e = live
                .entry(id)
                .or_insert((t.purpose, t.from, t.to, t.requested_tick, t.started_tick, 0.0, 0.0));
            let lim = v
                .segment
                .and_then(|s| w.roads.segments.get(s))
                .map_or(10.0, |s| s.speed_limit) as f64;
            let d = v.speed as f64 * dt;
            e.5 += d;
            e.6 += d / lim;
            seen.push(id);
        }
        let gone: Vec<VehicleId> = live.keys().copied().filter(|k| !w.vehicles.contains_key(*k)).collect();
        for id in gone {
            let (purpose, from, to, req, start, dist, ideal) = live.remove(&id).unwrap();
            if !matches!(purpose, TripPurpose::ToWork | TripPurpose::ToHome) {
                continue;
            }
            let s = (now - req) as f64 * dt;
            let est = day_estimate(pos(&w, from), pos(&w, to)) as f64;
            if s < 2.0 || dist < 20.0 {
                continue;
            }
            let k = (est / 60.0).min(3.0);
            let h = if std::env::var("BY_START").is_ok() {
                let back = (now - req) as u32;
                ((w.time.tick_of_day + balance::TICKS_PER_DAY - back % balance::TICKS_PER_DAY) % balance::TICKS_PER_DAY)
                    / balance::TICKS_PER_HOUR
            } else {
                w.time.tick_of_day / balance::TICKS_PER_HOUR
            };
            let acc = by_hour.entry(h).or_default();
            acc.n += 1.0;
            acc.w += k;
            acc.s += k * s / est;
            acc.est += k;
            acc.pending += k * (start.saturating_sub(req)) as f64 * dt / est;
            acc.ideal += k * ideal / est;
            acc.dist += k * dist / (geom::dist(pos(&w, from), pos(&w, to)) as f64).max(1.0);
        }
        let _ = seen;
    }
    println!("hour   trips | ratio = pending + ideal route + lost on road | route/straight");
    let mut tot = Acc::default();
    for (h, a) in &by_hour {
        println!(
            "{h:4} {:7.0} | {:5.2} = {:5.2} + {:5.2} + {:5.2} | {:5.2}",
            a.n,
            a.s / a.w,
            a.pending / a.w,
            a.ideal / a.w,
            (a.s - a.pending - a.ideal) / a.w,
            a.dist / a.w
        );
        tot.n += a.n;
        tot.w += a.w;
        tot.s += a.s;
        tot.pending += a.pending;
        tot.ideal += a.ideal;
        tot.dist += a.dist;
    }
    println!(
        "all  {:7.0} | {:5.2} = {:5.2} + {:5.2} + {:5.2} | {:5.2}",
        tot.n,
        tot.s / tot.w,
        tot.pending / tot.w,
        tot.ideal / tot.w,
        (tot.s - tot.pending - tot.ideal) / tot.w,
        tot.dist / tot.w
    );
    println!("delay factor at the end {:.2}", traffic::delay_factor(&w));
}

/// `day::estimate_s` (crate-private): straight line times the route factor
/// at the off-screen speed, plus a base.
fn day_estimate(a: [f32; 2], b: [f32; 2]) -> f32 {
    geom::dist(a, b) * balance::OFFSCREEN_ROUTE_FACTOR / balance::OFFSCREEN_SPEED + balance::OFFSCREEN_BASE_S
}
