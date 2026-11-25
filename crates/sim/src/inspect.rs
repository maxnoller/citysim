//! Read API for the interface: one citizen, one building, one vehicle, and
//! city-wide population statistics.
//!
//! Everything here is cheap enough to call every frame for one target:
//! lookups and small fixed-size loops over one household or one building's
//! lists; nothing allocates. City statistics are kept in
//! [`crate::World::people`]`.stats` ([`CityPeople`]), refreshed every
//! `balance::PEOPLE_STATS_INTERVAL_TICKS` ticks and at midnight, so reading
//! them is free; [`activity_shares`] is live.
//!
//! Names and looks: derive them from `Citizen::seed` (stable for the
//! citizen's life, different for every citizen).

use serde::{Deserialize, Serialize};

use crate::citizens::{Activity, Citizen, CitizenId, HappinessPart, JobTier, LifeStage, Location, TravelMode};
use crate::households::{Household, HouseholdKind};
use crate::traffic::{Place, TripPurpose};
use crate::{BuildingId, HouseholdId, VehicleId, World, Zone, balance};

/// Age bands of the pyramid, years: `0-9, 10-19, ..., 80-89, 90+`.
pub const AGE_BANDS: usize = 10;

/// Rent burden bands (rent / income): `<15 %, 15-30 %, 30-45 %, 45-60 %,
/// >=60 %` (no income counts as the last).
pub const BURDEN_BANDS: [f32; 4] = [0.15, 0.30, 0.45, 0.60];
/// Household money bands, dollars: `<0, 0-50, 50-150, 150-400, 400-1000,
/// >=1000`.
pub const MONEY_BANDS: [i64; 5] = [0, 50, 150, 400, 1000];

/// City-wide population statistics.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CityPeople {
    /// Tick these were computed.
    pub tick: u64,
    pub population: u32,
    pub households: u32,
    /// Citizens per life stage (`LifeStage::ALL` order).
    pub by_stage: [u32; 4],
    /// Citizens per 10-year age band ([`AGE_BANDS`]).
    pub age_pyramid: [u32; AGE_BANDS],
    /// Households per kind (`HouseholdKind::ALL` order).
    pub by_kind: [u32; 4],
    /// Employed citizens per job tier (`JobTier::ALL` order), and part-time
    /// (teens) among them.
    pub employed_by_tier: [u32; 3],
    pub part_time: u32,
    pub workforce: u32,
    pub employed: u32,
    pub retired: u32,
    pub homemakers: u32,
    /// Average happiness (`0..=1`) and of each component
    /// (`HappinessPart::ALL` order).
    pub avg_happiness: f32,
    pub avg_parts: [f32; 7],
    /// Citizens with happiness below `balance::UNHAPPY_BELOW`, and how many
    /// of them each part hurts most (`HappinessPart::ALL` order).
    pub unhappy: u32,
    pub unhappy_reasons: [u32; 7],
    /// Average household money, daily rent and rent burden (rent / daily
    /// income after tax, households with income).
    pub avg_money: f32,
    pub avg_rent: f32,
    pub avg_rent_burden: f32,
    /// Households per rent-burden band ([`BURDEN_BANDS`]) and money band
    /// ([`MONEY_BANDS`]).
    pub burden_hist: [u32; 5],
    pub money_hist: [u32; 6],
    /// Households cutting spending / with money below zero.
    pub austerity: u32,
    pub broke: u32,
    /// Average leisure need (`0..=1`).
    pub avg_leisure_need: f32,
}

impl CityPeople {
    /// The `n` most common reasons for unhappiness, most common first, with
    /// the number of unhappy citizens each (no allocation).
    pub fn top_reasons(&self, n: usize) -> impl Iterator<Item = (HappinessPart, u32)> + '_ {
        let mut order = HappinessPart::ALL;
        order.sort_by_key(|p| std::cmp::Reverse(self.unhappy_reasons[p.index()]));
        order
            .into_iter()
            .filter(|p| self.unhappy_reasons[p.index()] > 0)
            .take(n)
            .map(|p| (p, self.unhappy_reasons[p.index()]))
    }
}

/// Share of citizens in each activity right now (`Activity::ALL` order;
/// all zero in an empty city). Live.
pub fn activity_shares(world: &World) -> [f32; Activity::COUNT] {
    let c = &world.people.activity_counts;
    let total: u32 = c.iter().sum();
    let mut out = [0.0; Activity::COUNT];
    if total > 0 {
        for (o, &n) in out.iter_mut().zip(c) {
            *o = n as f32 / total as f32;
        }
    }
    out
}

