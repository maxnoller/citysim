//! Life in the city over the days: money, ageing and life events,
//! happiness and leaving, and the job search.
//!
//! * **Payday** (every midnight, booked with the city's budget): every
//!   employed citizen earns their daily wage, the city takes its residential
//!   tax from it (`tax rate * wage`), seniors get a pension, and every
//!   household pays its rent.
//! * **Midnight**: goods are used up, everyone ages a day (a year is
//!   `balance::DAYS_PER_YEAR` days), life stages change (13, 18, 65), seniors
//!   may die of old age; couples may have a child if there is room at home;
//!   grown-up children move out when there is a free home; two singles may
//!   move in together. Happiness is recomputed. A household short of money
//!   cuts spending, then moves to a cheaper home, then leaves; households
//!   that stay unhappy, or whose commutes stay hopeless, leave too.
//! * **Job search** (every `balance::JOB_MATCH_INTERVAL_TICKS`): seekers
//!   compare a few random vacancies reachable by road and take the best by
//!   wage and distance; each worker gets a shift of the workplace.

use crate::citizens::{Activity, CitizenId, HappinessPart, HouseholdRole, LifeEvent, LifeStage, Location, Shift};
use crate::derived::rect_center;
use crate::households::{self, HouseholdKind};
use crate::people::{self, habit, hours};
use crate::{BuildingId, HouseholdId, World, Zone, balance, geom};

// -------------------------------------------------------------- money ----

/// Daily rent of a home for `n` residents: per place by the building's
/// level, times the housing market (residential demand).
pub fn rent_for(world: &World, home: BuildingId, n: u32) -> i64 {
    let level = world.buildings.get(home).map_or(1, |b| b.level.clamp(1, 3));
    let base = balance::RENT_PER_PLACE[(level - 1) as usize] * n as f32;
    let market = 1.0 + balance::RENT_MARKET_WEIGHT * world.demand.residential.clamp(-1.0, 1.0);
    (base * market).round().max(0.0) as i64
}

/// Expected daily income of a household (wages after tax, pensions).
pub fn daily_income(world: &World, hid: HouseholdId) -> i64 {
    let Some(h) = world.households.get(hid) else { return 0 };
    let (mut wages, mut pensions) = (0u32, 0i64);
    for c in h.citizens.iter().filter_map(|&m| world.citizens.get(m)) {
        match (c.job, c.retired) {
            (Some(j), _) => wages += j.wage,
            (None, true) => pensions += balance::PENSION_PER_DAY,
            _ => {}
        }
    }
    wages as i64 - people::tax_on(wages, world.economy.tax_rate) + pensions
}

/// Goods a household uses per day.
fn goods_per_day(world: &World, hid: HouseholdId) -> f32 {
    world.households[hid]
        .citizens
        .iter()
        .filter_map(|&m| world.citizens.get(m))
        .map(|c| {
            if c.stage == LifeStage::Child {
                balance::GOODS_PER_CHILD_PER_DAY
            } else {
                balance::GOODS_PER_PERSON_PER_DAY
            }
        })
        .sum()
}

/// Pay wages and pensions, take tax and rent. Returns the residential tax
/// collected (the budget's `residential_tax`). Called by
/// `economy::end_of_day`.
pub(crate) fn payday(world: &mut World) -> i64 {
    let rate = world.economy.tax_rate;
    let mut total_tax = 0;
    let ids: Vec<HouseholdId> = world.households.keys().collect();
    for hid in ids {
        let (mut wages, mut pensions, mut tax) = (0i64, 0i64, 0i64);
        for &m in &world.households[hid].citizens {
            let Some(c) = world.citizens.get(m) else { continue };
            if let Some(j) = c.job {
                wages += j.wage as i64;
            } else if c.retired {
                pensions += balance::PENSION_PER_DAY;
            }
        }
        tax += people::tax_on(wages as u32, rate);
        let h = &mut world.households[hid];
        if h.citizens.is_empty() {
            continue;
        }
        let rent = h.rent;
        h.money += wages + pensions - tax - rent;
        for l in [&mut h.ledger, &mut h.today] {
            l.wages += wages;
            l.pensions += pensions;
            l.tax += tax;
            l.rent += rent;
        }
        h.yesterday = h.today;
        h.today = Default::default();
        total_tax += tax;
    }
    total_tax
}

// ------------------------------------------------------------- shifts ----

