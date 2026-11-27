//! A day of their own: each citizen decides what to do next from their
//! needs and the time of day, and their decisions create the trips.
//!
//! * Citizens think in rotating buckets (`balance::THINK_BUCKETS`), and only
//!   once their current activity is over (`Citizen::until_tick`), or when a
//!   trip ends (traffic calls [`trip_ended`]).
//! * Workers leave home a little before their shift (staggered per
//!   workplace and per person, with days off), work until the shift ends,
//!   then go home (or shopping on the way).
//! * At home, an adult goes shopping when the household's goods run low and
//!   nobody else is out shopping (choosing a shop by distance and stock), or
//!   out for leisure when their need is high and they have time (choosing a
//!   venue by distance and how well it restores the need; children come
//!   along). Otherwise they stay in, and sleep at night.
//! * Every trip by car is a car on the road while the vehicle budget allows
//!   (`balance::MAX_VEHICLES`; beyond it the trip happens off screen and
//!   takes the estimated drive time). Its driver and passengers are in the
//!   car from departure until it arrives, however long the drive takes on
//!   the calendar; what they do next starts when they arrive. Workers leave
//!   home early enough for the door-to-door time their last commute took.
//!   Without a free car an adult rides with a member going to a workplace
//!   nearby (car-sharing), or travels off screen for
//!   `balance::CARLESS_TIME_FACTOR` times as long. Children only travel as
//!   passengers; teens without a car travel off screen.

use crate::citizens::{Activity, CitizenId, Journey, LifeStage, Location, TravelMode};
use crate::derived::rect_center;
use crate::households::place_of;
use crate::people::{self, daily, habit, hours};
use crate::traffic::{self, Departure, Ended, Place, TripEnd, TripPurpose};
use crate::{BuildingId, TICKS_PER_SECOND, World, balance, geom};

const DAY: i64 = balance::TICKS_PER_DAY as i64;
const HOUR: u64 = balance::TICKS_PER_HOUR as u64;

/// Tick (may be negative before the game started) of the midnight that
/// starts day `day`.
fn midnight(world: &World, day: u32) -> i64 {
    let t = &world.time;
    t.tick as i64 - t.tick_of_day as i64 + (day as i64 - t.day as i64) * DAY
}

fn at(t: i64) -> u64 {
    t.max(0) as u64
}

/// Personal offset in hours, uniform in `-spread..spread`.
fn spread(seed: u32, salt: u64, spread: f32) -> f32 {
    (habit(seed, salt) * 2.0 - 1.0) * spread
}

/// Longest trip time a worker plans for, hours (a commute that took longer
/// once is not planned around).
const MAX_PLANNED_TRIP_H: f32 = 3.0;

/// Ticks a worker expects their trip to work to take, door to door: the
/// last one's (with a margin), or the estimate before the first.
pub(crate) fn expected_commute_ticks(world: &World, id: CitizenId) -> u64 {
    let c = &world.citizens[id];
    let t = if c.commute_ticks > 0 {
        c.commute_ticks as f32
    } else {
        // Before the first trip: the free-flow estimate, but at least most
        // of what a commute in this city takes these days.
        let home = world.households.get(c.household).map(|h| h.home);
        let est = secs_to_ticks(estimate_s(pos_of(world, home), pos_of(world, c.workplace()))) as f32;
        est.max(world.traffic.avg_commute_s * 0.75 * TICKS_PER_SECOND as f32)
    };
    (t * balance::COMMUTE_TIME_MARGIN).min(MAX_PLANNED_TRIP_H * balance::TICKS_PER_HOUR as f32) as u64
}

/// Shift times of citizen `id` on day `d`: (leave home, start, end),
/// absolute ticks. They leave the expected trip time plus a personal slack
/// before the start.
fn shift_times(world: &World, id: CitizenId, shift: crate::citizens::Shift, d: u32) -> (i64, i64, i64) {
    let seed = world.citizens[id].seed;
    let wobble = (daily(seed, d, 0x5717) * 2.0 - 1.0) * balance::SHIFT_DAILY_WOBBLE_H;
    let start = midnight(world, d) + shift.start as i64 + (wobble * balance::TICKS_PER_HOUR as f32) as i64;
    let slack = balance::COMMUTE_LEAD_H + habit(seed, 0x1EAD) * balance::COMMUTE_LEAD_SPREAD_H;
    let lead = (slack * balance::TICKS_PER_HOUR as f32) as i64 + expected_commute_ticks(world, id) as i64;
    (start - lead, start, start + shift.length as i64)
}

/// The next shift this citizen still has to go to: (day, leave, end).
pub(crate) fn next_shift(world: &World, id: CitizenId) -> Option<(u32, i64, i64)> {
    let c = &world.citizens[id];
    let job = c.job?;
    let now = world.time.tick as i64;
    let today = world.time.day;
    let min_work = (balance::MIN_WORK_H * balance::TICKS_PER_HOUR as f32) as i64;
    for d in today.saturating_sub(1).max(1)..=today + 1 {
        if !job.shift.works_on(d) || c.worked_day >= d {
            continue;
        }
        let (leave, _, end) = shift_times(world, id, job.shift, d);
        if now < end - min_work {
            return Some((d, leave, end));
        }
    }
    None
}

/// Waking and bedtime in hours for a citizen.
pub(crate) fn wake_bed(world: &World, id: CitizenId) -> (f32, f32) {
    let c = &world.citizens[id];
    let shift: f32 = match c.stage {
        LifeStage::Senior => -1.0,
        LifeStage::Teen => 1.0,
        LifeStage::Child => -1.5,
        LifeStage::Adult => 0.0,
    };
    let wake = balance::WAKE_H + spread(c.seed, 0x3A7E, balance::SLEEP_SPREAD_H) + shift.min(0.0) * 0.5;
    let bed = balance::BED_H + spread(c.seed, 0xBED, balance::SLEEP_SPREAD_H) + shift;
    (wake, bed)
}

/// Estimated drive time between two points, vehicle seconds.
pub(crate) fn estimate_s(from: [f32; 2], to: [f32; 2]) -> f32 {
    geom::dist(from, to) * balance::OFFSCREEN_ROUTE_FACTOR / balance::OFFSCREEN_SPEED + balance::OFFSCREEN_BASE_S
}

