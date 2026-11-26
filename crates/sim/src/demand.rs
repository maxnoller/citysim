//! RCI demand, each in `-1.0..=1.0`, from the state of the city.
//!
//! * Residential: a base pull, plus free jobs beyond the unemployed (per
//!   member of the workforce), plus how happy the citizens are, minus
//!   empty housing, taxes above default and long average commutes.
//! * Commercial: shops wanted per resident vs. shop jobs that exist, plus
//!   unemployment, minus unfilled shop jobs and taxes.
//! * Industrial: industry wanted per shop job, same corrections.
//!
//! The raw values are recomputed every `DEMAND_INTERVAL_TICKS` and the
//! published demand eases towards them (`DEMAND_SMOOTHING`), so it responds
//! within seconds but does not jump around.

use serde::{Deserialize, Serialize};

use crate::{World, Zone, balance};

/// Demand per zone type, each in `-1.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Demand {
    pub residential: f32,
    pub commercial: f32,
    pub industrial: f32,
}

/// What the demand would be right now, before smoothing, and how
/// attractive the city is to newcomers (residential demand ignoring empty
/// housing; gates moving in).
pub fn target(world: &World) -> (Demand, f32) {
    let mut pop = 0f32;
    let mut res_cap = 0f32;
    let (mut c_jobs, mut c_filled, mut i_jobs, mut i_filled) = (0f32, 0f32, 0f32, 0f32);
    for b in world.buildings.values() {
        match b.kind {
            Zone::Residential => {
                pop += b.occupied as f32;
                res_cap += b.capacity as f32;
            }
            Zone::Commercial => {
                c_jobs += b.capacity as f32;
                c_filled += b.occupied as f32;
            }
            Zone::Industrial => {
                i_jobs += b.capacity as f32;
                i_filled += b.occupied as f32;
            }
        }
    }
    let (population, employed, workforce) = crate::stats::labour(world);
    let households = workforce as f32;
    let employed = employed as f32;
    let unemployed = (households - employed).max(0.0);
    // Happy citizens draw newcomers, unhappy ones put them off.
    let mood = if population > 0 {
        balance::HAPPINESS_DEMAND_WEIGHT * (world.people.stats.avg_happiness - balance::HAPPINESS_NEUTRAL)
    } else {
        0.0
    };
    let small = balance::DEMAND_SMALL_CITY;
    let tax = tax_effect(world.economy.tax_rate);
    let f = &world.economy.fiscal;
    let service_gap = 1.0 - f.service_level;
    let debt = if world.economy.money < 0 {
        balance::DEBT_DEMAND_PENALTY
    } else {
        0.0
    };
    let res_fiscal = -balance::SERVICE_DEMAND_WEIGHT * service_gap - debt;
    let job_fiscal = -0.5 * balance::SERVICE_DEMAND_WEIGHT * service_gap - debt;

    let free_jobs = (c_jobs + i_jobs - c_filled - i_filled).max(0.0);
    // Damped for tiny towns: one empty new house is not a housing glut.
    let res_vacancy = (res_cap - pop).max(0.0) / (res_cap + 2.0 * small);
    let commute = world.traffic.avg_commute_s;
    let attract = balance::RES_BASE_DEMAND + balance::RES_JOBS_WEIGHT * (free_jobs - unemployed) / (households + small)
        - (commute - balance::COMMUTE_OK_S).max(0.0) / balance::COMMUTE_DEMAND_SCALE_S
        - (crate::traffic::delay_factor(world) - balance::DELAY_OK_FACTOR).max(0.0) * balance::DELAY_DEMAND_PER_UNIT
        + tax
        + res_fiscal
        + mood;
    let r = attract - balance::RES_VACANCY_WEIGHT * (res_vacancy - balance::RES_VACANCY_OK).max(0.0);

    let unemployment = if households > 0.0 { unemployed / households } else { 0.0 };
    // Jobs follow the workforce (every adult wants their own job).
    let want_c = households * balance::SHOP_JOBS_PER_WORKER;
    let want_i = want_c * balance::INDUSTRY_PER_SHOP;
    // People out of work call for the kind of jobs that is short: the
    // unemployment boost goes to commerce and industry by their shortfall
    // (alike when neither is short).
    let short = |want: f32, have: f32| ((want - have) / (want + small)).max(0.0);
    let (sc, si) = (short(want_c, c_jobs), short(want_i, i_jobs));
    let (share_c, share_i) = if sc + si > 0.0 {
        (sc / (sc + si), si / (sc + si))
    } else {
        (0.5, 0.5)
    };
    let jobs_term = |want: f32, have: f32, filled: f32, share: f32| {
        let vacancy = if have > 0.0 { (have - filled) / have } else { 0.0 };
        balance::JOB_SHORTFALL_WEIGHT * (want - have) / (want + small)
            + 2.0 * share * balance::UNEMPLOYMENT_JOB_BOOST * unemployment
            - balance::JOB_VACANCY_WEIGHT * (vacancy - balance::JOB_VACANCY_OK).max(0.0)
            + tax
            + job_fiscal
    };
    let demand = Demand {
        residential: r.clamp(-1.0, 1.0),
        commercial: jobs_term(want_c, c_jobs, c_filled, share_c).clamp(-1.0, 1.0),
        industrial: jobs_term(want_i, i_jobs, i_filled, share_i).clamp(-1.0, 1.0),
    };
    (demand, attract.clamp(-1.0, 1.0))
}

