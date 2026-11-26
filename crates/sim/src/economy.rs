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
    /// `ROAD_UPKEEP_PER_CELL_PER_DAY`, avenue cells at
    /// `AVENUE_UPKEEP_PER_CELL_PER_DAY`, and `SIGNAL_UPKEEP_PER_DAY` per
    /// junction with traffic lights (by category: [`RunningCosts`]).
    pub road_upkeep: i64,
    /// City services (police, fire, schools, rubbish... abstracted): a base
    /// cost per resident, more per resident in a bigger city and in denser
    /// homes (by category: [`RunningCosts`]), all scaled by the service
    /// level. Grows faster than the city, so the margin per resident gets
    /// thinner as it grows. (Saved in the save file's extension block; 0 in
    /// days booked by older saves.)
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
    /// The previous day's running costs by category ([`RunningCosts`];
    /// today's so far: [`running_costs`]).
    pub last_day_costs: RunningCosts,
    /// Shift hours scheduled and actually worked (commutes eat into them):
    /// shops and works pay their tax on the hours worked.
    pub attendance: Attendance,
    /// Commuting time lost in traffic today and what it costs in business
    /// tax (`traffic_loss_now` for today so far). Saved in the extension
    /// block.
    #[serde(skip)]
    pub traffic_today: TrafficLoss,
    /// The same for yesterday, as booked: `commercial_tax` and
    /// `industrial_tax` of `last_day` are already net of its
    /// `business_tax`.
    #[serde(skip)]
    pub last_day_traffic: TrafficLoss,
}

/// What congestion cost the city in a day: commuters' time beyond
/// free-flow (`balance::TRAFFIC_FREE_FACTOR` times the free-flow estimate),
/// the business output lost with it, and the business tax that output
/// would have paid (taken off the commercial and industrial tax lines).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct TrafficLoss {
    /// Commutes (to work and home) that ended.
    pub commutes: u32,
    /// Vehicle seconds they spent beyond free flow.
    pub lost_s: f64,
    /// Business tax lost (whole dollars; set when the day is booked, or by
    /// `traffic_loss_now`).
    pub business_tax: i64,
}

impl TrafficLoss {
    /// Average time lost per commute, vehicle seconds.
    pub fn per_commute_s(&self) -> f32 {
        (self.lost_s / self.commutes.max(1) as f64) as f32
    }
}

/// Count one finished commute of `took_s` against its free-flow estimate.
pub(crate) fn record_commute_time(world: &mut World, took_s: f32, free_s: f32) {
    let t = &mut world.economy.traffic_today;
    t.commutes += 1;
    t.lost_s += (took_s - free_s * balance::TRAFFIC_FREE_FACTOR).max(0.0) as f64;
}

/// Business tax lost to traffic today so far, before the cap
/// (`TRAFFIC_LOSS_MAX_SHARE` of the business tax).
fn traffic_tax(world: &World, lost_s: f64) -> f64 {
    lost_s * balance::TRAFFIC_OUTPUT_PER_S as f64 * world.economy.tax_rate as f64
}

/// Today's traffic losses so far, with the business tax they cost.
pub fn traffic_loss_now(world: &World) -> TrafficLoss {
    let mut t = world.economy.traffic_today;
    let b = budget_before_traffic(world);
    let business = (b.commercial_tax + b.industrial_tax) as f64;
    t.business_tax = traffic_tax(world, t.lost_s)
        .min(business * balance::TRAFFIC_LOSS_MAX_SHARE as f64)
        .round() as i64;
    t
}

/// Work time booked today and yesterday's share actually worked.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Attendance {
    /// Ticks of shifts workers set off to today.
    pub scheduled_ticks: u64,
    /// Ticks of those shifts left when the workers got there.
    pub worked_ticks: u64,
    /// Yesterday's `worked / scheduled` (1 when nobody worked).
    pub last_day_share: f32,
}

impl Default for Attendance {
    fn default() -> Self {
        Attendance {
            scheduled_ticks: 0,
            worked_ticks: 0,
            last_day_share: 1.0,
        }
    }
}

impl Attendance {
    /// Today's share of scheduled shift time worked so far (yesterday's
    /// until enough shifts have begun).
    pub fn share(&self) -> f32 {
        if self.scheduled_ticks < 50 * balance::TICKS_PER_HOUR as u64 {
            self.last_day_share
        } else {
            (self.worked_ticks as f64 / self.scheduled_ticks as f64).clamp(0.0, 1.0) as f32
        }
    }
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
            last_day_costs: RunningCosts::default(),
            attendance: Attendance::default(),
            traffic_today: TrafficLoss::default(),
            last_day_traffic: TrafficLoss::default(),
        }
    }
}