/// Expected time of a trip by road between two points right now: the
/// free-flow estimate times what the roads are doing to car trips
/// (`traffic::delay_factor`), vehicle seconds.
pub(crate) fn road_s(world: &World, from: [f32; 2], to: [f32; 2]) -> f32 {
    estimate_s(from, to) * traffic::delay_factor(world)
}

/// Off-screen time of a trip: commutes in today's traffic ([`road_s`],
/// what the delay factor is measured on), other trips at the free-flow
/// estimate.
fn trip_s(world: &World, purpose: TripPurpose, from: [f32; 2], to: [f32; 2]) -> f32 {
    if matches!(purpose, TripPurpose::ToWork | TripPurpose::ToHome) {
        road_s(world, from, to)
    } else {
        estimate_s(from, to)
    }
}

/// Vehicle seconds an off-screen trip estimated at `s` takes: at most
/// `OFFSCREEN_MAX_TRIP_H`, more in proportion when traffic is bad.
pub(crate) fn offscreen_s(world: &World, s: f32) -> f32 {
    let tps = TICKS_PER_SECOND as f32;
    let cap_s = balance::OFFSCREEN_MAX_TRIP_H * balance::TICKS_PER_HOUR as f32 / tps
        * (traffic::delay_factor(world) / balance::DELAY_OK_FACTOR).max(1.0);
    s.min(cap_s)
}

/// Ticks a trip of `s` estimated vehicle seconds takes, on the road or off
/// screen alike (vehicle time is real time, see the `balance` time scales).
fn secs_to_ticks(s: f32) -> u64 {
    (s * balance::OFFSCREEN_CALENDAR_FACTOR * TICKS_PER_SECOND as f32)
        .round()
        .max(1.0) as u64
}

/// Expected ticks of a car trip from `from` to `to` (see
/// `Journey::due_tick`).
pub(crate) fn due_after(world: &World, from: Option<BuildingId>, to: Option<BuildingId>) -> u64 {
    secs_to_ticks(estimate_s(pos_of(world, from), pos_of(world, to)))
}

pub(crate) fn pos_of(world: &World, b: Option<BuildingId>) -> [f32; 2] {
    match b.and_then(|b| world.buildings.get(b)) {
        Some(b) => rect_center(b.footprint),
        None => world
            .roads
            .entry
            .and_then(|e| world.roads.nodes.get(e))
            .map_or([0.0, crate::MAP_SIZE_M / 2.0], |n| n.pos),
    }
}

// --------------------------------------------------------------- tick ----

/// Let this tick's bucket of citizens decide (those whose activity is over).
pub(crate) fn tick(world: &mut World) {
    let buckets = balance::THINK_BUCKETS;
    let now = world.time.tick;
    let n = world.citizens.slot_count();
    if !world.cache.until_ok || world.cache.until.len() < n {
        let mut v = vec![u64::MAX; n];
        for (id, c) in world.citizens.iter() {
            v[crate::idmap::key_index(id)] = c.until_tick;
        }
        world.cache.until = v;
        world.cache.until_ok = true;
    }
    let mut i = (now % buckets) as usize;
    while i < n {
        if world.cache.until[i] <= now
            && let Some(id) = world.citizens.key_at(i)
        {
            if world.citizens[id].until_tick <= now {
                decide(world, id);
            } else {
                world.cache.until[i] = world.citizens[id].until_tick;
            }
        }
        i += buckets as usize;
    }
}

/// Decide what to do next (the current activity is over).
pub(crate) fn decide(world: &mut World, id: CitizenId) {
    people::catch_up_need(world, id);
    let now = world.time.tick;
    let c = &world.citizens[id];
    let hid = c.household;
    let Some(h) = world.households.get(hid) else { return };
    let home = h.home;
    match c.location {
        Location::Travelling(j) => {
            if let Some(t) = j.arrive_tick {
                if t <= now {
                    arrive(world, id);
                } else {
                    people::set_until(world, id, t);
                }
                return;
            }
            // On the road or waiting for the car: the traffic callback
            // ends it when the car arrives (or gives up, or could not
            // start). Look again in a while in case the car is lost.
            let lost = match j.mode {
                TravelMode::Passenger => j.driver.is_none_or(|d| {
                    world
                        .citizens
                        .get(d)
                        .is_none_or(|d| !matches!(d.location, Location::Travelling(dj) if dj.arrive_tick.is_none()))
                }),
                _ => match j.vehicle {
                    Some(v) => !world.vehicles.contains_key(v),
                    None => !world.traffic.pending.iter().any(|d| d.citizen == Some(id)),
                },
            };
            if lost {
                continue_off_screen(world, id);
            } else {
                if j.mode == TravelMode::Driving && j.to != home && closed_by_now(world, j.purpose) {
                    turn_back(world, id, home);
                }
                people::set_until(world, id, now + HOUR);
            }
        }
        Location::In(b) if b == home => at_home(world, id),
        Location::In(b) => away(world, id, b),
    }
}

/// Has the place a trip of `purpose` goes to closed by now (shops,
/// leisure venues)?
fn closed_by_now(world: &World, purpose: TripPurpose) -> bool {
    let tod = world.time.hour();
    match purpose {
        TripPurpose::Shopping => !(balance::SHOP_OPEN_H..balance::SHOP_CLOSE_H).contains(&tod),
        TripPurpose::Leisure => !(balance::LEISURE_OPEN_H..balance::LEISURE_LAST_H).contains(&tod),
        _ => false,
    }
}

/// The driver of a car stuck on its way to a shop or venue that has closed
/// turns back home, with everyone in the car.
fn turn_back(world: &mut World, driver: CitizenId, home: BuildingId) {
    let Location::Travelling(j) = world.citizens[driver].location else {
        return;
    };
    let Some(v) = j.vehicle else { return };
    let Some(place) = place_of(world, home) else { return };
    let Some(at) = world.vehicles.get(v).map(|car| car.pos) else {
        return;
    };
    traffic::redirect(world, v, place, Some(home));
    let due = world.time.tick + secs_to_ticks(estimate_s(at, pos_of(world, Some(home))));
    for m in std::iter::once(driver).chain(riders_of(world, driver)) {
        if let Location::Travelling(mj) = world.citizens[m].location {
            let nj = Journey {
                to: home,
                due_tick: due,
                ..mj
            };
            people::set_place(
                world,
                m,
                Activity::Travelling,
                Location::Travelling(nj),
                world.citizens[m].until_tick,
            );
        }
    }
}

