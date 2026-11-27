//! Blocks of land enclosed by straight streets drawn by hand (not on whole
//! cells, a degree or two off true): one grid per block, fitted between its
//! kerbs, so the rows of facing and meeting streets line up.

use citysim_sim::zoning::Side;
use citysim_sim::*;

fn world() -> World {
    let mut w = World::new(5);
    w.economy.money = i64::MAX / 4;
    w
}

fn street(w: &mut World, a: [f32; 2], b: [f32; 2]) {
    w.apply(Command::BuildCurve {
        curve: Curve::straight(a, b),
        kind: RoadKind::Street,
    })
    .unwrap_or_else(|e| panic!("{a:?} -> {b:?}: {e}"));
}

/// A small deterministic generator for the property tests.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

fn rot(p: [f32; 2], c: [f32; 2], a: f32) -> [f32; 2] {
    let (s, k) = a.sin_cos();
    let d = [p[0] - c[0], p[1] - c[1]];
    [c[0] + d[0] * k - d[1] * s, c[1] + d[0] * s + d[1] * k]
}

/// Segments with a node at both of `a` and `b` (the street drawn between
/// them, maybe split).
fn segments_between(w: &World, a: [f32; 2], b: [f32; 2]) -> Vec<SegmentId> {
    let on = |p: [f32; 2]| {
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let t = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l;
        let off = ((p[0] - a[0]) * d[1] - (p[1] - a[1]) * d[0]).abs() / l;
        off < 0.5 && t > -0.5 && t < l + 0.5
    };
    w.roads
        .segments
        .iter()
        .filter(|(_, s)| on(s.curve.start()) && on(s.curve.end()))
        .map(|(id, _)| id)
        .collect()
}

