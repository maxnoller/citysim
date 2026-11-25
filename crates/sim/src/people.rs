//! Bookkeeping shared by the population rules: per-building lists of
//! households and workers, presence counters, the workforce and wage
//! counters, and the only functions that change a citizen's place,
//! activity, job or life stage (so every counter stays exact).
//!
//! Everything here is serialised with the World and checked by
//! `World::check_sim_invariants` against a recount from the citizens.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::citizens::{Activity, Citizen, CitizenId, Job, JobTier, LifeEvent, LifeStage, Location, TravelMode};
use crate::inspect::CityPeople;
use crate::side::SideTable;
use crate::traffic::TripPurpose;
use crate::{Building, BuildingId, HouseholdId, HouseholdState, World, Zone, balance};

/// Who lives, works and is present in a building.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PlacePeople {
    /// Households living here (residential buildings), in arrival order.
    pub households: Vec<HouseholdId>,
    /// Citizens employed here (commercial / industrial), in hiring order.
    pub workers: Vec<CitizenId>,
    /// Workers on shift inside right now (a shop is open while it is > 0).
    pub staff_present: u16,
    /// Shoppers and leisure visitors inside right now.
    pub visitors: u16,
    /// Goods on the shelves (commercial), units (one unit = what one
    /// person uses in a day).
    pub stock: f32,
    /// Goods sold today / yesterday (commercial), units.
    pub sold_today: f32,
    pub sold_yesterday: f32,
    /// A delivery truck is on its way (or queued).
    pub delivery_pending: bool,
}

/// Life events in the city on one day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LifeCounts {
    pub births: u32,
    pub deaths: u32,
    /// Households that arrived / left the city.
    pub households_arrived: u32,
    pub households_left: u32,
    /// Citizens that arrived / left the city with their household.
    pub citizens_arrived: u32,
    pub citizens_left: u32,
    /// Grown-up children who moved into a home of their own.
    pub moved_out: u32,
    /// Couples formed (two singles moved in together).
    pub couples: u32,
    /// Households that moved to a cheaper home.
    pub moved_home: u32,
    pub retired: u32,
    /// Jobs found / lost.
    pub hired: u32,
    pub lost_jobs: u32,
}

/// One finished day of population history (for charts).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PeopleDay {
    pub day: u32,
    pub population: u32,
    /// Citizens per life stage (`LifeStage::ALL` order).
    pub by_stage: [u32; 4],
    /// Households per kind (`HouseholdKind::ALL` order).
    pub by_kind: [u32; 4],
    pub avg_happiness: f32,
    pub avg_money: f32,
    pub events: LifeCounts,
    /// Trips started that day per purpose (`TripPurpose::ALL` order).
    pub trips: [u32; TripPurpose::COUNT],
}

/// All population state besides `World::citizens` and the households.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct People {
    pub places: SideTable<BuildingId, PlacePeople>,
    /// Citizens in the workforce (`Citizen::in_workforce`).
    pub workforce: u32,
    /// Citizens per activity right now (`Activity::ALL` order).
    pub activity_counts: [u32; Activity::COUNT],
    /// Employed citizens per daily wage, sorted by wage (the residential
    /// tax of a day is computed from it).
    pub wage_counts: Vec<(u32, u32)>,
    /// Citizens waiting for the job search (re-queued while no job fits).
    pub seekers: VecDeque<CitizenId>,
    /// Shops waiting for a goods delivery.
    pub restock: VecDeque<BuildingId>,
    /// Households known to the population model (detects households added
    /// by writing public fields, which get citizens generated).
    pub known_households: usize,
    /// Trips started today / yesterday per purpose (all modes, on and off
    /// screen; goods deliveries included).
    pub trips_today: [u32; TripPurpose::COUNT],
    pub trips_yesterday: [u32; TripPurpose::COUNT],
    /// Trips started per hour of today / yesterday, per purpose.
    pub trips_by_hour: [[u32; TripPurpose::COUNT]; 24],
    pub trips_by_hour_yesterday: [[u32; TripPurpose::COUNT]; 24],
    pub events_today: LifeCounts,
    pub events_yesterday: LifeCounts,
    /// City-wide statistics, refreshed every `balance::PEOPLE_STATS_INTERVAL_TICKS`.
    pub stats: CityPeople,
    /// One entry per finished day.
    pub history: Vec<PeopleDay>,
    /// Households still to be reviewed tonight (the nightly pass is spread
    /// over the first `balance::NIGHT_CHUNKS` ticks after midnight).
    pub night: Vec<HouseholdId>,
    /// City statistics being gathered (one slice of citizens per tick,
    /// published to `stats` when complete).
    pub stats_acc: crate::inspect::StatsAcc,
}

