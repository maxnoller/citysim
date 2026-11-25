//! Money, taxes and upkeep. Booked once a day, at midnight.

use serde::{Deserialize, Serialize};

use crate::{World, Zone, balance};

/// One day's budget by category (all amounts positive, whole dollars).
///
/// `income()` is the sum of the three tax fields, `expenses()` the sum of
/// `road_upkeep` and `services`; the day's net is `income() - expenses()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Budget {
    /// Taxes paid by residents: the tax rate times the daily wages of the
    /// employed (rounded per household), taken from each household's money
    /// at payday (`people::residential_tax` projects today's). Pensions are
    /// not taxed.
    pub residential_tax: i64,
    /// Taxes paid per filled commercial job: filled jobs *
    /// `COMMERCIAL_TAX_BASE_PER_JOB_PER_DAY` * tax rate.
    pub commercial_tax: i64,
    /// Taxes paid per filled industrial job: filled jobs *
    /// `INDUSTRIAL_TAX_BASE_PER_JOB_PER_DAY` * tax rate.
    pub industrial_tax: i64,
    /// Road upkeep: road cells (highway stub included) *
    /// `ROAD_UPKEEP_PER_CELL_PER_DAY`.
    pub road_upkeep: i64,
    /// City services (police, fire, schools, rubbish... abstracted):
    /// residents * `SERVICES_COST_PER_RESIDENT_PER_DAY`. Grows with the
    /// city, so the margin per resident stays thin as it grows. (Saved in
    /// the save file's extension block; 0 in days booked by older saves.)
    #[serde(skip)]
    pub services: i64,
}

impl Budget {
    /// All tax income of the day.
    pub fn income(&self) -> i64 {
        self.residential_tax + self.commercial_tax + self.industrial_tax
    }

    /// All running costs of the day (road upkeep and services).
    pub fn expenses(&self) -> i64 {
        self.road_upkeep + self.services
    }

    /// `income() - expenses()`: what the day added to the money.
    pub fn net(&self) -> i64 {
        self.income() - self.expenses()
    }
}

/// End-of-day snapshot, for charts and the budget history.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct DaySummary {
    /// The day that just ended.
    pub day: u32,
    /// Residents at the end of the day (`Stats::population`).
    pub population: u32,
    /// Households (= workforce, one worker each) at the end of the day.
    pub households: u32,
    /// Job places at the end of the day (`Stats::jobs`).
    pub jobs: u32,
    /// Workers with a job at the end of the day (`Stats::employed`).
    pub employed: u32,
    /// Buildings at the end of the day.
    pub buildings: u32,
    /// Road cells at the end of the day (highway stub included).
    pub road_cells: u32,
    /// Money after booking the day.
    pub money: i64,
    /// Total income of the day (`budget.income()`).
    pub income: i64,
    /// Total expenses of the day (`budget.expenses()`).
    pub expenses: i64,
    /// Most cars on the road at once during the day.
    pub peak_vehicles: u32,
    /// Moving average of car commutes, vehicle seconds.
    pub avg_commute_s: f32,
    /// Demand `[R, C, I]` at the end of the day, each `-1.0..=1.0`.
    pub demand: [f32; 3],
    /// Highest share of road segments above 80 % of capacity during the day.
    pub peak_congestion: f32,
    /// The day's budget by category (`income` and `expenses` above are its
    /// totals). All zero for days booked before this field existed (games
    /// saved by older versions). (Saved in the save file's extension block.)
    #[serde(skip)]
    pub budget: Budget,
}

/// How the city's finances stand, for the HUD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FiscalState {
    /// Money in hand and the last day paid for itself.
    #[default]
    Healthy,
    /// Money in hand, but the last day ran at a loss.
    Deficit,
    /// Money below zero: city services are being cut (demand falls).
    InDebt,
    /// In debt for several days: services are badly cut and residents
    /// leave every night until the books are back in the black.
    Crisis,
}

/// Short reason behind the [`FiscalState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum FiscalReason {
    #[default]
    None,
    /// Expenses (road upkeep and services) exceed tax income.
    RunningAtALoss,
    /// Taxes are so low they do not pay the running costs.
    TaxTooLow,
    /// Money is below zero.
    InDebt,
    /// Money is below zero and services have been cut.
    ServicesCut,
    /// Long debt: residents are moving away.
    ResidentsLeaving,
    /// Back in the black, services still recovering.
    Recovering,
}

impl FiscalReason {
    /// A short sentence for the HUD.
    pub fn text(self) -> &'static str {
        match self {
            FiscalReason::None => "Budget is healthy",
            FiscalReason::RunningAtALoss => "Running at a loss",
            FiscalReason::TaxTooLow => "Taxes too low to pay the running costs",
            FiscalReason::InDebt => "In debt: services will be cut",
            FiscalReason::ServicesCut => "In debt: city services are cut",
            FiscalReason::ResidentsLeaving => "Debt crisis: residents are leaving",
            FiscalReason::Recovering => "Services recovering",
        }
    }
}

