use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;
use std::panic;
fn main() {
    let mut w = World::new(5);
    let mut p = GridPlayer::new(&Zone::ALL);
    while w.time.tick < 2 * balance::TICKS_PER_DAY as u64 + 600 {
        p.act(&mut w);
        w.step();
    }
    let bytes = w.save();
    eprintln!("save {} bytes", bytes.len());
    panic::set_hook(Box::new(|_| {}));
    let mut r = Rng::new(1);
    let (mut ok, mut err, mut panics, mut load_panics, mut inv_fail) = (0, 0, 0, 0, 0);
    for i in 0..3000 {
        let mut b = bytes.clone();
        let nflips = 1 + r.below(3);
        for _ in 0..nflips {
            let k = 8 + r.below((b.len() - 8) as u32) as usize;
            b[k] ^= 1 << r.below(8);
        }
        if i % 10 == 0 {
            let n = 8 + r.below((b.len() - 8) as u32) as usize;
            b.truncate(n);
        }
        eprintln!("iter {i} load");
        std::fs::write("/tmp/bad.sav", &b).unwrap();
        let res = panic::catch_unwind(|| World::load(&b));
        eprintln!("iter {i} loaded {}", res.as_ref().map(|r| r.is_ok()).unwrap_or(false));
        match res {
            Err(_) => {
                load_panics += 1;
            }
            Ok(Err(_)) => err += 1,
            Ok(Ok(mut lw)) => {
                ok += 1;
                if lw.check_sim_invariants().is_err() || lw.roads.validate().is_err() {
                    inv_fail += 1;
                }
                let r2 = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    for _ in 0..300 {
                        lw.step();
                    }
                    let _ = lw.apply(Command::Bulldoze {
                        min: Cell::new(0, 0),
                        max: Cell::new(255, 255),
                    });
                    for _ in 0..100 {
                        lw.step();
                    }
                }));
                if r2.is_err() {
                    panics += 1;
                }
            }
        }
    }
    eprintln!("ok {ok} err {err} load_panics {load_panics} step_panics {panics} invariant_failures {inv_fail}");
}
