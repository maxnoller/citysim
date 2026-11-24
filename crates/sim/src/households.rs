//! Households: a home, maybe a job, maybe a car on the road.
//!
//! * Newcomers arrive through the highway stub while there is free housing
//!   and residential demand is not too negative; their home is reserved on
//!   arrival at the city edge and they drive in.
//! * Each household has one worker. Unemployed households are matched to
//!   free jobs reachable by road, preferring nearer ones (nearest of a few
//!   random vacancies).
//! * Workers leave home in a staggered morning rush and come back in the
//!   evening. On a given day a household drives with probability
//!   `COMMUTE_DRIVE_SHARE`; otherwise it switches place off screen.
//! * Households leave town after staying unemployed or after repeated
//!   hopeless commutes.

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::derived::rect_center;
use crate::traffic::{self, Place, TripEnd, TripPurpose};
use crate::{BuildingId, CellRect, VehicleId, World, Zone, balance};

new_key_type! {
    /// Stable id of a [`Household`].
    pub struct HouseholdId;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HouseholdState {
    /// At home (also: about to leave, waiting for the car to start).
    #[default]
    AtHome,
    /// Driving (or waiting to drive) to work.
    CommutingToWork,
    AtWork,
    /// Driving home, or moving in from the highway.
    CommutingHome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Household {
    pub home: BuildingId,
    pub job: Option<BuildingId>,
    /// People in the household (counts towards population).
    pub members: u8,
    pub state: HouseholdState,
    /// The car while it is on the road.
    pub vehicle: Option<VehicleId>,
    /// Stable random number: departure times, who drives which day.
    #[serde(default)]
    pub seed: u32,
    /// Consecutive midnights without a job.
    #[serde(default)]
    pub unemployed_days: u8,
    /// Consecutive car commutes over `HOPELESS_COMMUTE_S`.
    #[serde(default)]
    pub hopeless_commutes: u8,
    /// Duration of the last car commute, vehicle seconds (0 = none yet).
    #[serde(default)]
    pub last_commute_s: f32,
    /// Day the household last set off to work.
    #[serde(default)]
    pub last_work_day: u32,
}

fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn unit(x: u64) -> f32 {
    (x >> 40) as f32 / (1u64 << 24) as f32
}

fn hour_tick(h: f32) -> u32 {
    (h * balance::TICKS_PER_HOUR as f32) as u32
}

/// Tick of day this household leaves in the morning / evening of `day`:
/// a personal habit plus a daily wobble, triangular around the rush peak.
fn departure(seed: u32, day: u32, start_h: f32, spread_h: f32, salt: u64) -> u32 {
    let a = unit(hash(seed as u64 ^ salt));
    let b = unit(hash((seed as u64) << 32 ^ day as u64 ^ salt.rotate_left(17)));
    hour_tick(start_h + spread_h * (2.0 * a + b) / 3.0)
}

fn drives_on(seed: u32, day: u32) -> bool {
    unit(hash((seed as u64) << 20 ^ day as u64 ^ 0xD71E)) < balance::COMMUTE_DRIVE_SHARE
}

fn place_of(world: &World, b: BuildingId) -> Option<Place> {
    world.buildings.get(b).map(|b| Place::At(rect_center(b.footprint)))
}

/// Per-tick household logic.
pub(crate) fn tick(world: &mut World) {
    if world.time.tick_of_day == 0 {
        midnight(world);
    }
    commutes(world);
    if world.time.tick.is_multiple_of(balance::JOB_MATCH_INTERVAL_TICKS) {
        match_jobs(world);
    }
    move_in(world);
}

fn commutes(world: &mut World) {
    let tod = world.time.tick_of_day;
    let day = world.time.day;
    let latest = hour_tick(balance::MORNING_DEPART_LATEST_H);
    let morning_start = hour_tick(balance::MORNING_DEPART_START_H);
    let mut trips: Vec<(HouseholdId, TripPurpose)> = Vec::new();
    for (id, h) in world.households.iter_mut() {
        match h.state {
            HouseholdState::AtHome
                if h.job.is_some()
                    && h.last_work_day != day
                    && tod < latest
                    && tod
                        >= departure(
                            h.seed,
                            day,
                            balance::MORNING_DEPART_START_H,
                            balance::MORNING_DEPART_SPREAD_H,
                            1,
                        ) =>
            {
                h.last_work_day = day;
                if drives_on(h.seed, day) {
                    h.state = HouseholdState::CommutingToWork;
                    trips.push((id, TripPurpose::ToWork));
                } else {
                    h.state = HouseholdState::AtWork;
                }
            }
            HouseholdState::AtWork
                if tod < morning_start
                    || tod
                        >= departure(
                            h.seed,
                            day,
                            balance::EVENING_DEPART_START_H,
                            balance::EVENING_DEPART_SPREAD_H,
                            2,
                        ) =>
            {
                if h.job.is_some() && drives_on(h.seed, day) {
                    h.state = HouseholdState::CommutingHome;
                    trips.push((id, TripPurpose::ToHome));
                } else {
                    h.state = HouseholdState::AtHome;
                }
            }
            _ => {}
        }
    }
    for (id, purpose) in trips {
        let h = &world.households[id];
        let (from, to) = match purpose {
            TripPurpose::ToWork => (Some(h.home), h.job),
            _ => (h.job, Some(h.home)),
        };
        match (
            from.and_then(|b| place_of(world, b)),
            to.and_then(|b| place_of(world, b)),
        ) {
            (Some(f), Some(t)) => traffic::request(world, Some(id), purpose, f, t),
            _ => trip_ended(world, (Some(id), purpose, TripEnd::Vanished)),
        }
    }
}

/// A trip of this household ended (called by traffic).
pub(crate) fn trip_ended(world: &mut World, (household, purpose, end): traffic::Ended) {
    let Some(id) = household else { return };
    let Some(h) = world.households.get_mut(id) else { return };
    h.vehicle = None;
    let secs = match end {
        TripEnd::Arrived(s) | TripEnd::GaveUp(s) => Some(s),
        _ => None,
    };
    let seed = h.seed;
    match purpose {
        TripPurpose::ToWork if end == TripEnd::NoRoute => {
            // Job unreachable: quit it and stay home.
            h.state = HouseholdState::AtHome;
            if let Some(j) = h.job.take()
                && let Some(b) = world.buildings.get_mut(j)
            {
                b.occupied = b.occupied.saturating_sub(1);
            }
        }
        TripPurpose::ToWork => {
            // Arrived after knocking-off time: gets a lift home off screen
            // instead of adding another car to the jam.
            let tod = world.time.tick_of_day;
            let evening = departure(
                seed,
                world.time.day,
                balance::EVENING_DEPART_START_H,
                balance::EVENING_DEPART_SPREAD_H,
                2,
            );
            h.state = if tod >= evening || tod < hour_tick(balance::MORNING_DEPART_START_H) {
                HouseholdState::AtHome
            } else {
                HouseholdState::AtWork
            };
        }
        TripPurpose::ToHome | TripPurpose::MoveIn => h.state = HouseholdState::AtHome,
        TripPurpose::MoveOut => {}
    }
    let secs = if end == TripEnd::Cancelled {
        Some(balance::MAX_DEPARTURE_WAIT_S)
    } else {
        secs
    };
    if let Some(s) = secs
        && matches!(purpose, TripPurpose::ToWork | TripPurpose::ToHome)
        && s > 0.0
    {
        let h = &mut world.households[id];
        h.last_commute_s = s;
        if s > balance::HOPELESS_COMMUTE_S || matches!(end, TripEnd::GaveUp(_) | TripEnd::Cancelled) {
            h.hopeless_commutes = h.hopeless_commutes.saturating_add(1);
        } else {
            h.hopeless_commutes = 0;
        }
    }
}

fn midnight(world: &mut World) {
    let mut leaving = Vec::new();
    for (id, h) in world.households.iter_mut() {
        if h.job.is_none() {
            h.unemployed_days = h.unemployed_days.saturating_add(1);
        } else {
            h.unemployed_days = 0;
        }
        if h.unemployed_days >= balance::UNEMPLOYED_LEAVE_DAYS
            || h.hopeless_commutes >= balance::HOPELESS_COMMUTES_TO_LEAVE
        {
            leaving.push(id);
        }
    }
    for id in leaving {
        leave(world, id, balance::MOVE_OUT_DRIVES);
    }
}

/// Remove a household from the city, freeing its home and job.
pub(crate) fn leave(world: &mut World, id: HouseholdId, drive_out: bool) {
    let Some(h) = world.households.remove(id) else { return };
    let home = world.buildings.get(h.home).map(|b| b.footprint);
    if let Some(b) = world.buildings.get_mut(h.home) {
        b.occupied = b.occupied.saturating_sub(h.members as u32);
    }
    if let Some(j) = h.job
        && let Some(b) = world.buildings.get_mut(j)
    {
        b.occupied = b.occupied.saturating_sub(1);
    }
    if let Some(v) = h.vehicle.and_then(|v| world.vehicles.get_mut(v)) {
        v.household = None;
    }
    if drive_out
        && let Some(r) = home
        && h.vehicle.is_none()
    {
        traffic::request(
            world,
            None,
            TripPurpose::MoveOut,
            Place::At(rect_center(r)),
            Place::Entry,
        );
    }
}

/// Buildings were removed (bulldozed or abandoned): their residents leave,
/// their workers lose the job.
pub(crate) fn buildings_removed(world: &mut World, ids: &[BuildingId]) {
    if ids.is_empty() {
        return;
    }
    let mut leaving = Vec::new();
    for (hid, h) in world.households.iter_mut() {
        if ids.contains(&h.home) {
            leaving.push(hid);
        } else if h.job.is_some_and(|j| ids.contains(&j)) {
            h.job = None;
        }
    }
    for hid in leaving {
        leave(world, hid, false);
    }
}

/// Can a building be reached from the highway?
pub(crate) fn connected(world: &World, r: CellRect) -> bool {
    world.cache.connected_near(r, balance::ZONE_ROAD_REACH + 1)
}

fn match_jobs(world: &mut World) {
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    let mut seekers: Vec<(HouseholdId, [f32; 2])> = Vec::new();
    for (id, h) in world.households.iter() {
        if h.job.is_none()
            && let Some(b) = world.buildings.get(h.home)
        {
            seekers.push((id, rect_center(b.footprint)));
            if seekers.len() >= balance::JOB_MATCH_PER_ROUND {
                break;
            }
        }
    }
    if seekers.is_empty() {
        return;
    }
    // Vacancies: (building, centre, free jobs).
    let mut vac: Vec<(BuildingId, [f32; 2], u32)> = world
        .buildings
        .iter()
        .filter(|(_, b)| b.kind != Zone::Residential && b.occupied < b.capacity)
        .filter(|(_, b)| connected(world, b.footprint))
        .map(|(id, b)| (id, rect_center(b.footprint), b.capacity - b.occupied))
        .collect();
    for (hid, home) in seekers {
        if vac.is_empty() {
            break;
        }
        let Some(hb) = world.buildings.get(world.households[hid].home) else {
            continue;
        };
        if !connected(world, hb.footprint) {
            continue;
        }
        let mut best: Option<(f32, usize)> = None;
        for _ in 0..balance::JOB_MATCH_SAMPLES.min(vac.len() * 2) {
            let i = world.rng.below(vac.len() as u32) as usize;
            let d = crate::geom::dist(home, vac[i].1);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, i));
            }
        }
        let (_, i) = best.expect("vac not empty");
        let (bid, _, free) = &mut vac[i];
        let bid = *bid;
        *free -= 1;
        if *free == 0 {
            vac.swap_remove(i);
        }
        world.buildings[bid].occupied += 1;
        let h = &mut world.households[hid];
        h.job = Some(bid);
        h.unemployed_days = 0;
        h.hopeless_commutes = 0;
    }
}

