//! Second-by-second signal trace of a bench junction under load:
//! `sigtrace` (avenue x street, signal, both roads saturated).
use citysim_sim::capacity::{Bench, build};
use citysim_sim::*;

fn main() {
    let b = Bench::crossing(RoadKind::Avenue, RoadKind::Street, JunctionSetting::Signal);
    let (mut w, node) = build(&b);
    let n = node.unwrap();
    let r = 39;
    let ends = [(128 - r, 64), (128 + r, 64), (128, 64 - r), (128, 64 + r)];
    let mut k = 0usize;
    for t in 0..200 * TICKS_PER_SECOND {
        if t % 6 == 0 && w.traffic.pending.len() < 40 {
            let a = k % 4;
            let to = [1, 0, 3, 2][a];
            k += 1;
            let (fx, fy) = ends[a];
            let (tx, ty) = ends[to];
            traffic::debug_request(&mut w, Cell::new(fx, fy).center(), Cell::new(tx, ty).center());
        }
        w.step();
        if t >= 120 * TICKS_PER_SECOND && t % TICKS_PER_SECOND == 0 {
            let j = junction::junction(&w, n).unwrap();
            let q: Vec<u16> = j.queue.clone();
            println!(
                "{:3}s phase {} {:?} {:3} ticks queue {:?} plan coord {}",
                t / TICKS_PER_SECOND,
                j.signal.phase,
                j.signal.stage,
                j.signal.ticks,
                q,
                j.plan.as_ref().map_or(false, |p| p.coordinated)
            );
        }
    }
}
