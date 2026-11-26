//! `Command::RestoreBuilding`: a demolished building comes back from the
//! record taken before the demolition (the interface's undo), with its
//! households and staff and the demolition cost refunded; refused, with the
//! reason, when the lot has changed; `preview` agrees with `apply`.

mod common;

use citysim_sim::*;
use common::{DAY, HOUR, check_consistency, check_full};

/// The two-district grid town, grown for a day and a half (everyone
/// housed, shops and works staffed).
fn town() -> World {
    let mut w = World::new(5);
    scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, true));
    while w.time.tick < DAY + 6 * HOUR {
        w.step();
    }
    w
}

/// The building's own outline, pulled in a little (so the neighbours stay).
fn inside(w: &World, b: BuildingId) -> [[f32; 2]; 4] {
    let q = w.buildings[b].corners();
    let c = w.buildings[b].center();
    q.map(|p| [p[0] + (c[0] - p[0]) * 0.2, p[1] + (c[1] - p[1]) * 0.2])
}

/// The fullest building of `kind` (by households for homes, staff else).
fn fullest(w: &World, kind: Zone) -> BuildingId {
    w.buildings
        .iter()
        .filter(|(_, b)| b.kind == kind)
        .max_by_key(|(id, b)| {
            let p = w.people.places.get(*id);
            let n = p.map_or(0, |p| p.households.len() + p.workers.len());
            (n, b.level, b.seed)
        })
        .map(|(id, _)| id)
        .expect("a building")
}

/// Demolish `b` alone; returns its record.
fn demolish(w: &mut World, b: BuildingId) -> BuildingRecord {
    let rec = w.building_record(b).expect("record");
    let out = w.apply(Command::BulldozeArea { quad: inside(w, b) }).expect("bulldoze");
    assert_eq!(out.buildings, vec![b], "only that building goes");
    assert!(out.removed.is_empty(), "no road goes");
    assert!(!w.buildings.contains_key(b));
    rec
}

fn restore(w: &mut World, rec: &BuildingRecord) -> Result<BuildingId, CommandError> {
    let cmd = Command::RestoreBuilding(Box::new(rec.clone()));
    let preview = w.preview(&cmd);
    let before: Vec<BuildingId> = w.buildings.keys().collect();
    let out = w.apply(cmd);
    assert_eq!(preview, out, "preview agrees with apply");
    out.map(|_| {
        w.buildings
            .keys()
            .find(|id| !before.contains(id))
            .expect("a new building")
    })
}

fn same_building(a: &Building, b: &Building) {
    assert_eq!(a.kind, b.kind);
    assert_eq!(a.level, b.level);
    assert_eq!(a.capacity, b.capacity);
    assert_eq!(a.facing, b.facing);
    assert_eq!(a.seed, b.seed);
    assert_eq!(a.created_tick, b.created_tick);
    assert_eq!(a.footprint, b.footprint);
}

#[test]
fn a_demolished_home_comes_back_with_its_households() {
    let mut w = town();
    let home = fullest(&w, Zone::Residential);
    let rec = demolish(&mut w, home);
    assert!(rec.residents() > 0, "the home had residents");
    let seeds: Vec<u32> = rec
        .households
        .iter()
        .flat_map(|h| h.members.iter().map(|c| c.seed))
        .collect();
    assert!(
        w.citizens.values().all(|c| !seeds.contains(&c.seed)),
        "its residents left"
    );
    let jobs_before: usize = rec
        .households
        .iter()
        .flat_map(|h| &h.members)
        .filter(|c| c.job.is_some())
        .count();
    let money = w.economy.money;
    let pop = w.citizens.len();

    let id = restore(&mut w, &rec).expect("restored");
    same_building(&w.buildings[id], &rec.building);
    assert_eq!(w.buildings[id].occupied as usize, rec.residents());
    assert_eq!(w.citizens.len(), pop + rec.residents(), "everyone is back");
    let back: Vec<&Citizen> = w.citizens.values().filter(|c| seeds.contains(&c.seed)).collect();
    assert_eq!(back.len(), seeds.len());
    assert!(back.iter().all(|c| w.households[c.household].home == id));
    let jobs_after = back.iter().filter(|c| c.job.is_some()).count();
    assert!(
        jobs_before > 0 && jobs_after == jobs_before,
        "{jobs_after} of {jobs_before} jobs back"
    );
    for hr in &rec.households {
        let h = w
            .households
            .values()
            .find(|h| h.seed == hr.household.seed && h.home == id)
            .expect("household back");
        assert_eq!(h.money, hr.household.money);
        assert_eq!(h.members as usize, hr.members.len());
    }
    assert!(w.economy.money > money, "the demolition is refunded");
    check_consistency(&w, false);
    check_full(&w);
    // Life goes on in it.
    for _ in 0..2 * HOUR {
        w.step();
    }
    check_consistency(&w, true);
    assert!(w.buildings.contains_key(id));
}

