//! Roads that clear their way (`Command::Clearing`): the grown buildings in
//! a new road's reserve are taken down with it, as the bulldozer would,
//! and paid for at its price; what the preview lists is exactly what goes;
//! nothing changes on a refusal; the road, the buildings and the money can
//! all be put back (the interface's undo: `RemoveRoad`, `ZoneCells`,
//! `RestoreBuilding`, `Refund`).

mod common;

use std::collections::BTreeSet;

use citysim_sim::*;
use common::{DAY, HOUR, check_consistency};

/// The two-district grid town, grown for a day and a half.
fn town() -> World {
    let mut w = World::new(5);
    scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, true));
    while w.time.tick < DAY + 6 * HOUR {
        w.step();
    }
    w
}

fn clearing(curve: Curve, kind: RoadKind) -> Command {
    Command::Clearing(Box::new(Command::BuildCurve { curve, kind }))
}

fn unit(v: [f32; 2]) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l]
}

/// Streets from a road straight out through the lot of a home (a T on the
/// road, a dead end beyond the lots), each refused without clearing
/// because of a building.
fn through_homes(w: &World, len: f32) -> Vec<Curve> {
    let mut out = Vec::new();
    for (_, b) in w.buildings.iter().filter(|(_, b)| b.kind == Zone::Residential) {
        let c = b.center();
        let Some((s, _, _)) = w.roads.nearest_segment(c, 40.0) else {
            continue;
        };
        let p = w.roads.segments[s].nearest(c).2;
        let d = unit([c[0] - p[0], c[1] - p[1]]);
        let curve = Curve::straight(p, [p[0] + d[0] * len, p[1] + d[1] * len]);
        let plain = Command::BuildCurve {
            curve,
            kind: RoadKind::Street,
        };
        if matches!(w.preview(&plain), Err(CommandError::BuildingInTheWay { .. }))
            && w.preview(&clearing(curve, RoadKind::Street)).is_ok()
        {
            out.push(curve);
        }
    }
    out
}

fn ids(w: &World) -> BTreeSet<BuildingId> {
    w.buildings.keys().collect()
}

/// Zones that cells lost between `before` and `after`, by cell centre.
fn lost_zones(before: &World, after: &World) -> Vec<([f32; 2], Zone)> {
    before
        .zoning
        .cells()
        .filter_map(|(c, x)| Some((c, x.zone?)))
        .filter(|&(c, z)| after.zoning.cell(c).is_none_or(|y| y.zone != Some(z)))
        .filter_map(|(c, z)| Some((before.zoning.cell_center(c)?, z)))
        .collect()
}

/// What the interface's undo does: the new road goes, the lost zoning
/// comes back, then the buildings, then the money paid for the road.
fn undo(w: &mut World, serial: u64, zones: &[([f32; 2], Zone)], recs: &[BuildingRecord], paid: &[(u64, i64)]) {
    loop {
        let next = w
            .roads
            .segments
            .iter()
            .find(|(_, s)| s.serial >= serial)
            .map(|(s, seg)| (s, seg.length));
        let Some((s, len)) = next else { break };
        w.apply(Command::RemoveRoad {
            segment: s,
            from_m: 0.0,
            to_m: len,
        })
        .expect("remove the new road");
    }
    for &(p, z) in zones {
        if let Some(c) = w.zoning.cell_at(p)
            && w.zoning.cell_center(c).is_some_and(|q| geom_dist(p, q) < 0.3)
        {
            w.apply(Command::ZoneCells {
                cells: vec![c],
                zone: Some(z),
            })
            .unwrap();
        }
    }
    for r in recs {
        w.apply(Command::RestoreBuilding(Box::new(r.clone())))
            .unwrap_or_else(|e| panic!("restore {:?}: {e}", r.center()));
    }
    for &(tick, amount) in paid {
        w.apply(Command::Refund { tick, amount }).expect("refund");
    }
}

