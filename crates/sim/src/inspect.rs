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
    /// Expected arrival (off screen: exact; by car: departure plus the
    /// expected door-to-door time; the car may be late, nobody gets out
    /// before it arrives).
    pub expected_tick: u64,
    /// Why they are going, in more words than `purpose` (a `Leisure` trip
    /// at midday is a lunch, not a night out).
    pub reason: TripReason,
}

/// Why a trip is made: its purpose with where it comes from and the time
/// it set off (`text()` for the interface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TripReason {
    ToWork,
    HomeFromWork,
    MovingIn,
    MovingOut,
    /// Shopping for the household (from home).
    Groceries,
    /// Shopping on the way home from work.
    ShoppingAfterWork,
    HomeFromShopping,
    /// A worker's lunch break, out of work to a venue nearby ...
    LunchBreak,
    /// ... and back to work.
    BackToWork,
    /// Someone with the day free out for lunch (a midday visit) ...
    OutForLunch,
    HomeFromLunch,
    /// ... and an evening out.
    NightOut,
    HomeFromNightOut,
    /// A truck bringing goods to a shop or taking them out of the city.
    Delivery,
}

impl TripReason {
    /// Short sentence for the interface ("Out for lunch").
    pub fn text(self) -> &'static str {
        match self {
            TripReason::ToWork => "On the way to work",
            TripReason::HomeFromWork => "Going home from work",
            TripReason::MovingIn => "Moving into town",
            TripReason::MovingOut => "Leaving town",
            TripReason::Groceries => "Shopping for the household",
            TripReason::ShoppingAfterWork => "Shopping on the way home from work",
            TripReason::HomeFromShopping => "Home from the shops",
            TripReason::LunchBreak => "Lunch break",
            TripReason::BackToWork => "Back to work after lunch",
            TripReason::OutForLunch => "Out for lunch",
            TripReason::HomeFromLunch => "Home from lunch",
            TripReason::NightOut => "A night out",
            TripReason::HomeFromNightOut => "Home from a night out",
            TripReason::Delivery => "Delivering goods",
        }
    }
}

/// The reason of a trip of `purpose` from `from` to `to` that set off at
/// `departed_tick`, made by someone working at `workplace` and living at
/// `home`.
fn reason_of(
    world: &World,
    purpose: TripPurpose,
    (from, to): (Option<BuildingId>, BuildingId),
    departed_tick: u64,
    (home, workplace): (BuildingId, Option<BuildingId>),
) -> TripReason {
    let t = &world.time;
    let since_midnight = t.tick as i64 - t.tick_of_day as i64;
    let tod = (departed_tick as i64 - since_midnight).rem_euclid(balance::TICKS_PER_DAY as i64);
    let hour = tod as f32 / balance::TICKS_PER_HOUR as f32;
    let at_work = |b: Option<BuildingId>| b.is_some() && b == workplace;
    match purpose {
        TripPurpose::ToWork => TripReason::ToWork,
        TripPurpose::ToHome => TripReason::HomeFromWork,
        TripPurpose::MoveIn => TripReason::MovingIn,
        TripPurpose::MoveOut => TripReason::MovingOut,
        TripPurpose::Goods => TripReason::Delivery,
        TripPurpose::Shopping if to == home => TripReason::HomeFromShopping,
        TripPurpose::Shopping if at_work(from) => TripReason::ShoppingAfterWork,
        TripPurpose::Shopping => TripReason::Groceries,
        TripPurpose::Leisure if at_work(Some(to)) => TripReason::BackToWork,
        TripPurpose::Leisure if at_work(from) => TripReason::LunchBreak,
        TripPurpose::Leisure if to == home && crate::day::midday_visit(hour) => TripReason::HomeFromLunch,
        TripPurpose::Leisure if to == home => TripReason::HomeFromNightOut,
        TripPurpose::Leisure if crate::day::midday_visit(hour) => TripReason::OutForLunch,
        TripPurpose::Leisure => TripReason::NightOut,
    }
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
    /// The household's money (shared by its members), dollars.
    pub fn money(&self) -> i64 {
        self.household.money
    }
    /// Happiness, `0..=1`.
    pub fn happiness(&self) -> f32 {
        self.citizen.happiness.total
    }
}

