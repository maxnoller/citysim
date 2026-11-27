//! `CITYSIM_DEMO=town|city|hierarchy|jam`: a real town built by the sim's own scripted
//! players (`citysim_sim::scenarios`, commands only) and fast-forwarded
//! before the first frame, so a developed city is one command away.
//!
//! * `town` — `GridPlayer` until ~1,500 residents.
//! * `city` — `GridPlayer` until ~5,000 residents (streets only).
//! * `hierarchy` — `GridPlayer::hierarchy` (avenues as arterials) until
//!   ~10,000 residents (about 20 s of fast-forward in a release build).
//! * `organic` — the sim's free-form town (`scenarios::organic`) until
//!   ~3,800 residents.
//! * `shapes` — gfx's road-shape test town (`shapes`) until ~3,000.
//! * `jam`  — the deliberately bad `two_districts(grid = false)` layout
//!   (every commuter through one junction) on its third day.
//!
//! `CITYSIM_DEMO_POP=n` overrides the population target, `CITYSIM_DEMO_HOUR=h`
//! the time of day the fast-forward stops at (default 08:00, morning rush).
//!
//! A built world is cached as a save (`~/.cache/citysim/demos`, shared by all
//! checkouts) and loaded in well under a second next time, until the
//! simulation's sources change; `CITYSIM_DEMO_CACHE=0` turns that off (see
//! [`cached_build`]).

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
        DemoKind::Organic => (Cell::new(88, 118), 1000.0),
        DemoKind::Shapes => (Cell::new(80, 125), 1000.0),
        DemoKind::Rings => (Cell::new(62, 128), 820.0),
        _ => (Cell::new(45, 128), 560.0),
    }
}

/// A street-level spot inside the town.
pub fn street(kind: DemoKind) -> Cell {
    match kind {
        DemoKind::Jam => Cell::new(22, 126),
        DemoKind::Organic => Cell::new(68, 119),
        DemoKind::Shapes => Cell::new(52, 115),
        DemoKind::Rings => Cell::new(86, 116),
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
        DemoKind::Organic | DemoKind::Shapes | DemoKind::Rings => {
            let target =
                env::<u32>("CITYSIM_DEMO_POP").unwrap_or(if kind == DemoKind::Organic { 3_800 } else { 3_000 });
            let mut refused = Vec::new();
            if kind == DemoKind::Rings {
                scenarios::prebuilt(&mut w, |w| refused = super::rings::rings(w));
            } else if kind == DemoKind::Organic {
                scenarios::prebuilt(&mut w, |w| {
                    refused = scenarios::organic(w)
                        .into_iter()
                        .map(|(c, e)| format!("{e}: {c:?}"))
                        .collect()
                });
            } else {
                scenarios::prebuilt(&mut w, |w| refused = super::shapes::shapes(w));
            }
            for r in refused {
                warn!("{kind:?} demo: refused {r}");
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
    let view = w.grid_view();
    while view.is_road(Cell::new(x + 1, 128)) {
        x += 1;
    }
    drop(view);
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

/// Where built demo worlds are kept: `CITYSIM_DEMO_CACHE=dir`, by default
/// `~/.cache/citysim/demos` (`$XDG_CACHE_HOME`), shared by every checkout;
/// `CITYSIM_DEMO_CACHE=0` builds every time.
fn cache_dir() -> Option<std::path::PathBuf> {
    match std::env::var_os("CITYSIM_DEMO_CACHE") {
        Some(d) if d == "0" || d == "off" => None,
        Some(d) if !d.is_empty() => Some(d.into()),
        _ => std::env::var_os("XDG_CACHE_HOME")
            .filter(|d| !d.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".cache")))
            .map(|d| d.join("citysim").join("demos")),
    }
}

/// A built demo world is reused from the cache (see [`cache_dir`]) when
/// nothing that decides it has changed: the key holds the kind, seed,
/// population target, hour, avenue flag and `CITYSIM_SIM_FINGERPRINT`
/// (`build.rs`: a hash of the simulation's sources, the scenario code and
/// `Cargo.lock`), so a simulation edit never loads a stale city. Saves load
/// in lockstep with the world that was saved (`World::save` / `load`).
fn cached_build(kind: DemoKind, seed: u64) -> World {
    let Some(dir) = cache_dir() else {
        return build(kind, seed);
    };
    let name = format!(
        "{kind:?}-seed{seed}-pop{}-hour{}-avenues{}-sim{}.citysave",
        std::env::var("CITYSIM_DEMO_POP").unwrap_or_default(),
        std::env::var("CITYSIM_DEMO_HOUR").unwrap_or_default(),
        std::env::var("CITYSIM_DEMO_AVENUES").unwrap_or_default(),
        env!("CITYSIM_SIM_FINGERPRINT"),
    );
    let path = dir.join(&name);
    if let Ok(bytes) = std::fs::read(&path) {
        let started = Instant::now();
        match World::load(&bytes) {
            Ok(w) => {
                // Mark it used, so `prune` keeps it.
                let _ = std::fs::File::options()
                    .append(true)
                    .open(&path)
                    .and_then(|f| f.set_modified(std::time::SystemTime::now()));
                info!(
                    "dev scenario {kind:?}: loaded {} in {:.2} s (CITYSIM_DEMO_CACHE=0 builds it)",
                    path.display(),
                    started.elapsed().as_secs_f32()
                );
                return w;
            }
            Err(e) => warn!("dev scenario: cannot load {}: {e:?}", path.display()),
        }
    }
    let w = build(kind, seed);
    // Write under a temporary name and rename: a checkout reading the same
    // file at the same time never sees half of it.
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&tmp, w.save()))
        .and_then(|_| std::fs::rename(&tmp, &path));
    match written {
        Ok(()) => prune(&dir),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            warn!("dev scenario: cannot write {}: {e}", path.display());
        }
    }
    w
}

/// Drop cached worlds nobody has used for a week (old fingerprints).
fn prune(dir: &std::path::Path) {
    let week = std::time::Duration::from_secs(7 * 24 * 3600);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if old && e.path().extension().is_some_and(|x| x == "citysave" || x == "tmp") {
            let _ = std::fs::remove_file(e.path());
        }
    }
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
