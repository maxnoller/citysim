//! Roads through grown buildings in a saved city (`Command::Clearing`):
//! finds streets across blocks from one road to the next, curves through
//! lots and long roads, and prints what each would take down, with its
//! points in sim metres (to draw them in the game: `scripts/cplay.py`).
//!
//! `cargo run -p citysim_sim --example clearprobe -- CITY.citysave [N]`

use citysim_sim::*;

fn unit(v: [f32; 2]) -> [f32; 2] {
    let l = (v[0] * v[0] + v[1] * v[1]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l]
}

fn add(a: [f32; 2], b: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] + b[0] * k, a[1] + b[1] * k]
}

/// Buildings, residents, jobs and cost of a clearing road.
type Takes = (usize, u32, u32, i64);

/// (buildings, residents, jobs, cost) of a clearing road, if it is allowed.
fn takes(w: &World, curve: Curve, kind: RoadKind) -> Option<Takes> {
    let p = w
        .preview(&Command::Clearing(Box::new(Command::BuildCurve { curve, kind })))
        .ok()?;
    let (mut res, mut jobs) = (0, 0);
    for b in &p.buildings {
        let b = &w.buildings[*b];
        if b.kind == Zone::Residential {
            res += b.occupied;
        } else {
            jobs += b.occupied;
        }
    }
    Some((p.buildings.len(), res, jobs, p.cost))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("a .citysave");
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(6);
    let w = World::load(&std::fs::read(path).expect("read")).expect("load");
    println!(
        "{} buildings, {} residents, money {}",
        w.buildings.len(),
        w.stats.population,
        w.economy.money
    );
    // `CLEARPROBE_LINE=x1,y1,x2,y2`: what that street takes, one by one.
    if let Ok(l) = std::env::var("CLEARPROBE_LINE") {
        let v: Vec<f32> = l.split(',').filter_map(|x| x.parse().ok()).collect();
        let curve = Curve::straight([v[0], v[1]], [v[2], v[3]]);
        let p = w.preview(&Command::Clearing(Box::new(Command::BuildCurve {
            curve,
            kind: RoadKind::Street,
        })));
        match p {
            Ok(p) => {
                for id in p.buildings {
                    let b = &w.buildings[id];
                    println!(
                        "{id:?} {:?} level {} at {:?} size {:?} occupied {} built {} (now {})",
                        b.kind,
                        b.level,
                        b.center(),
                        b.size(),
                        b.occupied,
                        b.created_tick,
                        w.time.tick
                    );
                }
            }
            Err(e) => println!("refused: {e}"),
        }
        return;
    }
    let mut across = Vec::new();
    let mut curves = Vec::new();
    for b in w
        .buildings
        .values()
        .filter(|b| b.kind == Zone::Residential && b.occupied > 0)
    {
        let c = b.center();
        let Some((s, d, _)) = w.roads.nearest_segment(c, 30.0) else {
            continue;
        };
        let seg = &w.roads.segments[s];
        let p = seg.nearest(c).2;
        let t = seg.pose(d, 0.0).dir;
        let dir = unit([c[0] - p[0], c[1] - p[1]]);
        // Across the block: to the next road beyond the home.
        let probe = Curve::straight(add(p, dir, 2.0), add(p, dir, 160.0));
        let mut hit: Option<[f32; 2]> = None;
        for (_, o) in w.roads.segments.iter() {
            for (dn, _, q) in probe.intersections(&o.curve) {
                if dn > 10.0 && hit.is_none_or(|h| (h[0] - p[0]).hypot(h[1] - p[1]) > dn + 2.0) {
                    hit = Some(q);
                }
            }
        }
        if let Some(q) = hit {
            let curve = Curve::straight(p, q);
            if let Some(x) = takes(&w, curve, RoadKind::Street) {
                across.push((x, curve));
            }
        }
        // Long roads from the road at an angle, through several blocks.
        if std::env::var("CLEARPROBE_LONG").is_ok() {
            for k in -4i32..=4 {
                let a = k as f32 * 15f32.to_radians();
                let (sn, cs) = (a.sin(), a.cos());
                let d2 = [dir[0] * cs - dir[1] * sn, dir[0] * sn + dir[1] * cs];
                for len in [80.0f32, 120.0, 160.0, 200.0] {
                    let curve = Curve::straight(p, add(p, d2, len));
                    if let Some(x) = takes(&w, curve, RoadKind::Street)
                        && x.0 > 10
                    {
                        across.push((x, curve));
                    }
                }
            }
        }
        // A curve from the road out through the lots, bending along them.
        for side in [1.0f32, -1.0] {
            let corner = add(p, dir, 28.0);
            let end = add(corner, t, 28.0 * side);
            let curve = Curve::through_corner(p, corner, end);
            if let Some(x) = takes(&w, curve, RoadKind::Street) {
                curves.push((x, curve));
            }
        }
    }
    let show = |name: &str, list: &mut Vec<(Takes, Curve)>, lo: usize, hi: usize| {
        list.sort_by_key(|(x, c)| (std::cmp::Reverse(x.1), (c.start()[0] * 10.0) as i64));
        list.dedup_by(|a, b| a.1 == b.1);
        println!("-- {name}");
        for (x, c) in list.iter().filter(|(x, _)| (lo..=hi).contains(&x.0)).take(n) {
            println!(
                "{} buildings, {} residents, {} jobs, ${}: {:?}",
                x.0,
                x.1,
                x.2,
                x.3,
                c.p.map(|p| [p[0].round(), p[1].round()])
            );
        }
    };
    show("across a block", &mut across, 2, 8);
    let mut small: Vec<_> = across.iter().filter(|(x, _)| x.1 + x.2 <= 60).cloned().collect();
    show("across a block, under 60 people", &mut small, 2, 8);
    show("across a block, large", &mut across, 11, 99);
    show("curves", &mut curves, 2, 8);
}
