//! Aggregate numbers for the HUD.

use serde::{Deserialize, Serialize};

use crate::{World, Zone};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Stats {
    /// Residents (citizens; sum of household members).
    pub population: u32,
    /// Job places (capacity of commercial + industrial buildings).
    pub jobs: u32,
    /// Citizens with a job (adults, and teens with a part-time job): the
    /// number of filled job places.
    pub employed: u32,
    /// Households living in the city (including those still driving in).
    pub households: u32,
    /// Buildings of all kinds.
    pub buildings: u32,
    /// Cars on the road right now.
    pub vehicles_on_road: u32,
    /// Road cells, highway stub included.
    pub road_cells: u32,
    /// Workers without a job: `workforce - employed`.
    #[serde(default)]
    pub unemployed: u32,
    /// Unemployment rate, `unemployed / workforce` (0 when nobody lives in
    /// town), `0.0..=1.0`.
    #[serde(default)]
    pub unemployment: f32,
    /// Resident capacity of all residential buildings.
    #[serde(default)]
    pub residential_capacity: u32,
    /// Job places by kind.
    #[serde(default)]
    pub commercial_jobs: u32,
    #[serde(default)]
    pub industrial_jobs: u32,
    /// Buildings by kind `[residential, commercial, industrial]`.
    #[serde(default)]
    pub buildings_by_kind: [u32; 3],
    /// Moving average of car commute durations, vehicle seconds (at 1x
    /// equal to real seconds).
    #[serde(default)]
    pub avg_commute_s: f32,
    /// Mean speed of the cars on the road, m/s (0 when there are none).
    #[serde(default)]
    pub avg_speed: f32,
    /// Trips waiting for their car to start.
    #[serde(default)]
    pub waiting_departures: u32,
    /// Segments with a direction over 80 % of capacity.
    #[serde(default)]
    pub congested_segments: u32,
    /// `congested_segments` / all segments, `0.0..=1.0`.
    #[serde(default)]
    pub congestion_share: f32,
    /// Avenue cells (pieces), included in `road_cells`.
    pub avenue_cells: u32,
    /// The workforce: citizens who want a job (adults 18-64 except
    /// stay-at-home parents, and teens with a part-time job; children and
    /// retirees are not in it). `employed + unemployed == workforce`.
    /// (Not stored in saves; restored on load.)
    #[serde(skip)]
    pub workforce: u32,
    /// Job places nobody fills: `jobs - employed`.
    /// (Not stored in saves; restored on load.)
    #[serde(skip)]
    pub vacant_jobs: u32,
}

/// The float fields are never NaN.
impl Eq for Stats {}

/// `(population, employed, workforce)` from the citizens (and, the way
/// round 2 counted them, from legacy households that have no citizens yet).
pub(crate) fn labour(world: &World) -> (u32, u32, u32) {
    let (mut pop, mut jobs, mut wf) = (0, 0, 0);
    if world.people.known_households != world.households.len() || world.citizens.is_empty() {
        for h in world.households.values().filter(|h| h.citizens.is_empty()) {
            pop += h.members as u32;
            wf += 1;
            jobs += u32::from(h.job.is_some());
        }
    }
    (
        world.citizens.len() as u32 + pop,
        world.people.wage_counts.iter().map(|&(_, n)| n).sum::<u32>() + jobs,
        world.people.workforce + wf,
    )
}

/// Recompute what can be derived cheaply from state.
pub(crate) fn update(world: &mut World) {
    let road_cells = world.cache.road_cells(&world.roads, world.versions.roads);
    let avenue_cells = world.cache.avenue_cells(&world.roads, world.versions.roads);
    let mut s = Stats {
        road_cells,
        avenue_cells,
        buildings: world.buildings.len() as u32,
        households: world.households.len() as u32,
        vehicles_on_road: world.vehicles.len() as u32,
        avg_commute_s: world.traffic.avg_commute_s,
        avg_speed: world.traffic.avg_speed,
        waiting_departures: world.traffic.pending.len() as u32,
        ..Stats::default()
    };
    for b in world.buildings.values() {
        match b.kind {
            Zone::Residential => {
                s.buildings_by_kind[0] += 1;
                s.residential_capacity += b.capacity;
            }
            Zone::Commercial => {
                s.buildings_by_kind[1] += 1;
                s.commercial_jobs += b.capacity;
            }
            Zone::Industrial => {
                s.buildings_by_kind[2] += 1;
                s.industrial_jobs += b.capacity;
            }
        }
    }
    s.jobs = s.commercial_jobs + s.industrial_jobs;
    (s.population, s.employed, s.workforce) = labour(world);
    s.unemployed = s.workforce.saturating_sub(s.employed);
    s.vacant_jobs = s.jobs.saturating_sub(s.employed);
    s.unemployment = if s.workforce > 0 {
        s.unemployed as f32 / s.workforce as f32
    } else {
        0.0
    };
    s.congested_segments = world
        .roads
        .segments
        .values()
        .filter(|seg| seg.congestion() > 0.8)
        .count() as u32;
    s.congestion_share = world.traffic.congestion_share;
    world.stats = s;
}