/// Trips started today so far, per purpose (`TripPurpose::ALL` order).
pub fn trips_today(world: &World) -> &[u32; TripPurpose::COUNT] {
    &world.people.trips_today
}

/// City statistics being gathered, slice by slice (sums and counts).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StatsAcc {
    stats: CityPeople,
    parts: [f64; 7],
    happy: f64,
    need: f64,
    money: f64,
    rent: f64,
    burden: f64,
    with_income: u32,
    households: u32,
}

/// Add the citizens and households of slots `i % n == k` to `a`.
fn accumulate(world: &World, a: &mut StatsAcc, k: usize, n: usize) {
    let s = &mut a.stats;
    let mut i = k;
    while i < world.citizens.slot_count() {
        if let Some(id) = world.citizens.key_at(i) {
            let c = &world.citizens[id];
            s.population += 1;
            s.by_stage[c.stage.index()] += 1;
            s.age_pyramid[(c.age_years() / 10).min(AGE_BANDS as u32 - 1) as usize] += 1;
            if let Some(j) = c.job {
                s.employed += 1;
                s.employed_by_tier[j.tier.index()] += 1;
                s.part_time += u32::from(j.part_time);
            }
            s.retired += u32::from(c.retired);
            s.homemakers += u32::from(c.homemaker);
            let h = &c.happiness;
            a.happy += h.total as f64;
            for p in HappinessPart::ALL {
                a.parts[p.index()] += h.part(p) as f64;
            }
            if h.total < balance::UNHAPPY_BELOW {
                s.unhappy += 1;
                if let Some(p) = h.worst() {
                    s.unhappy_reasons[p.index()] += 1;
                }
            }
            a.need += c.leisure_need as f64;
        }
        i += n;
    }
    let mut i = k;
    while i < world.households.slot_count() {
        if let Some(hid) = world.households.key_at(i) {
            let h = &world.households[hid];
            i += n;
            if h.citizens.is_empty() {
                continue;
            }
            a.households += 1;
            s.by_kind[h.kind.index()] += 1;
            a.money += h.money as f64;
            a.rent += h.rent as f64;
            let income = crate::life::daily_income(world, hid);
            let band = if income > 0 {
                let b = h.rent as f32 / income as f32;
                a.burden += b as f64;
                a.with_income += 1;
                BURDEN_BANDS.iter().filter(|&&x| b >= x).count()
            } else {
                BURDEN_BANDS.len()
            };
            s.burden_hist[band] += 1;
            let mband = if h.money < 0 {
                0
            } else {
                MONEY_BANDS.iter().filter(|&&x| h.money >= x).count()
            };
            s.money_hist[mband] += 1;
            s.austerity += u32::from(h.austerity);
            s.broke += u32::from(h.money < 0);
        } else {
            i += n;
        }
    }
}

/// Averages from the sums.
fn finish(world: &World, a: StatsAcc) -> CityPeople {
    let mut s = a.stats;
    s.tick = world.time.tick;
    s.households = world.households.len() as u32;
    s.workforce = world.people.workforce;
    if s.population > 0 {
        let n = s.population as f64;
        s.avg_happiness = (a.happy / n) as f32;
        for (x, p) in s.avg_parts.iter_mut().zip(a.parts) {
            *x = (p / n) as f32;
        }
        s.avg_leisure_need = (a.need / n) as f32;
    } else {
        s.avg_happiness = balance::HAPPINESS_START;
        s.avg_parts = [balance::HAPPINESS_START; 7];
    }
    if a.households > 0 {
        s.avg_money = (a.money / a.households as f64) as f32;
        s.avg_rent = (a.rent / a.households as f64) as f32;
    }
    if a.with_income > 0 {
        s.avg_rent_burden = (a.burden / a.with_income as f64) as f32;
    }
    s
}

/// Gather one slice of the city statistics; publish them when the sweep
/// is complete (every `balance::PEOPLE_STATS_INTERVAL_TICKS`).
pub(crate) fn gather(world: &mut World) {
    let k = world.time.tick % balance::PEOPLE_STATS_INTERVAL_TICKS;
    let n = balance::PEOPLE_STATS_CHUNKS;
    if k >= n {
        return;
    }
    if k == 0 {
        world.people.stats_acc = StatsAcc::default();
    }
    let mut a = std::mem::take(&mut world.people.stats_acc);
    accumulate(world, &mut a, k as usize, n as usize);
    if k + 1 == n {
        world.people.stats = finish(world, a);
    } else {
        world.people.stats_acc = a;
    }
}

