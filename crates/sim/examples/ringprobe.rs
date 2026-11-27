//! Roundabout probe: one bench junction as a ring, cars per minute, car
//! bodies overlapping, cars stuck.
//!
//! `cargo run -p citysim_sim --release --example ringprobe [main] [cross] [size] [left] [right] [secs]`

use citysim_sim::capacity::{Bench, run_traced};
use citysim_sim::{JunctionSetting as S, RoadKind as K, RoundaboutSize as R, traffic};

fn kind(s: &str) -> K {
    match s {
        "avenue" => K::Avenue,
        _ => K::Street,
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let main = kind(a.first().map_or("street", |s| s.as_str()));
    let cross = kind(a.get(1).map_or("street", |s| s.as_str()));
    let size = match a.get(2).map_or("small", |s| s.as_str()) {
        "mini" => Some(R::Mini),
        "small" => Some(R::Small),
        "medium" => Some(R::Medium),
        "large" => Some(R::Large),
        _ => None,
    };
    let left: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.15);
    let right: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.15);
    let secs: u32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(300);
    let mut b = Bench::crossing(main, cross, S::Priority).turning(left, right);
    if let Some(s) = size {
        b = b.roundabout(s);
    }
    let mut overlaps = 0u32;
    let mut worst = 0.0f32;
    let mut max_in_box = 0usize;
    let mut still: std::collections::BTreeMap<citysim_sim::VehicleId, u32> = Default::default();
    let seed: u64 = std::env::var("RING_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let warm: u32 = std::env::var("RING_WARM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let r = run_traced(&b, seed, warm, secs, &mut |w, _n, t| {
        if std::env::var("RING_STUCK").is_ok() {
            for (id, v) in w.vehicles.iter() {
                if v.junction.is_some() && v.speed < 0.3 {
                    let c = still.entry(id).or_default();
                    *c += 1;
                    if *c == 200 {
                        println!("t {t}: stuck 10 s");
                        for (_, o) in w.vehicles.iter().filter(|(_, o)| o.junction.is_none() && o.speed < 0.3) {
                            if citysim_sim::geom::dist(o.pos, [1028.0, 516.0]) < 40.0 {
                                println!(
                                    "    lane car seg {:?} fwd {} lane {} prog {:.1} at ({:.1},{:.1}) route {}",
                                    o.segment,
                                    o.forward,
                                    o.lane,
                                    o.progress,
                                    o.pos[0] - 1028.0,
                                    o.pos[1] - 516.0,
                                    o.route.len()
                                );
                            }
                        }
                        for (oid, o) in w.vehicles.iter().filter(|(_, o)| o.junction.is_some()) {
                            let jm = o.junction.unwrap();
                            let j = w.traffic.junction_states.get(jm.node).unwrap();
                            let m = &j.movements[jm.movement as usize];
                            let p = m.ring_path().unwrap();
                            let (pos, _) = m.pose_at(jm.distance);
                            println!(
                                "  {} {:?} m{} arm{}l{}->arm{}l{} ring{} d {:.1}/{:.1} (e {:.1} a {:.1}) v {:.1} at ({:.1},{:.1}) still {}",
                                if oid == id { "*" } else { " " },
                                m.turn,
                                jm.movement,
                                m.from_arm,
                                m.from_lane,
                                m.to_arm,
                                m.to_lane,
                                p.ring_lane,
                                jm.distance,
                                m.length,
                                p.entry_len,
                                p.arc_len,
                                o.speed,
                                pos[0] - 1028.0,
                                pos[1] - 516.0,
                                still.get(&oid).copied().unwrap_or(0)
                            );
                        }
                    }
                } else {
                    still.remove(&id);
                }
            }
        }
        if t % 5 == 0 {
            let bodies = traffic::car_bodies(w);
            let o = traffic::overlapping_bodies(&bodies, 0.05);
            overlaps += o.len() as u32;
            for x in &o {
                worst = worst.max(x.2);
                if std::env::var("RING_VERBOSE").is_ok() && x.2 > 0.05 {
                    let desc = |id: citysim_sim::VehicleId| {
                        let v = &w.vehicles[id];
                        match v.junction {
                            Some(jm) => {
                                let j = w.traffic.junction_states.get(jm.node).unwrap();
                                let m = &j.movements[jm.movement as usize];
                                let p = m.ring_path().unwrap();
                                format!(
                                    "ring m{} {:?} arm{}l{}->arm{}l{} ringlane{} d {:.1}/{:.1} (entry {:.1} arc {:.1}) v {:.1}",
                                    jm.movement,
                                    m.turn,
                                    m.from_arm,
                                    m.from_lane,
                                    m.to_arm,
                                    m.to_lane,
                                    p.ring_lane,
                                    jm.distance,
                                    m.length,
                                    p.entry_len,
                                    p.arc_len,
                                    v.speed
                                )
                            }
                            None => format!(
                                "lane seg {:?} fwd {} lane {} prog {:.1} v {:.1}",
                                v.segment, v.forward, v.lane, v.progress, v.speed
                            ),
                        }
                    };
                    println!(
                        "t {t} depth {:.2}\n  {}\n  {}",
                        x.2,
                        desc(bodies[x.0].0),
                        desc(bodies[x.1].0)
                    );
                }
            }
            max_in_box = max_in_box.max(w.vehicles.values().filter(|v| v.junction.is_some()).count());
        }
    });
    println!(
        "{:?} x {:?} {:?} left {left} right {right}: {:.1} cars/min (by arm {:?}), gave up {}, overlap samples {overlaps} worst {worst:.2} m, most in box {max_in_box}, by turn {:?}",
        main, cross, size, r.per_minute, r.by_arm, r.gave_up, r.by_turn
    );
}
