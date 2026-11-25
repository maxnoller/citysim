//! `CITYSIM_DEMO=town|city|hierarchy|jam`: a real town built by the sim's own scripted
//! players (`citysim_sim::scenarios`, commands only) and fast-forwarded
//! before the first frame, so a developed city is one command away.
//!
//! * `town` — `GridPlayer` until ~1,500 residents.
//! * `city` — `GridPlayer` until ~5,000 residents (streets only).
//! * `hierarchy` — `GridPlayer::hierarchy` (avenues as arterials) until
//!   ~10,000 residents (about 20 s of fast-forward in a release build).
//! * `organic` — the free-form sample town of `organic` (curves, a rotated
//!   grid, a ring) until ~3,000 residents.
//! * `jam`  — the deliberately bad `two_districts(grid = false)` layout
//!   (every commuter through one junction) on its third day.
//!
//! `CITYSIM_DEMO_POP=n` overrides the population target, `CITYSIM_DEMO_HOUR=h`
//! the time of day the fast-forward stops at (default 08:00, morning rush).

use std::time::Instant;

use bevy::prelude::*;
use citysim_sim::scenarios::{self, GridPlayer};
use citysim_sim::{Cell, World, Zone, balance};

use super::DemoKind;
use crate::shared::{SimClock, SimWorld};

/// Hard cap on fast-forwarding (in-game days) so a target that cannot be
/// reached does not hang the start.
const MAX_DAYS: u32 = 70;

/// Where the town's centre is, for camera presets.
pub fn focus(kind: DemoKind) -> (Cell, f32) {
    match kind {
        DemoKind::Jam => (Cell::new(48, 128), 560.0),
        DemoKind::City => (Cell::new(62, 128), 900.0),
        DemoKind::Hierarchy => (Cell::new(70, 128), 1100.0),
        DemoKind::Organic => (Cell::new(80, 125), 1000.0),
        _ => (Cell::new(45, 128), 560.0),
    }
}

/// A street-level spot inside the town.
pub fn street(kind: DemoKind) -> Cell {
    match kind {
        DemoKind::Jam => Cell::new(22, 126),
        DemoKind::Organic => Cell::new(52, 115),
        _ => Cell::new(30, 128),
    }
}

fn env<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

/// Step `w` until it is `hour` o'clock (within one tick).
fn run_to_hour(w: &mut World, hour: f32, mut each: impl FnMut(&mut World)) {
    let target = ((hour.rem_euclid(24.0) / 24.0) * balance::TICKS_PER_DAY as f32) as u32;
    for _ in 0..balance::TICKS_PER_DAY {
        if w.time.tick_of_day == target {
            break;
        }
        each(w);
        w.step();
    }
}

/// Build and fast-forward the scenario world.
pub fn build(kind: DemoKind, seed: u64) -> World {
    let mut w = World::new(seed);
    let hour = env::<f32>("CITYSIM_DEMO_HOUR").unwrap_or(8.0);
    let started = Instant::now();
    match kind {
        DemoKind::Jam => {
            scenarios::prebuilt(&mut w, |w| scenarios::two_districts(w, false));
            while w.time.day < 3 {
                w.step();
            }
            run_to_hour(&mut w, hour, |_| {});
        }
        DemoKind::Organic => {
            let target = env::<u32>("CITYSIM_DEMO_POP").unwrap_or(3_000);
            let mut refused = Vec::new();
            scenarios::prebuilt(&mut w, |w| refused = super::organic::organic(w));
            for r in refused {
                warn!("organic demo: refused {r}");
            }
            while w.stats.population < target && w.time.day <= MAX_DAYS {
                w.step();
            }
            run_to_hour(&mut w, hour, |_| {});
        }
        _ => {
            let target = env::<u32>("CITYSIM_DEMO_POP").unwrap_or(match kind {
                DemoKind::City => 5_000,
                DemoKind::Hierarchy => 10_000,
                _ => 1_500,
            });
            let mut player = if kind == DemoKind::Hierarchy {
                GridPlayer::hierarchy()
            } else {
                GridPlayer::new(&[Zone::Residential, Zone::Commercial, Zone::Industrial])
            };
            while w.stats.population < target && w.time.day <= MAX_DAYS {
                player.act(&mut w);
                w.step();
            }
            if env::<u32>("CITYSIM_DEMO_AVENUES") == Some(1) {
                upgrade_main_road(&mut w);
            }
            run_to_hour(&mut w, hour, |w| player.act(w));
        }
    }
    info!(
        "dev scenario {kind:?}: day {} {:05.2}h, {} residents, {} buildings, {} cars, built in {:.1} s",
        w.time.day,
        w.time.hour(),
        w.stats.population,
        w.stats.buildings,
        w.vehicles.len(),
        started.elapsed().as_secs_f32()
    );
    w
}

