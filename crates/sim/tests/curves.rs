//! Zoning along curved roads and in blocks drawn at an angle: strips that
//! follow the kerb, rows that meet tidily, enclosed land that fills, the
//! zone tool's queries, saves.

use citysim_sim::zoning::{self, Side};
use citysim_sim::*;

fn world() -> World {
    let mut w = World::new(5);
    w.economy.money = i64::MAX / 4;
    w
}

fn build(w: &mut World, c: Curve) {
    w.apply(Command::BuildCurve {
        curve: c,
        kind: RoadKind::Street,
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

fn arc(centre: [f32; 2], r: f32, from_deg: f32, to_deg: f32) -> Curve {
    let p = |d: f32| {
        let a = d.to_radians();
        [centre[0] + r * a.cos(), centre[1] + r * a.sin()]
    };
    let t = |d: f32| {
        let a = d.to_radians();
        let s = (to_deg - from_deg).signum();
        [-a.sin() * s, a.cos() * s]
    };
    Curve::from_tangents(p(from_deg), t(from_deg), p(to_deg), t(to_deg)).expect("arc")
}

/// A ring of four quarter arcs.
fn ring(w: &mut World, centre: [f32; 2], r: f32) {
    for k in 0..4 {
        let a = k as f32 * 90.0;
        build(w, arc(centre, r, a, a + 90.0));
    }
}

fn usable(w: &World) -> Vec<CellRef> {
    w.zoning.cells().filter(|(_, c)| c.usable()).map(|(c, _)| c).collect()
}

#[test]
fn a_curve_gets_one_strip_per_side_whose_cells_tile_without_gaps() {
    let mut w = world();
    let c = arc([1000.0, 1000.0], 80.0, 180.0, 270.0);
    build(&mut w, c);
    let strips: Vec<_> = w.zoning.lattices().filter(|(_, l)| l.is_strip()).collect();
    assert_eq!(strips.len(), 2, "one strip per side");
    // Neighbouring front cells share their edge exactly.
    for (id, l) in strips {
        let st = l.strip.as_ref().unwrap();
        for i in st.i0..st.i0 + st.columns() - 1 {
            let j = if l.cells.contains_key(&(i, 0)) { 0 } else { -1 };
            let (a, b) = (l.cell_corners(i, j), l.cell_corners(i + 1, j));
            assert!(
                geom::dist(a[1], b[0]) < 1e-3 && geom::dist(a[2], b[3]) < 1e-3,
                "{id:?} column {i}"
            );
        }
    }
    w.check_invariants().unwrap();
}

#[test]
fn the_inside_of_a_curve_is_a_continuous_row_of_lots() {
    let mut w = world();
    // A quarter circle of 60 m radius: its inside kerb is ~88 m long.
    build(&mut w, arc([1000.0, 1000.0], 60.0, 180.0, 270.0));
    let seg = w.roads.segments.keys().next().unwrap();
    let curve = w.roads.segments[seg].curve;
    // Which side is inside: the one towards the centre.
    let mid = curve.pose_at(curve.length() * 0.5, 0.0);
    let to_centre = geom::sub([1000.0, 1000.0], mid.pos);
    let right = curve::right_of(mid.dir);
    let inside = if geom::dot(to_centre, right) > 0.0 {
        Side::Right
    } else {
        Side::Left
    };
    let front = zoning::frontage(&w, seg, inside, 1);
    // Every column of the strip has its front cell, from one end to the
    // other (about one per 8 m of the row's middle line, 52 m from the
    // centre: 82 m).
    assert!(front.len() >= 9, "{} front cells inside", front.len());
    let deep = zoning::frontage(&w, seg, inside, 6);
    assert!(deep.len() >= front.len() * 3, "{} cells deep inside", deep.len());
    // The cells of the inside are all on one lattice, none contested.
    let l = front[0].lattice;
    assert!(deep.iter().all(|c| c.lattice == l));
    assert!(
        w.zoning
            .cells()
            .all(|(_, x)| x.blocked != Some(zoning::Blocked::Contested))
    );
}

#[test]
fn a_ring_can_be_zoned_all_the_way_round_inside_and_grows_facing_the_road() {
    let mut w = world();
    // Highway stub to the ring.
    let entry = w.roads.nodes[w.roads.entry.unwrap()].pos;
    let centre = [entry[0] + 200.0, entry[1]];
    build(&mut w, Curve::straight(entry, [centre[0] - 80.0, centre[1]]));
    ring(&mut w, centre, 80.0);
    let inner = zoning::fill_area(&w, [centre[0], centre[1] - 60.0], 6000);
    // A click in the bare middle fills the same land.
    let mut from_middle = zoning::fill_area(&w, centre, 6000);
    let mut sorted = inner.clone();
    sorted.sort();
    from_middle.sort();
    assert_eq!(from_middle, sorted);
    // The inside of the ring: front rows all the way round, several rows
    // deep (an annulus of 4..~44 m from the centre line).
    assert!(inner.len() > 120, "{} cells inside the ring", inner.len());
    let fronts = inner.iter().filter(|c| w.zoning.cell(**c).unwrap().fronts != 0).count();
    assert!(fronts >= 50, "{fronts} front cells inside");
    // Fill stays inside: nothing beyond the ring's outer side.
    for c in &inner {
        let p = w.zoning.cell_center(*c).unwrap();
        assert!(geom::dist(p, centre) < 80.0, "{c:?} at {p:?} is outside the ring");
    }
    w.apply(Command::ZoneCells {
        cells: inner.clone(),
        zone: Some(Zone::Residential),
    })
    .unwrap();
    // Jobs outside the ring.
    let outer: Vec<CellRef> = usable(&w)
        .into_iter()
        .filter(|c| w.zoning.cell(*c).unwrap().zone.is_none())
        .collect();
    w.apply(Command::ZoneCells {
        cells: outer,
        zone: Some(Zone::Commercial),
    })
    .unwrap();
    for _ in 0..4 * balance::TICKS_PER_DAY {
        w.step();
    }
    w.check_invariants().unwrap();
    let inside: Vec<&Building> = w
        .buildings
        .values()
        .filter(|b| geom::dist(b.center(), centre) < 76.0)
        .collect();
    assert!(inside.len() >= 10, "{} buildings inside the ring", inside.len());
    for b in inside {
        // Facing the ring road: its front points away from the centre.
        let out = geom::sub(b.center(), centre);
        assert!(geom::dot(b.front_dir(), out) > 0.0, "{b:?} turns its back to the road");
        assert!(growth::has_frontage(&w, &b.footprint, b.facing));
    }
}

#[test]
fn where_rows_of_two_roads_meet_the_cell_nearer_its_road_keeps_the_land() {
    let mut w = world();
    // A curve, then a straight road crossing its deep rows later: its front
    // rows win over the curve's deep rows (and the curve's front rows over
    // the straight road's deep rows).
    build(&mut w, arc([1000.0, 1000.0], 120.0, 180.0, 250.0));
    build(&mut w, Curve::straight([820.0, 940.0], [1000.0, 940.0]));
    // Front rows lose land only to the other road's front rows, where the
    // two roads cross.
    let cross = w
        .roads
        .nodes
        .values()
        .find(|n| n.segments.len() == 4)
        .expect("a crossing")
        .pos;
    for (c, x) in w.zoning.cells() {
        if x.blocked == Some(zoning::Blocked::Contested) && x.fronts != 0 {
            let p = w.zoning.cell_center(c).unwrap();
            assert!(
                geom::dist(p, cross) < 30.0,
                "{c:?} at {p:?}: a front row cell lost to deep rows"
            );
        }
    }
    w.check_invariants().unwrap();
}

#[test]
fn a_rotated_grid_of_whole_cell_blocks_is_one_lattice() {
    let mut w = world();
    let th = 13f32.to_radians();
    let (u, v) = ([th.cos(), th.sin()], [-th.sin(), th.cos()]);
    let o = [700.0, 900.0];
    let g = |i: f32, j: f32| [o[0] + (u[0] * i + v[0] * j) * 8.0, o[1] + (u[1] * i + v[1] * j) * 8.0];
    for j in [0.0, 9.0, 18.0] {
        build(&mut w, Curve::straight(g(0.0, j), g(27.0, j)));
    }
    for i in [0.0, 9.0, 18.0, 27.0] {
        build(&mut w, Curve::straight(g(i, 0.0), g(i, 18.0)));
    }
    let stub = w.roads.nodes[w.roads.entry.unwrap()].segments[0];
    let lattices: Vec<LatticeId> = w
        .zoning
        .blocks
        .iter()
        .filter(|b| b.segment != stub)
        .map(|b| b.lattice)
        .collect();
    assert!(lattices.iter().all(|l| *l == lattices[0]), "{lattices:?}");
    // The 8 cells between two streets are all zoneable (six-row blocks
    // from both sides meet), nothing contested.
    let all = usable(&w);
    assert!(
        w.zoning
            .cells()
            .all(|(_, x)| x.blocked != Some(zoning::Blocked::Contested))
    );
    let block = zoning::fill_area(&w, g(4.5, 4.5), 6000);
    assert_eq!(block.len(), 64, "a 9x9 block holds 8x8 cells");
    assert!(all.len() >= 64 * 6);
}

#[test]
fn a_straight_road_only_joins_a_lattice_it_lines_up_with_all_along() {
    let mut w = world();
    let th = 30f32.to_radians();
    let (u, v) = ([th.cos(), th.sin()], [-th.sin(), th.cos()]);
    let o = [700.0, 900.0];
    let g = |i: f32, j: f32| [o[0] + (u[0] * i + v[0] * j) * 8.0, o[1] + (u[1] * i + v[1] * j) * 8.0];
    build(&mut w, Curve::straight(g(0.0, 0.0), g(30.0, 0.0)));
    // Turned by half a degree: starts on the lattice, ends 2 m off it.
    let th2 = 30.5f32.to_radians();
    let p0 = g(0.0, 7.0);
    build(
        &mut w,
        Curve::straight(p0, [p0[0] + th2.cos() * 240.0, p0[1] + th2.sin() * 240.0]),
    );
    let stub = w.roads.nodes[w.roads.entry.unwrap()].segments[0];
    let l: Vec<LatticeId> = w
        .zoning
        .blocks
        .iter()
        .filter(|b| b.segment != stub)
        .map(|b| b.lattice)
        .collect();
    assert_ne!(l[0], l[l.len() - 1], "{l:?}");
    w.check_invariants().unwrap();
}

#[test]
fn dragging_along_a_road_zones_one_side_through_bends_and_junctions() {
    let mut w = world();
    // An L: a straight road, a curve continuing it, a straight road on.
    build(&mut w, Curve::straight([600.0, 1000.0], [800.0, 1000.0]));
    build(&mut w, arc([800.0, 920.0], 80.0, 90.0, 0.0));
    build(&mut w, Curve::straight([880.0, 920.0], [880.0, 700.0]));
    // A side street on the first road makes a junction on the way.
    build(&mut w, Curve::straight([700.0, 1000.0], [700.0, 1150.0]));
    let (s0, d0, _) = w.roads.nearest_segment([620.0, 1000.0], 5.0).unwrap();
    let (s1, d1, _) = w.roads.nearest_segment([880.0, 760.0], 5.0).unwrap();
    let walk = zoning::road_walk(&w, (s0, d0), (s1, d1), 4000.0).unwrap();
    assert!(walk.len() >= 4, "{walk:?}");
    // The side of the first piece towards +y (south), seen along the walk.
    let p = &walk[0];
    let seg = &w.roads.segments[p.segment];
    let r = curve::right_of(seg.pose(p.from_m, 0.0).dir);
    let r = if p.forward { r } else { [-r[0], -r[1]] };
    let side = if r[1] > 0.0 { Side::Right } else { Side::Left };
    let cells = zoning::frontage_walk(&w, &walk, side, 2);
    // Zoned along all of it (about 470 m, two rows) on the outside of the
    // bend, without one cell on the other side.
    assert!(cells.len() >= 90, "{} cells", cells.len());
    for c in &cells {
        let q = w.zoning.cell_center(*c).unwrap();
        let outside = q[1] > 1000.0 || q[0] > 880.0 || geom::dist(q, [800.0, 920.0]) > 80.0;
        assert!(outside, "{c:?} at {q:?} is on the inside");
    }
    let before = w.clone();
    w.apply(Command::ZoneCells {
        cells: cells.clone(),
        zone: Some(Zone::Commercial),
    })
    .unwrap();
    assert!(
        before
            .preview(&Command::ZoneCells {
                cells,
                zone: Some(Zone::Commercial)
            })
            .is_ok()
    );
    w.check_invariants().unwrap();
}

#[test]
fn splitting_a_curve_keeps_its_strip_and_zoning() {
    let mut w = world();
    let c = arc([1000.0, 1000.0], 100.0, 180.0, 270.0);
    build(&mut w, c);
    let all = usable(&w);
    w.apply(Command::ZoneCells {
        cells: all.clone(),
        zone: Some(Zone::Residential),
    })
    .unwrap();
    let lattices_before: Vec<LatticeId> = w.zoning.lattices().map(|(id, _)| id).collect();
    // A street joining the middle of the curve from the outside.
    let mid = c.pose_at(c.length() * 0.5, 0.0);
    let out = curve::right_of(mid.dir);
    let side = if geom::dot(geom::sub(mid.pos, [1000.0, 1000.0]), out) > 0.0 {
        1.0
    } else {
        -1.0
    };
    build(
        &mut w,
        Curve::straight(
            mid.pos,
            [mid.pos[0] + out[0] * side * 120.0, mid.pos[1] + out[1] * side * 120.0],
        ),
    );
    assert_eq!(w.roads.segments.len(), 4, "the stub, two halves and the street");
    // The same strips carry on (no new lattices for the halves) and the
    // zoning away from the new street is untouched.
    for (id, _) in w.zoning.lattices().filter(|(_, l)| l.is_strip()) {
        assert!(lattices_before.contains(&id), "new strip {id:?}");
    }
    let still = all
        .iter()
        .filter(|c| {
            w.zoning
                .cell(**c)
                .is_some_and(|x| x.usable() && x.zone == Some(Zone::Residential))
        })
        .count();
    assert!(still * 10 >= all.len() * 8, "{still} of {} zoned cells kept", all.len());
    w.check_invariants().unwrap();
}

#[test]
fn strips_survive_a_save_and_load() {
    let mut w = world();
    ring(&mut w, [1000.0, 1000.0], 70.0);
    let cells = usable(&w);
    w.apply(Command::ZoneCells {
        cells,
        zone: Some(Zone::Residential),
    })
    .unwrap();
    let bytes = w.save();
    let loaded = World::load(&bytes).unwrap();
    assert_eq!(loaded, w);
    assert!(loaded.zoning.lattices().any(|(_, l)| l.is_strip()));
    loaded.check_invariants().unwrap();
}

#[test]
fn a_version_6_town_with_curves_loads_onto_strips() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/curves_v6.citysave")).unwrap();
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 6);
    let mut w = World::load(&bytes).unwrap();
    w.check_invariants().unwrap();
    assert!(
        w.zoning.lattices().any(|(_, l)| l.is_strip()),
        "curves are on strips now"
    );
    // Nobody was thrown out: the 33 buildings stand (moved onto the new
    // lots where they fit), their residents stay.
    assert_eq!(w.buildings.len(), 33);
    assert!(w.stats.population >= 190, "{}", w.stats.population);
    let pop = w.stats.population;
    let room = |w: &World| w.buildings.values().map(|b| b.capacity).sum::<u32>();
    let room_before = room(&w);
    for _ in 0..2 * balance::TICKS_PER_DAY {
        w.step();
    }
    w.check_invariants().unwrap();
    // (Upgrades may merge lots into fewer, bigger buildings.)
    assert!(
        room(&w) >= room_before,
        "{} places in {} buildings after two days, {room_before} before",
        room(&w),
        w.buildings.len()
    );
    assert!(
        w.stats.population >= pop,
        "{} residents after two days",
        w.stats.population
    );
    // New lots along the curves are offered to growth at once: zone the
    // free land beside the curved roads and see it build.
    let on_strip = |w: &World, b: &Building| w.zoning.lattice(b.footprint.lattice).is_some_and(|l| l.is_strip());
    let curved: Vec<CellRef> = w
        .zoning
        .cells()
        .filter(|(c, x)| x.free() && x.zone.is_none() && w.zoning.lattice(c.lattice).is_some_and(|l| l.is_strip()))
        .map(|(c, _)| c)
        .collect();
    let before = w.buildings.values().filter(|b| on_strip(&w, b)).count();
    w.apply(Command::ZoneCells {
        cells: curved,
        zone: Some(Zone::Residential),
    })
    .unwrap();
    for _ in 0..2 * balance::TICKS_PER_DAY {
        w.step();
    }
    let after = w.buildings.values().filter(|b| on_strip(&w, b)).count();
    // (New ones: the exact count follows the town's day, which moves with
    // every rule about who goes where and how.)
    assert!(after > before, "{before} -> {after} buildings on the curves' strips");
}

#[test]
fn zoning_queries_find_strip_cells_from_anywhere() {
    let mut w = world();
    ring(&mut w, [1000.0, 1000.0], 70.0);
    // An area much bigger than any strip.
    let q = [[600.0, 600.0], [1400.0, 600.0], [1400.0, 1400.0], [600.0, 1400.0]];
    let found = zoning::cells_in_quad(&w, &q);
    let inside = w
        .zoning
        .cells()
        .filter(|(c, _)| zoning::in_quad(&q, w.zoning.cell_center(*c).unwrap()))
        .count();
    assert!(inside > 300);
    assert_eq!(found.len(), inside);
    // The cell under every usable cell's centre is that cell.
    for c in usable(&w) {
        let p = w.zoning.cell_center(c).unwrap();
        assert_eq!(w.zoning.cell_at(p), Some(c));
    }
}