/// A shift for a new worker of building `b`: a start time per workplace
/// (industry early, offices later, shops in an early and a late shift,
/// some night shifts in industry), a personal offset, two days off a week.
pub(crate) fn shift_for(world: &mut World, b: BuildingId, part_time: bool) -> Shift {
    let bld = &world.buildings[b];
    let bs = bld.seed;
    let base = |lo: f32, hi: f32| lo + habit(bs, 0x5A1F7) * (hi - lo);
    let r = world.rng.next_f32();
    let (start_h, len_h) = if part_time {
        (base(15.0, 17.0), balance::PART_TIME_SHIFT_H)
    } else {
        match (bld.kind, bld.level) {
            (Zone::Industrial, _) if r < balance::NIGHT_SHIFT_SHARE => (base(21.0, 22.5), balance::SHIFT_H),
            (Zone::Industrial, _) => (
                base(balance::INDUSTRY_START_H[0], balance::INDUSTRY_START_H[1]),
                balance::SHIFT_H,
            ),
            (Zone::Commercial, l) if l >= 3 => (
                base(balance::OFFICE_START_H[0], balance::OFFICE_START_H[1]),
                balance::SHIFT_H,
            ),
            _ if r < balance::LATE_SHIFT_SHARE => (
                base(balance::SHOP_START_H[0], balance::SHOP_START_H[1]) + balance::LATE_SHIFT_OFFSET_H,
                balance::SHIFT_H,
            ),
            _ => (
                base(balance::SHOP_START_H[0], balance::SHOP_START_H[1]),
                balance::SHIFT_H,
            ),
        }
    };
    // Triangular personal offset.
    let o = (world.rng.next_f32() + world.rng.next_f32() - 1.0) * balance::SHIFT_PERSONAL_SPREAD_H;
    let start = (start_h + o).rem_euclid(24.0);
    let first_off = world.rng.below(balance::DAYS_PER_WEEK);
    let days_off = if part_time {
        // Part-time: three days a week.
        let mut m = 0u8;
        for k in 0..4 {
            m |= 1 << ((first_off + k * 2) % balance::DAYS_PER_WEEK);
        }
        m
    } else {
        (1 << first_off) | (1 << ((first_off + 1) % balance::DAYS_PER_WEEK))
    };
    Shift {
        start: hours(start).min(balance::TICKS_PER_DAY - 1),
        length: hours(len_h),
        days_off,
    }
}

// ---------------------------------------------------------- job search ----

/// Match job seekers to vacancies.
pub(crate) fn match_jobs(world: &mut World) {
    if world.people.seekers.is_empty() {
        return;
    }
    world.cache.ensure_roads(&world.roads, world.versions.roads);
    // Vacancies: (building, centre, free places, wage, commercial).
    let mut vac: Vec<(BuildingId, [f32; 2], u32, f32, bool)> = world
        .buildings
        .iter()
        .filter(|(_, b)| b.kind != Zone::Residential && b.occupied < b.capacity)
        .filter(|(_, b)| households::connected(world, b.footprint))
        .map(|(id, b)| {
            let w = people::wage_of(people::job_tier(b), false) as f32;
            (
                id,
                rect_center(b.footprint),
                b.capacity - b.occupied,
                w,
                b.kind == Zone::Commercial,
            )
        })
        .collect();
    if vac.is_empty() {
        return;
    }
    let mut tries = world.people.seekers.len().min(balance::JOB_MATCH_PER_ROUND);
    let mut retry = Vec::new();
    while tries > 0 && !vac.is_empty() {
        tries -= 1;
        let Some(id) = world.people.seekers.pop_front() else {
            break;
        };
        let Some(c) = world.citizens.get(id) else { continue };
        if !c.job_seeker() {
            continue;
        }
        let teen = c.stage == LifeStage::Teen;
        let Some(h) = world.households.get(c.household) else {
            continue;
        };
        let Some(hb) = world.buildings.get(h.home) else {
            continue;
        };
        if !households::connected(world, hb.footprint) {
            // Not reachable yet: try again later.
            retry.push(id);
            continue;
        }
        let home = rect_center(hb.footprint);
        let mut best: Option<(f32, usize)> = None;
        for _ in 0..balance::JOB_MATCH_SAMPLES.min(vac.len() * 2) {
            let i = world.rng.below(vac.len() as u32) as usize;
            if teen && !vac[i].4 {
                continue;
            }
            let d = geom::dist(home, vac[i].1);
            let score = vac[i].3 * (1.0 - (d / balance::JOB_DIST_SCALE_M).min(0.9));
            if best.is_none_or(|b| score > b.0) {
                best = Some((score, i));
            }
        }
        let Some((_, i)) = best else {
            retry.push(id);
            continue;
        };
        let bid = vac[i].0;
        vac[i].2 -= 1;
        if vac[i].2 == 0 {
            vac.swap_remove(i);
        }
        let shift = shift_for(world, bid, teen);
        people::hire(world, id, bid, shift);
    }
    world.people.seekers.extend(retry);
}

// ----------------------------------------------------------- midnight ----