// ------------------------------------------------------------ helpers ----

pub(crate) fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Uniform `[0, 1)` from a hash.
pub(crate) fn unit(x: u64) -> f32 {
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// A stable per-citizen random number for `salt` (habits).
pub(crate) fn habit(seed: u32, salt: u64) -> f32 {
    unit(hash((seed as u64) ^ salt.wrapping_mul(0xA24B_AED4_963E_E407)))
}

/// A per-citizen random number for `salt` that changes every day.
pub(crate) fn daily(seed: u32, day: u32, salt: u64) -> f32 {
    unit(hash(
        ((seed as u64) << 32 | day as u64) ^ salt.wrapping_mul(0x9FB2_1C65_1E98_DF25),
    ))
}

pub(crate) fn hours(h: f32) -> u32 {
    (h * balance::TICKS_PER_HOUR as f32).max(0.0) as u32
}

pub(crate) fn place(world: &mut World, b: BuildingId) -> &mut PlacePeople {
    world.people.places.entry(b)
}

/// Pay grade of a job in building `b`.
pub fn job_tier(b: &Building) -> JobTier {
    match (b.kind, b.level) {
        (Zone::Commercial, l) if l >= 3 => JobTier::Professional,
        (_, l) if l >= 2 => JobTier::Skilled,
        _ => JobTier::Basic,
    }
}

pub fn wage_of(tier: JobTier, part_time: bool) -> u32 {
    if part_time {
        balance::PART_TIME_WAGE
    } else {
        balance::WAGE_BY_TIER[tier.index()]
    }
}

fn wage_count(world: &mut World, wage: u32, delta: i32) {
    let v = &mut world.people.wage_counts;
    match v.binary_search_by_key(&wage, |&(w, _)| w) {
        Ok(i) => {
            v[i].1 = (v[i].1 as i64 + delta as i64).max(0) as u32;
            if v[i].1 == 0 {
                v.remove(i);
            }
        }
        Err(i) if delta > 0 => v.insert(i, (wage, delta as u32)),
        Err(_) => {}
    }
}

/// Tax the city takes from one day's wages (whole dollars per worker).
pub fn tax_on(wage: u32, rate: f32) -> i64 {
    (wage as f64 * rate as f64).round() as i64
}

/// Residential tax of one day if it ended now: every employed citizen's
/// daily wage times the tax rate.
pub fn residential_tax(world: &World) -> i64 {
    let rate = world.economy.tax_rate;
    world
        .people
        .wage_counts
        .iter()
        .map(|&(w, n)| tax_on(w, rate) * n as i64)
        .sum()
}

// --------------------------------------------------- citizen changes ----

/// Change a citizen through `f`, keeping the workforce counter exact
/// (stage, homemaker and job changes). Not for place or activity (use
/// [`set_place`]) or jobs (use [`hire`] / [`fire`]).
pub(crate) fn update(world: &mut World, id: CitizenId, f: impl FnOnce(&mut Citizen)) {
    let Some(c) = world.citizens.get_mut(id) else { return };
    let before = c.in_workforce();
    f(c);
    let after = c.in_workforce();
    let wf = &mut world.people.workforce;
    *wf = (*wf + u32::from(after)).saturating_sub(u32::from(before));
}

fn presence(world: &mut World, activity: Activity, location: &Location, delta: i32) {
    let n = &mut world.people.activity_counts[activity.index()];
    *n = (*n as i64 + delta as i64).max(0) as u32;
    if let Location::In(b) = *location
        && world.buildings.contains_key(b)
    {
        let p = place(world, b);
        let counter = match activity {
            Activity::Working => &mut p.staff_present,
            Activity::Shopping | Activity::Leisure => &mut p.visitors,
            _ => return,
        };
        *counter = (*counter as i32 + delta).max(0) as u16;
    }
}

/// Bring a citizen's leisure need up to date (it changed at the rate of
/// what they have been doing since it was last updated).
pub(crate) fn catch_up_need(world: &mut World, id: CitizenId) {
    let Some(c) = world.citizens.get(id) else { return };
    let rate = crate::day::need_rate(world, c.activity, &c.location);
    let now = world.time.tick;
    let c = &mut world.citizens[id];
    let dt = now.saturating_sub(c.need_tick) as f32 / balance::TICKS_PER_HOUR as f32;
    c.leisure_need = (c.leisure_need + rate * dt).clamp(0.0, 1.0);
    c.need_tick = now;
}

/// Move a citizen and set what they are doing. The only place that
/// changes `location` and `activity`.
pub(crate) fn set_place(world: &mut World, id: CitizenId, activity: Activity, location: Location, until: u64) {
    catch_up_need(world, id);
    let Some(c) = world.citizens.get(id) else { return };
    let (old_a, old_l, hid) = (c.activity, c.location, c.household);
    presence(world, old_a, &old_l, -1);
    let c = &mut world.citizens[id];
    c.activity = activity;
    c.location = location;
    c.until_tick = until;
    note_until(world, id, until);
    presence(world, activity, &location, 1);
    refresh_household(world, hid);
}

/// Set only when the current activity ends (no change of place).
pub(crate) fn set_until(world: &mut World, id: CitizenId, until: u64) {
    if let Some(c) = world.citizens.get_mut(id) {
        c.until_tick = until;
        note_until(world, id, until);
    }
}

/// Keep the decision buckets' compact copy of `until_tick` in step.
fn note_until(world: &mut World, id: CitizenId, until: u64) {
    let cache = &mut world.cache;
    if !cache.until_ok {
        return;
    }
    let i = crate::idmap::key_index(id);
    if cache.until.len() <= i {
        cache.until.resize(i + 1, u64::MAX);
    }
    cache.until[i] = until;
}

/// Give `id` a job at `workplace` (which must have room).
pub(crate) fn hire(world: &mut World, id: CitizenId, workplace: BuildingId, shift: crate::citizens::Shift) {
    let Some(b) = world.buildings.get(workplace) else {
        return;
    };
    let Some(c) = world.citizens.get(id) else { return };
    if c.job.is_some() {
        return;
    }
    let part_time = c.stage == LifeStage::Teen;
    let hid = c.household;
    let tier = job_tier(b);
    let wage = wage_of(tier, part_time);
    let day = world.time.day;
    world.buildings[workplace].occupied += 1;
    place(world, workplace).workers.push(id);
    wage_count(world, wage, 1);
    update(world, id, |c| {
        c.job = Some(Job {
            workplace,
            tier,
            wage,
            part_time,
            shift,
            since_day: day,
        });
        c.unemployed_days = 0;
        c.hopeless_commutes = 0;
        c.log(day, LifeEvent::GotJob(workplace));
    });
    world.people.events_today.hired += 1;
    refresh_household(world, hid);
}

/// Take `id`'s job away (`log`: write "lost job" in the journal). A worker
/// on shift goes home off screen at their next decision.
pub(crate) fn fire(world: &mut World, id: CitizenId, log: bool) {
    let Some(c) = world.citizens.get(id) else { return };
    let Some(job) = c.job else { return };
    let hid = c.household;
    let was_working = c.activity == Activity::Working;
    let wp = job.workplace;
    if let Some(b) = world.buildings.get_mut(wp) {
        b.occupied = b.occupied.saturating_sub(1);
    }
    if let Some(p) = world.people.places.get_mut(wp)
        && let Some(i) = p.workers.iter().position(|&w| w == id)
    {
        p.workers.remove(i);
    }
    wage_count(world, job.wage, -1);
    let day = world.time.day;
    let now = world.time.tick;
    update(world, id, |c| {
        c.job = None;
        c.hopeless_commutes = 0;
        if log {
            c.log(day, LifeEvent::LostJob);
        }
    });
    if was_working {
        // Off shift: no longer counted as staff; decides again now.
        let loc = world.citizens[id].location;
        set_place(world, id, Activity::Home, loc, now);
        // `Home` away from home: the next decision sends them home.
    }
    if log {
        world.people.events_today.lost_jobs += 1;
    }
    seek(world, id);
    refresh_household(world, hid);
}

/// Change the wage of a worker whose workplace changed level.
pub(crate) fn retier(world: &mut World, workplace: BuildingId) {
    let Some(b) = world.buildings.get(workplace) else {
        return;
    };
    let tier = job_tier(b);
    let workers = world
        .people
        .places
        .get(workplace)
        .map(|p| p.workers.clone())
        .unwrap_or_default();
    for id in workers {
        let Some(job) = world.citizens.get(id).and_then(|c| c.job) else {
            continue;
        };
        if job.tier == tier {
            continue;
        }
        let wage = wage_of(tier, job.part_time);
        wage_count(world, job.wage, -1);
        wage_count(world, wage, 1);
        let j = world.citizens[id].job.as_mut().expect("checked");
        j.tier = tier;
        j.wage = wage;
    }
}

/// Queue a citizen for the job search (if they want a job).
pub(crate) fn seek(world: &mut World, id: CitizenId) {
    if world.citizens.get(id).is_some_and(|c| c.job_seeker()) {
        world.people.seekers.push_back(id);
    }
}

/// Count a trip started now.
pub(crate) fn count_trip(world: &mut World, purpose: TripPurpose) {
    let h = (world.time.hour() as usize).min(23);
    let p = &mut world.people;
    p.trips_today[purpose.index()] += 1;
    p.trips_by_hour[h][purpose.index()] += 1;
}

// ------------------------------------------------- household upkeep ----

/// Re-derive a household's summary fields (`members`, `job`, `state`,
/// `vehicle`, `kind`, `hopeless_commutes`, `last_commute_s`) from its
/// citizens.
pub(crate) fn refresh_household(world: &mut World, hid: HouseholdId) {
    let Some(h) = world.households.get(hid) else { return };
    let mut job = None;
    let mut primary: Option<&Citizen> = None;
    let mut vehicle = None;
    let mut hopeless = 0u8;
    let (mut adults, mut seniors, mut young) = (0u8, 0u8, 0u8);
    for &id in &h.citizens {
        let Some(c) = world.citizens.get(id) else { continue };
        if job.is_none()
            && let Some(j) = c.job
        {
            job = Some(j.workplace);
            primary = Some(c);
        }
        if vehicle.is_none()
            && let Location::Travelling(j) = &c.location
            && j.mode == TravelMode::Driving
            && j.vehicle.is_some()
        {
            vehicle = j.vehicle;
        }
        hopeless = hopeless.max(c.hopeless_commutes);
        match c.stage {
            LifeStage::Adult => adults += 1,
            LifeStage::Senior => seniors += 1,
            _ => young += 1,
        }
    }
    let primary = primary.or_else(|| h.citizens.first().and_then(|&id| world.citizens.get(id)));
    let state = primary.map_or(HouseholdState::AtHome, |c| household_state(c, h.home));
    let kind = crate::households::HouseholdKind::of(adults, seniors, young);
    let members = h.citizens.len().min(u8::MAX as usize) as u8;
    let h = &mut world.households[hid];
    h.job = job;
    h.state = state;
    h.vehicle = vehicle;
    h.kind = kind;
    h.hopeless_commutes = hopeless;
    if members > 0 {
        h.members = members;
    }
}

fn household_state(c: &Citizen, home: BuildingId) -> HouseholdState {
    match &c.location {
        Location::In(b) if *b == home => HouseholdState::AtHome,
        Location::In(_) if c.activity == Activity::Working => HouseholdState::AtWork,
        Location::In(_) => HouseholdState::AtHome,
        Location::Travelling(j) => match j.purpose {
            TripPurpose::ToWork => HouseholdState::CommutingToWork,
            TripPurpose::ToHome | TripPurpose::MoveIn => HouseholdState::CommutingHome,
            _ => HouseholdState::AtHome,
        },
    }
}

// --------------------------------------------------------- recounting ----

/// Recount everything this module keeps incrementally and compare
/// (`World::check_sim_invariants`).
pub(crate) fn check(world: &World) -> Result<(), String> {
    let p = &world.people;
    let mut workforce = 0u32;
    let mut acts = [0u32; Activity::COUNT];
    let mut wages: std::collections::BTreeMap<u32, u32> = Default::default();
    let mut staff: std::collections::BTreeMap<BuildingId, u16> = Default::default();
    let mut visitors: std::collections::BTreeMap<BuildingId, u16> = Default::default();
    let mut workers: std::collections::BTreeMap<BuildingId, Vec<CitizenId>> = Default::default();
    for (id, c) in world.citizens.iter() {
        let Some(h) = world.households.get(c.household) else {
            return Err(format!("{id:?} has missing household"));
        };
        if !h.citizens.contains(&id) {
            return Err(format!("{id:?} not listed by its household"));
        }
        workforce += u32::from(c.in_workforce());
        acts[c.activity.index()] += 1;
        if let Some(j) = c.job {
            if !world.buildings.contains_key(j.workplace) {
                return Err(format!("{id:?} works at a missing building"));
            }
            *wages.entry(j.wage).or_default() += 1;
            workers.entry(j.workplace).or_default().push(id);
        }
        match &c.location {
            Location::In(b) => {
                if !world.buildings.contains_key(*b) {
                    return Err(format!("{id:?} is in a missing building"));
                }
                match c.activity {
                    Activity::Working => *staff.entry(*b).or_default() += 1,
                    Activity::Shopping | Activity::Leisure => *visitors.entry(*b).or_default() += 1,
                    Activity::Travelling => return Err(format!("{id:?} travelling inside a building")),
                    _ => {}
                }
            }
            Location::Travelling(j) => {
                if c.activity != Activity::Travelling {
                    return Err(format!("{id:?} on a trip but {:?}", c.activity));
                }
                if let Some(v) = j.vehicle {
                    let driver = if j.mode == TravelMode::Passenger {
                        j.driver
                    } else {
                        Some(id)
                    };
                    let trip = world.traffic.trips.get(v);
                    if !world.vehicles.contains_key(v) || trip.is_none_or(|t| t.citizen != driver) {
                        return Err(format!("{id:?} rides {v:?} whose trip is not theirs"));
                    }
                }
                if j.vehicle.is_none() && j.arrive_tick.is_none() && j.mode != TravelMode::Passenger {
                    let waiting = world.traffic.pending.iter().any(|d| d.citizen == Some(id));
                    if !waiting {
                        return Err(format!("{id:?} waits for a car that is not queued"));
                    }
                }
            }
        }
    }
    if workforce != p.workforce {
        return Err(format!("workforce {} but citizens say {workforce}", p.workforce));
    }
    if acts != p.activity_counts {
        return Err(format!(
            "activity counts {:?} but citizens say {acts:?}",
            p.activity_counts
        ));
    }
    let wc: Vec<(u32, u32)> = wages.into_iter().collect();
    if wc != p.wage_counts {
        return Err(format!("wage counts {:?} but citizens say {wc:?}", p.wage_counts));
    }
    for (b, pl) in p.places.iter() {
        if !world.buildings.contains_key(b) {
            continue;
        }
        if pl.staff_present != staff.get(&b).copied().unwrap_or(0) {
            return Err(format!("{b:?} staff {} wrong", pl.staff_present));
        }
        if pl.visitors != visitors.get(&b).copied().unwrap_or(0) {
            return Err(format!("{b:?} visitors {} wrong", pl.visitors));
        }
        let mut have = pl.workers.clone();
        have.sort_unstable();
        let mut want = workers.remove(&b).unwrap_or_default();
        want.sort_unstable();
        if have != want {
            return Err(format!("{b:?} workers list wrong"));
        }
        for &hid in &pl.households {
            if world.households.get(hid).is_none_or(|h| h.home != b) {
                return Err(format!("{b:?} lists {hid:?} which does not live there"));
            }
        }
    }
    if let Some((b, _)) = workers.into_iter().next() {
        return Err(format!("{b:?} has workers but no list"));
    }
    for (hid, h) in world.households.iter() {
        if !world
            .people
            .places
            .get(h.home)
            .is_some_and(|p| p.households.contains(&hid))
        {
            return Err(format!("{hid:?} not listed by its home"));
        }
    }
    Ok(())
}
