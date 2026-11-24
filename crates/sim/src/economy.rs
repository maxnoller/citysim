//! Money, taxes and upkeep. Booked once a day, at midnight.

use serde::{Deserialize, Serialize};

use crate::{World, Zone, balance};

/// One day's budget by category (all amounts positive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Budget {
    /// Taxes paid by residents.
    pub residential_tax: i64,
    /// Taxes paid per filled commercial job.
    pub commercial_tax: i64,
    /// Taxes paid per filled industrial job.
    pub industrial_tax: i64,
    /// Road upkeep.
    pub road_upkeep: i64,
}

impl Budget {
    pub fn income(&self) -> i64 {
        self.residential_tax + self.commercial_tax + self.industrial_tax
    }

    pub fn expenses(&self) -> i64 {
        self.road_upkeep
    }
}

/// End-of-day snapshot, for charts.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct DaySummary {
    /// The day that just ended.
    pub day: u32,
    pub population: u32,
    pub households: u32,
    pub jobs: u32,
    pub employed: u32,
    pub buildings: u32,
    pub road_cells: u32,
    /// Money after booking the day.
    pub money: i64,
    pub income: i64,
    pub expenses: i64,
    /// Most cars on the road at once during the day.
    pub peak_vehicles: u32,
    /// Moving average of car commutes, vehicle seconds.
    pub avg_commute_s: f32,
    pub demand: [f32; 3],
    /// Highest share of road segments above 80 % of capacity during the day.
    pub peak_congestion: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Economy {
    pub money: i64,
    /// Fraction, `0.0..=balance::MAX_TAX_RATE`.
    pub tax_rate: f32,
    /// Income booked at the end of the previous day.
    pub last_day_income: i64,
    /// Expenses booked at the end of the previous day.
    pub last_day_expenses: i64,
    /// The previous day's budget by category (sums to the two above).
    #[serde(default)]
    pub last_day: Budget,
    /// One entry per finished day, oldest first.
    #[serde(default)]
    pub history: Vec<DaySummary>,
}

impl Default for Economy {
    fn default() -> Self {
        Economy {
            money: balance::STARTING_MONEY,
            tax_rate: balance::DEFAULT_TAX_RATE,
            last_day_income: 0,
            last_day_expenses: 0,
            last_day: Budget::default(),
            history: Vec::new(),
        }
    }
}

/// What today's budget would be if the day ended now.
pub fn budget_now(world: &World) -> Budget {
    let rate = world.economy.tax_rate;
    let (mut res, mut com, mut ind) = (0u64, 0u64, 0u64);
    for b in world.buildings.values() {
        match b.kind {
            Zone::Residential => res += b.occupied as u64,
            Zone::Commercial => com += b.occupied as u64,
            Zone::Industrial => ind += b.occupied as u64,
        }
    }
    let tax = |n: u64, base: f32| (n as f64 * base as f64 * rate as f64).round() as i64;
    Budget {
        residential_tax: tax(res, balance::RESIDENT_TAX_BASE_PER_DAY),
        commercial_tax: tax(com, balance::COMMERCIAL_TAX_BASE_PER_JOB_PER_DAY),
        industrial_tax: tax(ind, balance::INDUSTRIAL_TAX_BASE_PER_JOB_PER_DAY),
        road_upkeep: world.stats.road_cells as i64 * balance::ROAD_UPKEEP_PER_CELL_PER_DAY,
    }
}

/// Called once when a new day starts.
pub(crate) fn end_of_day(world: &mut World) {
    let b = budget_now(world);
    let e = &mut world.economy;
    e.money += b.income() - b.expenses();
    e.last_day = b;
    e.last_day_income = b.income();
    e.last_day_expenses = b.expenses();
    let s = &world.stats;
    let d = &world.demand;
    let summary = DaySummary {
        day: world.time.day.saturating_sub(1),
        population: s.population,
        households: s.households,
        jobs: s.jobs,
        employed: s.employed,
        buildings: s.buildings,
        road_cells: s.road_cells,
        money: e.money,
        income: b.income(),
        expenses: b.expenses(),
        peak_vehicles: world.traffic.peak_yesterday.max(world.traffic.peak_today),
        avg_commute_s: world.traffic.avg_commute_s,
        demand: [d.residential, d.commercial, d.industrial],
        peak_congestion: world
            .traffic
            .peak_congestion_yesterday
            .max(world.traffic.peak_congestion_today),
    };
    e.history.push(summary);
}