/// Life events, money rules, happiness and leaving, once a day: the
/// counters roll over at midnight, then every household is reviewed, in
/// `balance::NIGHT_CHUNKS` slices over the first ticks of the day (a single
/// pass over a big city would stall one step).
pub(crate) fn night(world: &mut World) {
    let tod = world.time.tick_of_day;
    if tod == 0 {
        let p = &mut world.people;
        p.events_yesterday = p.events_today;
        p.events_today = Default::default();
        p.trips_yesterday = p.trips_today;
        p.trips_today = Default::default();
        p.trips_by_hour_yesterday = p.trips_by_hour;
        p.trips_by_hour = Default::default();
        p.trip_times_yesterday = p.trip_times_today;
        p.trip_times_today = Default::default();
        p.commute_moves_yesterday = p.commute_moves_today;
        p.commute_moves_today = Default::default();
        for pl in p.places.values_mut() {
            pl.sold_yesterday = pl.sold_today;
            pl.sold_today = 0.0;
        }
        world.people.night = world.households.keys().collect();
        let s = &world.people.stats;
        let day = crate::people::PeopleDay {
            day: world.time.day.saturating_sub(1),
            population: world.citizens.len() as u32,
            by_stage: s.by_stage,
            by_kind: s.by_kind,
            avg_happiness: s.avg_happiness,
            avg_money: s.avg_money,
            events: world.people.events_yesterday,
            trips: world.people.trips_yesterday,
        };
        world.people.history.push(day);
    }
    // The review runs in the ticks after midnight (midnight itself is busy:
    // payday, budget, abandonment checks).
    let chunks = balance::NIGHT_CHUNKS;
    if tod == 0 || tod > chunks || world.people.night.is_empty() {
        return;
    }
    let tod = tod - 1;
    world.cache.ensure_access(&world.buildings, world.versions.buildings);
    let todo: Vec<HouseholdId> = world
        .people
        .night
        .iter()
        .enumerate()
        .filter(|(i, _)| *i as u32 % chunks == tod)
        .map(|(_, &h)| h)
        .collect();
    for hid in todo {
        review(world, hid);
    }
    if tod + 1 == chunks {
        world.people.night.clear();
        // Fresh job-seeker queue (drops duplicates and stale ids).
        let seekers: Vec<CitizenId> = world
            .citizens
            .iter()
            .filter(|(_, c)| c.job_seeker())
            .map(|(id, _)| id)
            .collect();
        world.people.seekers = seekers.into();
    }
}

/// One household's day: ageing, money, happiness, leaving, life events.
fn review(world: &mut World, hid: HouseholdId) {
    if !world.households.contains_key(hid) || world.households[hid].citizens.is_empty() {
        return;
    }
    age_household(world, hid);
    if !world.households.contains_key(hid) {
        return;
    }
    match household_day(world, hid) {
        Some(true) => households::leave(world, hid, balance::MOVE_OUT_DRIVES),
        Some(false) => {}
        None => events(world, hid),
    }
}

/// Ageing, stage changes and deaths in one household.
pub(crate) fn age_household(world: &mut World, hid: HouseholdId) {
    let day = world.time.day;
    let members = world.households[hid].citizens.clone();
    let mut died = Vec::new();
    for m in members {
        let Some(c) = world.citizens.get(m) else { continue };
        let old = c.stage;
        let age = c.age_days + 1;
        let new = LifeStage::of_years(age / balance::DAYS_PER_YEAR);
        people::update(world, m, |c| {
            c.age_days = age;
            c.stage = new;
        });
        if new != old {
            match new {
                LifeStage::Teen => world.citizens[m].log(day, LifeEvent::BecameTeen),
                LifeStage::Adult => {
                    world.citizens[m].log(day, LifeEvent::BecameAdult);
                    // Part-time teen job ends; looks for a real one.
                    if world.citizens[m].job.is_some_and(|j| j.part_time) {
                        people::fire(world, m, false);
                    }
                    people::seek(world, m);
                }
                LifeStage::Senior => {
                    let had_job = world.citizens[m].job.is_some();
                    people::fire(world, m, false);
                    people::update(world, m, |c| {
                        c.retired = true;
                        c.homemaker = false;
                    });
                    let _ = had_job;
                    world.people.events_today.retired += 1;
                    world.citizens[m].log(day, LifeEvent::Retired);
                }
                LifeStage::Child => {}
            }
        }
        // Old age.
        let years = age / balance::DAYS_PER_YEAR;
        if years >= balance::DEATH_FROM_YEARS {
            let p_year = if years >= balance::MAX_AGE_YEARS {
                1.0
            } else {
                balance::DEATH_RATE_AT_START
                    * libm::powf(balance::DEATH_RATE_GROWTH, (years - balance::DEATH_FROM_YEARS) as f32)
            };
            let p_day = (p_year / balance::DAYS_PER_YEAR as f32).min(1.0);
            let home = world.households[hid].home;
            if world.citizens[m].location == Location::In(home) && world.rng.next_f32() < p_day {
                died.push(m);
            }
        }
    }
    for m in died {
        households::remove_member(world, m);
        world.people.events_today.deaths += 1;
        for o in world.households[hid].citizens.clone() {
            world.citizens[o].log(day, LifeEvent::Bereaved(m));
        }
    }
    if world.households[hid].citizens.is_empty() {
        dissolve(world, hid);
    } else {
        settle_household(world, hid);
    }
}