fn move_in(world: &mut World) {
    let free = world.stats.residential_capacity.saturating_sub(world.stats.population) as f32;
    let rate = if world.growth.attractiveness > balance::MOVE_IN_MIN_DEMAND && free >= 1.0 {
        (free / 2.6 * balance::MOVE_IN_FILL_PER_HOUR).min(balance::MOVE_IN_MAX_PER_HOUR)
    } else {
        0.0
    };
    let g = &mut world.growth;
    g.move_in_acc = (g.move_in_acc + rate / balance::TICKS_PER_HOUR as f32).min(4.0);
    if rate == 0.0 {
        g.move_in_acc = 0.0;
    }
    if g.move_in_acc < 1.0 {
        return;
    }
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    let homes: Vec<(BuildingId, u32)> = world
        .buildings
        .iter()
        .filter(|(_, b)| b.kind == Zone::Residential && b.occupied < b.capacity)
        .filter(|(_, b)| connected(world, b.footprint))
        .map(|(id, b)| (id, b.capacity - b.occupied))
        .collect();
    while world.growth.move_in_acc >= 1.0 {
        world.growth.move_in_acc -= 1.0;
        if homes.is_empty() {
            world.growth.move_in_acc = 0.0;
            break;
        }
        let (home, free) = homes[world.rng.below(homes.len() as u32) as usize];
        let free = world.buildings[home]
            .capacity
            .saturating_sub(world.buildings[home].occupied)
            .min(free);
        if free == 0 {
            continue;
        }
        let w = balance::HOUSEHOLD_SIZE_WEIGHTS;
        let mut r = world.rng.below(w.iter().sum());
        let mut members = 1u32;
        for (i, &wi) in w.iter().enumerate() {
            if r < wi {
                members = i as u32 + 1;
                break;
            }
            r -= wi;
        }
        let members = members.min(free);
        world.buildings[home].occupied += members;
        let seed = world.rng.next_u64() as u32;
        let id = world.households.insert(Household {
            home,
            job: None,
            members: members as u8,
            state: HouseholdState::CommutingHome,
            vehicle: None,
            seed,
            unemployed_days: 0,
            hopeless_commutes: 0,
            last_commute_s: 0.0,
            last_work_day: 0,
        });
        let to = place_of(world, home).expect("home exists");
        traffic::request(world, Some(id), TripPurpose::MoveIn, Place::Entry, to);
    }
}