/// Demand change from the tax rate: steep above the default, gentle and
/// capped below it.
pub fn tax_effect(rate: f32) -> f32 {
    let d = rate - balance::DEFAULT_TAX_RATE;
    if d >= 0.0 {
        -balance::TAX_DEMAND_SLOPE * d
    } else {
        (-d * balance::LOW_TAX_DEMAND_SLOPE).min(balance::LOW_TAX_DEMAND_MAX)
    }
}

/// Set demand straight to its target (no smoothing): a new game, and
/// commands applied before the clock ever ran, show valid demand at once.
pub(crate) fn refresh(world: &mut World) {
    let (t, attract) = target(world);
    world.demand = t;
    world.growth.attractiveness = attract;
}

pub(crate) fn tick(world: &mut World) {
    if !world.time.tick.is_multiple_of(balance::DEMAND_INTERVAL_TICKS) {
        return;
    }
    let (t, attract) = target(world);
    let k = balance::DEMAND_SMOOTHING;
    world.growth.attractiveness += (attract - world.growth.attractiveness) * k;
    let d = &mut world.demand;
    d.residential += (t.residential - d.residential) * k;
    d.commercial += (t.commercial - d.commercial) * k;
    d.industrial += (t.industrial - d.industrial) * k;
}

/// Why people are out of work (`jobs_view`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JoblessReason {
    /// Hardly anyone (under `JOBLESS_OK` of the workforce).
    None,
    /// Jobs are free and reachable: they are being filled.
    Searching,
    /// The free jobs are in workplaces no road reaches from where the
    /// jobless live.
    NoJobsWithinReach,
    /// Not enough jobs and no free land zoned for the kind that is short
    /// (`JobsView::short_of`): zone more.
    NoLandZoned,
    /// Not enough jobs, land is zoned: workplaces are being built.
    BuildingsComing,
}

impl JoblessReason {
    /// A short sentence for the interface.
    pub fn text(self) -> &'static str {
        match self {
            JoblessReason::None => "Nearly everyone who wants a job has one",
            JoblessReason::Searching => "Looking for one of the free jobs",
            JoblessReason::NoJobsWithinReach => "No jobs within reach: free jobs are cut off by road",
            JoblessReason::NoLandZoned => "Not enough jobs: zone more land for work",
            JoblessReason::BuildingsComing => "Not enough jobs yet: workplaces are being built",
        }
    }
}

/// The city's jobs at a glance: who is out of work and why.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JobsView {
    pub workforce: u32,
    pub unemployed: u32,
    /// Free job places, and those in workplaces connected to the road
    /// network.
    pub vacancies: u32,
    pub vacancies_reachable: u32,
    /// Free zoned lots, cells per zone `[R, C, I]`.
    pub free_cells: [u32; 3],
    /// The kind of work the city is short of (relative to its workforce),
    /// if any: `Zone::Commercial` or `Zone::Industrial`.
    pub short_of: Option<Zone>,
    pub reason: JoblessReason,
}

impl JobsView {
    /// `unemployed / workforce`.
    pub fn rate(&self) -> f32 {
        self.unemployed as f32 / self.workforce.max(1) as f32
    }
}

/// Who is out of work and why (`O(buildings + zone cells)`: call it at
/// most every few frames).
pub fn jobs_view(world: &World) -> JobsView {
    let (_, employed, workforce) = crate::stats::labour(world);
    let unemployed = workforce.saturating_sub(employed);
    let (mut vac, mut reach) = (0u32, 0u32);
    let (mut c_jobs, mut i_jobs) = (0f32, 0f32);
    for b in world.buildings.values() {
        match b.kind {
            Zone::Residential => continue,
            Zone::Commercial => c_jobs += b.capacity as f32,
            Zone::Industrial => i_jobs += b.capacity as f32,
        }
        let free = b.capacity.saturating_sub(b.occupied);
        vac += free;
        if free > 0 && crate::households::connected(world, b.footprint) {
            reach += free;
        }
    }
    let mut free_cells = [0u32; 3];
    for (_, cd) in world.zoning.cells() {
        if cd.free()
            && let Some(z) = cd.zone
        {
            free_cells[crate::derived::zone_index(z)] += 1;
        }
    }
    let small = balance::DEMAND_SMALL_CITY;
    let want_c = workforce as f32 * balance::SHOP_JOBS_PER_WORKER;
    let want_i = want_c * balance::INDUSTRY_PER_SHOP;
    let (sc, si) = ((want_c - c_jobs) / (want_c + small), (want_i - i_jobs) / (want_i + small));
    let short_of = if sc.max(si) <= 0.0 && unemployed == 0 {
        None
    } else if si >= sc {
        Some(Zone::Industrial)
    } else {
        Some(Zone::Commercial)
    };
    let rate = unemployed as f32 / workforce.max(1) as f32;
    let reason = if rate < balance::JOBLESS_OK {
        JoblessReason::None
    } else if reach * 2 >= unemployed {
        JoblessReason::Searching
    } else if vac * 2 >= unemployed {
        JoblessReason::NoJobsWithinReach
    } else if short_of.is_some_and(|z| free_cells[crate::derived::zone_index(z)] < balance::JOBLESS_LAND_CELLS) {
        JoblessReason::NoLandZoned
    } else {
        JoblessReason::BuildingsComing
    };
    JobsView {
        workforce,
        unemployed,
        vacancies: vac,
        vacancies_reachable: reach,
        free_cells,
        short_of,
        reason,
    }
}