/// Remove a household that has nobody left (its home is free again).
fn dissolve(world: &mut World, hid: HouseholdId) {
    let Some(h) = world.households.remove(hid) else { return };
    world.people.known_households = world.households.len();
    if let Some(p) = world.people.places.get_mut(h.home) {
        p.households.retain(|&x| x != hid);
    }
    for v in world.vehicles.values_mut() {
        if v.household == Some(hid) {
            v.household = None;
        }
    }
}

/// Stay-at-home parents while there are young children; recompute the
/// summary fields.
pub(crate) fn settle_household(world: &mut World, hid: HouseholdId) {
    let h = &world.households[hid];
    let seed = h.seed;
    let mut heads: Vec<CitizenId> = Vec::new();
    let mut young_kids = false;
    for &m in &h.citizens {
        let c = &world.citizens[m];
        if c.role == HouseholdRole::Head && c.stage == LifeStage::Adult {
            heads.push(m);
        }
        young_kids |= c.stage == LifeStage::Child;
    }
    let share = if young_kids {
        balance::HOMEMAKER_SHARE
    } else {
        balance::STAY_AT_HOME_SHARE
    };
    let stay = heads.len() >= 2 && habit(seed, 0x40AE) < share;
    for (k, &m) in heads.iter().enumerate() {
        let want = stay && k == 1;
        if world.citizens[m].homemaker != want {
            if want {
                people::fire(world, m, false);
            }
            people::update(world, m, |c| c.homemaker = want);
            if !want {
                people::seek(world, m);
            }
        }
    }
    people::refresh_household(world, hid);
}

/// Money, happiness and the reasons to leave. `Some(true)`: leaves the
/// city; `Some(false)`: moved home (no other events today); `None`: stays.
fn household_day(world: &mut World, hid: HouseholdId) -> Option<bool> {
    let day = world.time.day;
    // Goods used up.
    let use_ = goods_per_day(world, hid);
    let h = &mut world.households[hid];
    h.goods = (h.goods - use_).max(0.0);
    let ran_out = h.goods <= 0.0;
    // Employment streaks.
    let mut workforce = 0;
    let mut employed = 0;
    for m in world.households[hid].citizens.clone() {
        let c = &mut world.citizens[m];
        if c.in_workforce() {
            workforce += 1;
            if c.job.is_some() {
                employed += 1;
                c.unemployed_days = 0;
            } else {
                c.unemployed_days = c.unemployed_days.saturating_add(1);
            }
        }
    }
    let n = world.households[hid].citizens.len() as u32;
    let rent = rent_for(world, world.households[hid].home, n);
    let costs = households::expected_daily_costs(n, rent);
    let income = daily_income(world, hid);
    let h = &mut world.households[hid];
    h.rent = rent;
    h.unemployed_days = if workforce > 0 && employed == 0 {
        h.unemployed_days.saturating_add(1)
    } else {
        0
    };
    // Money: cut spending, then a cheaper home, then leave.
    h.austerity = h.money < costs * balance::AUSTERITY_BELOW_DAYS as i64;
    // Broke: in the red, or cannot cover tomorrow with nothing coming in.
    if h.money < 0 || (h.money < costs && income <= 0) {
        h.broke_days = h.broke_days.saturating_add(1);
    } else {
        h.broke_days = 0;
    }
    let broke_days = h.broke_days;
    happiness(world, hid, income, costs, ran_out);
    let h = &mut world.households[hid];
    if h.happiness < balance::UNHAPPY_LEAVE_BELOW {
        h.unhappy_days = h.unhappy_days.saturating_add(1);
    } else {
        h.unhappy_days = 0;
    }
    let unhappy = h.unhappy_days >= balance::UNHAPPY_LEAVE_DAYS;
    let hopeless = h.hopeless_commutes >= balance::HOPELESS_COMMUTES_TO_LEAVE;
    if broke_days >= 1 {
        if broke_days == 1
            && let Some(home) = cheaper_home(world, hid, n, rent)
            && all_home(world, hid)
        {
            households::relocate(world, hid, home);
            world.households[hid].rent = rent_for(world, home, n);
            for m in world.households[hid].citizens.clone() {
                world.citizens[m].log(day, LifeEvent::MovedHome);
            }
            world.people.events_today.moved_home += 1;
            return Some(false);
        }
        if broke_days >= balance::BROKE_LEAVE_DAYS {
            return Some(true);
        }
    }
    if unhappy || hopeless {
        return Some(true);
    }
    // Fed up with the traffic: move nearer work, or leave.
    if let Some(work) = fed_up_with_commute(world, hid)
        && world.rng.next_f32() < balance::COMMUTE_FED_UP_CHANCE
    {
        if all_home(world, hid)
            && let Some(home) = nearer_home(world, hid, n, work)
        {
            world.people.commute_moves_today.moved_nearer += 1;
            households::relocate(world, hid, home);
            world.households[hid].rent = rent_for(world, home, n);
            for m in world.households[hid].citizens.clone() {
                world.citizens[m].log(day, LifeEvent::MovedHome);
            }
            world.people.events_today.moved_home += 1;
            return Some(false);
        }
        world.people.commute_moves_today.left += 1;
        return Some(true);
    }
    None
}

