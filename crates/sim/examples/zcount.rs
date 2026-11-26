//! Zoned cells and jobs of a scripted town (scratch diagnostics):
//! `zcount`.
use citysim_sim::*;

fn main() {
    let mut w = World::new(1);
    scenarios::prebuilt(&mut w, scenarios::short_of_industry);
    let mut n = [0u32; 3];
    for (_, c) in w.zoning.cells() {
        if let Some(z) = c.zone {
            n[z as usize] += 1;
        }
    }
    println!("zoned cells R/C/I {n:?}");
}