fn geom_dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[test]
fn a_street_through_homes_takes_them_down_and_pays_for_it() {
    let mut w = town();
    let curve = *through_homes(&w, 30.0).first().expect("a street through a home");
    let cmd = clearing(curve, RoadKind::Street);
    let preview = w.preview(&cmd).unwrap();
    assert!(!preview.buildings.is_empty());
    let area: i64 = preview
        .buildings
        .iter()
        .map(|&b| w.buildings[b].footprint.area() as i64)
        .sum();
    assert_eq!(preview.demolition_cost, area * balance::BULLDOZE_COST_PER_CELL);
    // The road's own price is what the plain command would charge with the
    // buildings already gone.
    let mut bare = w.clone();
    growth_free(&mut bare, &preview.buildings);
    let road = bare
        .preview(&Command::BuildCurve {
            curve,
            kind: RoadKind::Street,
        })
        .unwrap();
    assert_eq!(preview.cost, road.cost + preview.demolition_cost);
    assert_eq!(preview.road_cost, road.cost);
    let residents: u32 = preview
        .buildings
        .iter()
        .map(|&b| &w.buildings[b])
        .filter(|b| b.kind == Zone::Residential)
        .map(|b| b.occupied)
        .sum();
    assert!(residents > 0);
    let (money, people, before) = (w.economy.money, w.citizens.len(), ids(&w));
    let recs: Vec<BuildingRecord> = preview.buildings.iter().filter_map(|&b| w.building_record(b)).collect();
    let out = w.apply(cmd).unwrap();
    assert_eq!(out, preview, "apply does what the preview said");
    let gone: Vec<BuildingId> = before.difference(&ids(&w)).copied().collect();
    assert_eq!(gone, preview.buildings, "exactly the listed buildings go");
    assert_eq!(w.economy.money, money - preview.cost);
    assert!(w.citizens.len() + residents as usize <= people, "the residents left");
    // The lots' cells the road's reserve now covers lost their zone; the
    // rest of the land kept it.
    let (mut under, mut kept) = (0, 0);
    for r in &recs {
        for &(_, at) in &r.cells {
            let Some(x) = w.zoning.cell_at(at).and_then(|c| w.zoning.cell(c)) else {
                under += 1;
                continue;
            };
            if x.blocked == Some(Blocked::Road) {
                under += 1;
                assert!(x.zone.is_none(), "a cell under the road is still zoned");
            } else if x.zone == Some(r.building.kind) {
                kept += 1;
            }
        }
    }
    assert!(under > 0 && kept > 0, "{under} under the road, {kept} kept");
    // Nothing stands in the new road's reserve.
    assert!(w.buildings_in_reserve(&curve, RoadKind::Street).is_empty());
    check_consistency(&w, false);
    w.step();
    check_consistency(&w, true);
}

/// Take buildings down without the road (as the bulldozer would).
fn growth_free(w: &mut World, ids: &[BuildingId]) {
    for &b in ids {
        let q = w.buildings[b].corners();
        let c = w.buildings[b].center();
        let quad = q.map(|p| [p[0] + (c[0] - p[0]) * 0.2, p[1] + (c[1] - p[1]) * 0.2]);
        w.apply(Command::BulldozeArea { quad }).unwrap();
    }
}

#[test]
fn what_the_preview_marks_is_exactly_what_goes() {
    // Every street, curve and avenue through the town's homes: the
    // buildings that go are the ones the preview lists, never another.
    let w = town();
    let mut cases = 0;
    for (k, curve) in through_homes(&w, 34.0).into_iter().enumerate() {
        let curves = [
            curve,
            // A curve through the same lots, bending sideways.
            {
                let (a, b) = (curve.start(), curve.end());
                let d = unit([b[0] - a[0], b[1] - a[1]]);
                let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                let mid = [
                    (a[0] + b[0]) * 0.5 - d[1] * 6.0 * side,
                    (a[1] + b[1]) * 0.5 + d[0] * 6.0 * side,
                ];
                Curve::through_corner(a, mid, [b[0] - d[1] * 12.0 * side, b[1] + d[0] * 12.0 * side])
            },
        ];
        for c in curves {
            for kind in [RoadKind::Street, RoadKind::Avenue] {
                let cmd = clearing(c, kind);
                let Ok(p) = w.preview(&cmd) else { continue };
                let mut x = w.clone();
                let before = ids(&x);
                let out = x.apply(cmd).unwrap();
                assert_eq!(out, p);
                let gone: Vec<BuildingId> = before.difference(&ids(&x)).copied().collect();
                assert_eq!(gone, p.buildings, "{c:?} {kind:?}");
                cases += 1;
            }
        }
        if cases >= 24 {
            break;
        }
    }
    assert!(cases >= 8, "only {cases} roads tried");
}

#[test]
fn undo_puts_road_buildings_and_money_back() {
    let w0 = town();
    let curves = through_homes(&w0, 30.0);
    assert!(curves.len() >= 3);
    for &curve in curves.iter().step_by(curves.len() / 3).take(3) {
        let mut w = w0.clone();
        let before = w.clone();
        let cmd = clearing(curve, RoadKind::Street);
        let p = w.preview(&cmd).unwrap();
        let recs: Vec<BuildingRecord> = p.buildings.iter().filter_map(|&b| w.building_record(b)).collect();
        let serial = w.roads.next_serial;
        w.apply(cmd).unwrap();
        let zones = lost_zones(&before, &w);
        assert!(!zones.is_empty());
        undo(&mut w, serial, &zones, &recs, &[(w0.time.tick, p.road_cost)]);
        assert_eq!(w.citizens.len(), before.citizens.len(), "everyone is back");
        assert_eq!(w.buildings.len(), before.buildings.len(), "every building is back");
        for r in &recs {
            assert!(w.buildings.values().any(|b| b.seed == r.building.seed));
        }
        // Road and demolitions refunded; only taking the road away again
        // costs the bulldozer's fee.
        let fee = before.economy.money - w.economy.money;
        assert!((0..=curve.length() as i64).contains(&fee), "{fee}");
        assert_eq!(w.roads.segments.len(), before.roads.segments.len());
        check_consistency(&w, false);
    }
}