fn all_home(world: &World, hid: HouseholdId) -> bool {
    let h = &world.households[hid];
    h.citizens.iter().all(|&m| {
        world
            .citizens
            .get(m)
            .is_some_and(|c| c.location == Location::In(h.home))
    })
}

/// A home with room for `n` whose rent would be clearly lower.
fn cheaper_home(world: &mut World, hid: HouseholdId, n: u32, rent: i64) -> Option<BuildingId> {
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let homes = world.cache.homes.len();
    if homes == 0 {
        return None;
    }
    let cur = world.households[hid].home;
    let mut best: Option<(i64, BuildingId)> = None;
    for _ in 0..balance::HOME_SEARCH_SAMPLES {
        let b = world.cache.homes[world.rng.below(homes as u32) as usize];
        let Some(bb) = world.buildings.get(b) else { continue };
        if b == cur || bb.capacity < bb.occupied + n || !households::connected(world, bb.footprint) {
            continue;
        }
        let r = rent_for(world, b, n);
        if (r as f32) < rent as f32 * balance::CHEAPER_HOME_FACTOR && best.is_none_or(|x| r < x.0) {
            best = Some((r, b));
        }
    }
    best.map(|b| b.1)
}

/// A home with room for `n` whose rent stays within
/// `balance::ROOMIER_HOME_RENT_FACTOR` of `rent`.
fn roomier_home(world: &mut World, hid: HouseholdId, n: u32, rent: i64) -> Option<BuildingId> {
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let homes = world.cache.homes.len();
    if homes == 0 {
        return None;
    }
    let cur = world.households[hid].home;
    let near = home_pos(world, cur);
    let mut best: Option<(f32, BuildingId)> = None;
    for _ in 0..balance::HOME_SEARCH_SAMPLES {
        let b = world.cache.homes[world.rng.below(homes as u32) as usize];
        let Some(bb) = world.buildings.get(b) else { continue };
        if b == cur || bb.capacity < bb.occupied + n || !households::connected(world, bb.footprint) {
            continue;
        }
        let r = rent_for(world, b, n - 1);
        if r as f32 > (rent as f32 * balance::ROOMIER_HOME_RENT_FACTOR).max(rent as f32 + 1.0) {
            continue;
        }
        let d = geom::dist(near, rect_center(bb.footprint));
        if best.is_none_or(|x| d < x.0) {
            best = Some((d, b));
        }
    }
    best.map(|b| b.1)
}

/// A free home with room for one more, near `near`.
fn free_home(world: &mut World, near: [f32; 2], exclude: BuildingId) -> Option<BuildingId> {
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let homes = world.cache.homes.len();
    if homes == 0 {
        return None;
    }
    let mut best: Option<(f32, BuildingId)> = None;
    for _ in 0..balance::HOME_SEARCH_SAMPLES {
        let b = world.cache.homes[world.rng.below(homes as u32) as usize];
        let Some(bb) = world.buildings.get(b) else { continue };
        if b == exclude || bb.occupied >= bb.capacity || !households::connected(world, bb.footprint) {
            continue;
        }
        let d = geom::dist(near, rect_center(bb.footprint));
        if best.is_none_or(|x| d < x.0) {
            best = Some((d, b));
        }
    }
    best.map(|b| b.1)
}

