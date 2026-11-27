//! Dev aid: `CITYSIM_FOLLOW=car|arrive|walker` selects something to follow
//! and follows it, as a player would with a click and F (for the windowless
//! recorder, `CITYSIM_RECORD`, and its follow measurements).
//!
//! `CITYSIM_FOLLOW_AFTER` frames (90) after the scene is ready it looks
//! `CITYSIM_FOLLOW_SECONDS` (15) real seconds ahead at the current speed on
//! a copy of the world and picks, nearest to the middle of the view:
//! * `car`: a car that is on the map the whole time, drives through a
//!   junction and stops and starts again at least once;
//! * `arrive`: a car that arrives in the second half of that time and has
//!   an owner (the view carries on with them);
//! * `walker`: a person under way off the map for the whole time.
//!
//! `CITYSIM_FOLLOW_LOG=file.csv` writes, per frame, where the leader line's
//! dot is drawn (physical px of the view) for the follow measurements.

use std::io::Write as _;

use bevy::diagnostic::FrameCount;
use bevy::prelude::*;
use citysim_sim::{CitizenId, TICKS_PER_SECOND, VehicleId, World};

use super::UiState;
use super::widgets::UiAction;
use crate::shared::{SceneReady, SimClock, SimWorld, WorldCamera};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Car,
    Arrive,
    Walker,
}

#[derive(Resource)]
struct FollowDemo {
    kind: Kind,
    after: u32,
    seconds: f32,
    frames: u32,
    done: bool,
    log: Option<std::fs::File>,
}

pub struct FollowDemoPlugin;

impl FollowDemoPlugin {
    pub fn wanted() -> bool {
        std::env::var("CITYSIM_FOLLOW").is_ok_and(|v| !v.is_empty() && v != "0")
    }
}

impl Plugin for FollowDemoPlugin {
    fn build(&self, app: &mut App) {
        let kind = match std::env::var("CITYSIM_FOLLOW").as_deref() {
            Ok("arrive") => Kind::Arrive,
            Ok("walker") => Kind::Walker,
            _ => Kind::Car,
        };
        let num = |k: &str, d: f32| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let log = std::env::var("CITYSIM_FOLLOW_LOG")
            .ok()
            .filter(|p| !p.is_empty())
            .and_then(|p| std::fs::File::create(p).ok())
            .map(|mut f| {
                let _ = writeln!(f, "frame,leader_shown,leader_x,leader_y,scale");
                f
            });
        app.insert_resource(FollowDemo {
            kind,
            after: num("CITYSIM_FOLLOW_AFTER", 90.0) as u32,
            seconds: num("CITYSIM_FOLLOW_SECONDS", 15.0),
            frames: 0,
            done: false,
            log,
        })
        .add_systems(Update, pick.before(crate::shared::GameSet::Render))
        .add_systems(Last, log_leader);
    }
}

