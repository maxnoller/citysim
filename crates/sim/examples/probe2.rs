use citysim_sim::*;
use std::time::Instant;

fn main() {
    let edit = std::env::args().nth(1).is_some();
    let mut w = World::new(5);
    w.economy.money = 10_000_000;
    for x in (12..=250).step_by(4) {
        let _ = w.apply(Command::BuildRoad {
            from: Cell::new(x, 4),
            to: Cell::new(x, 250),
        });
    }
    for y in (4..=250).step_by(4) {
        let _ = w.apply(Command::BuildRoad {
            from: Cell::new(12, y),
            to: Cell::new(250, y),
        });
    }
    let _ = w.apply(Command::BuildRoad {
        from: Cell::new(11, 128),
        to: Cell::new(12, 128),
    });
    let mut r = Rng::new(3);
    let mut requested = 0u32;
    let t0 = Instant::now();
    let mut k = 0;
    let mut max_replan = 0;
    let mut step_max = std::time::Duration::ZERO;
    for t in 0..4000u32 {
        // keep ~3000 cars going: 10 requests per tick
        if t < 1200 {
            for _ in 0..3 {
                let a = Cell::new(12 + 4 * r.below(60) as i32, 4 + 4 * r.below(62) as i32).center();
                let b = Cell::new(12 + 4 * r.below(60) as i32, 4 + 4 * r.below(62) as i32).center();
                traffic::debug_request(&mut w, a, b);
                requested += 1;
            }
        }
        if edit && t % 20 == 0 && t > 200 {
            let x = 13 + (k % 200);
            k += 1;
            let _ = w.apply(Command::BuildRoad {
                from: Cell::new(x, 2),
                to: Cell::new(x, 3),
            });
        }
        let s = Instant::now();
        w.step();
        step_max = step_max.max(s.elapsed());
        max_replan = max_replan.max(w.traffic.replan.len());
        if t % 400 == 0 {
            eprintln!(
                "t {t} cars {} pending {} replan {} trips_today {} requested {requested} elapsed {:?} max step {:?}",
                w.vehicles.len(),
                w.traffic.pending.len(),
                w.traffic.replan.len(),
                w.traffic.trips_today,
                t0.elapsed(),
                step_max
            );
            step_max = std::time::Duration::ZERO;
        }
    }
    eprintln!("max_replan {max_replan}");
}