#[test]
fn a_refused_road_takes_nothing_down() {
    let mut w = town();
    let curve = *through_homes(&w, 30.0).first().unwrap();
    let cmd = clearing(curve, RoadKind::Street);
    let cost = w.preview(&cmd).unwrap().cost;
    // Not enough money for road and demolitions.
    w.economy.money = cost - 1;
    let (n, people) = (w.buildings.len(), w.citizens.len());
    assert!(matches!(
        w.apply(cmd.clone()),
        Err(CommandError::InsufficientFunds { .. })
    ));
    assert_eq!((w.buildings.len(), w.citizens.len()), (n, people));
    assert_eq!(w.economy.money, cost - 1);
    // Refused for another reason: runs right along an existing road.
    w.economy.money = 1_000_000;
    let (s, seg) = w
        .roads
        .segments
        .iter()
        .find(|(_, s)| s.kind == RoadKind::Street && s.length > 40.0 && s.curve.is_straight())
        .unwrap();
    let _ = s;
    let a = seg.curve.point_at(10.0, 3.0);
    let b = seg.curve.point_at(30.0, 3.0);
    let close = clearing(Curve::straight(a, b), RoadKind::Street);
    assert!(matches!(w.preview(&close), Err(CommandError::TooClose { .. })));
    assert!(w.apply(close).is_err());
    assert_eq!((w.buildings.len(), w.citizens.len()), (n, people));
}

#[test]
fn plain_road_commands_still_refuse_and_clearing_wraps_only_roads() {
    let w = town();
    let curve = *through_homes(&w, 30.0).first().unwrap();
    assert!(matches!(
        w.preview(&Command::BuildCurve {
            curve,
            kind: RoadKind::Street
        }),
        Err(CommandError::BuildingInTheWay { .. })
    ));
    assert_eq!(
        w.preview(&Command::Clearing(Box::new(Command::SetTaxRate(0.1)))),
        Err(CommandError::NotARoadCommand)
    );
    // Every building grown on zoned land gives way.
    assert!(w.buildings.values().all(|b| b.gives_way_to_roads()));
}

#[test]
fn an_upgrade_clears_nothing_and_refunds_once() {
    // Every kind keeps the same 4 m reserve, so a wider kind never meets a
    // building that the narrower one did not.
    let mut w = town();
    let (s, len) = w
        .roads
        .segments
        .iter()
        .find(|(_, s)| s.kind == RoadKind::Street && s.length > 30.0)
        .map(|(s, seg)| (s, seg.length))
        .unwrap();
    let cmd = Command::Clearing(Box::new(Command::SetRoadKind {
        segment: s,
        from_m: 0.0,
        to_m: len,
        kind: RoadKind::Avenue,
    }));
    let p = w.preview(&cmd).unwrap();
    assert!(p.buildings.is_empty());
    let money = w.economy.money;
    let out = w.apply(cmd).unwrap();
    assert_eq!(out, p);
    assert!(out.road_cost > 0 && out.road_cost == out.cost);
    let refund = Command::Refund {
        tick: w.time.tick,
        amount: out.road_cost,
    };
    assert_eq!(w.preview(&refund).unwrap().cost, -p.cost);
    let saved = w.save();
    w.apply(refund.clone()).unwrap();
    assert_eq!(w.economy.money, money);
    assert_eq!(w.apply(refund.clone()), Err(CommandError::NoSuchReceipt), "only once");
    // Payments are not saved: a loaded game has nothing to refund.
    let mut l = World::load(&saved).unwrap();
    assert_eq!(l.apply(refund), Err(CommandError::NoSuchReceipt));
}

#[test]
fn on_curved_lots_too_what_is_marked_is_what_goes() {
    // The organic town's curved streets have strip lots, fitted off the
    // road; a road through them still takes exactly what its preview lists.
    let mut w = World::new(7);
    scenarios::prebuilt(&mut w, |w| {
        scenarios::organic(w);
    });
    while w.time.tick < DAY {
        w.step();
    }
    let curved: BTreeSet<SegmentId> = w
        .roads
        .segments
        .iter()
        .filter(|(_, s)| !s.curve.is_straight())
        .map(|(s, _)| s)
        .collect();
    let mut cases = 0;
    let mut on_curves = 0;
    for curve in through_homes(&w, 34.0) {
        let Some((s, _, _)) = w.roads.nearest_segment(curve.start(), 1.0) else {
            continue;
        };
        let on_curve = curved.contains(&s);
        if !on_curve && cases >= 10 {
            continue;
        }
        let cmd = clearing(curve, RoadKind::Street);
        let p = w.preview(&cmd).unwrap();
        let mut x = w.clone();
        let before = ids(&x);
        assert_eq!(x.apply(cmd).unwrap(), p);
        let gone: Vec<BuildingId> = before.difference(&ids(&x)).copied().collect();
        assert_eq!(gone, p.buildings, "{curve:?}");
        cases += 1;
        on_curves += usize::from(on_curve);
        if on_curves >= 12 {
            break;
        }
    }
    assert!(on_curves >= 4, "{on_curves} roads from curved streets");
}