/// The ground point in the middle of the view.
fn view_centre(cams: &Query<&GlobalTransform, With<WorldCamera>>) -> [f32; 2] {
    let Ok(t) = cams.single() else {
        return [1024.0, 1024.0];
    };
    let o = t.translation();
    let d = t.forward().as_vec3();
    let k = if d.y < -1e-3 { -o.y / d.y } else { 0.0 };
    let p = o + d * k;
    [p.x, p.z]
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// What a car did on the copy of the world.
#[derive(Default, Clone, Copy)]
struct Seen {
    seed: u32,
    ticks: u32,
    junctions: u32,
    stops: u32,
    restarts: u32,
    still: u32,
    was_in_junction: bool,
    stopped: bool,
    gone_at: Option<u32>,
}

fn look_ahead(world: &World, ticks: u32) -> Vec<(VehicleId, Seen)> {
    let mut w = world.clone();
    let mut seen: Vec<(VehicleId, Seen)> = w
        .vehicles
        .iter()
        .map(|(id, v)| {
            (
                id,
                Seen {
                    seed: v.seed,
                    was_in_junction: v.junction.is_some(),
                    stopped: v.speed < 0.3,
                    ..default()
                },
            )
        })
        .collect();
    for t in 0..ticks {
        w.step();
        for (id, s) in &mut seen {
            if s.gone_at.is_some() {
                continue;
            }
            let Some(v) = w.vehicles.get(*id).filter(|v| v.seed == s.seed) else {
                s.gone_at = Some(t);
                continue;
            };
            s.ticks += 1;
            let inj = v.junction.is_some();
            if inj && !s.was_in_junction {
                s.junctions += 1;
            }
            s.was_in_junction = inj;
            if v.speed < 0.3 {
                s.still += 1;
                // Standing a whole second counts as a stop.
                if s.still == TICKS_PER_SECOND && !s.stopped {
                    s.stopped = true;
                    s.stops += 1;
                }
            } else {
                s.still = 0;
                if s.stopped && v.speed > 2.0 {
                    s.stopped = false;
                    s.restarts += 1;
                }
            }
        }
    }
    seen
}

fn pick(
    mut demo: ResMut<FollowDemo>,
    ready: Res<SceneReady>,
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    cams: Query<&GlobalTransform, With<WorldCamera>>,
    ui: Res<UiState>,
    mut actions: MessageWriter<UiAction>,
) {
    if demo.done || !ready.0 {
        return;
    }
    demo.frames += 1;
    if demo.frames < demo.after {
        return;
    }
    demo.done = true;
    let w = &sim.world;
    let ticks = (demo.seconds * (TICKS_PER_SECOND * clock.speed_steps()) as f32) as u32;
    let centre = view_centre(&cams);
    let near = |p: [f32; 2]| dist(p, centre);
    match demo.kind {
        Kind::Car | Kind::Arrive => {
            let seen = look_ahead(w, ticks);
            let best = seen
                .iter()
                .filter(|(id, s)| {
                    let Some(v) = w.vehicles.get(*id) else { return false };
                    match demo.kind {
                        Kind::Car => s.gone_at.is_none() && s.junctions >= 1 && s.restarts >= 1 && v.speed > 3.0,
                        _ => {
                            s.gone_at.is_some_and(|g| g > ticks / 3 && g < ticks * 3 / 4)
                                && w.traffic.trips.get(*id).is_some_and(|t| t.owner.is_some())
                                && v.speed > 3.0
                        }
                    }
                })
                .min_by(|a, b| {
                    let pa = citysim_sim::drive::vehicle_pose(w, &w.vehicles[a.0]).0;
                    let pb = citysim_sim::drive::vehicle_pose(w, &w.vehicles[b.0]).0;
                    // Prefer more junctions and stops, then the nearest.
                    let score = |s: &Seen, p| near(p) - 60.0 * (s.junctions.min(3) + s.restarts.min(2)) as f32;
                    score(&a.1, pa).total_cmp(&score(&b.1, pb))
                });
            match best {
                Some((id, s)) => {
                    info!(
                        "follow demo: car {id:?} ({} junctions, {} stops, gone at tick {:?} of {ticks})",
                        s.junctions, s.stops, s.gone_at
                    );
                    actions.write(UiAction::Vehicle(*id));
                }
                None => {
                    warn!("follow demo: no car fits");
                    return;
                }
            }
        }
        Kind::Walker => {
            let now = w.time.tick;
            let best: Option<(CitizenId, [f32; 2])> = w
                .citizens
                .iter()
                .filter_map(|(id, _)| {
                    let t = citysim_sim::inspect::citizen(w, id)?.trip?;
                    if t.vehicle.is_some_and(|v| w.vehicles.contains_key(v)) || t.from.is_none() {
                        return None;
                    }
                    (t.eta_tick? > now + ticks as u64).then_some(())?;
                    let at = super::people::person_position(w, id)?;
                    Some((id, at))
                })
                .min_by(|a, b| near(a.1).total_cmp(&near(b.1)));
            match best {
                Some((id, _)) => {
                    info!("follow demo: person {id:?} under way off the map");
                    actions.write(UiAction::Person(id));
                }
                None => {
                    warn!("follow demo: nobody under way off the map");
                    return;
                }
            }
        }
    }
    if !ui.following {
        actions.write(UiAction::Follow);
    }
}

/// The leader line's dot as drawn this frame.
fn log_leader(
    mut demo: ResMut<FollowDemo>,
    frame: Res<FrameCount>,
    nodes: Query<(&Name, &ComputedNode, &bevy::ui::UiGlobalTransform, &InheritedVisibility)>,
) {
    let Some(f) = demo.log.as_mut() else { return };
    let dot = nodes.iter().find(|(n, ..)| n.as_str() == "Leader dot");
    let (shown, x, y, s) = match dot {
        Some((_, node, tf, vis)) => (
            vis.get(),
            tf.translation.x,
            tf.translation.y,
            1.0 / node.inverse_scale_factor,
        ),
        None => (false, 0.0, 0.0, 1.0),
    };
    let _ = writeln!(f, "{},{},{x:.3},{y:.3},{s:.3}", frame.0, shown as u8);
}
