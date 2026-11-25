//! RCI demand, each in `-1.0..=1.0`, from the state of the city.
//!
//! * Residential: a base pull, plus free jobs beyond the unemployed, minus
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
    let households = world.households.len() as f32;
    let employed = world.households.values().filter(|h| h.job.is_some()).count() as f32;
    let unemployed = households - employed;
    let small = balance::DEMAND_SMALL_CITY;
    let tax = -balance::TAX_DEMAND_SLOPE * (world.economy.tax_rate - balance::DEFAULT_TAX_RATE);

    let free_jobs = (c_jobs + i_jobs - c_filled - i_filled).max(0.0);
    // Damped for tiny towns: one empty new house is not a housing glut.
    let res_vacancy = (res_cap - pop).max(0.0) / (res_cap + 2.0 * small);
    let commute = world.traffic.avg_commute_s;
    let attract = balance::RES_BASE_DEMAND + balance::RES_JOBS_WEIGHT * (free_jobs - unemployed) / (households + small)
        - (commute - balance::COMMUTE_OK_S).max(0.0) / balance::COMMUTE_DEMAND_SCALE_S
        + tax;
    let r = attract - balance::RES_VACANCY_WEIGHT * (res_vacancy - balance::RES_VACANCY_OK).max(0.0);

    let unemployment = if households > 0.0 { unemployed / households } else { 0.0 };
    let jobs_term = |want: f32, have: f32, filled: f32| {
        let vacancy = if have > 0.0 { (have - filled) / have } else { 0.0 };
        balance::JOB_SHORTFALL_WEIGHT * (want - have) / (want + small) + balance::UNEMPLOYMENT_JOB_BOOST * unemployment
            - balance::JOB_VACANCY_WEIGHT * (vacancy - balance::JOB_VACANCY_OK).max(0.0)
            + tax
    };
    let want_c = pop * balance::SHOP_JOBS_PER_RESIDENT;
    let want_i = want_c * balance::INDUSTRY_PER_SHOP;
    let demand = Demand {
        residential: r.clamp(-1.0, 1.0),
        commercial: jobs_term(want_c, c_jobs, c_filled).clamp(-1.0, 1.0),
        industrial: jobs_term(want_i, i_jobs, i_filled).clamp(-1.0, 1.0),
    };
    (demand, attract.clamp(-1.0, 1.0))
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