/// Recompute [`CityPeople`] at once.
pub(crate) fn refresh(world: &mut World) {
    let mut a = StatsAcc::default();
    accumulate(world, &mut a, 0, 1);
    world.people.stats = finish(world, a);
}

// ---------------------------------------------------------- citizens ----

/// A trip in progress, as the interface shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TripView {
    pub purpose: TripPurpose,
    /// Start and end building (`from` is `None` when coming from outside the
    /// city).
    pub from: Option<BuildingId>,
    pub to: BuildingId,
    pub mode: TravelMode,
    /// The car they are in, if it is on the road (driver or passenger).
    pub vehicle: Option<VehicleId>,
    /// Who drives, for passengers.
    pub driver: Option<CitizenId>,
    pub departed_tick: u64,
    /// Arrival tick of an off-screen trip.
    pub eta_tick: Option<u64>,
}

/// Everything about one citizen.
#[derive(Debug, Clone, Copy)]
pub struct CitizenView<'a> {
    pub id: CitizenId,
    pub citizen: &'a Citizen,
    pub household_id: HouseholdId,
    pub household: &'a Household,
    /// Household members, the citizen included.
    pub members: &'a [CitizenId],
    pub home: BuildingId,
    pub workplace: Option<BuildingId>,
    /// Building they are in right now.
    pub inside: Option<BuildingId>,
    pub trip: Option<TripView>,
}

impl CitizenView<'_> {
    pub fn age_years(&self) -> u32 {
        self.citizen.age_years()
    }
    pub fn stage(&self) -> LifeStage {
        self.citizen.stage
    }
    pub fn activity(&self) -> Activity {
        self.citizen.activity
    }
    /// Daily wage, if employed.
    pub fn wage(&self) -> Option<u32> {
        self.citizen.job.map(|j| j.wage)
    }
}

/// Look up a citizen (`None` if they no longer live in the city).
pub fn citizen(world: &World, id: CitizenId) -> Option<CitizenView<'_>> {
    let c = world.citizens.get(id)?;
    let h = world.households.get(c.household)?;
    let trip = match &c.location {
        Location::In(_) => None,
        Location::Travelling(j) => {
            let vehicle = match j.mode {
                TravelMode::Passenger => j
                    .driver
                    .and_then(|d| world.citizens.get(d))
                    .and_then(|d| d.location.journey())
                    .and_then(|dj| dj.vehicle),
                _ => j.vehicle,
            };
            Some(TripView {
                purpose: j.purpose,
                from: j.from,
                to: j.to,
                mode: j.mode,
                vehicle: vehicle.or(j.vehicle),
                driver: j.driver,
                departed_tick: j.departed_tick,
                eta_tick: j.arrive_tick,
            })
        }
    };
    Some(CitizenView {
        id,
        citizen: c,
        household_id: c.household,
        household: h,
        members: &h.citizens,
        home: h.home,
        workplace: c.workplace(),
        inside: c.location.building(),
        trip,
    })
}

// --------------------------------------------------------- buildings ----

/// Who a building houses or employs.
#[derive(Debug, Clone, Copy)]
pub struct BuildingView<'a> {
    pub kind: Zone,
    /// Residents (homes) or job places filled (workplaces), and the
    /// building's capacity.
    pub occupied: u32,
    pub capacity: u32,
    /// Households living here (homes).
    pub households: &'a [HouseholdId],
    /// Citizens employed here (workplaces).
    pub workers: &'a [CitizenId],
    /// Workers on shift and visitors (shoppers, leisure) inside now.
    pub staff_present: u16,
    pub visitors: u16,
    /// Shops: goods on the shelves and shelf space, units; sold today.
    pub stock: f32,
    pub stock_cap: f32,
    pub sold_today: f32,
    /// Homes: rent paid per day by all its households, dollars.
    pub rent_per_day: i64,
    /// Workplaces: wages paid per day to all its workers, dollars.
    pub wages_per_day: i64,
}