/// Decisions away from home: the activity there is over.
fn away(world: &mut World, id: CitizenId, b: BuildingId) {
    let c = &world.citizens[id];
    if c.stage == LifeStage::Child {
        // Children never travel alone: wait for the household (or go home
        // off screen if nobody who brought them is here any more).
        let hid = c.household;
        let carer = world.households[hid].citizens.iter().any(|&m| {
            m != id
                && world
                    .citizens
                    .get(m)
                    .is_some_and(|m| m.drives() && m.location == Location::In(b))
        });
        if carer {
            people::set_until(world, id, world.time.tick + HOUR / 2);
            return;
        }
    }
    let (activity, hid) = (c.activity, c.household);
    let now = world.time.tick as i64;
    let left = |end: i64| end - now > hours(balance::LUNCH_MIN_LEFT_H) as i64;
    match (activity, on_shift(world, id)) {
        // Lunch time at work (the shift goes on): out for lunch, or back
        // to work if there is nowhere to go.
        (Activity::Working, Some(end)) if left(end) => {
            if !go_lunch(world, id) {
                people::set_until(world, id, at(end));
            }
            return;
        }
        // Lunch is over: back to work.
        (Activity::Leisure, Some(end)) if end > now + HOUR as i64 / 4 => {
            if let Some(wp) = world.citizens[id].workplace() {
                go(world, id, wp, TripPurpose::Leisure, &[]);
                return;
            }
        }
        _ => {}
    }
    // Leaving work: maybe shopping on the way.
    if activity == Activity::Working && wants_to_shop(world, id, true) && go_shopping(world, id) {
        return;
    }
    let purpose = match activity {
        Activity::Shopping => TripPurpose::Shopping,
        Activity::Leisure => TripPurpose::Leisure,
        _ => TripPurpose::ToHome,
    };
    let home = world.households[hid].home;
    // Children here come home with this driver.
    let kids = companions_at(world, id, b, false);
    go(world, id, home, purpose, &kids);
}

/// Household members (children, and teens when `with_teens`) in building
/// `b` right now who can ride with `id`.
fn companions_at(world: &World, id: CitizenId, b: BuildingId, with_teens: bool) -> Vec<CitizenId> {
    let c = &world.citizens[id];
    if !c.drives() {
        return Vec::new();
    }
    world.households[c.household]
        .citizens
        .iter()
        .copied()
        .filter(|&m| {
            m != id
                && world.citizens.get(m).is_some_and(|m| {
                    m.location == Location::In(b)
                        && m.activity != Activity::Sleeping
                        && (m.stage == LifeStage::Child || (with_teens && m.stage == LifeStage::Teen))
                })
        })
        .take(4)
        .collect()
}

/// Goes shopping now? Goods are low, nobody else is out shopping, the
/// shops are open, and it is past this citizen's usual shopping time (or
/// they are out anyway, `on_the_way`).
fn wants_to_shop(world: &World, id: CitizenId, on_the_way: bool) -> bool {
    let c = &world.citizens[id];
    let h = &world.households[c.household];
    let n = h.citizens.len() as f32;
    let tod = world.time.hour();
    let usual = balance::SHOP_OPEN_H + habit(c.seed, 0x5409) * (balance::SHOP_CLOSE_H - 1.0 - balance::SHOP_OPEN_H);
    c.drives()
        && h.shopper.is_none()
        && h.goods < n * balance::SHOP_BELOW_DAYS
        && (balance::SHOP_OPEN_H..balance::SHOP_CLOSE_H).contains(&tod)
        && (on_the_way || tod >= usual)
}

/// Out for lunch today, and is it the time? A share of days
/// (`MIDDAY_OUT_SHARE`, per person and day), at a personal hour in
/// `MIDDAY_OUT_H` (a window of `HOME_RECHECK_H`: one look at home falls in
/// it). Not in austerity.
fn goes_out_for_lunch(world: &World, id: CitizenId, tod: f32) -> bool {
    let c = &world.citizens[id];
    let [lo, hi] = balance::MIDDAY_OUT_H;
    let start = lo + habit(c.seed, 0x1C4) * (hi - lo - balance::HOME_RECHECK_H);
    (start..start + balance::HOME_RECHECK_H).contains(&tod)
        && daily(c.seed, world.time.day, 0x1C4B) < balance::MIDDAY_OUT_SHARE * lunch_room(world)
        && !world.households[c.household].austerity
}

/// Share of the people who would go out for lunch that do, by how slow
/// the roads are (`balance::LUNCH_DELAY`).
pub(crate) fn lunch_room(world: &World) -> f32 {
    let [full, none] = balance::LUNCH_DELAY;
    ((none - crate::traffic::delay_factor(world)) / (none - full)).clamp(0.0, 1.0)
}

/// The end of the shift `id` is at or on a break from (they set off to
/// it and it is not over), absolute tick.
pub(crate) fn on_shift(world: &World, id: CitizenId) -> Option<i64> {
    let c = &world.citizens[id];
    let job = c.job?;
    let now = world.time.tick as i64;
    if c.worked_day == 0 || c.worked_day + 1 < world.time.day {
        return None;
    }
    let (_, _, end) = shift_times(world, id, job.shift, c.worked_day);
    (now < end).then_some(end)
}

/// When a worker leaves work for lunch on the shift of day `d` (`start`
/// and `end` ticks), if they take a break that day.
fn lunch_tick(world: &World, id: CitizenId, d: u32, start: i64, end: i64) -> Option<i64> {
    let seed = world.citizens[id].seed;
    if daily(seed, d, 0x10C4) >= balance::LUNCH_BREAK_SHARE * lunch_room(world) {
        return None;
    }
    let [lo, hi] = balance::LUNCH_BREAK_H;
    let t = midnight(world, d) + hours(lo + habit(seed, 0x10C5) * (hi - lo)) as i64;
    (t > start + HOUR as i64 && t + (hours(balance::LUNCH_MIN_LEFT_H) as i64) < end).then_some(t)
}

