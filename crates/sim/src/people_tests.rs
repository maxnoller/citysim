//! Rule tests for the population model, each in both directions, set up
//! with exact households (crate-internal access to the rules).

use crate::citizens::{Activity, CitizenId, HouseholdRole, LifeEvent, LifeStage, Location};
use crate::scenarios;
use crate::traffic::TripPurpose;
use crate::{BuildingId, Cell, Command, HouseholdId, RoadKind, World, Zone, balance, households, life, people};

const DAY: u64 = balance::TICKS_PER_DAY as u64;

/// One street with housing, shops and industry, grown for `days`.
fn town(seed: u64, days: u64) -> World {
    let mut w = World::new(seed);
    w.apply(Command::BuildRoad {
        from: Cell::new(11, 128),
        to: Cell::new(60, 128),
        kind: RoadKind::Street,
    })
    .unwrap();
    scenarios::zone(&mut w, (12, 126), (60, 127), Zone::Residential);
    scenarios::zone(&mut w, (12, 129), (34, 130), Zone::Commercial);
    scenarios::zone(&mut w, (35, 129), (60, 130), Zone::Industrial);
    for _ in 0..days * DAY {
        w.step();
    }
    w.check_sim_invariants().unwrap();
    w
}

fn homes(w: &World) -> Vec<BuildingId> {
    w.buildings
        .iter()
        .filter(|(_, b)| b.kind == Zone::Residential)
        .map(|(id, _)| id)
        .collect()
}

/// No room anywhere: every home's capacity set to its occupancy.
fn fill_all_homes(w: &mut World) {
    for b in w.buildings.values_mut() {
        if b.kind == Zone::Residential {
            b.capacity = b.occupied.max(1);
        }
    }
}

/// Put every member of `hid` at home, awake.
fn all_at_home(w: &mut World, hid: HouseholdId) {
    let home = w.households[hid].home;
    let now = w.time.tick;
    for m in w.households[hid].citizens.clone() {
        w.citizens[m].has_car = false;
        people::set_place(w, m, Activity::Home, Location::In(home), now);
    }
    w.traffic.pending.retain(|d| d.household != Some(hid));
}

/// A household with members of these ages (years) moved into a home with
/// room for them plus `spare`, everyone at home.
fn household(w: &mut World, ages: &[(u32, HouseholdRole)], spare: u32) -> HouseholdId {
    let n = ages.len() as u32;
    let home = homes(w)
        .into_iter()
        .find(|&b| w.buildings[b].capacity >= w.buildings[b].occupied + n + spare)
        .unwrap_or_else(|| {
            let b = homes(w)[0];
            w.buildings[b].capacity = w.buildings[b].occupied + n + spare;
            b
        });
    let hid = households::arrive(w, home, ages);
    w.households[hid].money = w.households[hid].money.max(0);
    all_at_home(w, hid);
    hid
}

const HEAD: HouseholdRole = HouseholdRole::Head;
const KID: HouseholdRole = HouseholdRole::Dependant;

#[test]
fn ageing_goes_through_every_stage() {
    let mut w = town(1, 1);
    let hid = household(&mut w, &[(40, HEAD), (12, KID)], 0);
    let kid = w.households[hid].citizens[1];
    let parent = w.households[hid].citizens[0];
    assert_eq!(w.citizens[kid].stage, LifeStage::Child);
    let set_age = |w: &mut World, c: CitizenId, years: u32| {
        people::update(w, c, |c| c.age_days = years * balance::DAYS_PER_YEAR - 1);
    };
    // 13: teen.
    set_age(&mut w, kid, 13);
    life::age_household(&mut w, hid);
    assert_eq!(w.citizens[kid].stage, LifeStage::Teen);
    assert!(w.citizens[kid].journal.iter().any(|e| e.event == LifeEvent::BecameTeen));
    // 18: adult (a student while at home), then in the workforce at 22.
    set_age(&mut w, kid, 18);
    life::age_household(&mut w, hid);
    let c = &w.citizens[kid];
    assert_eq!(c.stage, LifeStage::Adult);
    assert!(c.student() && !c.in_workforce());
    assert!(c.journal.iter().any(|e| e.event == LifeEvent::BecameAdult));
    set_age(&mut w, kid, balance::STUDENT_UNTIL_YEARS);
    life::age_household(&mut w, hid);
    assert!(w.citizens[kid].in_workforce());
    // 65: retired, job gone.
    let wp = w
        .buildings
        .iter()
        .find(|(_, b)| b.kind != Zone::Residential)
        .map(|(id, _)| id)
        .expect("a workplace");
    w.buildings[wp].capacity += 1;
    let shift = life::shift_for(&mut w, wp, false);
    people::hire(&mut w, parent, wp, shift);
    assert!(w.citizens[parent].job.is_some());
    set_age(&mut w, parent, balance::SENIOR_AGE_YEARS);
    life::age_household(&mut w, hid);
    let p = &w.citizens[parent];
    assert_eq!(p.stage, LifeStage::Senior);
    assert!(p.retired && p.job.is_none() && !p.in_workforce());
    assert!(p.journal.iter().any(|e| e.event == LifeEvent::Retired));
    // Younger than that: nothing changes.
    let before = w.citizens[kid].stage;
    life::age_household(&mut w, hid);
    assert_eq!(w.citizens[kid].stage, before);
    w.check_sim_invariants().unwrap();
}