/// Fiscal health, updated at every midnight.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FiscalStatus {
    pub state: FiscalState,
    pub reason: FiscalReason,
    /// Consecutive midnights with money below zero.
    pub days_in_debt: u16,
    /// Consecutive days that ran at a loss.
    pub days_in_deficit: u16,
    /// City services funded, `balance::SERVICE_LEVEL_MIN..=1.0`. Cut by
    /// debt, restored when money is positive again; low service lowers all
    /// demand and (in a crisis) makes residents leave. Services cost
    /// scales with it.
    pub service_level: f32,
    /// Households that left last night because of the crisis.
    pub left_last_night: u32,
}

impl Default for FiscalStatus {
    fn default() -> Self {
        FiscalStatus {
            state: FiscalState::Healthy,
            reason: FiscalReason::None,
            days_in_debt: 0,
            days_in_deficit: 0,
            service_level: 1.0,
            left_last_night: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Economy {
    /// Money in hand, dollars. Can go negative through daily costs; while
    /// it is below a command's cost the command fails.
    pub money: i64,
    /// Fraction, `0.0..=balance::MAX_TAX_RATE`.
    pub tax_rate: f32,
    /// Income booked at the end of the previous day.
    pub last_day_income: i64,
    /// Expenses booked at the end of the previous day.
    pub last_day_expenses: i64,
    /// The previous day's budget by category (its `income()` and
    /// `expenses()` are the two fields above). Per-day history: the
    /// `budget` of each entry in `history`.
    #[serde(default)]
    pub last_day: Budget,
    /// One entry per finished day, oldest first.
    #[serde(default)]
    pub history: Vec<DaySummary>,
    /// Debt and service health (see [`FiscalStatus`]).
    pub fiscal: FiscalStatus,
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
            fiscal: FiscalStatus::default(),
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
    let level = world.economy.fiscal.service_level as f64;
    let services = (res as f64 * balance::SERVICES_COST_PER_RESIDENT_PER_DAY as f64 * level).round() as i64;
    Budget {
        residential_tax: crate::people::residential_tax(world),
        commercial_tax: tax(com, balance::COMMERCIAL_TAX_BASE_PER_JOB_PER_DAY),
        industrial_tax: tax(ind, balance::INDUSTRIAL_TAX_BASE_PER_JOB_PER_DAY),
        road_upkeep: world.stats.road_cells as i64 * balance::ROAD_UPKEEP_PER_CELL_PER_DAY
            + world.stats.avenue_cells as i64
                * (balance::AVENUE_UPKEEP_PER_CELL_PER_DAY - balance::ROAD_UPKEEP_PER_CELL_PER_DAY),
        services,
    }
}

/// Called once when a new day starts.
pub(crate) fn end_of_day(world: &mut World) {
    crate::stats::update(world);
    // Wages, pensions, rent; the residential tax comes out of the wages.
    let tax = crate::life::payday(world);
    let mut b = budget_now(world);
    b.residential_tax = tax;
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
        budget: b,
    };
    e.history.push(summary);
    update_fiscal(world, b);
}

/// Midnight: debt cuts services, a long debt makes residents leave; money
/// back above zero restores services.
fn update_fiscal(world: &mut World, b: Budget) {
    let money = world.economy.money;
    let f = &mut world.economy.fiscal;
    f.days_in_deficit = if b.net() < 0 {
        f.days_in_deficit.saturating_add(1)
    } else {
        0
    };
    if money < 0 {
        f.days_in_debt = f.days_in_debt.saturating_add(1);
        f.service_level = (f.service_level - balance::SERVICE_CUT_PER_DAY).max(balance::SERVICE_LEVEL_MIN);
    } else {
        f.days_in_debt = 0;
        f.service_level = (f.service_level + balance::SERVICE_RECOVER_PER_DAY).min(1.0);
    }
    let crisis = money < 0 && f.days_in_debt >= balance::DEBT_CRISIS_DAYS;
    f.state = if crisis {
        FiscalState::Crisis
    } else if money < 0 {
        FiscalState::InDebt
    } else if f.days_in_deficit > 0 {
        FiscalState::Deficit
    } else {
        FiscalState::Healthy
    };
    let low_tax = world.economy.tax_rate < balance::DEFAULT_TAX_RATE * 0.5;
    f.reason = match f.state {
        FiscalState::Crisis => FiscalReason::ResidentsLeaving,
        FiscalState::InDebt if f.service_level < 1.0 => FiscalReason::ServicesCut,
        FiscalState::InDebt => FiscalReason::InDebt,
        FiscalState::Deficit if low_tax => FiscalReason::TaxTooLow,
        FiscalState::Deficit => FiscalReason::RunningAtALoss,
        FiscalState::Healthy if f.service_level < 1.0 => FiscalReason::Recovering,
        FiscalState::Healthy => FiscalReason::None,
    };
    f.left_last_night = 0;
    if crisis {
        let share = (1.0 - f.service_level) * balance::CRISIS_LEAVE_SHARE_PER_DAY;
        let ids: Vec<crate::HouseholdId> = world.households.keys().collect();
        let mut leaving = Vec::new();
        for id in ids {
            if world.rng.next_f32() < share {
                leaving.push(id);
            }
        }
        world.economy.fiscal.left_last_night = leaving.len() as u32;
        for id in leaving {
            crate::households::leave(world, id, balance::MOVE_OUT_DRIVES);
        }
    }
}