/// Out of work for lunch: to the nearest of a few open venues, by the car
/// they came in or on foot. False if there is none near enough.
fn go_lunch(world: &mut World, id: CitizenId) -> bool {
    let c = &world.citizens[id];
    let Some(from) = c.location.building() else {
        return false;
    };
    let car = c.has_car && c.drives();
    let fp = pos_of(world, Some(from));
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let n = world.cache.shops.len();
    if n == 0 {
        return false;
    }
    let mut best: Option<(f32, BuildingId)> = None;
    for _ in 0..balance::LUNCH_SAMPLES {
        let b = world.cache.shops[world.rng.below(n as u32) as usize];
        if b == from {
            continue;
        }
        let Some(p) = world.people.places.get(b) else { continue };
        if p.staff_present == 0 {
            continue;
        }
        let Some(bb) = world.buildings.get(b) else { continue };
        let d = geom::dist(fp, rect_center(bb.footprint));
        if (!car && d > balance::WALK_MAX_M) || !crate::households::connected(world, bb.footprint) {
            continue;
        }
        if best.is_none_or(|x| d < x.0) {
            best = Some((d, b));
        }
    }
    let Some((_, venue)) = best else { return false };
    let trip_h = secs_to_ticks(estimate_s(fp, pos_of(world, Some(venue)))) as f32 / HOUR as f32;
    if trip_h > balance::LUNCH_MAX_TRIP_H {
        return false;
    }
    go(world, id, venue, TripPurpose::Leisure, &[]);
    true
}

/// A visit to a venue starting at `tod` (hours) is a midday visit (lunch
/// out), not a night out.
pub(crate) fn midday_visit(tod: f32) -> bool {
    tod < balance::MIDDAY_VISIT_BEFORE_H
}

/// Decisions at home.
fn at_home(world: &mut World, id: CitizenId) {
    let now = world.time.tick;
    let tod = world.time.hour();
    let c = &world.citizens[id];
    let home = world.households[c.household].home;
    if c.has_car {
        world.citizens[id].has_car = false;
    }
    let next = next_shift(world, id);
    if let Some((d, leave, _)) = next
        && leave <= now as i64
    {
        commute(world, id, d);
        return;
    }
    let free_until = next.map_or(i64::MAX, |n| n.1);
    let free_h = (free_until - now as i64) as f32 / balance::TICKS_PER_HOUR as f32;
    let c = &world.citizens[id];
    let (wake, bed) = wake_bed(world, id);
    let night = tod >= bed || tod < wake;
    if !night && c.stage != LifeStage::Child {
        if free_h >= balance::SHOP_FREE_H && wants_to_shop(world, id, false) && go_shopping(world, id) {
            return;
        }
        if free_h >= balance::MIDDAY_FREE_H && goes_out_for_lunch(world, id, tod) && go_leisure(world, id) {
            return;
        }
        let c = &world.citizens[id];
        let h = &world.households[c.household];
        let usual = balance::LEISURE_OPEN_H
            + habit(c.seed, 0x1E15E) * (balance::LEISURE_CLOSE_H - 2.0 - balance::LEISURE_OPEN_H);
        // After a day's work only when the need is urgent: most go out on
        // their days off (in the daytime).
        let worked_today = c.worked_day == world.time.day;
        let urgent = c.leisure_need >= balance::LEISURE_URGENT_NEED;
        if c.leisure_need >= balance::LEISURE_GO_NEED
            && (tod >= usual || urgent)
            && (!worked_today || urgent)
            && free_h >= balance::LEISURE_FREE_H
            && !h.austerity
            && (balance::LEISURE_OPEN_H..balance::LEISURE_CLOSE_H).contains(&tod)
            && go_leisure(world, id)
        {
            return;
        }
    }
    // Stay in: sleep at night, else look again in a while.
    let today = world.time.day;
    let wake_tick = |d: u32| midnight(world, d) + hours(wake) as i64;
    let (activity, until) = if night {
        let w = if tod < wake {
            wake_tick(today)
        } else {
            wake_tick(today + 1)
        };
        (Activity::Sleeping, w.min(free_until))
    } else {
        let bed_t = midnight(world, today) + hours(bed) as i64;
        let recheck = now as i64 + (balance::HOME_RECHECK_H * balance::TICKS_PER_HOUR as f32) as i64;
        let mut u = recheck.min(free_until);
        if bed_t > now as i64 {
            u = u.min(bed_t);
        }
        (Activity::Home, u)
    };
    let until = at(until.max(now as i64 + 1));
    if world.citizens[id].activity == activity {
        people::set_until(world, id, until);
    } else {
        people::set_place(world, id, activity, Location::In(home), until);
    }
}

// -------------------------------------------------------------- trips ----

/// Set off to the shift of day `d`.
fn commute(world: &mut World, id: CitizenId, d: u32) {
    let Some(job) = world.citizens[id].job else { return };
    if !world.buildings.contains_key(job.workplace) {
        return;
    }
    world.citizens[id].worked_day = d;
    world.economy.attendance.scheduled_ticks += job.shift.length as u64;
    let hid = world.citizens[id].household;
    world.households[hid].last_work_day = world.time.day;
    let driver_takes_last_car =
        world.citizens[id].drives() && world.households[hid].free_cars(world) == 1 && !world.citizens[id].has_car;
    // Car-sharing: a member leaving soon for a workplace near this one
    // rides along when this takes the last car.
    let mut riders = Vec::new();
    if driver_takes_last_car {
        let now = world.time.tick as i64;
        let wp = pos_of(world, Some(job.workplace));
        let window = (balance::CARSHARE_WINDOW_H * balance::TICKS_PER_HOUR as f32) as i64;
        for m in world.households[hid].citizens.clone() {
            if m == id {
                continue;
            }
            let Some(mc) = world.citizens.get(m) else { continue };
            let Some(mj) = mc.job else { continue };
            if mc.location != Location::In(world.households[hid].home) || mc.has_car {
                continue;
            }
            if let Some((md, leave, _)) = next_shift(world, m)
                && leave - now <= window
                && geom::dist(wp, pos_of(world, Some(mj.workplace))) <= balance::CARSHARE_RADIUS_M
            {
                world.citizens[m].worked_day = md;
                riders.push(m);
            }
        }
    }
    go(world, id, job.workplace, TripPurpose::ToWork, &riders);
}

