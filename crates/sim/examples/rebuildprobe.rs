//! Can every road of a grown town be put back right after it is removed?
//! For each segment: remove it on a copy, then preview the identical road.
use citysim_sim::*;

fn main() {
    let days: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let seed: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(7);
    let mut w = World::new(seed);
    scenarios::prebuilt(&mut w, |w| {
        scenarios::organic(w);
    });
    let day = balance::TICKS_PER_DAY as u64;
    while w.time.tick < days * day {
        w.step();
    }
    println!("pop {} buildings {}", w.stats.population, w.buildings.len());
    // Building pairs overlapping deeper than 0.2 m (separating axis test).
    let polys: Vec<[[f32; 2]; 4]> = w.buildings.values().map(|b| b.corners()).collect();
    let depth = |a: &[[f32; 2]; 4], b: &[[f32; 2]; 4]| -> f32 {
        let mut best = f32::INFINITY;
        for q in [a, b] {
            for k in 0..4 {
                let (p0, p1) = (q[k], q[(k + 1) % 4]);
                let (nx, ny) = (p0[1] - p1[1], p1[0] - p0[0]);
                let n = (nx * nx + ny * ny).sqrt();
                let (nx, ny) = (nx / n, ny / n);
                let pr = |s: &[[f32; 2]; 4]| {
                    let v: Vec<f32> = s.iter().map(|p| p[0] * nx + p[1] * ny).collect();
                    (
                        v.iter().cloned().fold(f32::INFINITY, f32::min),
                        v.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                    )
                };
                let ((a0, a1), (b0, b1)) = (pr(a), pr(b));
                best = best.min(a1.min(b1) - a0.max(b0));
            }
        }
        best
    };
    let mut over = 0;
    let mut worst: f32 = 0.0;
    for i in 0..polys.len() {
        for j in i + 1..polys.len() {
            let d = depth(&polys[i], &polys[j]);
            if d > 0.2 {
                over += 1;
                worst = worst.max(d);
            }
        }
    }
    println!("overlapping building pairs (>0.2 m): {over}, deepest {worst:.2} m");
    let segs: Vec<SegmentId> = w.roads.segments.keys().collect();
    let (mut ok, mut bad) = (0, 0);
    for s in segs {
        let seg = &w.roads.segments[s];
        if seg.kind == RoadKind::Highway {
            continue;
        }
        let (curve, kind, len) = (seg.curve, seg.kind, seg.length);
        let mut c = w.clone();
        if c.apply(Command::RemoveRoad {
            segment: s,
            from_m: 0.0,
            to_m: len,
        })
        .is_err()
        {
            continue;
        }
        match c.preview(&Command::BuildCurve { curve, kind }) {
            Ok(_) => ok += 1,
            Err(e) => {
                bad += 1;
                let straight = curve.is_straight();
                if let CommandError::BuildingInTheWay { building, .. } = e {
                    let b = &w.buildings[building];
                    let d = b
                        .corners()
                        .iter()
                        .map(|p| curve.nearest(*p).1)
                        .fold(f32::INFINITY, f32::min);
                    println!(
                        "{s:?} {kind:?} straight={straight} len={len:.0}: {e} ({:?} lvl {} corner-dist {d:.2} m)",
                        b.kind, b.level
                    );
                    let fp = b.footprint;
                    println!(
                        "   size {:?} cells {:?}..{:?} strip={} created {} rot {:.2}",
                        fp.size,
                        fp.min,
                        fp.max,
                        w.zoning.lattice(fp.lattice).is_some_and(|l| l.is_strip()),
                        b.created_tick,
                        fp.rotation
                    );
                    for (o, os) in w.roads.segments.iter() {
                        let dd = b
                            .corners()
                            .iter()
                            .map(|p| os.curve.nearest(*p).1)
                            .fold(f32::INFINITY, f32::min);
                        if dd < 4.5 {
                            println!(
                                "   near {o:?} {:?} straight={} corner-dist {dd:.2}",
                                os.kind,
                                os.curve.is_straight()
                            );
                        }
                    }
                } else {
                    println!("{s:?} {kind:?} straight={straight}: {e}");
                }
            }
        }
    }
    println!("rebuilt {ok}, refused {bad}");
}