#[test]
fn a_child_is_born_only_where_there_is_room() {
    let mut w = town(2, 1);
    let hid = household(&mut w, &[(28, HEAD), (29, HEAD)], 0);
    fill_all_homes(&mut w);
    for _ in 0..400 {
        all_at_home(&mut w, hid);
        life::events(&mut w, hid);
    }
    assert_eq!(w.households[hid].citizens.len(), 2, "no room: no baby");
    // Room for one more at home.
    let home = w.households[hid].home;
    w.buildings[home].capacity += 1;
    let mut born = false;
    for _ in 0..400 {
        all_at_home(&mut w, hid);
        life::events(&mut w, hid);
        if w.households[hid].citizens.len() == 3 {
            born = true;
            break;
        }
    }
    assert!(born, "room: a baby is born");
    let baby = w.households[hid].citizens[2];
    assert_eq!(w.citizens[baby].stage, LifeStage::Child);
    assert!(w.citizens[baby].journal.iter().any(|e| e.event == LifeEvent::Born));
    assert!(
        w.citizens[w.households[hid].citizens[0]]
            .journal
            .iter()
            .any(|e| e.event == LifeEvent::ChildBorn(baby))
    );
    assert_eq!(w.buildings[home].occupied, w.buildings[home].capacity);
    w.check_sim_invariants().unwrap();
}

#[test]
fn a_young_adult_moves_out_only_when_a_home_is_free() {
    let mut w = town(3, 1);
    // A single parent (no births) with a grown-up child.
    let hid = household(&mut w, &[(47, HEAD), (24, KID)], 0);
    let child = w.households[hid].citizens[1];
    fill_all_homes(&mut w);
    for _ in 0..300 {
        all_at_home(&mut w, hid);
        life::events(&mut w, hid);
    }
    assert_eq!(w.citizens[child].household, hid, "no free home: stays");
    assert_eq!(w.citizens[child].role, KID);
    // Free one place elsewhere.
    let home = w.households[hid].home;
    let other = homes(&w).into_iter().find(|&b| b != home).expect("another home");
    w.buildings[other].capacity += 1;
    for _ in 0..300 {
        all_at_home(&mut w, hid);
        life::events(&mut w, hid);
        if w.citizens[child].household != hid {
            break;
        }
    }
    let c = &w.citizens[child];
    assert_ne!(c.household, hid, "a home is free: moves out");
    assert_eq!(c.role, HEAD);
    assert_eq!(w.households[c.household].home, other);
    assert!(c.journal.iter().any(|e| e.event == LifeEvent::MovedOut));
    w.check_sim_invariants().unwrap();
}

#[test]
fn a_grown_child_who_cannot_move_out_is_unhappy_about_housing() {
    let mut w = town(4, 1);
    let hid = household(&mut w, &[(50, HEAD), (balance::MOVE_OUT_MIN_YEARS + 3, KID)], 0);
    let child = w.households[hid].citizens[1];
    fill_all_homes(&mut w);
    for _ in 0..2 * DAY {
        w.step();
    }
    if w.citizens.get(child).is_some_and(|c| c.household == hid) {
        assert!(w.citizens[child].happiness.housing < 0.5);
        let parent = w.households[hid].citizens[0];
        assert_eq!(w.citizens[parent].happiness.housing, 1.0);
    }
}

#[test]
fn two_workers_make_two_commutes_to_their_own_workplaces() {
    let mut w = town(5, 3);
    let hid = w
        .households
        .iter()
        .find(|(_, h)| {
            // (Not one whose rush-hour departures keep failing on this one
            // busy street: it may leave town.)
            let jobs: Vec<BuildingId> = h.citizens.iter().filter_map(|&c| w.citizens[c].workplace()).collect();
            jobs.len() >= 2 && jobs[0] != jobs[1] && h.citizens.iter().all(|&c| w.citizens[c].hopeless_commutes == 0)
        })
        .map(|(id, _)| id)
        .expect("a two-earner household with two workplaces");
    let workers: Vec<CitizenId> = w.households[hid]
        .citizens
        .iter()
        .copied()
        .filter(|&c| w.citizens[c].job.is_some())
        .take(2)
        .collect();
    let mut commuted = [0u32; 2];
    let mut arrived = [false; 2];
    for _ in 0..3 * DAY {
        w.step();
        for (k, &c) in workers.iter().enumerate() {
            let Some(ci) = w.citizens.get(c) else { continue };
            if let Location::Travelling(j) = ci.location
                && j.purpose == TripPurpose::ToWork
                && j.departed_tick + 1 == w.time.tick
            {
                assert_eq!(Some(j.to), ci.workplace(), "commutes to their own workplace");
                commuted[k] += 1;
            }
            arrived[k] |= ci.activity == Activity::Working && ci.location.building() == ci.workplace();
        }
    }
    assert!(commuted[0] >= 1 && commuted[1] >= 1, "{commuted:?}");
    assert!(arrived[0] && arrived[1]);
}

