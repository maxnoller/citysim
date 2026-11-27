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
    let r = run_traced(&b, 1, 60, secs, &mut |w, _n, t| {
        if t % 5 == 0 {
            let bodies = traffic::car_bodies(w);
            let o = traffic::overlapping_bodies(&bodies, 0.05);
            overlaps += o.len() as u32;
            for x in &o {
                worst = worst.max(x.2);
            }
            max_in_box = max_in_box.max(w.vehicles.values().filter(|v| v.junction.is_some()).count());
        }
    });
    println!(
        "{:?} x {:?} {:?} left {left} right {right}: {:.1} cars/min (by arm {:?}), gave up {}, overlap samples {overlaps} worst {worst:.2} m, most in box {max_in_box}, by turn {:?}",
        main, cross, size, r.per_minute, r.by_arm, r.gave_up, r.by_turn
    );
}