// ------------------------------------------------------------- plans ----

/// What comes after the current activity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NextStep {
    /// What they will be doing (`Travelling` for setting off on a trip).
    pub activity: Activity,
    /// Why they set off, when `activity` is `Travelling`.
    pub purpose: Option<TripPurpose>,
    /// Where it happens (or where the trip goes).
    pub place: Option<BuildingId>,
    /// When it starts (tick), if known. For a car trip under way: the
    /// expected arrival (it may be late: the next activity starts when the
    /// car arrives).
    pub at_tick: Option<u64>,
}

/// A citizen's day as the interface shows it: what now, until when, what
/// next. Cheap (a few lookups).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayPlan {
    pub activity: Activity,
    /// Building they are in, or the trip under way.
    pub inside: Option<BuildingId>,
    pub trip: Option<TripView>,
    /// When the current activity is expected to end, if it has a set end
    /// (a shift, a night's sleep, a visit, an off-screen trip; a car trip:
    /// the expected arrival). `None` while at home with nothing planned.
    pub until_tick: Option<u64>,
    pub next: Option<NextStep>,
    /// Their next shift still to come: (leave home, end) ticks.
    pub next_shift: Option<(u64, u64)>,
}

/// What `id` is doing now, until when, and what comes next.
pub fn plan(world: &World, id: CitizenId) -> Option<DayPlan> {
    let v = citizen(world, id)?;
    let c = v.citizen;
    let now = world.time.tick;
    let shift = crate::day::next_shift(world, id).map(|(_, leave, end)| (leave.max(0) as u64, end.max(0) as u64));
    let home = v.home;
    let step = |activity, purpose, place, at_tick| {
        Some(NextStep {
            activity,
            purpose,
            place,
            at_tick,
        })
    };
    let (until, next) = match (c.activity, v.trip) {
        (_, Some(t)) => {
            let at = Some(t.expected_tick);
            let next = match t.purpose {
                // To work, or back to work from a lunch break.
                TripPurpose::ToWork | TripPurpose::Leisure if Some(t.to) == v.workplace => {
                    step(Activity::Working, None, Some(t.to), at)
                }
                TripPurpose::Shopping if t.to != home => step(Activity::Shopping, None, Some(t.to), at),
                TripPurpose::Leisure if t.to != home => step(Activity::Leisure, None, Some(t.to), at),
                _ => step(Activity::Home, None, Some(home), at),
            };
            (at, next)
        }
        // At lunch during a shift: back to work after it.
        (Activity::Leisure, None) if crate::day::on_shift(world, id).is_some() => {
            let until = Some(c.until_tick.max(now));
            (until, step(Activity::Working, None, v.workplace, until))
        }
        // Working: until the shift ends (a lunch break on the way is not
        // planned ahead here; its trip says so when it comes).
        (Activity::Working, None) => {
            let end = crate::day::on_shift(world, id).map_or(c.until_tick, |e| e.max(0) as u64);
            let until = Some(end.max(now));
            (
                until,
                step(Activity::Travelling, Some(TripPurpose::ToHome), Some(home), until),
            )
        }
        (Activity::Shopping | Activity::Leisure, None) => {
            let until = Some(c.until_tick.max(now));
            (
                until,
                step(Activity::Travelling, Some(TripPurpose::ToHome), Some(home), until),
            )
        }
        (Activity::Sleeping, None) => {
            let wake = c.until_tick.max(now);
            match shift {
                Some((leave, _)) if leave <= wake => (
                    Some(leave),
                    step(
                        Activity::Travelling,
                        Some(TripPurpose::ToWork),
                        v.workplace,
                        Some(leave),
                    ),
                ),
                _ => (Some(wake), step(Activity::Home, None, Some(home), Some(wake))),
            }
        }
        _ => {
            // At home (or turned back): the next shift if any, else bed.
            let (_, bed) = crate::day::wake_bed(world, id);
            let midnight = now - world.time.tick_of_day as u64;
            let bed_t = midnight + (bed * balance::TICKS_PER_HOUR as f32) as u64;
            match shift {
                Some((leave, _)) if leave < bed_t || (bed_t <= now && leave <= now) => (
                    None,
                    step(
                        Activity::Travelling,
                        Some(TripPurpose::ToWork),
                        v.workplace,
                        Some(leave.max(now)),
                    ),
                ),
                _ => (None, step(Activity::Sleeping, None, Some(home), Some(bed_t.max(now)))),
            }
        }
    };
    Some(DayPlan {
        activity: c.activity,
        inside: v.inside,
        trip: v.trip,
        until_tick: until,
        next,
        next_shift: shift,
    })
}