fn go_shopping(world: &mut World, id: CitizenId) -> bool {
    let c = &world.citizens[id];
    let Some(from) = c.location.building() else {
        return false;
    };
    let h = &world.households[c.household];
    let want = (h.citizens.len() as f32 * balance::BUY_UP_TO_DAYS - h.goods).max(1.0);
    let hid = c.household;
    let fp = pos_of(world, Some(from));
    let Some(shop) = pick_shop(world, fp, want) else {
        return false;
    };
    // Only if they get there (in today's traffic) before the shop shuts.
    let arrive_h = world.time.hour() + secs_to_ticks(road_s(world, fp, pos_of(world, Some(shop)))) as f32 / HOUR as f32;
    if arrive_h > balance::SHOP_CLOSE_H - balance::SHOP_LAST_ENTRY_BEFORE_CLOSE_H {
        return false;
    }
    world.households[hid].shopper = Some(id);
    go(world, id, shop, TripPurpose::Shopping, &[]);
    true
}

pub(crate) fn go_leisure(world: &mut World, id: CitizenId) -> bool {
    let c = &world.citizens[id];
    let Some(from) = c.location.building() else {
        return false;
    };
    let car = c.drives() && (c.has_car || world.households[c.household].free_cars(world) > 0);
    let fp = pos_of(world, Some(from));
    let Some(venue) = pick_venue(world, fp) else {
        return false;
    };
    // Only if getting there takes less than the night out itself (a long
    // trip without a car is not worth it) and there is time for it before
    // the venue closes.
    let vp = pos_of(world, Some(venue));
    let factor = if car || geom::dist(fp, vp) <= balance::WALK_MAX_M {
        1.0
    } else {
        balance::CARLESS_TIME_FACTOR
    };
    let trip_h = secs_to_ticks(estimate_s(fp, vp) * factor) as f32 / HOUR as f32;
    // In today's traffic (by car), there and back.
    let road_h = if car {
        trip_h * traffic::delay_factor(world)
    } else {
        trip_h
    };
    let hour = world.time.hour();
    if trip_h > balance::LEISURE_DURATION_H
        || hour + road_h + balance::LEISURE_DURATION_H > balance::LEISURE_LAST_H
        || hour + 2.0 * road_h + balance::LEISURE_DURATION_H > balance::LEISURE_HOME_BY_H
    {
        return false;
    }
    let kids = companions_at(world, id, from, true);
    go(world, id, venue, TripPurpose::Leisure, &kids);
    true
}

/// Nearest of a few random open shops with goods, preferring one that has
/// all that is wanted.
fn pick_shop(world: &mut World, from: [f32; 2], want: f32) -> Option<BuildingId> {
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let n = world.cache.shops.len();
    if n == 0 {
        return None;
    }
    let mut best: Option<(f32, BuildingId)> = None;
    for _ in 0..balance::SHOP_SAMPLES {
        let b = world.cache.shops[world.rng.below(n as u32) as usize];
        let Some(p) = world.people.places.get(b) else { continue };
        if p.staff_present == 0 || p.stock < 1.0 {
            continue;
        }
        let Some(bb) = world.buildings.get(b) else { continue };
        if !crate::households::connected(world, bb.footprint) {
            continue;
        }
        let short = if p.stock < want {
            balance::SHOP_SHORT_STOCK_M
        } else {
            0.0
        };
        let score = geom::dist(from, rect_center(bb.footprint)) + short;
        if best.is_none_or(|x| score < x.0) {
            best = Some((score, b));
        }
    }
    best.map(|b| b.1)
}

/// How well a venue restores the leisure need, per hour.
pub(crate) fn restore_rate(level: u8) -> f32 {
    balance::LEISURE_RESTORE_PER_HOUR[(level.clamp(1, 3) - 1) as usize]
}

/// Best of a few random open venues by restoration over travel (a midday
/// visit stays nearer, `MIDDAY_DIST_SCALE_M`).
fn pick_venue(world: &mut World, from: [f32; 2]) -> Option<BuildingId> {
    let scale = if midday_visit(world.time.hour()) {
        balance::MIDDAY_DIST_SCALE_M
    } else {
        balance::LEISURE_DIST_SCALE_M
    };
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let n = world.cache.shops.len();
    if n == 0 {
        return None;
    }
    let mut best: Option<(f32, BuildingId)> = None;
    for _ in 0..balance::LEISURE_SAMPLES {
        let b = world.cache.shops[world.rng.below(n as u32) as usize];
        let Some(p) = world.people.places.get(b) else { continue };
        if p.staff_present == 0 {
            continue;
        }
        let Some(bb) = world.buildings.get(b) else { continue };
        if !crate::households::connected(world, bb.footprint) {
            continue;
        }
        let d = geom::dist(from, rect_center(bb.footprint));
        let score = restore_rate(bb.level) / (1.0 + d / scale);
        if best.is_none_or(|x| score > x.0) {
            best = Some((score, b));
        }
    }
    best.map(|b| b.1)
}

