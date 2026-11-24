//! Development-only tooling, owned by neither gfx nor ui. Uses only
//! `crate::shared`, Bevy input and the sim.
//!
//! Environment variables:
//! * `CITYSIM_DEMO=town|stress` — fake living town (see `crate::dev_demo`).
//! * `CITYSIM_DEMO=script` (alias `1`) — scripted ~15 s tool demo for videos;
//!   `CITYSIM_DEMO_DELAY=secs` holds still first.
//! * `CITYSIM_AUTOSHOT=out.png` (or `--autoshot out.png`) — screenshot and
//!   exit; `CITYSIM_SHOT_VIEW=wide|street|close`, `CITYSIM_SHOT_DELAY=secs`,
//!   `CITYSIM_SHOT_OVERLAY=none|zones|traffic`.
//! * `CITYSIM_LOG_FPS=1` — log frame time / FPS every second.

mod autoshot;
mod script;

use bevy::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoKind {
    /// Scripted tool demo (drives the real ui through synthetic input).
    Script,
    /// Small fake town with a few hundred buildings and cars.
    Town,
    /// 6,000 buildings and 3,000 cars.
    Stress,
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
    /// Seconds the script demo holds still before starting.
    pub script_delay: f32,
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
            Some("script" | "1") => Some(DemoKind::Script),
            Some("town") => Some(DemoKind::Town),
            Some("stress") => Some(DemoKind::Stress),
            Some("" | "0") | None => None,
            Some(other) => {
                eprintln!("unknown CITYSIM_DEMO={other:?} (use script, town or stress)");
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
            script_delay: env_f32("CITYSIM_DEMO_DELAY").unwrap_or(0.0),
        }
    }
}

pub struct DevPlugin {
    pub config: DevConfig,
}

impl Plugin for DevPlugin {
    fn build(&self, app: &mut App) {
        let c = &self.config;
        match c.demo {
            Some(DemoKind::Town) => {
                app.add_plugins(crate::dev_demo::DevDemoPlugin { stress: false });
            }
            Some(DemoKind::Stress) => {
                app.add_plugins(crate::dev_demo::DevDemoPlugin { stress: true });
            }
            Some(DemoKind::Script) => {
                app.add_plugins(script::ScriptPlugin { delay: c.script_delay });
            }
            None => {}
        }
        if std::env::var("CITYSIM_LOG_FPS").is_ok_and(|v| !v.is_empty() && v != "0") {
            app.add_plugins((
                bevy::diagnostic::FrameTimeDiagnosticsPlugin::default(),
                bevy::diagnostic::LogDiagnosticsPlugin::default(),
            ));
        }
        if let Some(shot) = &c.shot {
            app.add_plugins(autoshot::AutoshotPlugin {
                shot: shot.clone(),
                demo: c.demo,
            });
        }
    }
}
