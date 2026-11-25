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

use crate::derived::Access;
use crate::economy::Economy;
use crate::growth::GrowthState;
use crate::junction::JunctionState;
use crate::roads::{NodeId, RoadNetwork};
use crate::side::SideTable;
use crate::stats::Stats;
use crate::traffic::{Departure, Place, TrafficState, Trip, TripPurpose, Vehicle, VehicleId};
use crate::{BuildingId, Demand, Grid, Household, HouseholdId, HouseholdState, IdMap, Rng, SimTime, Versions, World};

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
    junction_states: SideTable<NodeId, JunctionState>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorldV4 {
    seed: u64,
    rng: Rng,
    time: SimTime,
    roads: RoadNetwork,
    grid: Grid,
    buildings: IdMap<BuildingId, crate::Building>,
    households: IdMap<HouseholdId, HouseholdV4>,
    vehicles: IdMap<VehicleId, Vehicle>,
    economy: Economy,
    demand: Demand,
    stats: Stats,
    versions: Versions,
    traffic: TrafficStateV4,
    growth: GrowthState,
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
    }
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
            junction_states: t.junction_states,
            junctions_version: t.junctions_version,
            pending: t.pending.into_iter().map(DepartureV4::upgrade).collect(),
            replan: t.replan,
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
        };
        let mut w = World::new(self.seed);
        w.rng = self.rng;
        w.time = self.time;
        w.roads = self.roads;
        w.grid = self.grid;
        w.buildings = self.buildings;
        w.households = self.households.map_values(|h| h.upgrade(day));
        w.vehicles = self.vehicles;
        w.economy = self.economy;
        w.demand = self.demand;
        w.stats = self.stats;
        w.versions = self.versions;
        w.traffic = traffic;
        w.growth = self.growth;
        w.cache = Default::default();
        w
    }
}

/// Buildings with round-2 occupancy: residents per home, one worker per
/// household job (what older saves hold).
pub(crate) fn legacy_buildings(w: &World) -> IdMap<BuildingId, crate::Building> {
    let mut b = w.buildings.clone();
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
        roads: w.roads.clone(),
        grid: w.grid.clone(),
        buildings: legacy_buildings(w),
        households: w.households.clone().map_values(|h| HouseholdV4::of(&h)),
        vehicles: w.vehicles.clone(),
        economy: w.economy.clone(),
        demand: w.demand,
        stats: w.stats.clone(),
        versions: w.versions.clone(),
        traffic: TrafficStateV4 {
            trips,
            junction_states: t.junction_states.clone(),
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
        growth: w.growth.clone(),
    };
    let mut out = Vec::new();
    out.extend_from_slice(super::MAGIC);
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&postcard::to_stdvec(&world).expect("serialisable"));
    let ext = postcard::to_stdvec(&(1u32, super::ExtensionV1::of(w))).expect("serialisable");
    out.extend_from_slice(&ext);
    out
}