/// Where a citizen is, sim metres: the centre of the building they are in,
/// the car they ride, or (off screen) a point on the straight line of their
/// trip by the time elapsed (approximate).
pub fn citizen_pos(world: &World, id: CitizenId) -> Option<[f32; 2]> {
    let v = citizen(world, id)?;
    let centre = |b: Option<BuildingId>| crate::day::pos_of(world, b);
    match v.trip {
        None => v.inside.map(|b| centre(Some(b))),
        Some(t) => {
            if let Some(car) = t.vehicle.and_then(|car| world.vehicles.get(car)) {
                return Some(car.pos);
            }
            let (a, b) = (centre(t.from), centre(Some(t.to)));
            let span = t.expected_tick.saturating_sub(t.departed_tick).max(1) as f32;
            let f = (world.time.tick.saturating_sub(t.departed_tick) as f32 / span).clamp(0.0, 1.0);
            Some([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f])
        }
    }
}

/// One happiness component in words.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HappinessReason {
    pub part: HappinessPart,
    /// The component's value, `0..=1` (1 = content).
    pub value: f32,
    /// How much it pulls the total down (weighted shortfall).
    pub loss: f32,
    /// A short sentence about this citizen ("Out of work for 2 days").
    pub text: &'static str,
    /// Good news rather than a complaint.
    pub good: bool,
}