/// `CITYSIM_DEMO_AVENUES=1`: upgrade the main road (`y = 128`, east of the
/// highway stub) to an avenue (paid for; dev aid for looking at avenues).
fn upgrade_main_road(w: &mut World) {
    let mut x = 12;
    while w
        .grid
        .get(Cell::new(x + 1, 128))
        .is_some_and(|c| c.occupant == citysim_sim::Occupant::Road)
    {
        x += 1;
    }
    w.economy.money += 1_000_000;
    let r = w.apply(citysim_sim::Command::BuildRoad {
        from: Cell::new(12, 128),
        to: Cell::new(x, 128),
        kind: citysim_sim::RoadKind::Avenue,
    });
    w.economy.money -= 1_000_000;
    info!("dev scenario: main road to avenue (12..{x}, 128): {r:?}");
}

pub struct ScenarioPlugin {
    pub kind: DemoKind,
}

#[derive(Resource)]
struct Pending(DemoKind);

impl Plugin for ScenarioPlugin {
    fn build(&self, app: &mut App) {
        // PreStartup: before the ui decides whether this is a fresh game.
        app.insert_resource(Pending(self.kind))
            .add_systems(PreStartup, load_scenario);
    }
}

fn load_scenario(pending: Res<Pending>, mut sim: ResMut<SimWorld>, mut clock: ResMut<SimClock>) {
    let seed = sim.world.seed;
    sim.replace(cached_build(pending.0, seed));
    clock.paused = false;
}

/// `CITYSIM_DEMO_CACHE=dir`: keep each built demo world as a save in `dir`
/// (keyed by kind, seed, population target and hour) and load it instead
/// of building it again (dev aid: the 10,000-resident city takes ~50 s).
fn cached_build(kind: DemoKind, seed: u64) -> World {
    let Some(dir) = std::env::var_os("CITYSIM_DEMO_CACHE").filter(|d| !d.is_empty()) else {
        return build(kind, seed);
    };
    let name = format!(
        "{kind:?}-seed{seed}-pop{}-hour{}-avenues{}.citysave",
        std::env::var("CITYSIM_DEMO_POP").unwrap_or_default(),
        std::env::var("CITYSIM_DEMO_HOUR").unwrap_or_default(),
        std::env::var("CITYSIM_DEMO_AVENUES").unwrap_or_default(),
    );
    let path = std::path::Path::new(&dir).join(name);
    if let Ok(bytes) = std::fs::read(&path) {
        match World::load(&bytes) {
            Ok(w) => {
                info!("dev scenario {kind:?}: loaded {}", path.display());
                return w;
            }
            Err(e) => warn!("dev scenario: cannot load {}: {e:?}", path.display()),
        }
    }
    let w = build(kind, seed);
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, w.save())) {
        warn!("dev scenario: cannot write {}: {e}", path.display());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn town_scenario_reaches_its_target() {
        let w = build(DemoKind::Town, 1);
        assert!(w.stats.population >= 1_500, "{}", w.stats.population);
        assert!(w.check_sim_invariants().is_ok());
    }
}
