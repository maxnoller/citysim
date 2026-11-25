//! Households: the people who live together in one home and share its
//! money, goods and cars.
//!
//! * Households arrive through the highway stub while there is free housing
//!   and the city is attractive ([`arrivals`]): single adults, couples,
//!   families with children and seniors, in a mix that follows jobs,
//!   rents and happiness. Their home is reserved on arrival at the city edge
//!   and they drive in together.
//! * Each household has its members ([`Household::citizens`]), money, a
//!   daily rent, a stock of goods and one or two cars. Wages, pensions, rent
//!   and tax are booked at midnight (`life`), shopping when it happens
//!   (`day`).
//! * A household leaves the city when it cannot pay its way, when a
//!   member's commute is hopeless again and again, when it has been unhappy
//!   for days, in a debt crisis, or when its home is removed.
//!
//! The fields from round 2 (`job`, `members`, `state`, `vehicle`, ...) are
//! kept and derived from the citizens (see each field).

use serde::{Deserialize, Serialize};
use slotmap::new_key_type;

use crate::citizens::{Activity, Citizen, CitizenId, HouseholdRole, LifeEvent, LifeStage, Location, TravelMode};
use crate::derived::rect_center;
use crate::people::{self, habit};
use crate::traffic::{self, Place, TripPurpose};
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

/// Kind of household, from its members (updated whenever they change).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub enum HouseholdKind {
    /// One adult.
    #[default]
    Single,
    /// Two (or more) adults, no children.
    Couple,
    /// Adults with children or teens (also grown-up children still at home
    /// while there are younger ones).
    Family,
    /// Only seniors.
    Seniors,
}

impl HouseholdKind {
    pub const ALL: [HouseholdKind; 4] = [
        HouseholdKind::Single,
        HouseholdKind::Couple,
        HouseholdKind::Family,
        HouseholdKind::Seniors,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Kind for a household of `adults` (18-64), `seniors` and `young`
    /// (children and teens) members.
    pub fn of(adults: u8, seniors: u8, young: u8) -> HouseholdKind {
        if young > 0 {
            HouseholdKind::Family
        } else if adults == 0 && seniors > 0 {
            HouseholdKind::Seniors
        } else if adults + seniors <= 1 {
            HouseholdKind::Single
        } else {
            HouseholdKind::Couple
        }
    }
}

/// Money in and out of a household, whole dollars (all amounts positive
/// except `transfers`). The household's money always equals `opening +
/// wages + pensions + transfers - tax - rent - shopping - leisure` of its
/// lifetime ledger (`Household::ledger`), checked by
/// `World::check_sim_invariants`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Ledger {
    /// Money brought along on arrival (lifetime ledger only).
    pub opening: i64,
    /// Gross wages of its workers.
    pub wages: i64,
    /// Pensions of its seniors (not taxed).
    pub pensions: i64,
    /// Money from (+) or to (-) other households: a grown-up child moving
    /// out takes a share along, a couple pools its savings.
    pub transfers: i64,
    /// Residential tax taken from the wages (goes to the city).
    pub tax: i64,
    pub rent: i64,
    /// Goods bought in shops.
    pub shopping: i64,
    /// Nights out.
    pub leisure: i64,
}

