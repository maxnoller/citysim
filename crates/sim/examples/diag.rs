//! Demand components over time in the scripted grid city.
use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn main() {
    let mut w = World::new(7);
    let mut p = GridPlayer::new(&Zone::ALL);
    for _ in 0..20 * balance::TICKS_PER_DAY as u64 {
        p.act(&mut w);
        w.step();
        if w.time.tick.is_multiple_of(balance::TICKS_PER_DAY as u64 / 2) {
            let s = &w.stats;
            let free_jobs = s.jobs as i64 - s.employed as i64;
            println!(
                "day {:2} h{:4.1} pop {:5} rescap {:5} vac {:.2} hh {:4} unemp {:3} jobs {:5} free_jobs {:4} commute {:5.1} R {:+.2} C {:+.2} I {:+.2} attract {:+.2} pending {} blocks {} L2+ {}",
                w.time.day,
                w.time.hour(),
                s.population,
                s.residential_capacity,
                (s.residential_capacity as f32 - s.population as f32) / (s.residential_capacity as f32 + 80.0),
                s.households,
                s.unemployed,
                s.jobs,
                free_jobs,
                s.avg_commute_s,
                w.demand.residential,
                w.demand.commercial,
                w.demand.industrial,
                w.growth.attractiveness,
                s.waiting_departures,
                p.blocks_built,
                w.buildings.values().filter(|b| b.level > 1).count()
            );
        }
    }
}