#[test]
fn a_demolished_workplace_takes_its_staff_back() {
    let mut w = town();
    let work = fullest(&w, Zone::Industrial);
    let rec = demolish(&mut w, work);
    assert!(!rec.workers.is_empty(), "it had staff");
    let id = restore(&mut w, &rec).expect("restored");
    same_building(&w.buildings[id], &rec.building);
    let staff = w.people.places.get(id).map_or(0, |p| p.workers.len());
    assert_eq!(staff, rec.workers.len(), "the staff is back");
    assert!(
        rec.workers
            .iter()
            .all(|r| w.citizens[r.citizen].workplace() == Some(id))
    );
    check_consistency(&w, false);
    check_full(&w);
}

#[test]
fn staff_who_found_other_work_stay_there() {
    let mut w = town();
    let work = fullest(&w, Zone::Commercial);
    let rec = demolish(&mut w, work);
    assert!(!rec.workers.is_empty(), "it had staff");
    for _ in 0..DAY / 2 {
        w.step();
    }
    let elsewhere: Vec<CitizenId> = rec
        .workers
        .iter()
        .filter(|r| w.citizens.get(r.citizen).is_some_and(|c| c.job.is_some()))
        .map(|r| r.citizen)
        .collect();
    let jobs: Vec<Option<BuildingId>> = elsewhere.iter().map(|&c| w.citizens[c].workplace()).collect();
    let id = restore(&mut w, &rec).expect("restored");
    let now: Vec<Option<BuildingId>> = elsewhere.iter().map(|&c| w.citizens[c].workplace()).collect();
    assert_eq!(jobs, now, "nobody is taken from their new job");
    assert!(now.iter().all(|&j| j != Some(id)));
    check_consistency(&w, false);
}

#[test]
fn a_restore_says_why_the_lot_has_changed() {
    let mut w = town();
    let home = fullest(&w, Zone::Residential);
    let rec = demolish(&mut w, home);
    let cells: Vec<CellRef> = rec.cells.iter().map(|&(c, _)| c).collect();

    // Zoned for something else.
    let mut x = w.clone();
    x.apply(Command::ZoneCells {
        cells: cells.clone(),
        zone: Some(Zone::Commercial),
    })
    .expect("rezone");
    assert_eq!(
        restore(&mut x, &rec),
        Err(CommandError::CannotRestore(RestoreProblem::Rezoned))
    );

    // Unzoned.
    let mut x = w.clone();
    x.apply(Command::ZoneCells {
        cells: cells.clone(),
        zone: None,
    })
    .expect("unzone");
    assert_eq!(
        restore(&mut x, &rec),
        Err(CommandError::CannotRestore(RestoreProblem::Rezoned))
    );

    // Built on (the same record twice).
    let mut x = w.clone();
    restore(&mut x, &rec).expect("first restore");
    assert_eq!(
        restore(&mut x, &rec),
        Err(CommandError::CannotRestore(RestoreProblem::BuiltOn))
    );

    // The road in front removed: the land beside it is gone.
    let mut x = w.clone();
    let front = rec.center();
    let (seg, _, _) = x.roads.nearest_segment(front, 40.0).expect("road nearby");
    let len = x.roads.segments[seg].length;
    x.apply(Command::RemoveRoad {
        segment: seg,
        from_m: 0.0,
        to_m: len,
    })
    .expect("remove road");
    let e = restore(&mut x, &rec).expect_err("refused");
    assert!(
        matches!(
            e,
            CommandError::CannotRestore(RestoreProblem::LandChanged | RestoreProblem::NoRoad)
        ),
        "{e:?}"
    );
    // Nothing changed by the refusals.
    let mut y = w.clone();
    let id = restore(&mut y, &rec).expect("still restorable here");
    assert!(y.buildings.contains_key(id));
}

