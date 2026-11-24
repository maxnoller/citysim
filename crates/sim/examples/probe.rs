use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;
use std::time::Instant;
const DAY: u64 = balance::TICKS_PER_DAY as u64;
const HOUR: u64 = balance::TICKS_PER_HOUR as u64;

fn run_to(w: &mut World, p: &mut GridPlayer, t: u64) {
    while w.time.tick < t {
        p.act(w);
        w.step();
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let mut w = World::new(5);
    let mut p = GridPlayer::new(&Zone::ALL);
    if std::env::var("MESH").is_ok() {
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
        w.economy.money = 50_000;
    }
    let t0 = Instant::now();
    run_to(&mut w, &mut p, 3 * DAY + HOUR);
    eprintln!(
        "grown in {:?}: {} buildings {} households {} cars {} segs pending {} replan {}",
        t0.elapsed(),
        w.buildings.len(),
        w.households.len(),
        w.vehicles.len(),
        w.roads.segments.len(),
        w.traffic.pending.len(),
        w.traffic.replan.len()
    );
    w.economy.money = 10_000_000;
    let hopeless0: u32 = w.households.values().map(|h| h.hopeless_commutes as u32).sum();
    let end = w.time.tick + 6 * HOUR;
    let mut k = 0;
    let mut max_pending = 0;
    let mut max_replan = 0;
    let mut edits = 0;
    while w.time.tick < end {
        if mode == "edit" && w.time.tick.is_multiple_of(20) {
            // toggle a tiny spur road far from traffic flow
            let x = 150 + (k % 50);
            k += 1;
            let _ = w.apply(Command::BuildRoad {
                from: Cell::new(x, 60),
                to: Cell::new(x, 61),
            });
            edits += 1;
        }
        p.act(&mut w);
        w.step();
        max_pending = max_pending.max(w.traffic.pending.len());
        max_replan = max_replan.max(w.traffic.replan.len());
        if w.time.tick.is_multiple_of(HOUR) {
            let hopeless: u32 = w.households.values().map(|h| h.hopeless_commutes as u32).sum();
            eprintln!(
                "{:5.1}h cars {} pending {} replan {} hopeless {} trips_today {} households {}",
                w.time.hour(),
                w.vehicles.len(),
                w.traffic.pending.len(),
                w.traffic.replan.len(),
                hopeless,
                w.traffic.trips_today,
                w.households.len()
            );
        }
    }
    let hopeless: u32 = w.households.values().map(|h| h.hopeless_commutes as u32).sum();
    eprintln!("edits {edits} max_pending {max_pending} max_replan {max_replan} hopeless {hopeless0}->{hopeless}");
}
