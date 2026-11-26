//! Development-only tooling, owned by neither gfx nor ui. Uses only
//! `crate::shared`, Bevy input and the sim.
//!
//! Environment variables:
//! * `CITYSIM_DEMO=town|city|hierarchy|organic|shapes|jam` — a real town built by the sim's scripted
//!   players and fast-forwarded before the first frame (see `scenario`).
//! * `CITYSIM_AUTOSHOT=out.png` (or `--autoshot out.png`) — screenshot and
//!   exit; `CITYSIM_SHOT_VIEW=wide|street|close`, `CITYSIM_SHOT_DELAY=secs`,
//!   `CITYSIM_SHOT_OVERLAY=none|zones|traffic`, `CITYSIM_SHOT_SIZE=1920x1080`.
//! * `CITYSIM_STILLS=dir` — windowless screenshots, several from one start
//!   (`CITYSIM_STILLS_LIST`, see `stills`).
//! * `CITYSIM_LOG_FPS=1` — log frame time / FPS every second.
//! * `CITYSIM_CONTROL=dir` — file control channel for scripted play-throughs
//!   (`scripts/play.py`, see `control`).

mod autoshot;
mod control;
mod scenario;
mod shapes;
mod stills;

use bevy::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoKind {
    /// Real town of ~1,500 residents (scripted grid player).
    Town,
    /// Real city of ~5,000 residents (scripted grid player).
    City,
    /// Bad layout with one overloaded junction, morning rush.
    Jam,
    /// Round-2 hierarchy city: avenues as arterials, streets inside the
    /// blocks (`GridPlayer::hierarchy`), ~10,000 residents.
    Hierarchy,
    /// The sim's free-form town (`scenarios::organic`: curved avenue, a
    /// grid turned 17 degrees, a ring, a cul-de-sac estate), ~4,000
    /// residents.
    Organic,
    /// gfx's road-shape test town (`shapes`): odd-angle junctions, a
    /// rotated grid, a ring, cul-de-sacs.
    Shapes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotView {
    Wide,
    Street,
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShotConfig {
    pub path: String,
    pub view: ShotView,
    /// Real seconds to wait before capturing.
    pub delay: f32,
    /// Overlay to show (`CITYSIM_SHOT_OVERLAY=none|zones|traffic`).
    pub overlay: crate::shared::OverlayMode,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DevConfig {
    pub demo: Option<DemoKind>,
    pub shot: Option<ShotConfig>,
    /// Control channel directory (`CITYSIM_CONTROL`).
    pub control: Option<std::path::PathBuf>,
}

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

impl DevConfig {
    pub fn from_env() -> DevConfig {
        let args: Vec<String> = std::env::args().collect();
        let arg_after = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        let demo = match arg_after("--demo")
            .or_else(|| std::env::var("CITYSIM_DEMO").ok())
            .as_deref()
        {
            Some("town") => Some(DemoKind::Town),
            Some("city") => Some(DemoKind::City),
            Some("jam") => Some(DemoKind::Jam),
            Some("hierarchy") => Some(DemoKind::Hierarchy),
            Some("organic") => Some(DemoKind::Organic),
            Some("shapes") => Some(DemoKind::Shapes),
            Some("" | "0") | None => None,
            Some(other) => {
                eprintln!("unknown CITYSIM_DEMO={other:?} (use town, city, hierarchy, organic, shapes or jam)");
                None
            }
        };
        let shot = arg_after("--autoshot")
            .or_else(|| std::env::var("CITYSIM_AUTOSHOT").ok())
            .filter(|p| !p.is_empty())
            .map(|path| ShotConfig {
                path,
                view: match std::env::var("CITYSIM_SHOT_VIEW").as_deref() {
                    Ok("street") => ShotView::Street,
                    Ok("close") => ShotView::Close,
                    _ => ShotView::Wide,
                },
                delay: env_f32("CITYSIM_SHOT_DELAY")
                    .or_else(|| env_f32("CITYSIM_AUTOSHOT_DELAY"))
                    .unwrap_or(3.0),
                overlay: match std::env::var("CITYSIM_SHOT_OVERLAY").as_deref() {
                    Ok("zones") => crate::shared::OverlayMode::Zones,
                    Ok("traffic") => crate::shared::OverlayMode::Traffic,
                    _ => crate::shared::OverlayMode::None,
                },
            });
        DevConfig {
            demo,
            shot,
            control: std::env::var_os("CITYSIM_CONTROL")
                .filter(|d| !d.is_empty())
                .map(Into::into),
        }
    }
}

pub struct DevPlugin {
    pub config: DevConfig,
}

impl Plugin for DevPlugin {
    fn build(&self, app: &mut App) {
        let c = &self.config;
        if let Some(kind) = c.demo {
            app.add_plugins(scenario::ScenarioPlugin { kind });
        }
        if std::env::var("CITYSIM_LOG_FPS").is_ok_and(|v| !v.is_empty() && v != "0") {
            app.add_plugins((
                bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
                bevy::diagnostic::LogDiagnosticsPlugin::default(),
            ));
        }
        if let Some(dir) = &c.control {
            app.add_plugins(control::ControlPlugin { dir: dir.clone() });
        }
        if stills::enabled() {
            app.add_plugins(stills::StillsPlugin { demo: c.demo });
        }
        if let Some(shot) = &c.shot {
            app.add_plugins(autoshot::AutoshotPlugin {
                shot: shot.clone(),
                demo: c.demo,
            });
        }
    }
}