/// Move one citizen into another household (keeping their job).
fn transfer(world: &mut World, id: CitizenId, to: HouseholdId) {
    let c = &world.citizens[id];
    let from = c.household;
    if from == to {
        return;
    }
    let old_home = world.households[from].home;
    let new_home = world.households[to].home;
    world.households[from].citizens.retain(|&m| m != id);
    if world.households[from].shopper == Some(id) {
        world.households[from].shopper = None;
    }
    if let Some(b) = world.buildings.get_mut(old_home) {
        b.occupied = b.occupied.saturating_sub(1);
    }
    world.buildings[new_home].occupied += 1;
    world.households[to].citizens.push(id);
    let loc = world.citizens[id].location;
    people::update(world, id, |c| {
        c.household = to;
        c.role = HouseholdRole::Head;
        c.has_car = false;
    });
    let now = world.time.tick;
    if loc == Location::In(old_home) {
        let a = world.citizens[id].activity;
        people::set_place(world, id, a, Location::In(new_home), now);
    }
    people::refresh_household(world, from);
    people::refresh_household(world, to);
}

fn move_money(world: &mut World, from: HouseholdId, to: HouseholdId, amount: i64) {
    if amount == 0 {
        return;
    }
    let a = &mut world.households[from];
    a.money -= amount;
    a.ledger.transfers -= amount;
    let b = &mut world.households[to];
    b.money += amount;
    b.ledger.transfers += amount;
}

/// Births, moving out, couples.
pub(crate) fn events(world: &mut World, hid: HouseholdId) {
    let day = world.time.day;
    let h = &world.households[hid];
    let home = h.home;
    let everyone_home = all_home(world, hid);
    let members: Vec<CitizenId> = h.citizens.clone();
    let heads: Vec<CitizenId> = members
        .iter()
        .copied()
        .filter(|&m| world.citizens[m].role == HouseholdRole::Head)
        .collect();
    let children = members.len() - heads.len();
    let room = world.buildings.get(home).is_some_and(|b| b.occupied < b.capacity);
    // A child is born.
    let parents_age = heads.iter().any(|&m| {
        let y = world.citizens[m].age_years();
        world.citizens[m].stage == LifeStage::Adult
            && (balance::BIRTH_MIN_YEARS..=balance::BIRTH_MAX_YEARS).contains(&y)
    });
    let wants_child = heads.len() >= 2
        && parents_age
        && children < balance::MAX_CHILDREN as usize
        && world.households[hid].money >= 0
        && world.economy.fiscal.service_level >= balance::BIRTH_MIN_SERVICE_LEVEL
        && world.rng.next_f32() < balance::BIRTH_CHANCE_PER_DAY;
    if wants_child && !room && everyone_home {
        // No room for a baby: move to a home with room for one more (rent
        // not much higher), if there is one.
        let n = members.len() as u32;
        let rent = world.households[hid].rent;
        if let Some(bigger) = roomier_home(world, hid, n + 1, rent) {
            households::relocate(world, hid, bigger);
            world.households[hid].rent = rent_for(world, bigger, n);
            for &m in &members {
                world.citizens[m].log(day, LifeEvent::MovedHome);
            }
            world.people.events_today.moved_home += 1;
        }
        return;
    }
    if wants_child && room {
        let mut c = households::new_citizen(world, hid, 0, HouseholdRole::Dependant);
        c.log(day, LifeEvent::Born);
        c.activity = Activity::Sleeping;
        let kid = households::add_member(world, c);
        for &p in &heads {
            world.citizens[p].log(day, LifeEvent::ChildBorn(kid));
        }
        world.people.events_today.births += 1;
        settle_household(world, hid);
        return;
    }
    if !everyone_home {
        return;
    }
    // A grown-up child moves out (only when a home is free).
    for &m in &members {
        let c = &world.citizens[m];
        if c.role != HouseholdRole::Dependant
            || c.stage != LifeStage::Adult
            || c.age_years() < balance::MOVE_OUT_MIN_YEARS
            || world.rng.next_f32() >= balance::MOVE_OUT_CHANCE_PER_DAY
        {
            continue;
        }
        let near = c
            .workplace()
            .and_then(|w| world.buildings.get(w))
            .map_or_else(|| home_pos(world, home), |b| rect_center(b.footprint));
        let Some(new_home) = free_home(world, near, home) else {
            continue;
        };
        let seed = world.rng.next_u64() as u32;
        let mut nh = households::Household::empty(new_home, seed, 0, day);
        nh.cars = 1;
        nh.goods = balance::ARRIVAL_GOODS_DAYS;
        let nid = households::insert(world, nh);
        transfer(world, m, nid);
        let share = (world.households[hid].money as f32 * balance::MOVE_OUT_MONEY_SHARE).max(0.0) as i64;
        move_money(world, hid, nid, share);
        world.households[nid].rent = rent_for(world, new_home, 1);
        world.citizens[m].log(day, LifeEvent::MovedOut);
        world.people.events_today.moved_out += 1;
        settle_household(world, hid);
        settle_household(world, nid);
        return;
    }
    // Two singles move in together.
    if world.households[hid].kind == HouseholdKind::Single
        && heads.len() == 1
        && members.len() == 1
        && world.rng.next_f32() < balance::COUPLE_CHANCE_PER_DAY
    {
        let me = heads[0];
        let my_age = world.citizens[me].age_years();
        if !(balance::COUPLE_MIN_YEARS..=balance::COUPLE_MAX_YEARS).contains(&my_age) {
            return;
        }
        let n = world.households.slot_count();
        for _ in 0..balance::COUPLE_SAMPLES {
            let Some(other) = world.households.key_at(world.rng.below(n as u32) as usize) else {
                continue;
            };
            if other == hid {
                continue;
            }
            let o = &world.households[other];
            if o.kind != HouseholdKind::Single || o.citizens.len() != 1 || !all_home(world, other) {
                continue;
            }
            let them = o.citizens[0];
            let age = world.citizens[them].age_years();
            if age.abs_diff(my_age) > balance::COUPLE_MAX_AGE_GAP_YEARS
                || world.citizens[them].stage != LifeStage::Adult
            {
                continue;
            }
            // They move in here if there is room, else this one moves there.
            let other_home = o.home;
            let (stay, go) = if room {
                (hid, other)
            } else if world.buildings.get(other_home).is_some_and(|b| b.occupied < b.capacity) {
                (other, hid)
            } else {
                continue;
            };
            let mover = world.households[go].citizens[0];
            let money = world.households[go].money;
            move_money(world, go, stay, money);
            transfer(world, mover, stay);
            let cars = world.households[go].cars;
            let goods = world.households[go].goods;
            let s = &mut world.households[stay];
            s.cars = (s.cars + cars).min(2);
            s.goods += goods;
            let ledger_left = world.households[go].ledger;
            debug_assert_eq!(ledger_left.net(), world.households[go].money);
            dissolve(world, go);
            let (a, b) = (me, them);
            world.citizens[a].log(day, LifeEvent::Partner(b));
            world.citizens[b].log(day, LifeEvent::Partner(a));
            world.people.events_today.couples += 1;
            settle_household(world, stay);
            return;
        }
    }
}