/// Start a trip from where `id` is to `to`, taking `passengers` along when
/// travelling by car.
pub(crate) fn go(world: &mut World, id: CitizenId, to: BuildingId, purpose: TripPurpose, passengers: &[CitizenId]) {
    let now = world.time.tick;
    let c = &world.citizens[id];
    let hid = c.household;
    let from = c.location.building();
    let at_home = from == Some(world.households[hid].home);
    let (fp, tp) = (pos_of(world, from), pos_of(world, Some(to)));
    let near = geom::dist(fp, tp) <= balance::WALK_MAX_M;
    let mode = if c.has_car && c.drives() {
        TravelMode::Driving
    } else if near {
        TravelMode::Walking
    } else if c.drives() && at_home && world.households[hid].free_cars(world) > 0 {
        TravelMode::Driving
    } else {
        TravelMode::NoCar
    };
    // On foot the roads do not matter; by car or otherwise they do.
    let est = if mode == TravelMode::Walking {
        estimate_s(fp, tp)
    } else {
        trip_s(world, purpose, fp, tp)
    };
    // Every car trip is a car on the road while the budget allows.
    let lunch = purpose == TripPurpose::Leisure && midday_visit(world.time.hour());
    let drawn = if lunch {
        traffic::drawn_lunch(world, id)
    } else {
        traffic::drawn_driver(world, id)
    };
    let visible = mode == TravelMode::Driving && traffic::has_room(world) && drawn;
    if mode == TravelMode::Driving {
        world.citizens[id].has_car = true;
    }
    let f = if mode == TravelMode::NoCar {
        balance::CARLESS_TIME_FACTOR
    } else {
        1.0
    };
    let arrive_tick = (!visible).then(|| now + secs_to_ticks(offscreen_s(world, est * f)));
    let expected = if purpose == TripPurpose::ToWork && world.citizens[id].workplace() == Some(to) {
        expected_commute_ticks(world, id)
    } else {
        secs_to_ticks(est)
    };
    let due_tick = arrive_tick.unwrap_or(now + expected);
    let journey = Journey {
        purpose,
        from,
        to,
        mode,
        driver: None,
        vehicle: None,
        departed_tick: now,
        arrive_tick,
        due_tick,
    };
    if visible {
        let mut d = Departure::new(world, purpose, Place::At(fp), Place::At(tp));
        d.household = Some(hid);
        d.citizen = Some(id);
        d.from_building = from;
        d.to_building = Some(to);
        traffic::request_trip(world, d);
    }
    let until = arrive_tick.unwrap_or(now + HOUR);
    people::set_place(world, id, Activity::Travelling, Location::Travelling(journey), until);
    people::count_trip(world, purpose);
    for &p in passengers {
        if purpose == TripPurpose::ToWork {
            match world.citizens[p].workplace() {
                // Car-sharing: off at their own workplace.
                Some(w) if mode == TravelMode::Driving => {
                    let j = Journey {
                        mode: TravelMode::Passenger,
                        driver: Some(id),
                        to: w,
                        ..journey
                    };
                    people::set_place(world, p, Activity::Travelling, Location::Travelling(j), until);
                }
                // No lift after all: they make their own way.
                Some(w) => go(world, p, w, TripPurpose::ToWork, &[]),
                None => {}
            }
            continue;
        }
        // Children (and teens) come along however the adult travels.
        let j = if mode == TravelMode::Driving {
            Journey {
                mode: TravelMode::Passenger,
                driver: Some(id),
                ..journey
            }
        } else {
            journey
        };
        people::set_place(world, p, Activity::Travelling, Location::Travelling(j), until);
    }
}

/// Newcomers drive in from the highway together (`driver` drives).
pub(crate) fn move_in(world: &mut World, hid: crate::HouseholdId, driver: CitizenId, members: &[CitizenId], now: u64) {
    let home = world.households[hid].home;
    let drives = world.households[hid].cars > 0 && world.citizens[driver].drives();
    // Drawn on the road (the car share and the budget allow), or driving in
    // off screen.
    let has_car = drives && traffic::has_room(world) && traffic::drawn_driver(world, driver);
    let journey = Journey {
        purpose: TripPurpose::MoveIn,
        from: None,
        to: home,
        mode: if drives { TravelMode::Driving } else { TravelMode::NoCar },
        driver: None,
        vehicle: None,
        departed_tick: now,
        arrive_tick: None,
        due_tick: now + due_after(world, None, Some(home)),
    };
    let to = place_of(world, home).expect("home exists");
    let journey = if has_car {
        world.citizens[driver].has_car = true;
        let mut d = Departure::new(world, TripPurpose::MoveIn, Place::Entry, to);
        d.household = Some(hid);
        d.citizen = Some(driver);
        d.to_building = Some(home);
        traffic::request_trip(world, d);
        journey
    } else {
        let (from, to) = (pos_of(world, None), pos_of(world, Some(home)));
        let t = if drives {
            now + secs_to_ticks(offscreen_s(world, road_s(world, from, to)))
        } else {
            now + secs_to_ticks(estimate_s(from, to) * balance::CARLESS_TIME_FACTOR)
        };
        Journey {
            arrive_tick: Some(t),
            due_tick: journey.due_tick.max(t),
            ..journey
        }
    };
    let until = journey.arrive_tick.unwrap_or(now + HOUR);
    people::set_place(
        world,
        driver,
        Activity::Travelling,
        Location::Travelling(journey),
        until,
    );
    people::count_trip(world, TripPurpose::MoveIn);
    for &m in members {
        if m == driver {
            continue;
        }
        let j = if has_car {
            Journey {
                mode: TravelMode::Passenger,
                driver: Some(driver),
                ..journey
            }
        } else {
            journey
        };
        people::set_place(world, m, Activity::Travelling, Location::Travelling(j), until);
    }
}

/// A trip that cannot (or can no longer) be on the road finishes off
/// screen: arrival after the estimated time from now.
pub(crate) fn continue_off_screen(world: &mut World, id: CitizenId) {
    let now = world.time.tick;
    let Some(c) = world.citizens.get(id) else { return };
    let Location::Travelling(j) = c.location else { return };
    let est = trip_s(world, j.purpose, pos_of(world, j.from), pos_of(world, Some(j.to)));
    let t = now + secs_to_ticks(offscreen_s(world, est) * 0.5);
    let j = Journey {
        vehicle: None,
        arrive_tick: Some(t),
        due_tick: j.due_tick.max(t),
        mode: if j.mode == TravelMode::Passenger {
            TravelMode::NoCar
        } else {
            j.mode
        },
        driver: None,
        ..j
    };
    people::set_place(world, id, Activity::Travelling, Location::Travelling(j), t);
}

/// Members riding with `driver` right now.
fn riders_of(world: &World, driver: CitizenId) -> Vec<CitizenId> {
    let Some(c) = world.citizens.get(driver) else {
        return Vec::new();
    };
    world.households[c.household]
        .citizens
        .iter()
        .copied()
        .filter(|&m| {
            world.citizens.get(m).is_some_and(
                |m| matches!(&m.location, Location::Travelling(j) if j.mode == TravelMode::Passenger && j.driver == Some(driver)),
            )
        })
        .collect()
}