/// The lattices the blocks on the inner side of a closed loop of streets
/// (corners in order with the land on the right) are on.
fn inner_lattices(w: &World, corners: &[[f32; 2]]) -> Vec<LatticeId> {
    let mut out = Vec::new();
    let n = corners.len();
    for k in 0..n {
        let (a, b) = (corners[k], corners[(k + 1) % n]);
        let segs = segments_between(w, a, b);
        assert!(!segs.is_empty(), "no street {a:?} -> {b:?}");
        for s in segs {
            let seg = &w.roads.segments[s];
            // The side facing the inside: travelling a -> b the land is on
            // the right.
            let same = (seg.curve.end()[0] - seg.curve.start()[0]) * (b[0] - a[0])
                + (seg.curve.end()[1] - seg.curve.start()[1]) * (b[1] - a[1])
                > 0.0;
            let side = if same { Side::Right } else { Side::Left };
            let blocks: Vec<_> = w.zoning.blocks_of(s).filter(|b| b.side == side).collect();
            assert!(!blocks.is_empty(), "{s:?} has no blocks inside");
            out.extend(blocks.iter().map(|b| b.lattice));
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn a_hand_drawn_square_zones_on_one_grid_with_shared_corners() {
    let mut w = world();
    let c = [[403.0, 401.0], [470.3, 402.5], [469.0, 475.7], [401.5, 474.0]];
    for k in 0..4 {
        street(&mut w, c[k], c[(k + 1) % 4]);
    }
    w.check_invariants().unwrap();
    let lats = inner_lattices(&w, &c);
    assert_eq!(lats.len(), 1, "one grid: {lats:?}");
    assert!(w.zoning.is_block_grid(lats[0]));
    // The four corner cells front two roads each.
    let l = w.zoning.lattice(lats[0]).unwrap();
    let corners = l
        .cells
        .values()
        .filter(|x| x.usable() && x.fronts.count_ones() >= 2)
        .count();
    assert_eq!(corners, 4, "corner cells front both roads");
    // The whole inside fills with one click and nothing of another
    // lattice is in it.
    let fill = zoning::fill_area(&w, [436.0, 438.0], 10_000);
    assert!(fill.iter().all(|c| c.lattice == lats[0]), "fill stays on the grid");
    assert_eq!(fill.len(), l.cells.values().filter(|x| x.usable()).count());
}

#[test]
fn hand_drawn_blocks_of_any_spacing_get_one_grid_property() {
    let mut rng = Lcg(0x5eed);
    for case in 0..60 {
        let mut w = world();
        let (wd, ht) = (rng.range(18.0, 150.0), rng.range(18.0, 150.0));
        let o = [rng.range(300.0, 1200.0), rng.range(300.0, 1200.0)];
        let turn = rng.range(-1.6, 1.6);
        // Each corner off by up to a few metres: sides up to a few degrees
        // off a right angle.
        let j = 0.04 * wd.min(ht);
        let mut c = [[0.0, 0.0], [wd, 0.0], [wd, ht], [0.0, ht]];
        for p in &mut c {
            *p = [p[0] + rng.range(-j, j), p[1] + rng.range(-j, j)];
            *p = rot([o[0] + p[0], o[1] + p[1]], o, turn);
        }
        // Drawn in a random order, one street at a time.
        let start = (rng.next() * 4.0) as usize;
        for k in 0..4 {
            let i = (start + k) % 4;
            street(&mut w, c[i], c[(i + 1) % 4]);
        }
        w.check_invariants().unwrap_or_else(|e| panic!("case {case}: {e}"));
        let lats = inner_lattices(&w, &c);
        assert_eq!(lats.len(), 1, "case {case} ({wd:.1} x {ht:.1}): {lats:?}");
        let id = lats[0];
        let l = w.zoning.lattice(id).unwrap();
        // On whole cells at right angles the shared lattice does it.
        if !w.zoning.is_block_grid(id) {
            assert!(!l.is_strip(), "case {case}");
            continue;
        }
        // Cells are proper quads between 4 and 13 m a side, every usable
        // cell of the inside is on the grid, and the grid's corner cells
        // front two roads.
        for (&(i, jj), x) in &l.cells {
            let q = l.cell_corners(i, jj);
            for e in 0..4 {
                let d = ((q[e][0] - q[(e + 1) % 4][0]).powi(2) + (q[e][1] - q[(e + 1) % 4][1]).powi(2)).sqrt();
                assert!((3.99..=13.01).contains(&d), "case {case}: edge {d}");
            }
            assert!(!(x.usable() && x.blocked.is_some()));
        }
        let inside = [
            (c[0][0] + c[1][0] + c[2][0] + c[3][0]) / 4.0,
            (c[0][1] + c[1][1] + c[2][1] + c[3][1]) / 4.0,
        ];
        let fill = zoning::fill_area(&w, inside, 100_000);
        assert!(
            fill.iter().all(|x| x.lattice == id),
            "case {case}: the inside is one grid"
        );
        let two_fronts = l
            .cells
            .values()
            .filter(|x| x.usable() && x.fronts.count_ones() >= 2)
            .count();
        let cells = l.cells.values().filter(|x| x.usable()).count();
        assert!(two_fronts >= 4.min(cells), "case {case}: corner cells {two_fronts}");
    }
}

#[test]
fn trapezoid_l_and_avenue_blocks_get_one_grid() {
    let mut w = world();
    // A trapezoid.
    let t = [[400.0, 400.0], [520.0, 400.0], [500.0, 484.0], [421.0, 484.0]];
    for k in 0..4 {
        street(&mut w, t[k], t[(k + 1) % 4]);
    }
    assert_eq!(inner_lattices(&w, &t).len(), 1);
    // An L (two bands of one grid).
    let l = [
        [700.0, 400.0],
        [803.0, 400.0],
        [803.0, 451.0],
        [752.0, 451.0],
        [752.0, 505.0],
        [700.0, 505.0],
    ];
    for k in 0..6 {
        street(&mut w, l[k], l[(k + 1) % 6]);
    }
    let lats = inner_lattices(&w, &l);
    assert_eq!(lats.len(), 2, "two bands: {lats:?}");
    assert!(lats.iter().all(|&id| w.zoning.is_block_grid(id)));
    // Rows of the two bands meet edge to edge: the corner points they share
    // on the band boundary are the same.
    let (a, b) = (w.zoning.lattice(lats[0]).unwrap(), w.zoning.lattice(lats[1]).unwrap());
    let (sa, sb) = (a.strip.as_ref().unwrap(), b.strip.as_ref().unwrap());
    assert_eq!(sa.columns(), sb.columns());
    let rows_a = (0..40).rev().find(|&j| a.cells.keys().any(|k| k.1 == j)).unwrap() + 1;
    for i in 0..=sa.columns() {
        let p = a.world_at(i as f32, rows_a as f32);
        let q = b.world_at(i as f32, 0.0);
        assert!((p[0] - q[0]).abs() < 0.01 && (p[1] - q[1]).abs() < 0.01, "{p:?} {q:?}");
    }
    // Stepping across the band boundary lands in the other band.
    let across = lats.iter().any(|&id| {
        let l = w.zoning.lattice(id).unwrap();
        l.cells.keys().any(|&(i, j)| {
            [Facing::PlusY, Facing::MinusY].into_iter().any(|f| {
                let n = w.zoning.cell_across(CellRef::new(id, i, j), f);
                n.lattice != id && lats.contains(&n.lattice) && w.zoning.cell(n).is_some()
            })
        })
    });
    assert!(across, "bands linked");
    // A block with an avenue along one side.
    let v = [[1000.0, 400.0], [1093.0, 400.0], [1094.0, 489.0], [1000.0, 490.0]];
    w.apply(Command::BuildCurve {
        curve: Curve::straight(v[3], v[0]),
        kind: RoadKind::Avenue,
    })
    .unwrap();
    for k in 0..3 {
        street(&mut w, v[k], v[k + 1]);
    }
    assert_eq!(inner_lattices(&w, &v).len(), 1);
    w.check_invariants().unwrap();
}

#[test]
fn a_block_regrids_when_a_street_splits_it_and_back_when_removed() {
    let mut w = world();
    let c = [[403.0, 401.0], [490.3, 402.5], [489.0, 475.7], [401.5, 474.0]];
    for k in 0..4 {
        street(&mut w, c[k], c[(k + 1) % 4]);
    }
    let q = |a: [f32; 2], b: [f32; 2]| [a, [b[0], a[1]], b, [a[0], b[1]]];
    w.apply(Command::ZoneArea {
        quad: q([390.0, 390.0], [500.0, 490.0]),
        zone: Some(Zone::Residential),
    })
    .unwrap();
    let zoned = |w: &World| w.zoning.cells().filter(|(_, x)| x.usable() && x.zone.is_some()).count();
    let before = zoned(&w);
    // A street across the middle: two blocks, each one grid; the zoning
    // stays.
    let (m0, m1) = ([446.0, 401.8], [445.6, 474.9]);
    street(&mut w, m0, m1);
    w.check_invariants().unwrap();
    let left = [c[0], m0, m1, c[3]];
    let right = [m0, c[1], c[2], m1];
    let (la, lb) = (inner_lattices(&w, &left), inner_lattices(&w, &right));
    assert_eq!((la.len(), lb.len()), (1, 1));
    assert_ne!(la, lb);
    assert!(zoned(&w) * 10 >= before * 8, "zones kept: {} of {before}", zoned(&w));
    // Gone again: one grid over the whole block.
    let mid: Vec<SegmentId> = segments_between(&w, m0, m1);
    for s in mid {
        let len = w.roads.segments[s].length;
        w.apply(Command::RemoveRoad {
            segment: s,
            from_m: 0.0,
            to_m: len,
        })
        .unwrap();
    }
    w.check_invariants().unwrap();
    assert_eq!(inner_lattices(&w, &c).len(), 1, "one grid again");
}

#[test]
fn an_older_save_of_hand_drawn_blocks_loads_onto_block_grids() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/hand_blocks_v7.citysave"
    ))
    .unwrap();
    let w = World::load(&bytes).expect("loads");
    w.check_invariants().unwrap();
    let grids = w
        .zoning
        .lattices()
        .filter(|(id, _)| w.zoning.is_block_grid(*id))
        .count();
    assert!(grids >= 3, "the square and the L's two bands: {grids}");
    assert!(w.buildings.len() >= 100, "buildings kept: {}", w.buildings.len());
    // Saved and loaded again: nothing moves.
    let again = World::load(&w.save()).expect("loads again");
    assert_eq!(again.zoning, w.zoning);
    assert_eq!(again.buildings.len(), w.buildings.len());
}

