//! Who is where during the day in a grown city: citizens by kind and
//! activity at a few hours (to tune the day). `dayprobe [hierarchy|streets] [day]`
use citysim_sim::scenarios::GridPlayer;
use citysim_sim::*;

fn main() {
    let which = std::env::args().nth(1).unwrap_or("hierarchy".into());
    let day: u32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let mut w = World::new(1);
    let mut p = if which == "streets" {
        GridPlayer::new(&Zone::ALL)
    } else {
        GridPlayer::hierarchy()
    };
    let mut next_h = 0u32;
    let hours = [7u32, 10, 12, 14, 17, 19, 21, 22, 23];
    loop {
        p.act(&mut w);
        w.step();
        if w.time.day < day {
            continue;
        }
        if w.time.day > day {
            break;
        }
        let h = w.time.tick_of_day / balance::TICKS_PER_HOUR;
        if !w.time.tick_of_day.is_multiple_of(balance::TICKS_PER_HOUR)
            || next_h >= hours.len() as u32
            || h != hours[next_h as usize]
        {
            continue;
        }
        next_h += 1;
        // kinds: worker, homemaker, senior, student/teen, child, unemployed
        let mut t = [[0u32; Activity::COUNT]; 6];
        for c in w.citizens.values() {
            let k = if c.job.is_some() {
                0
            } else if c.stage == LifeStage::Child {
                4
            } else if c.stage == LifeStage::Senior || c.retired {
                2
            } else if c.homemaker {
                1
            } else if c.stage == LifeStage::Teen || c.student() {
                3
            } else {
                5
            };
            t[k][c.activity as usize] += 1;
        }
        println!(
            "day {} {:02}:00 pop {} cars {}",
            w.time.day,
            h,
            w.stats.population,
            w.vehicles.len()
        );
        for (k, name) in ["worker", "homemkr", "senior", "student", "child", "jobless"]
            .iter()
            .enumerate()
        {
            println!("  {name:8} {:?}", t[k]);
        }
    }
    println!("activities: {:?}", Activity::ALL);
}