/// Every happiness component of a citizen in words, the biggest complaint
/// first (no allocation).
pub fn happiness_reasons(world: &World, id: CitizenId) -> Option<[HappinessReason; 7]> {
    let c = world.citizens.get(id)?;
    let h = world.households.get(c.household)?;
    let hp = &c.happiness;
    let w = balance::HAPPINESS_WEIGHTS;
    let ok = 0.75;
    let mut out = HappinessPart::ALL.map(|part| {
        let value = hp.part(part);
        let good = value >= ok;
        let text = match part {
            HappinessPart::Employment => match (c.job.is_some(), c.retired, c.in_workforce(), good) {
                (true, _, _, _) => "Has a job",
                (_, true, _, _) => "Retired on a pension",
                (_, _, true, _) => match crate::demand::jobs_view(world).reason {
                    crate::demand::JoblessReason::NoLandZoned => "Out of work: not enough jobs in the city",
                    crate::demand::JoblessReason::NoJobsWithinReach => "Out of work: no jobs within reach",
                    crate::demand::JoblessReason::BuildingsComing => "Out of work until new workplaces open",
                    _ if c.unemployed_days >= 2 => "Out of work for days",
                    _ => "Looking for a job",
                },
                (_, _, false, true) => "The household has an income",
                _ => "Nobody in the household earns",
            },
            HappinessPart::Commute if c.job.is_none() => "No commute",
            HappinessPart::Commute if c.hopeless_commutes > 0 => "Hopeless commute",
            HappinessPart::Commute if good => "Short commute",
            // Slow for the distance: the roads, not where they live.
            HappinessPart::Commute if crate::life::commute_ratio(world, id) >= balance::COMMUTE_RATIO_OK + 0.4 => {
                "Stuck in traffic every day"
            }
            HappinessPart::Commute => "Long commute",
            HappinessPart::Affordability if h.money < 0 => "The household is in debt",
            HappinessPart::Affordability if h.austerity => "Money is tight: essentials only",
            HappinessPart::Affordability if good => "Can afford rent and living",
            HappinessPart::Affordability => "Rent and costs too high",
            HappinessPart::Access if h.goods <= 0.0 => "Ran out of goods at home",
            HappinessPart::Access if good => "Shops and leisure nearby",
            HappinessPart::Access if c.leisure_need >= balance::LEISURE_URGENT_NEED => "Needs a night out",
            HappinessPart::Access => "Few shops or leisure nearby",
            HappinessPart::Taxes if good => "Taxes are fair",
            HappinessPart::Taxes => "Taxes too high",
            HappinessPart::Finances if good => "City services are good",
            HappinessPart::Finances if world.economy.money < 0 => "The city is in debt",
            HappinessPart::Finances => "City services cut",
            HappinessPart::Housing if good => "A home of their own",
            HappinessPart::Housing => "Still living with their parents",
        };
        HappinessReason {
            part,
            value,
            loss: (1.0 - value) * w[part.index()],
            text,
            good,
        }
    });
    out.sort_by(|a, b| b.loss.total_cmp(&a.loss));
    Some(out)
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
                expected_tick: j.arrive_tick.unwrap_or(j.due_tick),
                reason: reason_of(
                    world,
                    j.purpose,
                    (j.from, j.to),
                    j.departed_tick,
                    (h.home, c.workplace()),
                ),
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

/// Everyone inside a building right now: residents at home, staff on
/// shift, shoppers and visitors (no allocation).
pub fn inside(world: &World, id: BuildingId) -> impl Iterator<Item = CitizenId> + '_ {
    let at_home = residents(world, id).filter(move |&c| world.citizens[c].location == Location::In(id));
    let guests = world
        .people
        .places
        .get(id)
        .into_iter()
        .flat_map(|p| p.guests.iter().copied());
    at_home.chain(guests)
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
    /// People in the car, driver included (citizens, plus `party`).
    pub occupants: u8,
    pub origin: Place,
    pub destination: Place,
    /// Buildings at the two ends (none for the highway; a building that
    /// has been removed since the car set off is still named).
    pub origin_building: Option<BuildingId>,
    pub destination_building: Option<BuildingId>,
    /// People aboard a car of a household that is leaving the city (they
    /// are no longer citizens); 0 otherwise.
    pub party: u8,
    /// Goods aboard a truck, units.
    pub cargo: f32,
    /// Tick the trip was asked for (the people left the building) and the
    /// tick the car pulled out onto the road.
    pub requested_tick: u64,
    pub started_tick: u64,
    /// Seconds of vehicle time since the car pulled out.
    pub on_road_s: f32,
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
    let now = world.time.tick;
    Some(VehicleView {
        purpose: t.purpose,
        household: v.household,
        driver,
        owner: t.owner.filter(|&c| world.citizens.contains_key(c)),
        occupants: (occupants(world, id).count() + t.party as usize).min(u8::MAX as usize) as u8,
        origin: t.from,
        destination: t.to,
        origin_building: t.from_building.or_else(|| at(t.from)),
        destination_building: t.to_building.or_else(|| at(t.to)),
        party: t.party,
        cargo: t.cargo,
        requested_tick: t.requested_tick,
        started_tick: t.started_tick,
        on_road_s: now.saturating_sub(t.started_tick) as f32 / crate::TICKS_PER_SECOND as f32,
    })
}