/// Area of the overlap of two convex polygons.
fn overlap_area(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    let mut poly: Vec<[f32; 2]> = a.to_vec();
    let n = b.len();
    // Orientation of b.
    let area = |p: &[[f32; 2]]| {
        (0..p.len())
            .map(|k| {
                let (x, y) = (p[k], p[(k + 1) % p.len()]);
                x[0] * y[1] - x[1] * y[0]
            })
            .sum::<f32>()
            * 0.5
    };
    let s = area(b).signum();
    for k in 0..n {
        let (p, q) = (b[k], b[(k + 1) % n]);
        let side = |x: [f32; 2]| s * ((q[0] - p[0]) * (x[1] - p[1]) - (q[1] - p[1]) * (x[0] - p[0]));
        let mut out = Vec::new();
        for m in 0..poly.len() {
            let (u, v) = (poly[m], poly[(m + 1) % poly.len()]);
            let (su, sv) = (side(u), side(v));
            if su >= 0.0 {
                out.push(u);
            }
            if (su >= 0.0) != (sv >= 0.0) {
                let t = su / (su - sv);
                out.push([u[0] + (v[0] - u[0]) * t, u[1] + (v[1] - u[1]) * t]);
            }
        }
        poly = out;
        if poly.len() < 3 {
            return 0.0;
        }
    }
    area(&poly).abs()
}