#[test]
fn low_goods_send_one_member_shopping_and_restock_for_money() {
    let mut w = town(6, 3);
    // A household with two drivers at home, goods gone.
    let hid = w
        .households
        .iter()
        .find(|(_, h)| h.citizens.iter().filter(|&&c| w.citizens[c].drives()).count() >= 2)
        .map(|(id, _)| id)
        .expect("household");
    w.households[hid].goods = 0.0;
    let spent0 = w.households[hid].ledger.shopping;
    let money0 = w.households[hid].money;
    let sold0: f32 = w.people.places.values().map(|p| p.sold_today + p.sold_yesterday).sum();
    let mut shoppers = std::collections::BTreeSet::new();
    let mut max_out = 0;
    for _ in 0..DAY {
        w.step();
        let Some(h) = w.households.get(hid) else { break };
        let out = h
            .citizens
            .iter()
            .filter(|&&c| {
                let c = &w.citizens[c];
                c.activity == Activity::Shopping
                    || matches!(c.location, Location::Travelling(j) if j.purpose == TripPurpose::Shopping && j.mode != crate::TravelMode::Passenger)
            })
            .count();
        max_out = max_out.max(out);
        if let Some(s) = h.shopper {
            shoppers.insert(s);
        }
        if h.goods > 0.0 && h.shopper.is_none() {
            break;
        }
    }
    let h = &w.households[hid];
    assert!(h.goods > 0.0, "goods restored");
    assert!(h.ledger.shopping > spent0, "paid for");
    assert!(h.money < money0 + h.ledger.wages + h.ledger.pensions, "money spent");
    assert_eq!(h.money, h.ledger.net());
    assert!(max_out <= 1, "one member goes");
    assert!(!shoppers.is_empty());
    let sold1: f32 = w.people.places.values().map(|p| p.sold_today + p.sold_yesterday).sum();
    assert!(sold1 > sold0, "a shop sold it");
    // The other way: a full cupboard, nobody shops.
    let mut w = town(6, 3);
    w.households[hid].goods = 100.0;
    let spent = w.households[hid].ledger.shopping;
    for _ in 0..DAY {
        w.step();
        assert!(w.households.get(hid).is_none_or(|h| h.shopper.is_none()));
    }
    if let Some(h) = w.households.get(hid) {
        assert_eq!(h.ledger.shopping, spent);
    }
}

#[test]
fn payday_conserves_money_wages_tax_and_rent() {
    let mut w = town(7, 2);
    let before: Vec<(HouseholdId, i64, crate::Ledger)> =
        w.households.iter().map(|(id, h)| (id, h.money, h.ledger)).collect();
    let tax = life::payday(&mut w);
    let mut tax_sum = 0;
    for (id, money, ledger) in before {
        let h = &w.households[id];
        let l = h.ledger;
        let (wages, pensions, t, rent) = (
            l.wages - ledger.wages,
            l.pensions - ledger.pensions,
            l.tax - ledger.tax,
            l.rent - ledger.rent,
        );
        assert_eq!(h.money - money, wages + pensions - t - rent, "{id:?}");
        assert_eq!(h.money, l.net());
        let want: u32 = h
            .citizens
            .iter()
            .filter_map(|&c| w.citizens[c].job)
            .map(|j| j.wage)
            .sum();
        assert_eq!(wages, want as i64);
        assert_eq!(t, people::tax_on(want, w.economy.tax_rate));
        tax_sum += t;
    }
    assert_eq!(tax, tax_sum, "the city gets exactly what households paid");
    assert!(tax > 0);
}

#[test]
fn every_citizen_is_in_one_place_and_counted_once() {
    let mut w = town(8, 1);
    for _ in 0..2 * DAY {
        w.step();
        if w.time.tick.is_multiple_of(25) {
            w.check_sim_invariants().unwrap();
            let total: u32 = w.people.activity_counts.iter().sum();
            assert_eq!(total as usize, w.citizens.len());
            let inside = w.citizens.values().filter(|c| c.location.building().is_some()).count();
            let travelling = w.citizens.values().filter(|c| c.location.journey().is_some()).count();
            assert_eq!(inside + travelling, w.citizens.len());
            assert_eq!(
                travelling as u32,
                w.people.activity_counts[Activity::Travelling.index()]
            );
        }
    }
}
