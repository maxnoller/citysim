//! Happiness by reason in a saved city (`simint build` makes one): runs one
//! day, then prints the parts of happiness, the workers' commute ratios
//! and the unhappy citizens' main reasons.
//!
//! `cargo run -p citysim_sim --release --example happy -- <save> [hours]`
use citysim_sim::*;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).expect("save");
    let mut w = World::load(&bytes).expect("load");
    let hours: u32 = a.get(1).and_then(|h| h.parse().ok()).unwrap_or(24);
    for _ in 0..hours * balance::TICKS_PER_HOUR {
        w.step();
    }
    let mut ratios: Vec<f32> = Vec::new();
    let mut secs: Vec<f32> = Vec::new();
    let mut parts = [0.0f64; 7];
    let mut n = 0u32;
    let mut worst = [0u32; 7];
    let mut texts: std::collections::BTreeMap<&str, u32> = Default::default();
    for (id, c) in w.citizens.iter() {
        n += 1;
        for p in HappinessPart::ALL {
            parts[p.index()] += c.happiness.part(p) as f64;
        }
        if let Some(p) = c
            .happiness
            .worst()
            .filter(|_| c.happiness.total < balance::UNHAPPY_BELOW)
        {
            worst[p.index()] += 1;
        }
        if c.job.is_some() && c.last_commute_s > 0.0 {
            ratios.push(life::commute_ratio(&w, id));
            secs.push(c.last_commute_s);
            if let Some(r) = inspect::happiness_reasons(&w, id) {
                *texts.entry(r[0].text).or_default() += 1;
            }
        }
    }
    ratios.sort_by(f32::total_cmp);
    secs.sort_by(f32::total_cmp);
    let q = |v: &[f32], p: f32| v[((v.len() - 1) as f32 * p) as usize];
    println!(
        "pop {} day {} delay factor {:.2} avg happiness {:.3}",
        w.stats.population,
        w.time.day,
        traffic::delay_factor(&w),
        w.people.stats.avg_happiness
    );
    println!("mean parts [employment, commute, afford, access, taxes, finances, housing]:");
    println!("  {:?}", parts.map(|x| (x / n as f64 * 1000.0).round() / 1000.0));
    println!("unhappy citizens by worst part: {worst:?}");
    println!(
        "workers' commute ratio p10 {:.2} p50 {:.2} p90 {:.2}; commute s p10 {:.0} p50 {:.0} p90 {:.0}",
        q(&ratios, 0.1),
        q(&ratios, 0.5),
        q(&ratios, 0.9),
        q(&secs, 0.1),
        q(&secs, 0.5),
        q(&secs, 0.9)
    );
    println!("workers' first reason: {texts:?}");
    println!(
        "commute moves yesterday {:?}, traffic loss yesterday {:?}",
        w.people.commute_moves_yesterday, w.economy.last_day_traffic
    );
}