fn home_pos(world: &World, home: BuildingId) -> [f32; 2] {
    world
        .buildings
        .get(home)
        .map_or([0.0, 0.0], |b| rect_center(b.footprint))
}

// ----------------------------------------------------------- happiness ----

fn lin(x: f32, good: f32, bad: f32) -> f32 {
    ((x - bad) / (good - bad)).clamp(0.0, 1.0)
}

/// Recompute every member's happiness and the household average.
fn happiness(world: &mut World, hid: HouseholdId, income: i64, costs: i64, ran_out: bool) {
    let h = &world.households[hid];
    let home = h.home;
    let rate = world.economy.tax_rate;
    let f = &world.economy.fiscal;
    let earners = h
        .citizens
        .iter()
        .filter(|&&m| world.citizens.get(m).is_some_and(|c| c.job.is_some() || c.retired))
        .count();
    // Household: affordability.
    let burden = h.rent as f32 / income.max(1) as f32;
    let reserve_days = h.money as f32 / costs.max(1) as f32;
    let mut afford = 0.6 * lin(burden, balance::RENT_BURDEN_OK, balance::RENT_BURDEN_BAD)
        + 0.4 * lin(reserve_days, balance::RESERVE_GOOD_DAYS, 0.0);
    if h.austerity {
        afford *= 0.7;
    }
    if h.money < 0 {
        afford = 0.0;
    }
    // City: taxes and finances.
    let taxes = if rate <= balance::DEFAULT_TAX_RATE {
        1.0
    } else {
        lin(rate, balance::DEFAULT_TAX_RATE, balance::MAX_TAX_RATE)
    };
    let mut finances = lin(f.service_level, 1.0, balance::SERVICE_LEVEL_MIN);
    if world.economy.money < 0 {
        finances = (finances - 0.3).max(0.0);
    }
    // Home: shops and leisure nearby.
    let (shop_d, venue_d) = world.cache.access_at(world.buildings.get(home).map(|b| b.footprint));
    let shop = lin(shop_d, balance::ACCESS_GOOD_M, balance::ACCESS_BAD_M);
    let venue = lin(venue_d, balance::ACCESS_GOOD_M, balance::ACCESS_BAD_M * 1.3);
    let mut sum = 0.0;
    let members = h.citizens.clone();
    for &m in &members {
        let c = &world.citizens[m];
        let employment = if c.in_workforce() {
            if c.job.is_some() {
                1.0
            } else {
                (0.45 - 0.15 * c.unemployed_days as f32).max(0.0)
            }
        } else if c.retired || earners > 0 {
            0.9
        } else {
            0.3
        };
        let commute = if c.job.is_some() {
            if c.last_commute_s <= 0.0 {
                0.8
            } else {
                // Long in seconds, or long for the distance (their own
                // commute over its free-flow time), whichever is worse.
                lin(c.last_commute_s, balance::COMMUTE_OK_S, balance::HOPELESS_COMMUTE_S).min(lin(
                    commute_ratio(world, m),
                    balance::COMMUTE_RATIO_OK,
                    balance::COMMUTE_RATIO_BAD,
                ))
            }
        } else {
            0.85
        };
        let access = 0.35 * shop + 0.25 * venue + 0.2 * if ran_out { 0.0 } else { 1.0 } + 0.2 * (1.0 - c.leisure_need);
        let housing = if c.role == HouseholdRole::Dependant
            && c.stage == LifeStage::Adult
            && c.age_years() >= balance::MOVE_OUT_MIN_YEARS + 2
        {
            0.3
        } else {
            1.0
        };
        let hp = &mut world.citizens[m].happiness;
        hp.employment = employment;
        hp.commute = commute;
        hp.affordability = afford;
        hp.access = access;
        hp.taxes = taxes;
        hp.finances = finances;
        hp.housing = housing;
        hp.recompute_total();
        sum += hp.total;
    }
    let h = &mut world.households[hid];
    h.happiness = if members.is_empty() {
        balance::HAPPINESS_START
    } else {
        sum / members.len() as f32
    };
}