/// Look up who lives or works in a building.
pub fn building(world: &World, id: BuildingId) -> Option<BuildingView<'_>> {
    const NONE_H: &[HouseholdId] = &[];
    const NONE_C: &[CitizenId] = &[];
    let b = world.buildings.get(id)?;
    let p = world.people.places.get(id);
    let households = p.map_or(NONE_H, |p| &p.households[..]);
    let workers = p.map_or(NONE_C, |p| &p.workers[..]);
    let rent_per_day = households
        .iter()
        .filter_map(|&h| world.households.get(h))
        .map(|h| h.rent)
        .sum();
    let wages_per_day = workers
        .iter()
        .filter_map(|&c| world.citizens.get(c)?.job)
        .map(|j| j.wage as i64)
        .sum();
    Some(BuildingView {
        kind: b.kind,
        occupied: b.occupied,
        capacity: b.capacity,
        households,
        workers,
        staff_present: p.map_or(0, |p| p.staff_present),
        visitors: p.map_or(0, |p| p.visitors),
        stock: p.map_or(0.0, |p| p.stock),
        stock_cap: crate::errands::stock_cap(b),
        sold_today: p.map_or(0.0, |p| p.sold_today),
        rent_per_day,
        wages_per_day,
    })
}

/// Everyone who lives in a building (no allocation).
pub fn residents(world: &World, id: BuildingId) -> impl Iterator<Item = CitizenId> + '_ {
    world
        .people
        .places
        .get(id)
        .into_iter()
        .flat_map(|p| p.households.iter())
        .filter_map(|&h| world.households.get(h))
        .flat_map(|h| h.citizens.iter().copied())
}

/// Building at a point (sim metres), if any.
pub fn building_at(world: &World, pos: [f32; 2]) -> Option<BuildingId> {
    crate::zoning::building_at(world, pos)
}

// ---------------------------------------------------------- vehicles ----

/// Who is in a car and where it goes.
#[derive(Debug, Clone, Copy)]
pub struct VehicleView {
    pub purpose: TripPurpose,
    /// The household it belongs to (none for trucks and cars of households
    /// that have left).
    pub household: Option<HouseholdId>,
    /// The citizen driving (none for trucks, and once its people have
    /// arrived on the calendar: see `Journey::due_tick`).
    pub driver: Option<CitizenId>,
    /// The citizen whose trip this car draws, also after they got out (the
    /// car is finishing the drive to where they now are).
    pub owner: Option<CitizenId>,
    /// Citizens in the car, driver included.
    pub occupants: u8,
    pub origin: Place,
    pub destination: Place,
    /// Buildings at the two ends (none for the highway).
    pub origin_building: Option<BuildingId>,
    pub destination_building: Option<BuildingId>,
}

/// Look up a vehicle's trip and occupants.
pub fn vehicle(world: &World, id: VehicleId) -> Option<VehicleView> {
    let v = world.vehicles.get(id)?;
    let t = world.traffic.trips.get(id)?;
    let at = |p: Place| match p {
        Place::At(pos) => building_at(world, pos),
        Place::Entry => None,
    };
    let driver = t.citizen.filter(|&c| world.citizens.contains_key(c));
    Some(VehicleView {
        purpose: t.purpose,
        household: v.household,
        driver,
        owner: t.owner.filter(|&c| world.citizens.contains_key(c)),
        occupants: occupants(world, id).count().min(u8::MAX as usize) as u8,
        origin: t.from,
        destination: t.to,
        origin_building: at(t.from),
        destination_building: at(t.to),
    })
}

/// Citizens in a vehicle, driver first (no allocation).
pub fn occupants(world: &World, id: VehicleId) -> impl Iterator<Item = CitizenId> + '_ {
    let driver = world
        .traffic
        .trips
        .get(id)
        .and_then(|t| t.citizen)
        .filter(|&c| world.citizens.contains_key(c));
    let members: &[CitizenId] = driver
        .and_then(|d| world.households.get(world.citizens[d].household))
        .map_or(&[], |h| &h.citizens[..]);
    driver.into_iter().chain(members.iter().copied().filter(move |&m| {
        Some(m) != driver
            && world.citizens.get(m).is_some_and(|c| {
                matches!(&c.location, Location::Travelling(j) if j.mode == TravelMode::Passenger && j.driver == driver)
            })
    }))
}

/// Households of each kind and their share (for a legend).
pub fn household_mix(world: &World) -> [(HouseholdKind, u32); 4] {
    let k = world.people.stats.by_kind;
    HouseholdKind::ALL.map(|x| (x, k[x.index()]))
}

/// Employment by job tier (for a legend).
pub fn employment_by_tier(world: &World) -> [(JobTier, u32); 3] {
    let k = world.people.stats.employed_by_tier;
    JobTier::ALL.map(|x| (x, k[x.index()]))
}