/// A car trip ended (traffic callback).
pub(crate) fn trip_ended(world: &mut World, e: Ended) {
    if e.purpose == TripPurpose::Goods {
        crate::errands::delivered(world, e.to, e.to_building);
        return;
    }
    if let Some(hid) = e.household
        && let Some(h) = world.households.get_mut(hid)
        && h.citizens.is_empty()
    {
        // Legacy household without citizens: nothing more to do.
        h.vehicle = None;
        return;
    }
    let Some(id) = e.citizen.filter(|&c| world.citizens.contains_key(c)) else {
        // A car whose people already got out on the calendar finished its
        // drive: its commute still counts for its owner.
        if let Some(o) = e.owner.filter(|&c| world.citizens.contains_key(c))
            && let TripEnd::Arrived(s) | TripEnd::GaveUp(s) = e.end
        {
            record_commute(world, o, e.purpose, s, matches!(e.end, TripEnd::GaveUp(_)));
        }
        return;
    };
    let Location::Travelling(j) = world.citizens[id].location else {
        return;
    };
    let riders = riders_of(world, id);
    let now = world.time.tick;
    match e.end {
        TripEnd::Arrived(s) => {
            record_commute(world, id, j.purpose, s, false);
            arrive(world, id);
            for r in riders {
                arrive(world, r);
            }
        }
        TripEnd::GaveUp(_) | TripEnd::Vanished => {
            // The car was taken off the road (stuck too long, or its road
            // was removed): its people make the rest of the way off screen,
            // from where the car was.
            if let TripEnd::GaveUp(s) = e.end {
                record_commute(world, id, j.purpose, s, true);
            }
            let from = e.at.unwrap_or_else(|| pos_of(world, j.from));
            let t = now + secs_to_ticks(trip_s(world, j.purpose, from, pos_of(world, Some(j.to))));
            for m in std::iter::once(id).chain(riders) {
                if let Location::Travelling(mj) = world.citizens[m].location {
                    let nj = Journey {
                        vehicle: None,
                        arrive_tick: Some(t),
                        due_tick: t,
                        ..mj
                    };
                    people::set_place(world, m, Activity::Travelling, Location::Travelling(nj), t);
                }
            }
        }
        TripEnd::Cancelled => {
            // Could not start in time: off screen, counted as a hopeless
            // commute (the driveway or the network was jammed).
            record_commute(world, id, j.purpose, balance::MAX_DEPARTURE_WAIT_S, true);
            let est = trip_s(world, j.purpose, pos_of(world, j.from), pos_of(world, Some(j.to)));
            let t = now + secs_to_ticks(est);
            for m in std::iter::once(id).chain(riders) {
                if let Location::Travelling(mj) = world.citizens[m].location {
                    let nj = Journey {
                        vehicle: None,
                        arrive_tick: Some(t),
                        due_tick: mj.due_tick.max(t),
                        ..mj
                    };
                    people::set_place(world, m, Activity::Travelling, Location::Travelling(nj), t);
                }
            }
        }
        TripEnd::NoRoute => {
            // No way there: back where they started. A job that cannot be
            // reached is given up.
            let hid = world.citizens[id].household;
            let home = world.households[hid].home;
            for m in std::iter::once(id).chain(riders) {
                let back = j.from.filter(|b| world.buildings.contains_key(*b)).unwrap_or(home);
                if back == home {
                    world.citizens[m].has_car = false;
                }
                people::set_place(world, m, Activity::Home, Location::In(back), now + HOUR / 2);
                if j.purpose == TripPurpose::ToWork {
                    people::fire(world, m, true);
                }
            }
            let h = &mut world.households[hid];
            if h.shopper == Some(id) {
                h.shopper = None;
            }
        }
    }
}

fn record_commute(world: &mut World, id: CitizenId, purpose: TripPurpose, s: f32, gave_up: bool) {
    if !matches!(purpose, TripPurpose::ToWork | TripPurpose::ToHome) || s <= 0.0 {
        return;
    }
    let c = &mut world.citizens[id];
    c.last_commute_s = s;
    if s > balance::HOPELESS_COMMUTE_S || gave_up {
        c.hopeless_commutes = c.hopeless_commutes.saturating_add(1);
    } else {
        c.hopeless_commutes = 0;
    }
    let hid = c.household;
    world.households[hid].last_commute_s = s;
    people::refresh_household(world, hid);
}