/// A day's running costs by what they pay for (whole dollars, positive),
/// for the budget panel. `streets + avenues + highway + signals` is
/// [`Budget::road_upkeep`], `services_base + services_city_size +
/// services_density` is [`Budget::services`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RunningCosts {
    /// Street cells at `ROAD_UPKEEP_PER_CELL_PER_DAY`.
    pub streets: i64,
    /// Avenue cells at `AVENUE_UPKEEP_PER_CELL_PER_DAY`.
    pub avenues: i64,
    /// The highway stub's cells at `ROAD_UPKEEP_PER_CELL_PER_DAY`.
    pub highway: i64,
    /// Junctions with traffic lights at `SIGNAL_UPKEEP_PER_DAY`.
    pub signals: i64,
    /// How many junctions have traffic lights.
    pub signal_count: u32,
    /// Residents at `SERVICES_COST_PER_RESIDENT_PER_DAY`.
    pub services_base: i64,
    /// The extra cost of a big city's services
    /// (`SERVICES_CITY_SIZE_PER_RESIDENT`, more per resident the bigger
    /// the city).
    pub services_city_size: i64,
    /// The extra cost of residents in dense homes
    /// (`SERVICES_DENSITY_PER_RESIDENT` by home level).
    pub services_density: i64,
}

impl RunningCosts {
    pub fn road_upkeep(&self) -> i64 {
        self.streets + self.avenues + self.highway + self.signals
    }

    pub fn services(&self) -> i64 {
        self.services_base + self.services_city_size + self.services_density
    }
}

/// Running costs if the day ended now (`O(buildings + junctions)`).
pub fn running_costs(world: &World) -> RunningCosts {
    let mut res = 0u64;
    let mut dense = 0.0f64;
    for b in world.buildings.values() {
        if b.kind == Zone::Residential {
            res += b.occupied as u64;
            let extra = balance::SERVICES_DENSITY_PER_RESIDENT[(b.level.clamp(1, 3) - 1) as usize];
            dense += b.occupied as f64 * extra as f64;
        }
    }
    let level = world.economy.fiscal.service_level as f64;
    let p2 = (res as f64).powi(2);
    let size =
        balance::SERVICES_CITY_SIZE_PER_RESIDENT as f64 * p2 / (p2 + (balance::SERVICES_CITY_SIZE_POP as f64).powi(2));
    let signal_count = world
        .traffic
        .junction_states
        .values()
        .filter(|j| j.control == crate::JunctionControl::Signal)
        .count() as u32;
    let s = &world.stats;
    let highway_cells = (balance::HIGHWAY_STUB_CELLS as u32).min(s.road_cells);
    let street_cells = s.road_cells.saturating_sub(s.avenue_cells + highway_cells);
    RunningCosts {
        streets: street_cells as i64 * balance::ROAD_UPKEEP_PER_CELL_PER_DAY,
        avenues: s.avenue_cells as i64 * balance::AVENUE_UPKEEP_PER_CELL_PER_DAY,
        highway: highway_cells as i64 * balance::ROAD_UPKEEP_PER_CELL_PER_DAY,
        signals: signal_count as i64 * balance::SIGNAL_UPKEEP_PER_DAY,
        signal_count,
        services_base: (res as f64 * balance::SERVICES_COST_PER_RESIDENT_PER_DAY as f64 * level).round() as i64,
        services_city_size: (res as f64 * size * level).round() as i64,
        services_density: (dense * level).round() as i64,
    }
}

/// What today's budget would be if the day ended now (business tax net of
/// what traffic cost it, `traffic_loss_now`).
pub fn budget_now(world: &World) -> Budget {
    let mut b = budget_before_traffic(world);
    let lost = traffic_loss_now(world).business_tax;
    let business = b.commercial_tax + b.industrial_tax;
    if business > 0 && lost > 0 {
        let c = (lost as f64 * b.commercial_tax as f64 / business as f64).round() as i64;
        b.commercial_tax -= c;
        b.industrial_tax -= lost - c;
    }
    b
}

fn budget_before_traffic(world: &World) -> Budget {
    let rate = world.economy.tax_rate;
    let (mut com, mut ind) = (0u64, 0u64);
    for b in world.buildings.values() {
        match b.kind {
            Zone::Residential => {}
            Zone::Commercial => com += b.occupied as u64,
            Zone::Industrial => ind += b.occupied as u64,
        }
    }
    // Business pays on the hours its workers were actually there.
    let worked = world.economy.attendance.share() as f64;
    let tax = |n: u64, base: f32| (n as f64 * base as f64 * rate as f64 * worked).round() as i64;
    let costs = running_costs(world);
    Budget {
        residential_tax: crate::people::residential_tax(world),
        commercial_tax: tax(com, balance::COMMERCIAL_TAX_BASE_PER_JOB_PER_DAY),
        industrial_tax: tax(ind, balance::INDUSTRIAL_TAX_BASE_PER_JOB_PER_DAY),
        road_upkeep: costs.road_upkeep(),
        services: costs.services(),
    }
}

/// Called once when a new day starts.
pub(crate) fn end_of_day(world: &mut World) {
    crate::stats::update(world);
    // Wages, pensions, rent; the residential tax comes out of the wages.
    let tax = crate::life::payday(world);
    let mut b = budget_now(world);
    b.residential_tax = tax;
    let costs = running_costs(world);
    let traffic = traffic_loss_now(world);
    let e = &mut world.economy;
    e.last_day_costs = costs;
    e.last_day_traffic = traffic;
    e.traffic_today = TrafficLoss::default();
    e.attendance = Attendance {
        scheduled_ticks: 0,
        worked_ticks: 0,
        last_day_share: e.attendance.share(),
    };
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
