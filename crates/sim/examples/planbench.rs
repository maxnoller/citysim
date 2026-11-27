//! What checking a large road plan costs on a grown city: a street grid
//! (8 x 13 blocks of 40 m by default) next to the hierarchy city, checked
//! the way the road tool checks it (a copy of the city's road, zoning and
//! building state, then every street applied in order).
//!
//!   cargo run -p citysim_sim --release --example planbench -- [pop] [nu] [nv]
//!
//! The grown city is kept in `/tmp/planbench-<pop>.citysave`.
use std::time::Instant;

use citysim_sim::*;

fn city(pop: u32) -> World {
    let path = format!("/tmp/planbench-{pop}.citysave");
    if let Ok(bytes) = std::fs::read(&path)
        && let Ok(w) = World::load(&bytes)
    {
        return w;
    }
    let t = Instant::now();
    let mut w = World::new(3);
    let mut player = scenarios::GridPlayer::hierarchy();
    while w.stats.population < pop && w.time.day <= 60 {
        player.act(&mut w);
        w.step();
    }
    eprintln!(
        "built {} residents in {:.0} s",
        w.stats.population,
        t.elapsed().as_secs_f32()
    );
    let _ = std::fs::write(&path, w.save());
    w
}

fn scratch(world: &World) -> World {
    let mut w = World::new(world.seed);
    w.roads = world.roads.clone();
    w.zoning = world.zoning.clone();
    w.buildings = world.buildings.clone();
    w.growth = world.growth.clone();
    w.economy = world.economy.clone();
    w.time = world.time.clone();
    w.versions = world.versions.clone();
    w.versions.roads += 1;
    w.versions.grid += 1;
    w.versions.buildings += 1;
    w
}

fn grid(a: [f32; 2], u: [f32; 2], v: [f32; 2], nu: u32, nv: u32, block: f32) -> Vec<Curve> {
    let at = |i: f32, j: f32| {
        [
            a[0] + u[0] * i * block + v[0] * j * block,
            a[1] + u[1] * i * block + v[1] * j * block,
        ]
    };
    let mut out = Vec::new();
    for j in 0..=nv {
        out.push(Curve::straight(at(0.0, j as f32), at(nu as f32, j as f32)));
    }
    for i in 0..=nu {
        out.push(Curve::straight(at(i as f32, 0.0), at(i as f32, nv as f32)));
    }
    out
}

fn main() {
    let arg = |i: usize, d: u32| std::env::args().nth(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (pop, nu, nv) = (arg(1, 13_800), arg(2, 8), arg(3, 13));
    let mut w = city(pop);
    w.economy.money = 10_000_000;
    println!(
        "city: {} residents, {} buildings, {} segments, {} lattices",
        w.stats.population,
        w.buildings.len(),
        w.roads.segments.len(),
        w.zoning.lattices.len()
    );
    // A grid against the city's edge: from an outer node, outwards.
    let nodes: Vec<[f32; 2]> = w.roads.nodes.values().map(|n| n.pos).collect();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in &nodes {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let mut found = None;
    'search: for &(a, u, v) in &[
        ([hi[0], lo[1]], [1.0, 0.0], [0.0, 1.0]),
        ([lo[0], hi[1]], [0.0, 1.0], [1.0, 0.0]),
        ([lo[0], lo[1]], [0.0, -1.0], [1.0, 0.0]),
    ] {
        // Slide the corner onto a node of the edge if one is near.
        for k in 0..40 {
            let a2 = [a[0] + v[0] * k as f32 * 8.0, a[1] + v[1] * k as f32 * 8.0];
            let curves = grid(a2, u, v, nu, nv, 40.0);
            let mut s = scratch(&w);
            if curves.iter().all(|&curve| {
                s.apply(Command::BuildCurve {
                    curve,
                    kind: RoadKind::Street,
                })
                .is_ok()
            }) {
                found = Some(curves);
                break 'search;
            }
        }
    }
    let curves = found.expect("no place for the grid");
    println!("grid {nu} x {nv}: {} streets", curves.len());
    for round in 0..3 {
        let t = Instant::now();
        let base = scratch(&w);
        let t_scratch = t.elapsed();
        let t = Instant::now();
        let mut s = base.clone();
        let t_clone = t.elapsed();
        let mut per = Vec::new();
        let mut prev = Vec::new();
        for &curve in &curves {
            let cmd = Command::BuildCurve {
                curve,
                kind: RoadKind::Street,
            };
            let t = Instant::now();
            let _ = s.preview(&cmd);
            prev.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            s.apply(cmd).unwrap();
            per.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let total: f64 = per.iter().sum();
        let ptotal: f64 = prev.iter().sum();
        println!(
            "round {round}: scratch {:.2} ms, clone {:.2} ms, apply total {total:.2} ms (max {:.2}), preview total {ptotal:.2} ms",
            t_scratch.as_secs_f64() * 1e3,
            t_clone.as_secs_f64() * 1e3,
            per.iter().cloned().fold(0.0, f64::max)
        );
        if round == 0 {
            println!(
                "  per apply: {:?}",
                per.iter().map(|x| (x * 100.0).round() / 100.0).collect::<Vec<_>>()
            );
        }
    }
    // The real world: building it.
    let t = Instant::now();
    let mut real = w.clone();
    let t_clone = t.elapsed();
    let t = Instant::now();
    for &curve in &curves {
        real.apply(Command::BuildCurve {
            curve,
            kind: RoadKind::Street,
        })
        .unwrap();
    }
    println!(
        "real world: clone {:.1} ms, applying the grid {:.1} ms",
        t_clone.as_secs_f64() * 1e3,
        t.elapsed().as_secs_f64() * 1e3
    );
}