/// A car route the traffic would plan right now between two buildings:
/// the segment the car would pull out on and in which direction, the
/// segments after it (the last one is the segment the destination lies
/// on), and where it would stop. Read only: for showing a trip that has
/// not started yet (or happens off the map). The live route of a car on
/// the road is `Vehicle::route`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedRoute {
    pub start: crate::Access,
    /// Driving `start.segment` from `a` towards `b`.
    pub forward: bool,
    pub segments: Vec<crate::SegmentId>,
    pub end: crate::Access,
}

/// [`PlannedRoute`] from one building to another, by expected travel time
/// at today's loads and junction delays (the search a departing car makes).
/// `None` when either has no road nearby or no route joins them. Allocates
/// its own search buffers: call it when the plan changes, not every frame.
pub fn planned_route(world: &World, from: BuildingId, to: BuildingId) -> Option<PlannedRoute> {
    let access = |b: BuildingId| {
        let p = crate::derived::rect_center(world.buildings.get(b)?.footprint);
        world.cache.access_near(&world.roads, p)
    };
    let (start, end) = (access(from)?, access(to)?);
    let seg = world.roads.segments.get(start.segment)?;
    if start.segment == end.segment {
        return Some(PlannedRoute {
            start,
            forward: end.from_a >= start.from_a,
            segments: Vec::new(),
            end,
        });
    }
    let sources = [
        (seg.a, crate::path::drive_time(seg, false, start.from_a)),
        (seg.b, crate::path::drive_time(seg, true, seg.length - start.from_a)),
    ];
    let mut scratch = crate::path::PathScratch::default();
    let mut budget = i64::MAX;
    let found = scratch.search(
        &world.roads,
        &world.cache.route_delay,
        &world.cache.route_turns,
        &sources,
        crate::path::Target::Access(end),
        &mut budget,
    )?;
    Some(PlannedRoute {
        start,
        forward: found.first == seg.b,
        segments: found.segments,
        end,
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

#[cfg(test)]
mod route_tests {
    use super::*;
    use crate::{Cell, Command, RoadKind, scenarios};

    #[test]
    fn a_planned_route_joins_the_two_buildings_segment_by_segment() {
        let mut w = World::new(5);
        w.apply(Command::BuildRoad {
            from: Cell::new(11, 128),
            to: Cell::new(60, 128),
            kind: RoadKind::Street,
        })
        .unwrap();
        // A side street, so the route has to turn at a junction.
        w.apply(Command::BuildRoad {
            from: Cell::new(35, 128),
            to: Cell::new(35, 150),
            kind: RoadKind::Street,
        })
        .unwrap();
        scenarios::zone(&mut w, (12, 126), (60, 127), Zone::Residential);
        scenarios::zone(&mut w, (36, 135), (37, 150), Zone::Commercial);
        for _ in 0..balance::TICKS_PER_DAY / 3 {
            w.step();
        }
        let pick = |z: Zone, far: bool| {
            let mut v: Vec<(BuildingId, f32)> = w
                .buildings
                .iter()
                .filter(|(_, b)| b.kind == z)
                .map(|(id, b)| (id, b.center()[0] + b.center()[1]))
                .collect();
            v.sort_by(|a, b| a.1.total_cmp(&b.1));
            if far {
                v.last().map(|x| x.0)
            } else {
                v.first().map(|x| x.0)
            }
        };
        let (home, shop) = (
            pick(Zone::Residential, false).unwrap(),
            pick(Zone::Commercial, true).unwrap(),
        );
        let r = planned_route(&w, home, shop).expect("a route");
        assert!(!r.segments.is_empty());
        assert_eq!(*r.segments.last().unwrap(), r.end.segment);
        // Each segment starts where the one before ends.
        let seg = &w.roads.segments[r.start.segment];
        let mut at = if r.forward { seg.b } else { seg.a };
        for &s in &r.segments {
            let seg = &w.roads.segments[s];
            assert!(seg.a == at || seg.b == at, "segments joined end to end");
            at = seg.other(at);
        }
        // Same building: no segments, still a route.
        let same = planned_route(&w, home, home).unwrap();
        assert!(same.segments.is_empty());
    }
}