/// A street off the highway bending north, zoned on both sides (the
/// bend's lots are on strip lattices), grown for a day.
fn curved_town() -> World {
    let mut w = World::new(9);
    scenarios::prebuilt(&mut w, |w| {
        let a = Cell::new(11, 128).center();
        let b = [a[0] + 160.0, a[1]];
        let end = [b[0] + 180.0, b[1] - 180.0];
        w.apply(Command::BuildCurve {
            curve: Curve::straight(a, b),
            kind: RoadKind::Street,
        })
        .expect("street");
        let bend = Curve::from_tangents(b, [1.0, 0.0], end, [0.0, -1.0]).expect("bend");
        w.apply(Command::BuildCurve {
            curve: bend,
            kind: RoadKind::Street,
        })
        .expect("bend");
        let zone = |w: &mut World, lo: [f32; 2], hi: [f32; 2], z: Zone| {
            w.apply(Command::ZoneArea {
                quad: [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]],
                zone: Some(z),
            })
            .expect("zone");
        };
        zone(w, [a[0] + 20.0, a[1] - 60.0], [b[0], a[1] + 60.0], Zone::Commercial);
        zone(
            w,
            [b[0], end[1] - 10.0],
            [end[0] + 60.0, b[1] + 60.0],
            Zone::Residential,
        );
    });
    while w.time.tick < DAY {
        w.step();
    }
    w
}

#[test]
fn a_home_on_a_bend_comes_back_after_its_road_is_rebuilt() {
    let mut w = curved_town();
    let home = w
        .buildings
        .iter()
        .filter(|(id, b)| {
            b.kind == Zone::Residential
                && w.zoning.lattice(b.footprint.lattice).is_some_and(|l| l.is_strip())
                && w.people.places.get(*id).is_some_and(|p| !p.households.is_empty())
        })
        .map(|(id, _)| id)
        .next()
        .expect("a lived-in home on the bend");
    let seed = w.buildings[home].seed;
    roads_and_buildings_come_back(&mut w, home);
    let back = w.buildings.values().find(|b| b.seed == seed).expect("back");
    assert!(w.zoning.lattice(back.footprint.lattice).is_some_and(|l| l.is_strip()));
}

#[test]
fn roads_and_buildings_of_an_area_come_back_together() {
    let mut w = town();
    let home = fullest(&w, Zone::Residential);
    roads_and_buildings_come_back(&mut w, home);
    check_consistency(&w, true);
}

/// Bulldoze an area over `home` and a stretch of its road, build the road
/// again and put every building back.
fn roads_and_buildings_come_back(w: &mut World, home: BuildingId) {
    let b = w.buildings[home].clone();
    // An area over the building and a stretch of its road.
    let (seg, _, _) = w.roads.nearest_segment(b.center(), 40.0).expect("road");
    let s = &w.roads.segments[seg];
    let (d, _, _) = s.nearest(b.center());
    let p = s.curve.pose_at(d, 0.0).pos;
    let c = b.center();
    let (lo, hi) = (
        [p[0].min(c[0]) - 6.0, p[1].min(c[1]) - 6.0],
        [p[0].max(c[0]) + 6.0, p[1].max(c[1]) + 6.0],
    );
    let quad = [[lo[0], lo[1]], [hi[0], lo[1]], [hi[0], hi[1]], [lo[0], hi[1]]];
    let kinds: Vec<RoadKind> = vec![s.kind];
    let doomed = w.preview(&Command::BulldozeArea { quad }).expect("preview").buildings;
    assert!(doomed.contains(&home));
    let records: Vec<BuildingRecord> = doomed.iter().filter_map(|&id| w.building_record(id)).collect();
    let out = w.apply(Command::BulldozeArea { quad }).expect("bulldoze");
    assert!(!out.removed.is_empty(), "a piece of road goes too");
    // Undo: the road first, then the buildings.
    for (curve, kind) in out.removed.iter().zip(kinds.iter().cycle()) {
        w.apply(Command::BuildCurve {
            curve: *curve,
            kind: *kind,
        })
        .expect("road back");
    }
    let mut back = 0;
    for rec in &records {
        if let Ok(id) = restore(w, rec) {
            same_building(
                &w.buildings[id],
                &Building {
                    footprint: w.buildings[id].footprint,
                    ..rec.building.clone()
                },
            );
            let (a, b) = (w.buildings[id].center(), rec.building.center());
            assert!(geom::dist(a, b) < 0.3, "same place");
            back += 1;
        }
    }
    assert_eq!(back, records.len(), "every building came back");
    w.step();
    // (`check_consistency` assumes straight roads.)
    w.check_sim_invariants().expect("sim invariants");
    check_full(w);
}