/// The citizen reaches the end of their journey.
fn arrive(world: &mut World, id: CitizenId) {
    let now = world.time.tick;
    let c = &world.citizens[id];
    let Location::Travelling(j) = c.location else { return };
    let hid = c.household;
    let home = world.households[hid].home;
    let to = if world.buildings.contains_key(j.to) { j.to } else { home };
    world.people.trip_times_today.add(
        now.saturating_sub(j.departed_tick),
        matches!(j.purpose, TripPurpose::ToWork | TripPurpose::ToHome),
        j.purpose.index() as u8,
        j.arrive_tick.is_some(),
    );
    if j.purpose == TripPurpose::ToWork && c.workplace() == Some(to) {
        // Door to door, waiting for the car included: they plan the next
        // commute by it.
        let took = now.saturating_sub(j.departed_tick).min(u32::MAX as u64) as u32;
        world.citizens[id].commute_ticks = took.max(1);
    }
    let c = &world.citizens[id];
    if j.arrive_tick.is_some() && matches!(j.purpose, TripPurpose::ToWork | TripPurpose::ToHome) && c.job.is_some() {
        // Off screen: the estimated drive (the commute a car would have,
        // in today's traffic; on foot without it).
        let (a, b) = (pos_of(world, j.from), pos_of(world, Some(j.to)));
        let s = offscreen_s(
            world,
            match j.mode {
                TravelMode::Walking => estimate_s(a, b),
                TravelMode::NoCar => road_s(world, a, b) * balance::CARLESS_TIME_FACTOR,
                _ => road_s(world, a, b),
            },
        );
        world.citizens[id].last_commute_s = s;
    }
    // Commuters' time lost in traffic costs business (by the driver /
    // traveller: each person's own trip).
    if matches!(j.purpose, TripPurpose::ToWork | TripPurpose::ToHome) && world.citizens[id].job.is_some() {
        let took = now.saturating_sub(j.departed_tick) as f32 / TICKS_PER_SECOND as f32;
        let free = estimate_s(pos_of(world, j.from), pos_of(world, Some(j.to)));
        crate::economy::record_commute_time(world, took, free);
    }
    if to == home {
        world.citizens[id].has_car = false;
        let h = &mut world.households[hid];
        if h.shopper == Some(id) {
            h.shopper = None;
        }
        people::set_place(world, id, Activity::Home, Location::In(home), now);
        return;
    }
    let c = &world.citizens[id];
    if j.purpose == TripPurpose::Leisure
        && c.workplace() == Some(to)
        && let Some(end) = on_shift(world, id)
    {
        // Back from a lunch break: work until the shift ends.
        people::set_place(world, id, Activity::Working, Location::In(to), at(end));
        return;
    }
    match j.purpose {
        TripPurpose::ToWork if c.workplace() == Some(to) => {
            let job = c.job.expect("checked");
            let (_, _, end) = shift_times(world, id, job.shift, c.worked_day);
            let left = (end - now as i64).clamp(0, job.shift.length as i64) as u64;
            world.economy.attendance.worked_ticks += left;
            if end > now as i64 + HOUR as i64 / 4 {
                let (_, start, _) = shift_times(world, id, job.shift, c.worked_day);
                let until = lunch_tick(world, id, c.worked_day, start, end)
                    .filter(|&t| t > now as i64)
                    .map_or(end, |t| t.min(end));
                people::set_place(world, id, Activity::Working, Location::In(to), at(until));
            } else {
                // Too late for the shift: turn round.
                people::set_place(world, id, Activity::Home, Location::In(to), now);
            }
        }
        TripPurpose::Shopping => {
            let tod = world.time.hour();
            let close = now + hours((balance::SHOP_CLOSE_H - tod).max(0.0)) as u64;
            if !(balance::SHOP_OPEN_H..balance::SHOP_CLOSE_H).contains(&tod) {
                // Shut by the time they got there: home again.
                people::set_place(world, id, Activity::Home, Location::In(to), now);
                return;
            }
            buy(world, id, to);
            let t = (now + hours(balance::SHOP_DURATION_H) as u64).min(close.max(now + 1));
            people::set_place(world, id, Activity::Shopping, Location::In(to), t);
        }
        TripPurpose::Leisure => {
            let tod = world.time.hour();
            if !(balance::LEISURE_OPEN_H..balance::LEISURE_LAST_H).contains(&tod) {
                // The venue has closed: home again.
                people::set_place(world, id, Activity::Home, Location::In(to), now);
                return;
            }
            let close = now + hours(balance::LEISURE_LAST_H - tod) as u64;
            let c = &world.citizens[id];
            let d = if on_shift(world, id).is_some() {
                balance::LUNCH_BREAK_VISIT_H
            } else if midday_visit(tod) {
                balance::MIDDAY_VISIT_H + habit(c.seed, 0x1E1) * balance::MIDDAY_VISIT_SPREAD_H
            } else {
                balance::LEISURE_DURATION_H + habit(c.seed, 0x1E1) * balance::LEISURE_DURATION_SPREAD_H
            };
            if c.drives() {
                let h = &mut world.households[hid];
                h.money -= balance::LEISURE_COST;
                h.ledger.leisure += balance::LEISURE_COST;
                h.today.leisure += balance::LEISURE_COST;
            }
            // Home by `LEISURE_HOME_BY_H` in today's traffic, if the
            // visit can still be a short one.
            let trip_home = secs_to_ticks(road_s(world, pos_of(world, Some(to)), pos_of(world, Some(home))));
            let home_by =
                at(midnight(world, world.time.day) + hours(balance::LEISURE_HOME_BY_H) as i64 - trip_home as i64);
            let end = (now + hours(d) as u64)
                .min(close.max(now + 1))
                .min(home_by.max(now + HOUR / 4));
            people::set_place(world, id, Activity::Leisure, Location::In(to), end);
        }
        _ => people::set_place(world, id, Activity::Home, Location::In(to), now),
    }
}

/// Buy goods for the household at `shop`.
fn buy(world: &mut World, id: CitizenId, shop: BuildingId) {
    let hid = world.citizens[id].household;
    let h = &world.households[hid];
    let n = h.citizens.len() as f32;
    let days = if h.austerity {
        balance::AUSTERITY_BUY_DAYS
    } else {
        balance::BUY_UP_TO_DAYS
    };
    let want = (n * days - h.goods).max(0.0);
    let cap = world.buildings.get(shop).map_or(0.0, crate::errands::stock_cap);
    let p = people::place(world, shop);
    let got = want.min(p.stock).floor();
    if got <= 0.0 {
        return;
    }
    p.stock -= got;
    p.sold_today += got;
    let restock = p.stock < cap * balance::RESTOCK_BELOW && !p.delivery_pending;
    if restock {
        p.delivery_pending = true;
        world.people.restock.push_back(shop);
    }
    let cost = (got * balance::GOODS_PRICE).round() as i64;
    let h = &mut world.households[hid];
    h.goods += got;
    h.money -= cost;
    h.ledger.shopping += cost;
    h.today.shopping += cost;
}

/// Leisure need change per hour while doing `activity` at `location`.
pub(crate) fn need_rate(world: &World, activity: Activity, location: &Location) -> f32 {
    match activity {
        Activity::Sleeping => 0.0,
        Activity::Working => balance::LEISURE_NEED_PER_HOUR * balance::LEISURE_NEED_WORK_FACTOR,
        Activity::Leisure => {
            let level = location
                .building()
                .and_then(|b| world.buildings.get(b))
                .map_or(1, |b| b.level);
            -restore_rate(level)
        }
        Activity::Home => balance::LEISURE_NEED_PER_HOUR - balance::LEISURE_HOME_RESTORE_PER_HOUR,
        _ => balance::LEISURE_NEED_PER_HOUR,
    }
}