/// A worker's last commute over its free-flow estimate (home to work) plus
/// `COMMUTE_RATIO_SLACK_S`, at least 1; 1 without a job or a commute yet.
pub fn commute_ratio(world: &World, id: CitizenId) -> f32 {
    let Some(c) = world.citizens.get(id) else { return 1.0 };
    let (Some(job), Some(h)) = (c.job, world.households.get(c.household)) else {
        return 1.0;
    };
    if c.last_commute_s <= 0.0 {
        return 1.0;
    }
    let free = crate::day::estimate_s(home_pos(world, h.home), home_pos(world, job.workplace));
    (c.last_commute_s / (free + balance::COMMUTE_RATIO_SLACK_S)).max(1.0)
}

/// Is every worker of the household fed up with their commute
/// (`balance::COMMUTE_FED_UP_BELOW`)? The first worker's job if so.
fn fed_up_with_commute(world: &World, hid: HouseholdId) -> Option<BuildingId> {
    let h = &world.households[hid];
    let mut first = None;
    for &m in &h.citizens {
        let Some(c) = world.citizens.get(m) else { continue };
        let Some(job) = c.job else { continue };
        if c.last_commute_s <= 0.0 || c.happiness.commute >= balance::COMMUTE_FED_UP_BELOW {
            return None;
        }
        first.get_or_insert(job.workplace);
    }
    first
}

/// A free home with room for `n` at most `COMMUTE_MOVE_NEARER` times as far
/// from `work` as the current one.
fn nearer_home(world: &mut World, hid: HouseholdId, n: u32, work: BuildingId) -> Option<BuildingId> {
    world.cache.ensure_places(&world.buildings, world.versions.buildings);
    let homes = world.cache.homes.len();
    if homes == 0 {
        return None;
    }
    let cur = world.households[hid].home;
    let wp = home_pos(world, work);
    let now_d = geom::dist(wp, home_pos(world, cur));
    let mut best: Option<(f32, BuildingId)> = None;
    for _ in 0..balance::HOME_SEARCH_SAMPLES {
        let b = world.cache.homes[world.rng.below(homes as u32) as usize];
        let Some(bb) = world.buildings.get(b) else { continue };
        if b == cur || bb.capacity < bb.occupied + n || !households::connected(world, bb.footprint) {
            continue;
        }
        let d = geom::dist(wp, rect_center(bb.footprint));
        if d <= now_d * balance::COMMUTE_MOVE_NEARER && best.is_none_or(|x| d < x.0) {
            best = Some((d, b));
        }
    }
    best.map(|b| b.1)
}

/// The happiness part most often worst among unhappy citizens (for tests
/// and tools; the interface reads `CityPeople::unhappy_reasons`).
pub fn worst_part(world: &World) -> Option<HappinessPart> {
    let r = world.people.stats.unhappy_reasons;
    HappinessPart::ALL
        .iter()
        .copied()
        .filter(|p| r[p.index()] > 0)
        .max_by_key(|p| r[p.index()])
}

/// Driving members of a household who hold a car right now (tests).
pub fn cars_out(world: &World, hid: HouseholdId) -> usize {
    world.households.get(hid).map_or(0, |h| {
        h.citizens
            .iter()
            .filter(|&&m| world.citizens.get(m).is_some_and(|c| c.has_car))
            .count()
    })
}
