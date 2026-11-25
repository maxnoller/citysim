//! The round-1 save layout (format version 3), frozen.
//!
//! Postcard is not self-describing: a body decodes only into types with
//! exactly the field order and types it was written from. These are copies
//! of the round-1 definitions of every type whose layout changed since
//! (types that did not change are reused from the live crate). Never edit
//! them; [`WorldV3::upgrade`] turns a decoded version-3 world into a live
//! one.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::v4::{DepartureV4, HouseholdV4};
use super::v5::{BuildingV5, GridV5, upgrade_buildings};
use crate::derived::Access;
use crate::economy::{Budget, DaySummary, Economy};
use crate::growth::{BuildingMeta, GrowthState};
use crate::roads::{NodeId, RoadKind, RoadNetwork, RoadNode, RoadSegment, SegmentId};
use crate::side::SideTable;
use crate::stats::Stats;
use crate::traffic::{TrafficState, Vehicle, VehicleId};
use crate::{BuildingId, Demand, HouseholdId, IdMap, Rng, SimTime, Versions, World, balance};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WorldV3 {
    seed: u64,
    rng: Rng,
    time: SimTime,
    roads: RoadNetworkV3,
    grid: GridV5,
    buildings: IdMap<BuildingId, BuildingV5>,
    households: IdMap<HouseholdId, HouseholdV4>,
    vehicles: IdMap<VehicleId, VehicleV3>,
    economy: EconomyV3,
    demand: Demand,
    stats: StatsV3,
    versions: Versions,
    traffic: TrafficStateV3,
    growth: GrowthStateV3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RoadKindV3 {
    Street,
    Highway,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoadNodeV3 {
    pos: [f32; 2],
    segments: Vec<SegmentId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoadSegmentV3 {
    a: NodeId,
    b: NodeId,
    kind: RoadKindV3,
    length: f32,
    speed_limit: f32,
    capacity: [f32; 2],
    load: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoadNetworkV3 {
    nodes: IdMap<NodeId, RoadNodeV3>,
    segments: IdMap<SegmentId, RoadSegmentV3>,
    entry: Option<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VehicleV3 {
    pos: [f32; 2],
    heading: f32,
    segment: Option<SegmentId>,
    progress: f32,
    forward: bool,
    speed: f32,
    route: Vec<SegmentId>,
    destination: Option<NodeId>,
    household: Option<HouseholdId>,
    seed: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct BudgetV3 {
    residential_tax: i64,
    commercial_tax: i64,
    industrial_tax: i64,
    road_upkeep: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct DaySummaryV3 {
    day: u32,
    population: u32,
    households: u32,
    jobs: u32,
    employed: u32,
    buildings: u32,
    road_cells: u32,
    money: i64,
    income: i64,
    expenses: i64,
    peak_vehicles: u32,
    avg_commute_s: f32,
    demand: [f32; 3],
    peak_congestion: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EconomyV3 {
    money: i64,
    tax_rate: f32,
    last_day_income: i64,
    last_day_expenses: i64,
    last_day: BudgetV3,
    history: Vec<DaySummaryV3>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StatsV3 {
    population: u32,
    jobs: u32,
    employed: u32,
    households: u32,
    buildings: u32,
    vehicles_on_road: u32,
    road_cells: u32,
    unemployed: u32,
    unemployment: f32,
    residential_capacity: u32,
    commercial_jobs: u32,
    industrial_jobs: u32,
    buildings_by_kind: [u32; 3],
    avg_commute_s: f32,
    avg_speed: f32,
    waiting_departures: u32,
    congested_segments: u32,
    congestion_share: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct JunctionV3 {
    busy_until: u64,
    last_served: u8,
    phase_count: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TripV3 {
    purpose: crate::TripPurpose,
    to: crate::Place,
    dest: Option<Access>,
    requested_tick: u64,
    wait_ticks: u32,
    needs_route: bool,
    queued: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrafficStateV3 {
    trips: SideTable<VehicleId, TripV3>,
    junctions: Vec<JunctionV3>,
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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct BuildingMetaV3 {
    empty_days: u8,
    unreachable_days: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrowthStateV3 {
    acc: [f32; 3],
    move_in_acc: f32,
    meta: SideTable<BuildingId, BuildingMetaV3>,
    abandoned_total: u32,
    attractiveness: f32,
}

fn budget(b: BudgetV3) -> Budget {
    Budget {
        residential_tax: b.residential_tax,
        commercial_tax: b.commercial_tax,
        industrial_tax: b.industrial_tax,
        road_upkeep: b.road_upkeep,
        services: 0,
    }
}

impl WorldV3 {
    /// The live world this round-1 world becomes. Vehicles keep their
    /// place and route (on lane 0, outside any junction); junction control
    /// starts on `Auto` everywhere; old second-row buildings are converted
    /// by `growth::convert_legacy_lots` after loading.
    pub(crate) fn upgrade(self) -> World {
        let mut roads = RoadNetwork::new();
        roads.entry = self.roads.entry;
        roads.nodes = self.roads.nodes.map_values(|n| RoadNode {
            pos: n.pos,
            segments: n.segments,
        });
        let mut serial = 0;
        let nodes = &roads.nodes;
        roads.segments = self.roads.segments.map_values(|s| {
            let kind = match s.kind {
                RoadKindV3::Street => RoadKind::Street,
                RoadKindV3::Highway => RoadKind::Highway,
            };
            let (pa, pb) = (
                nodes.get(s.a).map_or([0.0; 2], |n| n.pos),
                nodes.get(s.b).map_or([0.0; 2], |n| n.pos),
            );
            serial += 1;
            RoadSegment::with_curve(s.a, s.b, kind, crate::Curve::straight(pa, pb), serial - 1, s.load)
        });
        roads.next_serial = serial;
        let vehicles = self.vehicles.map_values(|v| Vehicle {
            pos: v.pos,
            heading: v.heading,
            segment: v.segment,
            progress: v.progress,
            forward: v.forward,
            speed: v.speed,
            route: v.route,
            destination: v.destination,
            household: v.household,
            seed: v.seed,
            ..Vehicle::default()
        });
        let e = self.economy;
        let economy = Economy {
            money: e.money,
            tax_rate: e.tax_rate,
            last_day_income: e.last_day_income,
            last_day_expenses: e.last_day_expenses,
            last_day: budget(e.last_day),
            history: e
                .history
                .into_iter()
                .map(|d| DaySummary {
                    day: d.day,
                    population: d.population,
                    households: d.households,
                    jobs: d.jobs,
                    employed: d.employed,
                    buildings: d.buildings,
                    road_cells: d.road_cells,
                    money: d.money,
                    income: d.income,
                    expenses: d.expenses,
                    peak_vehicles: d.peak_vehicles,
                    avg_commute_s: d.avg_commute_s,
                    demand: d.demand,
                    peak_congestion: d.peak_congestion,
                    ..DaySummary::default()
                })
                .collect(),
            ..Economy::default()
        };
        let s = self.stats;
        let stats = Stats {
            population: s.population,
            jobs: s.jobs,
            employed: s.employed,
            households: s.households,
            buildings: s.buildings,
            vehicles_on_road: s.vehicles_on_road,
            road_cells: s.road_cells,
            unemployed: s.unemployed,
            unemployment: s.unemployment,
            residential_capacity: s.residential_capacity,
            commercial_jobs: s.commercial_jobs,
            industrial_jobs: s.industrial_jobs,
            buildings_by_kind: s.buildings_by_kind,
            avg_commute_s: s.avg_commute_s,
            avg_speed: s.avg_speed,
            waiting_departures: s.waiting_departures,
            congested_segments: s.congested_segments,
            congestion_share: s.congestion_share,
            workforce: s.households,
            vacant_jobs: s.jobs.saturating_sub(s.employed),
            ..Stats::default()
        };
        let t = self.traffic;
        let mut trips = SideTable::default();
        for (id, v) in vehicles.iter() {
            if let Some(tr) = t.trips.get(id) {
                let tr = super::v4::TripV4 {
                    purpose: tr.purpose,
                    to: tr.to,
                    dest: tr.dest,
                    requested_tick: tr.requested_tick,
                    wait_ticks: tr.wait_ticks,
                    needs_route: tr.needs_route,
                    queued: tr.queued,
                };
                trips.insert(id, super::v4::trip_from(tr, v.pos));
            }
        }
        let traffic = TrafficState {
            trips,
            pending: t.pending.into_iter().map(DepartureV4::upgrade).collect(),
            replan: t.replan,
            // Force a re-anchoring pass (lanes, junction plans) on the
            // first tick.
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
            ..TrafficState::default()
        };
        let g = self.growth;
        let mut meta = SideTable::default();
        for (id, _) in self.buildings.iter() {
            if let Some(m) = g.meta.get(id) {
                meta.insert(
                    id,
                    BuildingMeta {
                        empty_days: m.empty_days,
                        unreachable_days: m.unreachable_days,
                        unreachable_since: None,
                    },
                );
            }
        }
        let growth = GrowthState {
            acc: g.acc,
            move_in_acc: g.move_in_acc,
            meta,
            abandoned_total: g.abandoned_total,
            attractiveness: g.attractiveness,
            ..GrowthState::default()
        };
        let mut versions = self.versions;
        // Everything is new to the renderer.
        versions.roads += 1;
        versions.grid += 1;
        versions.buildings += 1;
        let day = self.time.day;
        let mut w = World::new(self.seed);
        w.rng = self.rng;
        w.time = super::v4::upgrade_time(self.time);
        w.roads = roads;
        w.buildings = upgrade_buildings(self.buildings);
        w.households = self.households.map_values(|h| h.upgrade(day));
        w.vehicles = vehicles;
        w.economy = economy;
        w.demand = self.demand;
        w.stats = stats;
        w.versions = versions;
        w.traffic = traffic;
        w.growth = growth;
        w.cache = Default::default();
        crate::zoning::adopt_grid(&mut w, &self.grid.zones());
        w
    }
}

/// Write a version-3 save of a world built only from round-1 features
/// (streets, no avenues): used by tests to check that round-1 saves load.
/// Live state without a round-1 equivalent is dropped.
pub fn save_as_v3(w: &World) -> Vec<u8> {
    let roads = RoadNetworkV3 {
        nodes: w.roads.nodes.clone().map_values(|n| RoadNodeV3 {
            pos: n.pos,
            segments: n.segments,
        }),
        segments: w.roads.segments.clone().map_values(|s| RoadSegmentV3 {
            a: s.a,
            b: s.b,
            kind: if s.kind == RoadKind::Highway {
                RoadKindV3::Highway
            } else {
                RoadKindV3::Street
            },
            length: s.length,
            speed_limit: s.speed_limit,
            capacity: s.capacity,
            load: s.load,
        }),
        entry: w.roads.entry,
    };
    let vehicles = w.vehicles.clone().map_values(|v| VehicleV3 {
        pos: v.pos,
        heading: v.heading,
        segment: v.segment,
        progress: v.progress,
        forward: v.forward,
        speed: v.speed,
        route: v.route,
        destination: v.destination,
        household: v.household,
        seed: v.seed,
    });
    let b3 = |b: Budget| BudgetV3 {
        residential_tax: b.residential_tax,
        commercial_tax: b.commercial_tax,
        industrial_tax: b.industrial_tax,
        road_upkeep: b.road_upkeep,
    };
    let e = &w.economy;
    let economy = EconomyV3 {
        money: e.money,
        tax_rate: e.tax_rate,
        last_day_income: e.last_day_income,
        last_day_expenses: e.last_day_expenses,
        last_day: b3(e.last_day),
        history: e
            .history
            .iter()
            .map(|d| DaySummaryV3 {
                day: d.day,
                population: d.population,
                households: d.households,
                jobs: d.jobs,
                employed: d.employed,
                buildings: d.buildings,
                road_cells: d.road_cells,
                money: d.money,
                income: d.income,
                expenses: d.expenses,
                peak_vehicles: d.peak_vehicles,
                avg_commute_s: d.avg_commute_s,
                demand: d.demand,
                peak_congestion: d.peak_congestion,
            })
            .collect(),
    };
    let s = &w.stats;
    let stats = StatsV3 {
        population: s.population,
        jobs: s.jobs,
        employed: s.employed,
        households: s.households,
        buildings: s.buildings,
        vehicles_on_road: s.vehicles_on_road,
        road_cells: s.road_cells,
        unemployed: s.unemployed,
        unemployment: s.unemployment,
        residential_capacity: s.residential_capacity,
        commercial_jobs: s.commercial_jobs,
        industrial_jobs: s.industrial_jobs,
        buildings_by_kind: s.buildings_by_kind,
        avg_commute_s: s.avg_commute_s,
        avg_speed: s.avg_speed,
        waiting_departures: s.waiting_departures,
        congested_segments: s.congested_segments,
        congestion_share: s.congestion_share,
    };
    let t = &w.traffic;
    let mut trips = SideTable::default();
    for (id, _) in w.vehicles.iter() {
        if let Some(tr) = t.trips.get(id) {
            trips.insert(
                id,
                TripV3 {
                    purpose: tr.purpose,
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
    let traffic = TrafficStateV3 {
        trips,
        junctions: vec![
            JunctionV3 {
                busy_until: 0,
                last_served: 0,
                phase_count: 0,
            };
            w.roads.nodes.slot_count()
        ],
        pending: t.pending.iter().map(DepartureV4::of).collect(),
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
    };
    let mut meta = SideTable::default();
    for (id, _) in w.buildings.iter() {
        if let Some(m) = w.growth.meta.get(id) {
            meta.insert(
                id,
                BuildingMetaV3 {
                    empty_days: m.empty_days,
                    unreachable_days: m.unreachable_days,
                },
            );
        }
    }
    let g = &w.growth;
    let world = WorldV3 {
        seed: w.seed,
        rng: w.rng.clone(),
        time: w.time.clone(),
        roads,
        grid: GridV5::of_world(w),
        buildings: super::v4::legacy_buildings(w),
        households: w.households.clone().map_values(|h| HouseholdV4::of(&h)),
        vehicles,
        economy,
        demand: w.demand,
        stats,
        versions: w.versions.clone(),
        traffic,
        growth: GrowthStateV3 {
            acc: g.acc,
            move_in_acc: g.move_in_acc,
            meta,
            abandoned_total: g.abandoned_total,
            attractiveness: g.attractiveness,
        },
    };
    let mut out = Vec::new();
    out.extend_from_slice(super::MAGIC);
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&postcard::to_stdvec(&world).expect("serialisable"));
    let ext = postcard::to_stdvec(&(1u32, super::ExtensionV1::of(w))).expect("serialisable");
    out.extend_from_slice(&ext);
    let _ = balance::TICKS_PER_DAY;
    out
}