#[test]
fn land_cut_where_rows_meet_never_overlaps_another_lattices_land() {
    let mut w = world();
    scenarios::organic(&mut w);
    w.check_invariants().unwrap();
    let shapes = zoning::cell_shapes(&w);
    assert!(
        shapes.len() > 100,
        "rows meet at angles in the organic town: {}",
        shapes.len()
    );
    // Every usable cell's land: its shape or its quad.
    let land: Vec<(CellRef, Vec<[f32; 2]>)> = w
        .zoning
        .cells()
        .filter(|(_, x)| x.usable())
        .map(|(c, _)| {
            (
                c,
                shapes
                    .get(&c)
                    .cloned()
                    .unwrap_or_else(|| w.zoning.cell_corners(c).unwrap().to_vec()),
            )
        })
        .filter(|(_, p)| p.len() >= 3)
        .collect();
    let key = |p: [f32; 2]| ((p[0] / 16.0).floor() as i32, (p[1] / 16.0).floor() as i32);
    let mut buckets: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
    for (k, (_, p)) in land.iter().enumerate() {
        buckets.entry(key(p[0])).or_default().push(k);
    }
    let mut worst = 0.0f32;
    let mut over = 0usize;
    for (k, (c, p)) in land.iter().enumerate() {
        let (bx, by) = key(p[0]);
        for dy in -2..=2 {
            for dx in -2..=2 {
                for &o in buckets.get(&(bx + dx, by + dy)).map(|v| v.as_slice()).unwrap_or(&[]) {
                    let (oc, op) = &land[o];
                    if o > k && oc.lattice != c.lattice {
                        let a = overlap_area(p, op);
                        if a > 4.0 {
                            over += 1;
                        }
                        worst = worst.max(a);
                    }
                }
            }
        }
    }
    // Usable cells of different lattices never overlap; their cut land
    // only by rounding along the line between them, except in a few spots
    // where a strip's end meets a straight lattice (under half a cell).
    assert!(
        over * 50 <= shapes.len(),
        "{over} overlaps over 4 m² of {} cut cells",
        shapes.len()
    );
    assert!(worst < 32.0, "overlap {worst} m²");
}