impl Ledger {
    /// Income minus spending.
    pub fn net(&self) -> i64 {
        self.opening + self.wages + self.pensions + self.transfers - self.tax - self.rent - self.shopping - self.leisure
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Household {
    pub home: BuildingId,
    /// Workplace of its first employed member (in `citizens` order), if
    /// anyone works. Every member's own job is `Citizen::job`.
    pub job: Option<BuildingId>,
    /// People in the household (`citizens.len()`).
    pub members: u8,
    /// Where its primary member is (the first with a job, else the first):
    /// at home, commuting, at work.
    pub state: HouseholdState,
    /// A car of the household on the road, if any (driven by a member;
    /// with two cars out, the first member's). Each member's trip:
    /// `Citizen::location`.
    pub vehicle: Option<VehicleId>,
    /// Stable random number.
    #[serde(default)]
    pub seed: u32,
    /// Consecutive midnights in which nobody in the household had a job
    /// although someone wanted one.
    #[serde(default)]
    pub unemployed_days: u8,
    /// Longest current streak of hopeless commutes among its members.
    #[serde(default)]
    pub hopeless_commutes: u8,
    /// Duration of the last commute of any member, vehicle seconds (0 =
    /// none yet).
    #[serde(default)]
    pub last_commute_s: f32,
    /// Day a member last set off to work.
    #[serde(default)]
    pub last_work_day: u32,
    // ------------------------------------------------ round 3 ----
    /// Kind, from the members.
    pub kind: HouseholdKind,
    /// Members, oldest household role first (heads, then dependants).
    pub citizens: Vec<CitizenId>,
    /// Money, whole dollars. May go negative briefly (see `broke_days`).
    pub money: i64,
    /// Rent per day for the current home, dollars (from the home's level,
    /// the household size and the housing market; updated at midnight).
    pub rent: i64,
    /// Goods at home, units (one unit = one person-day of goods).
    pub goods: f32,
    /// Cars owned (1 or 2).
    pub cars: u8,
    /// Member on a shopping trip right now (only one goes at a time).
    pub shopper: Option<CitizenId>,
    /// Money is short: buys only essentials and skips nights out.
    pub austerity: bool,
    /// Consecutive midnights with money below zero.
    pub broke_days: u8,
    /// Consecutive midnights with average happiness below
    /// `balance::UNHAPPY_LEAVE_BELOW`.
    pub unhappy_days: u8,
    /// Average happiness of its members (updated at midnight).
    pub happiness: f32,
    /// Lifetime money flows (see [`Ledger`]).
    pub ledger: Ledger,
    /// Money flows of yesterday (booked at the last midnight, plus
    /// shopping and leisure of that day).
    pub yesterday: Ledger,
    /// Money flows so far today (shopping, leisure).
    pub today: Ledger,
    /// Day the household arrived (or formed).
    pub arrived_day: u32,
}

impl Household {
    /// A household with the given home and nobody in it yet (members are
    /// added with `add_member`).
    pub(crate) fn empty(home: BuildingId, seed: u32, money: i64, day: u32) -> Household {
        Household {
            home,
            job: None,
            members: 0,
            state: HouseholdState::AtHome,
            vehicle: None,
            seed,
            unemployed_days: 0,
            hopeless_commutes: 0,
            last_commute_s: 0.0,
            last_work_day: 0,
            kind: HouseholdKind::Single,
            citizens: Vec::new(),
            money,
            rent: 0,
            goods: 0.0,
            cars: 1,
            shopper: None,
            austerity: false,
            broke_days: 0,
            unhappy_days: 0,
            happiness: balance::HAPPINESS_START,
            ledger: Ledger {
                opening: money,
                ..Ledger::default()
            },
            yesterday: Ledger::default(),
            today: Ledger::default(),
            arrived_day: day,
        }
    }

    /// A household of `members` people living at `home`, written the round-2
    /// way (tools, tests): it has no citizens yet; the population model
    /// generates them (and money, goods, cars) at the next step or load.
    pub fn without_citizens(home: BuildingId, members: u8, seed: u32) -> Household {
        let mut h = Household::empty(home, seed, 0, 0);
        h.members = members;
        h
    }

    /// Cars not taken out by a member right now.
    pub fn free_cars(&self, world: &World) -> u8 {
        let out = self
            .citizens
            .iter()
            .filter(|&&id| world.citizens.get(id).is_some_and(|c| c.has_car))
            .count() as u8;
        self.cars.saturating_sub(out)
    }
}

/// Can a building be reached from the highway?
pub(crate) fn connected(world: &World, r: CellRect) -> bool {
    world.cache.connected_near(r, balance::ZONE_ROAD_REACH + 1)
}

pub(crate) fn place_of(world: &World, b: BuildingId) -> Option<Place> {
    world.buildings.get(b).map(|b| Place::At(rect_center(b.footprint)))
}

// ----------------------------------------------------------- members ----

/// A new citizen of `age_days`, not yet anywhere (the caller places them).
pub(crate) fn new_citizen(world: &mut World, hid: HouseholdId, age_days: u32, role: HouseholdRole) -> Citizen {
    let stage = LifeStage::of_years(age_days / balance::DAYS_PER_YEAR);
    let seed = world.rng.next_u64() as u32;
    let home = world.households[hid].home;
    let now = world.time.tick;
    Citizen {
        household: hid,
        role,
        age_days,
        stage,
        seed,
        education: Default::default(),
        job: None,
        retired: stage == LifeStage::Senior,
        homemaker: false,
        activity: Activity::Home,
        location: Location::In(home),
        until_tick: now,
        leisure_need: habit(seed, 0x1E15) * 0.5,
        need_tick: now,
        has_car: false,
        happiness: Default::default(),
        journal: Vec::new(),
        last_commute_s: 0.0,
        hopeless_commutes: 0,
        unemployed_days: 0,
        worked_day: 0,
        arrived_day: world.time.day,
    }
}

/// Add a citizen to a household (the home's occupancy grows by one).
pub(crate) fn add_member(world: &mut World, c: Citizen) -> CitizenId {
    let hid = c.household;
    let home = world.households[hid].home;
    let (activity, location, until) = (c.activity, c.location, c.until_tick);
    let wf = c.in_workforce();
    // Insert at a neutral state; `set_place` books the presence.
    let id = world.citizens.insert(Citizen {
        activity: Activity::Home,
        location: Location::In(home),
        ..c
    });
    world.people.activity_counts[Activity::Home.index()] += 1;
    world.people.workforce += u32::from(wf);
    let h = &mut world.households[hid];
    let pos = if world.citizens[id].role == HouseholdRole::Head {
        h.citizens
            .iter()
            .position(|&m| world.citizens.get(m).is_some_and(|m| m.role != HouseholdRole::Head))
            .unwrap_or(h.citizens.len())
    } else {
        h.citizens.len()
    };
    h.citizens.insert(pos, id);
    if let Some(b) = world.buildings.get_mut(home) {
        b.occupied += 1;
    }
    people::set_place(world, id, activity, location, until);
    people::seek(world, id);
    id
}

/// Take a citizen out of their household and the city's books (job,
/// presence, home occupancy, any car trip they are on). The household
/// itself stays, even if now empty.
pub(crate) fn remove_member(world: &mut World, id: CitizenId) -> Option<Citizen> {
    people::fire(world, id, false);
    let c = world.citizens.get(id)?;
    let hid = c.household;
    // Off the books: presence and workforce.
    people::set_place(world, id, Activity::Home, c.location, 0);
    let home = world.households.get(hid).map(|h| h.home);
    let c = world.citizens.remove(id)?;
    let n = &mut world.people.activity_counts[Activity::Home.index()];
    *n = n.saturating_sub(1);
    if c.in_workforce() {
        world.people.workforce = world.people.workforce.saturating_sub(1);
    }
    // A car they drive keeps going without them.
    if let Location::Travelling(j) = c.location
        && j.mode != TravelMode::Passenger
    {
        if let Some(v) = j.vehicle
            && let Some(t) = world.traffic.trips.get_mut(v)
        {
            t.citizen = None;
        }
        for d in world.traffic.pending.iter_mut().filter(|d| d.citizen == Some(id)) {
            d.citizen = None;
        }
    }
    if let Some(h) = world.households.get_mut(hid) {
        h.citizens.retain(|&m| m != id);
        if h.shopper == Some(id) {
            h.shopper = None;
        }
    }
    if let Some(b) = home.and_then(|home| world.buildings.get_mut(home)) {
        b.occupied = b.occupied.saturating_sub(1);
    }
    // Passengers of a driver who is gone travel on by themselves.
    let riders: Vec<CitizenId> = world
        .households
        .get(hid)
        .map(|h| h.citizens.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|&m| {
            world.citizens.get(m).is_some_and(
                |m| matches!(&m.location, Location::Travelling(j) if j.mode == TravelMode::Passenger && j.driver == Some(id)),
            )
        })
        .collect();
    for m in riders {
        crate::day::continue_off_screen(world, m);
    }
    people::refresh_household(world, hid);
    Some(c)
}

/// Register a new household with its home (members are added after).
pub(crate) fn insert(world: &mut World, h: Household) -> HouseholdId {
    let home = h.home;
    let id = world.households.insert(h);
    people::place(world, home).households.push(id);
    world.people.known_households = world.households.len();
    id
}

/// Remove a household and all its members from the city, freeing its home
/// and their jobs. `drive_out`: a car drives out to the highway (when
/// nobody is on the road already).
pub(crate) fn leave(world: &mut World, id: HouseholdId, drive_out: bool) {
    let Some(h) = world.households.get(id) else { return };
    let on_road = h.vehicle;
    let home = h.home;
    let members = h.citizens.clone();
    let n = members.len() as u32;
    for m in members {
        remove_member(world, m);
    }
    let Some(h) = world.households.remove(id) else { return };
    world.people.known_households = world.households.len();
    if let Some(p) = world.people.places.get_mut(home) {
        p.households.retain(|&x| x != id);
    }
    if let Some(b) = world.buildings.get_mut(home) {
        // Members of a legacy household without citizens.
        if n == 0 {
            b.occupied = b.occupied.saturating_sub(h.members as u32);
        }
    }
    for v in world.vehicles.values_mut() {
        if v.household == Some(id) {
            v.household = None;
        }
    }
    world.people.events_today.households_left += 1;
    world.people.events_today.citizens_left += n;
    if drive_out
        && on_road.is_none()
        && let Some(from) = place_of(world, home)
    {
        traffic::request(world, None, TripPurpose::MoveOut, from, Place::Entry);
        people::count_trip(world, TripPurpose::MoveOut);
    }
}

/// Move a whole household (everyone at home) to another home.
pub(crate) fn relocate(world: &mut World, id: HouseholdId, to: BuildingId) {
    let Some(h) = world.households.get(id) else { return };
    let from = h.home;
    let n = h.citizens.len() as u32;
    if from == to {
        return;
    }
    if let Some(b) = world.buildings.get_mut(from) {
        b.occupied = b.occupied.saturating_sub(n);
    }
    if let Some(p) = world.people.places.get_mut(from) {
        p.households.retain(|&x| x != id);
    }
    world.buildings[to].occupied += n;
    people::place(world, to).households.push(id);
    world.households[id].home = to;
    let now = world.time.tick;
    for m in world.households[id].citizens.clone() {
        let c = &world.citizens[m];
        if c.location == Location::In(from) {
            let a = c.activity;
            people::set_place(world, m, a, Location::In(to), now);
        }
    }
}

/// Buildings were removed (bulldozed or abandoned): their residents leave,
/// their workers lose the job, people inside go home.
pub(crate) fn buildings_removed(world: &mut World, ids: &[BuildingId]) {
    if ids.is_empty() {
        return;
    }
    let mut leaving = Vec::new();
    let mut fired = Vec::new();
    for (hid, h) in world.households.iter() {
        if ids.contains(&h.home) {
            leaving.push(hid);
        }
    }
    for &b in ids {
        if let Some(p) = world.people.places.get(b) {
            fired.extend(p.workers.iter().copied());
        }
    }
    for hid in leaving {
        leave(world, hid, false);
    }
    for c in fired {
        people::fire(world, c, true);
    }
    // Legacy households (no citizens) that worked there.
    for h in world.households.values_mut() {
        if h.citizens.is_empty() && h.job.is_some_and(|j| ids.contains(&j)) {
            h.job = None;
        }
    }
    // Anyone still inside (shoppers, visitors, fired staff) goes home.
    let now = world.time.tick;
    let inside: Vec<CitizenId> = world
        .citizens
        .iter()
        .filter(|(_, c)| matches!(c.location, Location::In(b) if ids.contains(&b)))
        .map(|(id, _)| id)
        .collect();
    for id in inside {
        let hid = world.citizens[id].household;
        let home = world.households[hid].home;
        if world.citizens[id].has_car {
            world.citizens[id].has_car = false;
        }
        people::set_place(world, id, Activity::Home, Location::In(home), now);
        if world.households[hid].shopper == Some(id) {
            world.households[hid].shopper = None;
        }
    }
    for b in ids {
        world.people.places.remove(*b);
    }
}

/// A trip ended (called by traffic).
pub(crate) fn trip_ended(world: &mut World, e: traffic::Ended) {
    crate::day::trip_ended(world, e);
}

// ----------------------------------------------------------- arrivals ----

/// Household types arriving, with relative weights for the current city
/// (`balance::ARRIVAL_WEIGHTS`, shifted by free jobs and rents).
fn arrival_weights(world: &World) -> [f32; 4] {
    let mut w = balance::ARRIVAL_WEIGHTS;
    let s = &world.stats;
    let jobs_free = s.vacant_jobs as f32 / (s.workforce as f32 + balance::DEMAND_SMALL_CITY);
    // Free jobs draw working households; no jobs leaves mostly retirees.
    let work = (0.5 + jobs_free * balance::ARRIVAL_JOBS_PULL).clamp(0.2, 1.5);
    w[HouseholdKind::Single.index()] *= work;
    w[HouseholdKind::Couple.index()] *= work;
    w[HouseholdKind::Family.index()] *= work;
    // High rent burden puts families off.
    let burden = world.people.stats.avg_rent_burden;
    if burden > balance::RENT_BURDEN_OK {
        w[HouseholdKind::Family.index()] *= (1.0 - (burden - balance::RENT_BURDEN_OK) * 2.0).max(0.3);
    }
    w
}

fn pick(world: &mut World, w: &[f32]) -> usize {
    let total: f32 = w.iter().sum();
    let mut r = world.rng.next_f32() * total;
    for (i, &x) in w.iter().enumerate() {
        if r < x {
            return i;
        }
        r -= x;
    }
    w.len() - 1
}

/// Ages in years of the members of a new household of `kind` (heads
/// first, then children), fitting in `room` places.
fn arrival_ages(world: &mut World, kind: HouseholdKind, room: u32) -> Vec<(u32, HouseholdRole)> {
    let r = |world: &mut World, lo: u32, hi: u32| lo + world.rng.below(hi - lo + 1);
    let head = HouseholdRole::Head;
    let mut v = match kind {
        HouseholdKind::Single => vec![(r(world, 19, 44), head)],
        HouseholdKind::Couple => {
            let a = r(world, 21, 52);
            let b = (a as i32 + world.rng.below(9) as i32 - 4).clamp(19, 60) as u32;
            vec![(a, head), (b, head)]
        }
        HouseholdKind::Family => {
            let a = r(world, 26, 46);
            let mut v = vec![(a, head)];
            if world.rng.next_f32() >= balance::SINGLE_PARENT_SHARE {
                let b = (a as i32 + world.rng.below(7) as i32 - 3).clamp(20, 60) as u32;
                v.push((b, head));
            }
            let kids = 1 + pick(world, &balance::FAMILY_CHILDREN_WEIGHTS) as u32;
            let oldest = (a - 19).min(17);
            for _ in 0..kids {
                let k = world.rng.below(oldest + 1);
                v.push((k, HouseholdRole::Dependant));
            }
            v
        }
        HouseholdKind::Seniors => {
            let a = r(world, 65, 84);
            let mut v = vec![(a, head)];
            if world.rng.next_f32() < balance::SENIOR_COUPLE_SHARE {
                v.push(((a as i32 + world.rng.below(7) as i32 - 3).max(62) as u32, head));
            }
            v
        }
    };
    v.truncate(room.max(1) as usize);
    v
}

/// Places newcomers leave free in a home (for births and moves within the
/// city): one in buildings of `balance::RESERVE_FROM_CAPACITY` places or
/// more.
pub(crate) fn reserve(b: &crate::Building) -> u32 {
    u32::from(b.capacity >= balance::RESERVE_FROM_CAPACITY)
}

/// Daily costs a household of `n` expects (rent at level 1, goods), for
/// arrival savings and austerity.
pub(crate) fn expected_daily_costs(n: u32, rent: i64) -> i64 {
    rent + (n as f32 * balance::GOODS_PER_PERSON_PER_DAY * balance::GOODS_PRICE).round() as i64
}

/// Newcomers: every tick, pending arrivals accumulate with the free housing
/// while the city is attractive; each arrival is one household of a type
/// drawn from [`arrival_weights`], moving into a random free home it fits.
pub(crate) fn arrivals(world: &mut World) {
    let free = world.stats.residential_capacity.saturating_sub(world.stats.population) as f32;
    let happy = world.people.stats.avg_happiness;
    let mood = if world.stats.population > 0 {
        (1.0 + (happy - balance::HAPPINESS_NEUTRAL) * balance::ARRIVAL_HAPPINESS_WEIGHT).clamp(0.3, 1.5)
    } else {
        1.0
    };
    let rate = if world.growth.attractiveness > balance::MOVE_IN_MIN_DEMAND && free >= 1.0 {
        (free / balance::ARRIVAL_AVG_SIZE * balance::MOVE_IN_FILL_PER_HOUR * mood).min(balance::MOVE_IN_MAX_PER_HOUR)
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
    let homes: Vec<BuildingId> = world
        .buildings
        .iter()
        .filter(|(_, b)| b.kind == Zone::Residential && b.occupied + reserve(b) < b.capacity)
        .filter(|(_, b)| connected(world, b.footprint))
        .map(|(id, _)| id)
        .collect();
    while world.growth.move_in_acc >= 1.0 {
        world.growth.move_in_acc -= 1.0;
        if homes.is_empty() {
            world.growth.move_in_acc = 0.0;
            break;
        }
        let w = arrival_weights(world);
        let kind = HouseholdKind::ALL[pick(world, &w)];
        let mut ages = arrival_ages(world, kind, u32::MAX);
        // A home it fits in (the roomiest of a few if none does).
        let mut best: Option<(u32, BuildingId)> = None;
        for _ in 0..balance::HOME_SEARCH_SAMPLES {
            let h = homes[world.rng.below(homes.len() as u32) as usize];
            let b = &world.buildings[h];
            let room = b.capacity.saturating_sub(b.occupied + reserve(b));
            if room >= ages.len() as u32 {
                best = Some((room, h));
                break;
            }
            if room > 0 && best.is_none_or(|b| room > b.0) {
                best = Some((room, h));
            }
        }
        let Some((room, home)) = best else { continue };
        ages.truncate(room as usize);
        arrive(world, home, &ages);
    }
}

/// A household with members of these ages (years) arrives at the highway
/// and drives to `home`.
pub(crate) fn arrive(world: &mut World, home: BuildingId, ages: &[(u32, HouseholdRole)]) -> HouseholdId {
    let n = ages.len() as u32;
    let rent = crate::life::rent_for(world, home, n);
    let costs = expected_daily_costs(n, rent);
    let savings = (costs as f32 * (balance::ARRIVAL_SAVINGS_DAYS + world.rng.next_f32())).round() as i64;
    let seed = world.rng.next_u64() as u32;
    let day = world.time.day;
    let mut h = Household::empty(home, seed, savings, day);
    h.rent = rent;
    h.goods = n as f32 * balance::ARRIVAL_GOODS_DAYS;
    let hid = insert(world, h);
    let now = world.time.tick;
    let mut ids = Vec::new();
    for &(years, role) in ages {
        let days = years * balance::DAYS_PER_YEAR + world.rng.below(balance::DAYS_PER_YEAR);
        let mut c = new_citizen(world, hid, days, role);
        c.log(day, LifeEvent::MovedIn);
        ids.push(add_member(world, c));
    }
    crate::life::settle_household(world, hid);
    let adults = ids.iter().filter(|&&id| world.citizens[id].drives()).count();
    world.households[hid].cars = cars_for(world, seed, adults);
    let e = &mut world.people.events_today;
    e.households_arrived += 1;
    e.citizens_arrived += n;
    // Everyone drives in together; the first driver drives.
    let driver = ids
        .iter()
        .copied()
        .find(|&id| world.citizens[id].drives())
        .unwrap_or(ids[0]);
    crate::day::move_in(world, hid, driver, &ids, now);
    hid
}

/// Cars of a household with `adults` drivers.
pub(crate) fn cars_for(_world: &World, seed: u32, adults: usize) -> u8 {
    match adults {
        0 => 0,
        1 => 1,
        _ if habit(seed, 0xCA25) < balance::TWO_CAR_SHARE => 2,
        _ => 1,
    }
}

// --------------------------------------------------------------- tick ----

/// Per-tick population logic.
pub(crate) fn tick(world: &mut World) {
    // Reachability checks below need the road index of the current roads
    // (a loaded world starts without it).
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    if world.people.known_households != world.households.len() {
        adopt(world);
    }
    crate::life::night(world);
    if world.time.tick.is_multiple_of(balance::JOB_MATCH_INTERVAL_TICKS) {
        crate::life::match_jobs(world);
    }
    arrivals(world);
    crate::day::tick(world);
    crate::inspect::gather(world);
    // Nobody left to commute: the average describes a city that is gone.
    // Forget it (0 = no data) so it cannot pin demand at the bottom with no
    // commute ever finishing to correct it.
    if world.households.is_empty() {
        world.traffic.avg_commute_s = 0.0;
    }
}

/// Households added or removed behind the population model's back (old
/// saves, worlds put together by writing public fields): give households
/// without citizens plausible members, money and places; drop citizens
/// whose household is gone.
pub(crate) fn adopt(world: &mut World) {
    let orphans: Vec<CitizenId> = world
        .citizens
        .iter()
        .filter(|(_, c)| !world.households.contains_key(c.household))
        .map(|(id, _)| id)
        .collect();
    for id in orphans {
        if let Some(c) = world.citizens.remove(id) {
            let n = &mut world.people.activity_counts[c.activity.index()];
            *n = n.saturating_sub(1);
            if c.in_workforce() {
                world.people.workforce = world.people.workforce.saturating_sub(1);
            }
        }
    }
    let legacy: Vec<HouseholdId> = world
        .households
        .iter()
        .filter(|(_, h)| h.citizens.is_empty())
        .map(|(id, _)| id)
        .collect();
    for hid in legacy {
        populate(world, hid);
    }
    world.people.known_households = world.households.len();
}

/// Generate the members of a legacy household (round-1 and round-2 saves:
/// a home, maybe one job, a member count, maybe a car on the road).
fn populate(world: &mut World, hid: HouseholdId) {
    let h = &world.households[hid];
    let home = h.home;
    if !world.buildings.contains_key(home) {
        world.households.remove(hid);
        return;
    }
    let n = (h.members.max(1) as u32).min(8);
    let (seed, job, state, vehicle) = (h.seed, h.job, h.state, h.vehicle);
    let job = job.filter(|j| world.buildings.get(*j).is_some_and(|b| b.kind != Zone::Residential));
    // Ages: seniors only if nobody works.
    let r = habit(seed, 0x1E6A);
    let head = HouseholdRole::Head;
    let mut ages: Vec<(u32, HouseholdRole)> = Vec::new();
    let rr = |k: u64, lo: u32, hi: u32| lo + (habit(seed, k) * (hi - lo + 1) as f32) as u32;
    match n {
        1 if job.is_none() && r < balance::LEGACY_SENIOR_SHARE => ages.push((rr(1, 65, 82), head)),
        1 => ages.push((rr(1, 20, 55), head)),
        2 if job.is_none() && r < balance::LEGACY_SENIOR_SHARE => {
            ages.push((rr(1, 65, 82), head));
            ages.push((rr(2, 65, 82), head));
        }
        2 => {
            ages.push((rr(1, 22, 55), head));
            ages.push((rr(2, 22, 55), head));
        }
        _ => {
            let a = rr(1, 28, 48);
            ages.push((a, head));
            ages.push(((a as i32 + rr(2, 0, 6) as i32 - 3) as u32, head));
            for k in 0..n - 2 {
                ages.push((rr(10 + k as u64, 0, (a - 19).min(17)), HouseholdRole::Dependant));
            }
        }
    }
    // The legacy counts move to the citizens.
    let hb = &mut world.buildings[home];
    hb.occupied = hb.occupied.saturating_sub(n);
    if let Some(j) = job {
        let jb = &mut world.buildings[j];
        jb.occupied = jb.occupied.saturating_sub(1);
    }
    let rent = crate::life::rent_for(world, home, n);
    let money = (expected_daily_costs(n, rent) as f32 * balance::LEGACY_SAVINGS_DAYS).round() as i64;
    {
        let h = &mut world.households[hid];
        h.money = money;
        h.ledger = Ledger {
            opening: money,
            ..Ledger::default()
        };
        h.rent = rent;
        h.goods = n as f32 * balance::ARRIVAL_GOODS_DAYS;
        h.arrived_day = world.time.day;
        h.vehicle = None;
        h.job = None;
    }
    if !world
        .people
        .places
        .get(home)
        .is_some_and(|p| p.households.contains(&hid))
    {
        people::place(world, home).households.push(hid);
    }
    let mut ids = Vec::new();
    for (k, &(years, role)) in ages.iter().enumerate() {
        let days =
            years * balance::DAYS_PER_YEAR + (habit(seed, 100 + k as u64) * balance::DAYS_PER_YEAR as f32) as u32;
        let c = new_citizen(world, hid, days, role);
        ids.push(add_member(world, c));
    }
    let adults = ids.iter().filter(|&&id| world.citizens[id].drives()).count();
    world.households[hid].cars = cars_for(world, seed, adults);
    // The old job goes to the first adult.
    let worker = ids
        .iter()
        .copied()
        .find(|&id| world.citizens[id].stage == LifeStage::Adult);
    if let (Some(j), Some(w)) = (job, worker) {
        let shift = crate::life::shift_for(world, j, false);
        people::hire(world, w, j, shift);
    }
    crate::life::settle_household(world, hid);
    // Where they are: the worker as the old household state says.
    let now = world.time.tick;
    let who = worker.unwrap_or(ids[0]);
    let trip = vehicle.filter(|v| world.vehicles.contains_key(*v)).and_then(|v| {
        let t = world.traffic.trips.get(v)?;
        Some((v, t.purpose))
    });
    match (state, trip) {
        (_, Some((v, purpose))) if world.citizens[who].drives() => {
            let (from, to) = match purpose {
                TripPurpose::ToWork => (Some(home), job.unwrap_or(home)),
                TripPurpose::MoveIn => (None, home),
                _ => (job, home),
            };
            if let Some(t) = world.traffic.trips.get_mut(v) {
                t.citizen = Some(who);
            }
            world.citizens[who].has_car = true;
            let j = crate::citizens::Journey {
                purpose,
                from,
                to,
                mode: TravelMode::Driving,
                driver: None,
                vehicle: Some(v),
                departed_tick: now,
                arrive_tick: None,
            };
            people::set_place(world, who, Activity::Travelling, Location::Travelling(j), now + 200);
            if purpose == TripPurpose::MoveIn {
                for &m in &ids {
                    if m != who {
                        let pj = crate::citizens::Journey {
                            mode: TravelMode::Passenger,
                            driver: Some(who),
                            ..j
                        };
                        people::set_place(world, m, Activity::Travelling, Location::Travelling(pj), now + 200);
                    }
                }
            }
        }
        (HouseholdState::AtWork, _) if world.citizens[who].job.is_some() => {
            let wp = world.citizens[who].job.expect("checked").workplace;
            let end = now + (balance::TICKS_PER_HOUR * 4) as u64;
            world.citizens[who].worked_day = world.time.day;
            people::set_place(world, who, Activity::Working, Location::In(wp), end);
        }
        _ => {}
    }
    // A departure queued for the household is the worker's (or drops).
    for d in world.traffic.pending.iter_mut().filter(|d| d.household == Some(hid)) {
        d.household = None;
    }
    if let Some(t) = trip.map(|t| t.0)
        && world.citizens[who].location.journey().is_none()
        && let Some(v) = world.vehicles.get_mut(t)
    {
        v.household = None;
    }
    people::refresh_household(world, hid);
}
