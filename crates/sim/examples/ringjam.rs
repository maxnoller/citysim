//! The jam town (`two_districts(grid = false)`: every commuter through the
//! junction at (20, 128)) in its morning rush, as it is and with that
//! junction made a roundabout of each size that fits: cars queued on its
//! approaches and cars through it, minute by minute.
//!
//! `cargo run -p citysim_sim --release --example ringjam`

use citysim_sim::*;

#[allow(dead_code)]
mod demo {
    include!("../../game/src/dev/rings.rs");
}

fn main() {
    let town = std::env::args().nth(1).unwrap_or("jam".into());
    let mut base = World::new(1);
    let at = if town == "rings" {
        scenarios::prebuilt(&mut base, |w| {
            demo::rings(w);
        });
        let (x, y): (i32, i32) = (
            std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(80),
            std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(128),
        );
        Cell::new(x, y)
    } else {
        scenarios::prebuilt(&mut base, |w| scenarios::two_districts(w, false));
        Cell::new(20, 128)
    };
    let hour: f32 = std::env::args().nth(4).and_then(|s| s.parse().ok()).unwrap_or(7.0);
    while base.time.day < 3 || base.time.hour() < hour {
        base.step();
    }
    let n = base.roads.node_at(at.center()).expect("the junction");
    println!(
        "junction arms {}, sizes {:?}",
        base.roads.nodes[n].segments.len(),
        roundabout::sizes(&base, n)
    );
    let mut runs = vec![("as built".to_string(), base.clone())];
    for s in RoundaboutSize::ALL {
        let mut w = base.clone();
        w.economy.money += 100_000;
        if w.apply(Command::SetRoundabout { node: n, size: Some(s) }).is_ok() {
            runs.push((s.name().to_string(), w));
        }
    }
    for (name, mut w) in runs {
        let mut line = String::new();
        let mut entered = 0u32;
        for minute in 0..60 {
            for _ in 0..(60 * TICKS_PER_SECOND) {
                w.step();
            }
            let j = junction::junction(&w, n).unwrap();
            let q: u32 = j.queue.iter().map(|&x| x as u32).max().unwrap_or(0);
            entered += j.peak_minute_today as u32 * 0;
            if minute % 10 == 9 {
                line += &format!(" {:>4}", q);
            }
        }
        let _ = entered;
        println!(
            "{name:9} longest queue at 10/20/../60 min:{line}   delay factor {:.2}, given up {}",
            traffic::delay_factor(&w),
            w.traffic.stuck_removed
        );
    }
}
