//! The round-2 save layout (format version 4), frozen.
//!
//! Copies of the round-2 definitions of every type whose layout changed in
//! round 3 (households, departures, trips, the traffic state and the World
//! itself); unchanged types are reused from the live crate. Never edit
//! them. `WorldV4::upgrade` turns a decoded version-4 world into a live
//! one whose households have no citizens yet: `households::adopt`
//! generates them right after loading.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::v5::{
    BuildingV5, GridV5, GrowthStateV5, JunctionStateV5, RoadNetworkV5, VehicleV5, VersionsV5, buildings_of,
    junctions_of, upgrade_buildings, upgrade_junctions, upgrade_vehicles, vehicles_of,
};
use crate::derived::Access;
use crate::economy::Economy;
use crate::roads::NodeId;
use crate::side::SideTable;
use crate::stats::Stats;
use crate::traffic::{Departure, Place, TrafficState, Trip, TripPurpose, VehicleId};
use crate::{BuildingId, Demand, Household, HouseholdId, HouseholdState, IdMap, Rng, SimTime, World};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct HouseholdV4 {
    pub home: BuildingId,
    pub job: Option<BuildingId>,
    pub members: u8,
    pub state: HouseholdState,
    pub vehicle: Option<VehicleId>,
    pub seed: u32,
    pub unemployed_days: u8,
    pub hopeless_commutes: u8,
    pub last_commute_s: f32,
    pub last_work_day: u32,
}

impl HouseholdV4 {
    /// A live household without citizens (adopted after loading).
    pub(crate) fn upgrade(self, day: u32) -> Household {
        let mut h = Household::empty(self.home, self.seed, 0, day);
        h.job = self.job;
        h.members = self.members;
        h.state = self.state;
        h.vehicle = self.vehicle;
        h.unemployed_days = self.unemployed_days;
        h.hopeless_commutes = self.hopeless_commutes;
        h.last_commute_s = self.last_commute_s;
        h.last_work_day = self.last_work_day;
        h
    }

    pub(crate) fn of(h: &Household) -> HouseholdV4 {
        HouseholdV4 {
            home: h.home,
            job: h.job,
            members: h.members,
            state: h.state,
            vehicle: h.vehicle,
            seed: h.seed,
            unemployed_days: h.unemployed_days,
            hopeless_commutes: h.hopeless_commutes,
            last_commute_s: h.last_commute_s,
            last_work_day: h.last_work_day,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DepartureV4 {
    pub household: Option<HouseholdId>,
    pub purpose: TripPurpose,
    pub from: Place,
    pub to: Place,
    pub requested_tick: u64,
}

impl DepartureV4 {
    pub(crate) fn upgrade(self) -> Departure {
        Departure {
            household: self.household,
            citizen: None,
            purpose: self.purpose,
            from: self.from,
            to: self.to,
            requested_tick: self.requested_tick,
            from_building: None,
            to_building: None,
            party: 0,
            cargo: 0.0,
        }
    }

    pub(crate) fn of(d: &Departure) -> DepartureV4 {
        DepartureV4 {
            household: d.household,
            purpose: d.purpose,
            from: d.from,
            to: d.to,
            requested_tick: d.requested_tick,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct TripV4 {
    pub purpose: TripPurpose,
    pub to: Place,
    pub dest: Option<Access>,
    pub requested_tick: u64,
    pub wait_ticks: u32,
    pub needs_route: bool,
    pub queued: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrafficStateV4 {
    trips: SideTable<VehicleId, TripV4>,
    junction_states: SideTable<NodeId, JunctionStateV5>,
    junctions_version: u64,
    pending: VecDeque<DepartureV4>,
    replan: VecDeque<VehicleId>,
    roads_version: u64,
    avg_commute_s: f32,
    avg_speed: f32,
    peak_today: u32,
    peak_yesterday: u32,
    trips_today: u32,
    errand_acc: [f32; 2],
    congestion_share: f32,
    peak_congestion_today: f32,
    peak_congestion_yesterday: f32,
    stuck_removed: u32,
    deadlocks_resolved: u32,
    control_version: u64,
}

/// `Economy` as rounds 2-3 saved it (no running-cost breakdown).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct EconomyV4 {
    money: i64,
    tax_rate: f32,
    last_day_income: i64,
    last_day_expenses: i64,
    last_day: crate::Budget,
    history: Vec<crate::DaySummary>,
    fiscal: crate::FiscalStatus,
}

impl EconomyV4 {
    fn upgrade(self) -> Economy {
        Economy {
            money: self.money,
            tax_rate: self.tax_rate,
            last_day_income: self.last_day_income,
            last_day_expenses: self.last_day_expenses,
            last_day: self.last_day,
            history: self.history,
            fiscal: self.fiscal,
            last_day_costs: Default::default(),
            attendance: Default::default(),
        }
    }

    fn of(e: &Economy) -> EconomyV4 {
        EconomyV4 {
            money: e.money,
            tax_rate: e.tax_rate,
            last_day_income: e.last_day_income,
            last_day_expenses: e.last_day_expenses,
            last_day: e.last_day,
            history: e.history.clone(),
            fiscal: e.fiscal,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorldV4 {
    seed: u64,
    rng: Rng,
    time: SimTime,
    roads: RoadNetworkV5,
    grid: GridV5,
    buildings: IdMap<BuildingId, BuildingV5>,
    households: IdMap<HouseholdId, HouseholdV4>,
    vehicles: IdMap<VehicleId, VehicleV5>,
    economy: EconomyV4,
    demand: Demand,
    stats: Stats,
    versions: VersionsV5,
    traffic: TrafficStateV4,
    growth: GrowthStateV5,
}

/// A live trip from a frozen one: the driver is unknown (set when the
/// household is adopted), the origin is where the car is now.
pub(crate) fn trip_from(t: TripV4, here: [f32; 2]) -> Trip {
    Trip {
        purpose: t.purpose,
        to: t.to,
        dest: t.dest,
        requested_tick: t.requested_tick,
        wait_ticks: t.wait_ticks,
        needs_route: t.needs_route,
        queued: t.queued,
        citizen: None,
        owner: None,
        from: Place::At(here),
        from_building: None,
        to_building: None,
        party: 0,
        cargo: if t.purpose == TripPurpose::Goods {
            crate::balance::TRUCK_LOAD
        } else {
            0.0
        },
        started_tick: t.requested_tick,
    }
}

/// The clock of a save from before the 16-minute day (rounds 1-3 had a
/// 4-minute day of 4,800 ticks): the same time of day.
pub(crate) fn upgrade_time(t: SimTime) -> SimTime {
    const OLD_TICKS_PER_DAY: u32 = 4 * 60 * crate::TICKS_PER_SECOND;
    let tod = (t.tick_of_day.min(OLD_TICKS_PER_DAY - 1) as u64 * crate::balance::TICKS_PER_DAY as u64
        / OLD_TICKS_PER_DAY as u64) as u32;
    SimTime { tick_of_day: tod, ..t }
}

impl WorldV4 {
    pub(crate) fn upgrade(self) -> World {
        let day = self.time.day;
        let t = self.traffic;
        let mut trips = SideTable::default();
        for (id, v) in self.vehicles.iter() {
            if let Some(tr) = t.trips.get(id) {
                trips.insert(id, trip_from(tr.clone(), v.pos));
            }
        }
        let traffic = TrafficState {
            trips,
            junction_states: upgrade_junctions(t.junction_states),
            junctions_version: 0,
            pending: t.pending.into_iter().map(DepartureV4::upgrade).collect(),
            replan: t.replan,
            roads_version: 0,
            avg_commute_s: t.avg_commute_s,
            avg_speed: t.avg_speed,
            peak_today: t.peak_today,
            peak_yesterday: t.peak_yesterday,
            trips_today: t.trips_today,
            errand_acc: t.errand_acc,
            congestion_share: t.congestion_share,
            peak_congestion_today: t.peak_congestion_today,
            peak_congestion_yesterday: t.peak_congestion_yesterday,
            stuck_removed: t.stuck_removed,
            deadlocks_resolved: t.deadlocks_resolved,
            control_version: t.control_version,
            stopped_share: 0.0,
            delay_factor: 1.0,
        };
        let mut w = World::new(self.seed);
        w.rng = self.rng;
        w.time = upgrade_time(self.time);
        w.roads = self.roads.upgrade();
        w.buildings = upgrade_buildings(self.buildings);
        w.households = self.households.map_values(|h| h.upgrade(day));
        w.vehicles = upgrade_vehicles(self.vehicles);
        w.economy = self.economy.upgrade();
        w.demand = self.demand;
        w.stats = self.stats;
        w.versions = self.versions.upgrade();
        w.traffic = traffic;
        w.growth = self.growth.upgrade();
        w.cache = Default::default();
        crate::zoning::adopt_grid(&mut w, &self.grid.zones());
        w
    }
}

/// Buildings with round-2 occupancy: residents per home, one worker per
/// household job (what older saves hold).
pub(crate) fn legacy_buildings(w: &World) -> IdMap<BuildingId, BuildingV5> {
    let mut b = buildings_of(&w.buildings);
    for x in b.values_mut() {
        x.occupied = 0;
    }
    for h in w.households.values() {
        if let Some(x) = b.get_mut(h.home) {
            x.occupied += h.members as u32;
        }
        if let Some(x) = h.job.and_then(|j| b.get_mut(j)) {
            x.occupied += 1;
        }
    }
    b
}

/// Write a version-4 save of a world (round-2 layout): used by tests to
/// check that round-2 saves load. Citizens and other round-3 state are
/// dropped; each household keeps its home, first job and member count.
pub fn save_as_v4(w: &World) -> Vec<u8> {
    let t = &w.traffic;
    let mut trips = SideTable::default();
    for (id, _) in w.vehicles.iter() {
        if let Some(tr) = t.trips.get(id) {
            trips.insert(
                id,
                TripV4 {
                    purpose: if tr.purpose == TripPurpose::Leisure {
                        TripPurpose::Shopping
                    } else {
                        tr.purpose
                    },
                    to: tr.to,
                    dest: tr.dest,
                    requested_tick: tr.requested_tick,
                    wait_ticks: tr.wait_ticks,
                    needs_route: tr.needs_route,
                    queued: tr.queued,
                },
            );
        }
    }
    let world = WorldV4 {
        seed: w.seed,
        rng: w.rng.clone(),
        time: w.time.clone(),
        roads: RoadNetworkV5::of(&w.roads),
        grid: GridV5::of_world(w),
        buildings: legacy_buildings(w),
        households: w.households.clone().map_values(|h| HouseholdV4::of(&h)),
        vehicles: vehicles_of(&w.vehicles),
        economy: EconomyV4::of(&w.economy),
        demand: w.demand,
        stats: w.stats.clone(),
        versions: VersionsV5::of(&w.versions),
        traffic: TrafficStateV4 {
            trips,
            junction_states: junctions_of(&t.junction_states),
            junctions_version: t.junctions_version,
            pending: t
                .pending
                .iter()
                .filter(|d| d.purpose != TripPurpose::Leisure)
                .map(DepartureV4::of)
                .collect(),
            replan: t.replan.clone(),
            roads_version: t.roads_version,
            avg_commute_s: t.avg_commute_s,
            avg_speed: t.avg_speed,
            peak_today: t.peak_today,
            peak_yesterday: t.peak_yesterday,
            trips_today: t.trips_today,
            errand_acc: t.errand_acc,
            congestion_share: t.congestion_share,
            peak_congestion_today: t.peak_congestion_today,
            peak_congestion_yesterday: t.peak_congestion_yesterday,
            stuck_removed: t.stuck_removed,
            deadlocks_resolved: t.deadlocks_resolved,
            control_version: t.control_version,
        },
        growth: GrowthStateV5::of(&w.growth),
    };
    let mut out = Vec::new();
    out.extend_from_slice(super::MAGIC);
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&postcard::to_stdvec(&world).expect("serialisable"));
    let ext = postcard::to_stdvec(&(1u32, super::ExtensionV1::of(w))).expect("serialisable");
    out.extend_from_slice(&ext);
    out
}
