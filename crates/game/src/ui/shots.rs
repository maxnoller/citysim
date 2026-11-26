//! `CITYSIM_UI_SHOTS=dir` (best with `CITYSIM_DEMO=organic`): walks through
//! every interface state and saves `dir/NN-name.png` for each, then exits.
//! `CITYSIM_UI_SHOTS_ONLY=name,name` limits it to some states (names in
//! [`steps`]). `CITYSIM_UI_TOUR=1` plays a scripted walk through the tools
//! for videos.
//!
//! Drives the real ui through a synthetic cursor and forced tool state
//! (the points of a road being drawn, a held drag), so every state is
//! reproducible on any town.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::ui::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use citysim_sim::curve::right_of;
use citysim_sim::{Building, BuildingId, Command, Curve, LatticeId, RoadKind, Zone};

use super::fsnap::{dist, rotate};
use super::shapes::{Draft, Shape};
use super::toasts::{Toast, ToastKind};
use super::tools::{Press, ToolState};
use super::widgets::{UiAction, UiButton};
use super::{CityPage, InfoTray, UiState};
use crate::shared::{
    ActiveTool, BulldozeMode, CameraOverride, CameraPose, GameSet, OverlayMode, RoadChoice, RoadMode, SimClock,
    SimCommand, SimWorld, ToolModes, WorldCamera, WorldHighlight, ZoneMode, sim_to_world,
};

/// The scenario's world while a step shows a modified copy (broke, day 1);
/// `reset` puts it back.
#[derive(Resource, Default)]
struct SavedWorld(Option<citysim_sim::World>);

fn stash_world(w: &mut World) {
    let copy = w.resource::<SimWorld>().world.clone();
    let mut saved = w.resource_mut::<SavedWorld>();
    if saved.0.is_none() {
        saved.0 = Some(copy);
    }
}

pub struct ShotsPlugin {
    pub dir: PathBuf,
}

impl Plugin for ShotsPlugin {
    fn build(&self, app: &mut App) {
        let only: Option<Vec<String>> = std::env::var("CITYSIM_UI_SHOTS_ONLY")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.split(',').map(|p| p.trim().to_string()).collect());
        let steps: Vec<Step> = steps()
            .into_iter()
            .filter(|s| only.as_ref().is_none_or(|o| o.iter().any(|n| n == s.name)))
            .collect();
        app.insert_resource(Runner {
            dir: self.dir.clone(),
            steps,
            index: 0,
            t: 0.0,
            entered: false,
            shot: false,
            frames: 0,
        })
        .init_resource::<Pointer>()
        .init_resource::<SavedWorld>()
        .add_systems(
            Update,
            run_shots.in_set(GameSet::Input).before(super::widgets::button_clicks),
        )
        .add_systems(
            Update,
            drive_pointer
                .in_set(GameSet::Input)
                .after(run_shots)
                .before(super::widgets::button_clicks),
        );
    }
}

/// Where the synthetic pointer is, whether the left button is held, and
/// tool state forced every frame.
#[derive(Resource, Default, Clone)]
struct Pointer {
    target: PointerTarget,
    hold: bool,
    /// Glide towards the target instead of jumping (tour mode).
    smooth: bool,
    current: Option<Vec2>,
    /// A road being drawn (re-applied every frame: a tool change resets it).
    draft: Option<Draft>,
    /// A held press (drags).
    press: Option<Press>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum PointerTarget {
    #[default]
    None,
    Ground([f32; 2]),
    Button(UiAction),
    /// A point stored in `TourData`.
    Data(usize),
}

type Setup = fn(&mut World);

struct Step {
    name: &'static str,
    /// Seconds to let the state settle before the capture.
    wait: f32,
    setup: Setup,
}

#[derive(Resource)]
struct Runner {
    dir: PathBuf,
    steps: Vec<Step>,
    index: usize,
    t: f32,
    entered: bool,
    shot: bool,
    frames: u32,
}

fn sim(w: &World) -> &citysim_sim::World {
    &w.resource::<SimWorld>().world
}

/// Middle of the town (mean of the buildings), or the highway's end.
fn town_focus(w: &World) -> [f32; 2] {
    let s = sim(w);
    if s.buildings.is_empty() {
        return [150.0, 1028.0];
    }
    let n = s.buildings.len() as f32;
    let (x, y) = s
        .buildings
        .values()
        .fold((0.0, 0.0), |a, b| (a.0 + b.center()[0], a.1 + b.center()[1]));
    [x / n, y / n]
}

fn pose_at(p: [f32; 2], distance: f32) -> CameraPose {
    CameraPose {
        focus: sim_to_world(p, 0.0),
        yaw: 0.35,
        pitch: 0.92,
        distance,
    }
}

fn look(w: &mut World, p: [f32; 2], distance: f32) {
    w.resource_mut::<CameraOverride>().0 = Some(pose_at(p, distance));
}

fn point(w: &mut World, p: Option<[f32; 2]>) {
    if let Some(p) = p {
        w.resource_mut::<Pointer>().target = PointerTarget::Ground(p);
    }
}

fn reset(world: &mut World) {
    if let Some(saved) = world.resource_mut::<SavedWorld>().0.take() {
        world.resource_mut::<SimWorld>().replace(saved);
    }
    let f = town_focus(world);
    look(world, f, 380.0);
    *world.resource_mut::<ActiveTool>() = ActiveTool::Inspect;
    *world.resource_mut::<OverlayMode>() = OverlayMode::None;
    world.resource_mut::<ToolState>().reset();
    *world.resource_mut::<ToolModes>() = ToolModes::default();
    *world.resource_mut::<RoadChoice>() = RoadChoice::Street;
    world.resource_mut::<WorldHighlight>().selected = None;
    let mut ui = world.resource_mut::<UiState>();
    ui.tray = None;
    ui.window = None;
    ui.before_demolish = None;
    ui.hint_visible = false;
    ui.menu_open = false;
    ui.settings_open = false;
    ui.keys_open = false;
    ui.following = false;
    // Directly (a ClosePeople message would also drop a selection the
    // step makes this frame).
    world.resource_mut::<super::panel::PeopleView>().stack.clear();
    world.resource_mut::<SimClock>().paused = false;
    world.resource_mut::<super::top::PinnedAlert>().0 = None;
    world.resource_mut::<super::loading::HoldLoading>().0 = false;
    *world.resource_mut::<Pointer>() = Pointer::default();
}

/// The building nearest to the town's middle that matches `pred`.
fn building_where(w: &World, pred: impl Fn(&Building) -> bool) -> Option<BuildingId> {
    let f = town_focus(w);
    let s = sim(w);
    s.buildings
        .iter()
        .filter(|(_, b)| pred(b))
        .min_by(|a, b| dist(a.1.center(), f).total_cmp(&dist(b.1.center(), f)))
        .map(|(id, _)| id)
}

fn building_pos(w: &World, id: Option<BuildingId>) -> Option<[f32; 2]> {
    Some(sim(w).buildings.get(id?)?.center())
}

/// Open land (no road within `half` metres, no buildings) near the town:
/// its centre.
fn open_land(w: &World, half: f32) -> Option<[f32; 2]> {
    let s = sim(w);
    let f = town_focus(w);
    for k in 0..60 {
        let r = 100.0 + k as f32 * 20.0;
        for a in 0..24 {
            let d = rotate([r, 0.0], a as f32 * 15.0);
            let p = [f[0] + d[0], f[1] + d[1]];
            let edge = citysim_sim::MAP_SIZE_M - half - 20.0;
            if p[0] < half + 20.0 || p[1] < half + 20.0 || p[0] > edge || p[1] > edge {
                continue;
            }
            if s.roads.nearest_segment(p, half).is_some() {
                continue;
            }
            if !s.buildings.values().any(|b| dist(b.center(), p) < half) {
                return Some(p);
            }
        }
    }
    None
}

/// A point in the middle of a long street near the town's middle (curved
/// when `curved`), and its direction.
fn street_spot(w: &World, curved: bool) -> Option<([f32; 2], [f32; 2])> {
    let s = sim(w);
    let f = town_focus(w);
    s.roads
        .segments
        .values()
        .filter(|g| g.kind != RoadKind::Highway && g.length > 60.0 && g.curve.is_straight() != curved)
        .min_by(|a, b| {
            let ma = a.curve.point_at(a.length * 0.5, 0.0);
            let mb = b.curve.point_at(b.length * 0.5, 0.0);
            dist(ma, f).total_cmp(&dist(mb, f))
        })
        .map(|g| {
            let p = g.curve.pose_at(g.length * 0.5, 0.0);
            (p.pos, p.dir)
        })
}

/// Centre and half-size of a set of points.
fn extent(pts: &[[f32; 2]]) -> ([f32; 2], f32) {
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in pts {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    (c, ((hi[0] - lo[0]).max(hi[1] - lo[1]) * 0.5).max(1.0))
}

fn add(a: [f32; 2], b: [f32; 2], k: f32) -> [f32; 2] {
    [a[0] + b[0] * k, a[1] + b[1] * k]
}

/// A gently curved street built across open land (kept for `reset`).
fn new_curve(w: &mut World) -> Option<Curve> {
    let o = open_land(w, 130.0)?;
    let c = Curve::through_corner(
        add(o, [-1.0, 0.3], 120.0),
        add(o, [0.2, -1.0], 30.0),
        add(o, [1.0, 0.5], 120.0),
    );
    build_now(
        w,
        &[Command::BuildCurve {
            curve: c,
            kind: RoadKind::Street,
        }],
    );
    Some(c)
}

/// Keep the scenario's world for `reset`, then apply `cmds` to the live
/// one (with money to spare).
fn build_now(w: &mut World, cmds: &[Command]) {
    stash_world(w);
    let mut sim = w.resource_mut::<SimWorld>();
    let money = sim.world.economy.money;
    sim.world.economy.money = money.max(0) + 1_000_000;
    for c in cmds {
        let _ = sim.world.apply(c.clone());
    }
    sim.world.economy.money = money;
}

/// Set the road tool and a mode, then force a road being drawn.
fn drawing(w: &mut World, mode: RoadMode, shape: Shape, points: &[[f32; 2]], pieces: &[Curve]) {
    *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
    w.resource_mut::<ToolModes>().road = mode;
    let block_m = w.resource::<super::tools::ToolOptions>().block_m();
    let mut d = Draft::new(shape, block_m);
    d.pieces = pieces.to_vec();
    d.points = points.to_vec();
    if shape == Shape::Continuous {
        d.start_dir = pieces.last().map(|c| c.end_dir());
    }
    w.resource_mut::<Pointer>().draft = Some(d);
}

/// Hold the button down from `at` (a drag in progress).
fn press(w: &mut World, at: [f32; 2]) {
    let lattice = sim(w).zoning.cell_at(at).map_or(LatticeId::WORLD, |c| c.lattice);
    let mut p = w.resource_mut::<Pointer>();
    p.press = Some(Press {
        at,
        time: 0.0,
        moved: true,
        lattice,
    });
    p.hold = true;
}

/// The owner's chosen mockup in the real game: a curved avenue being
/// drawn while a driving resident is shown in the side panel.
fn mockup(w: &mut World) {
    reset(w);
    // A driver near open land, so the road being drawn has room.
    let spot = open_land(w, 45.0).unwrap_or_else(|| town_focus(w));
    let s = sim(w);
    let who = s
        .citizens
        .iter()
        .filter_map(|(id, c)| match &c.location {
            citysim_sim::citizens::Location::Travelling(j)
                if c.job.is_some() && j.vehicle.is_some_and(|v| s.vehicles.contains_key(v)) =>
            {
                let at = super::people::person_position(s, id)?;
                Some((dist(at, spot), id, at))
            }
            _ => None,
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let focus = match who {
        Some((_, id, at)) => {
            w.write_message(UiAction::Person(id));
            at
        }
        None => spot,
    };
    *w.resource_mut::<RoadChoice>() = RoadChoice::Avenue;
    // Screen axes of the shot camera (yaw 0.35): right and down, sim metres.
    let (s35, c35) = 0.35f32.sin_cos();
    let right = [c35, -s35];
    let down = [s35, c35];
    let at = |x: f32, y: f32| add(add(focus, right, x), down, y);
    // A curve across the open land, left of the side panel and above the
    // tray, ending at the cursor.
    let rel = |p: [f32; 2]| {
        let d = [p[0] - focus[0], p[1] - focus[1]];
        (d[0] * right[0] + d[1] * right[1], d[0] * down[0] + d[1] * down[1])
    };
    let (sx, sy) = rel(spot);
    let a = at(sx - 40.0, sy - 10.0);
    let corner = at(sx + 5.0, sy - 12.0);
    drawing(w, RoadMode::Curve, Shape::Curve, &[a, corner], &[]);
    point(w, Some(at(sx + 22.0, sy + 18.0)));
    // The car right of the middle of the screen, held still; the ribbon
    // says what the hour is like (the clock is paused for the picture).
    look(w, at(sx - 22.0, sy - 16.0), 190.0);
    w.resource_mut::<SimClock>().paused = true;
    let rush = super::top::Alert {
        kind: super::top::AlertKind::Warning,
        text: "Evening rush is building on the avenue".into(),
    };
    w.resource_mut::<super::top::PinnedAlert>().0 = Some(rush);
}

fn steps() -> Vec<Step> {
    vec![
        Step {
            name: "idle",
            wait: 6.0,
            setup: reset,
        },
        Step {
            name: "mockup",
            wait: 2.0,
            setup: mockup,
        },
        Step {
            name: "maps",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<OverlayMode>() = OverlayMode::Traffic;
                w.resource_mut::<UiState>().tray = Some(InfoTray::Maps);
            },
        },
        Step {
            name: "stats",
            wait: 1.2,
            setup: |w| {
                reset(w);
                w.resource_mut::<UiState>().window = Some(CityPage::Stats);
            },
        },
        Step {
            name: "keys",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::OpenMenu);
                w.write_message(UiAction::Keys);
            },
        },
        Step {
            name: "inspect-junction",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let s = sim(w);
                let f = town_focus(w);
                let node = s
                    .roads
                    .nodes
                    .iter()
                    .filter(|(_, n)| n.segments.len() >= 3)
                    .min_by(|a, b| dist(a.1.pos, f).total_cmp(&dist(b.1.pos, f)))
                    .map(|(id, n)| (id, n.pos));
                if let Some((node, pos)) = node {
                    w.resource_mut::<WorldHighlight>().selected =
                        Some(crate::shared::WorldTarget::Junction { node, pos });
                    look(w, pos, 160.0);
                }
            },
        },
        Step {
            name: "inspect-home",
            wait: 1.0,
            setup: |w| {
                reset(w);
                let b = building_where(w, |b| b.kind == Zone::Residential && b.level > 1 && b.occupied > 0);
                let p = building_pos(w, b);
                point(w, p);
            },
        },
        Step {
            name: "inspect-shop",
            wait: 1.0,
            setup: |w| {
                reset(w);
                let p = building_pos(w, building_where(w, |b| b.kind == Zone::Commercial));
                point(w, p);
            },
        },
        Step {
            name: "inspect-cutoff",
            wait: 1.0,
            setup: |w| {
                reset(w);
                stash_world(w);
                // The sim's own countdown for a home found cut off from the
                // network 7 in-game hours ago (paused, so the hourly check
                // does not clear it again).
                w.resource_mut::<SimClock>().paused = true;
                let b = building_where(w, |b| b.kind == Zone::Residential && b.occupied > 0);
                let p = building_pos(w, b);
                let world = &mut w.resource_mut::<SimWorld>().into_inner().world;
                if let Some(id) = b {
                    let hour = citysim_sim::balance::TICKS_PER_HOUR as u64;
                    let since = (world.time.tick / hour).saturating_sub(7) * hour;
                    world.growth.meta.entry(id).unreachable_since = Some(since);
                }
                point(w, p);
            },
        },
        Step {
            name: "inspect-road",
            wait: 1.0,
            setup: |w| {
                reset(w);
                let p = street_spot(w, true).map(|s| s.0);
                point(w, p);
            },
        },
        Step {
            name: "inspect-car",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<SimClock>().paused = true;
                let f = town_focus(w);
                let s = sim(w);
                let car = s
                    .vehicles
                    .values()
                    .filter(|v| v.segment.is_some())
                    .min_by(|a, b| dist(a.pos, f).total_cmp(&dist(b.pos, f)))
                    .map(|v| citysim_sim::drive::vehicle_pose(s, v).0);
                if let Some(c) = car {
                    look(w, c, 120.0);
                }
                point(w, car);
            },
        },
        Step {
            name: "tooltip",
            wait: 1.2,
            setup: |w| {
                reset(w);
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Tool(ActiveTool::Road));
            },
        },
        Step {
            name: "road-straight",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some((p, d)) = street_spot(w, false) else { return };
                let r = right_of(d);
                let a = add(p, d, 12.0);
                drawing(w, RoadMode::Straight, Shape::Straight, &[a], &[]);
                // Nearly at right angles: the snap pulls it square.
                point(w, Some(add(add(a, r, 92.0), d, 5.0)));
                look(w, add(a, r, 45.0), 220.0);
            },
        },
        Step {
            name: "road-curve",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(o) = open_land(w, 150.0) else { return };
                // Start, corner placed; the end near the right angle from
                // the corner, so its guide shows.
                let a = add(o, [-1.0, 0.0], 130.0);
                let corner = add(o, [0.0, -1.0], 40.0);
                let d = super::fsnap::rotate(citysim_sim::curve::unit([corner[0] - a[0], corner[1] - a[1]]), 90.0);
                drawing(w, RoadMode::Curve, Shape::Curve, &[a, corner], &[]);
                *w.resource_mut::<RoadChoice>() = RoadChoice::Avenue;
                point(w, Some(add(add(corner, d, 110.0), [d[1], -d[0]], 3.0)));
                look(w, add(o, [-1.0, 0.0], 30.0), 380.0);
            },
        },
        Step {
            name: "road-continuous",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(o) = open_land(w, 140.0) else { return };
                let a = add(o, [-1.0, 0.0], 120.0);
                let c1 = Curve::continuing(a, [1.0, 0.3], add(o, [-0.3, -1.0], 30.0));
                let c2 = Curve::continuing(c1.end(), c1.end_dir(), add(o, [1.0, -0.2], 70.0));
                drawing(w, RoadMode::Continuous, Shape::Continuous, &[c2.end()], &[c1, c2]);
                // Near straight on from the last piece: its guide shows.
                let dir = c2.end_dir();
                point(w, Some(add(add(c2.end(), dir, 72.0), right_of(dir), 3.0)));
                look(w, add(o, dir, 30.0), 360.0);
            },
        },
        Step {
            name: "road-grid",
            wait: 1.4,
            setup: |w| {
                reset(w);
                let Some(o) = open_land(w, 150.0) else { return };
                // A grid turned by 27°.
                let u = rotate([1.0, 0.0], 27.0);
                let a = add(add(o, u, -100.0), right_of(u), -60.0);
                let b = add(a, u, 200.0);
                drawing(w, RoadMode::Grid, Shape::Grid, &[a, b], &[]);
                point(w, Some(add(b, right_of(u), 125.0)));
                look(w, o, 380.0);
            },
        },
        Step {
            name: "road-refused-tight",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(o) = open_land(w, 120.0) else { return };
                *w.resource_mut::<RoadChoice>() = RoadChoice::Avenue;
                let a = add(o, [-1.0, 0.0], 70.0);
                let corner = add(o, [1.0, 0.0], 10.0);
                drawing(w, RoadMode::Curve, Shape::Curve, &[a, corner], &[]);
                point(w, Some(add(corner, [-0.6, 1.0], 26.0)));
                look(w, o, 220.0);
            },
        },
        Step {
            name: "road-refused-close",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some((p, d)) = street_spot(w, false) else { return };
                let r = right_of(d);
                // Alongside the street, 5 m off its centre line.
                let a = add(add(p, d, -50.0), r, 5.0);
                drawing(w, RoadMode::Straight, Shape::Straight, &[a], &[]);
                point(w, Some(add(add(p, d, 40.0), r, 5.0)));
                look(w, p, 200.0);
            },
        },
        Step {
            name: "road-broke",
            wait: 1.2,
            setup: |w| {
                reset(w);
                stash_world(w);
                let Some((p, d)) = street_spot(w, false) else { return };
                w.resource_mut::<SimWorld>().world.economy.money = -420;
                let r = right_of(d);
                drawing(w, RoadMode::Straight, Shape::Straight, &[p], &[]);
                point(w, Some(add(p, r, 80.0)));
                look(w, add(p, r, 40.0), 220.0);
            },
        },
        Step {
            name: "road-upgrade",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(c) = new_curve(w) else { return };
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                w.resource_mut::<ToolModes>().road = RoadMode::Upgrade;
                *w.resource_mut::<RoadChoice>() = RoadChoice::Avenue;
                let len = c.length();
                press(w, c.point_at(len * 0.25, 0.0));
                point(w, Some(c.point_at(len * 0.8, 0.0)));
                look(w, c.point_at(len * 0.5, 0.0), 300.0);
            },
        },
        Step {
            name: "junction-tool",
            wait: 1.5,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                w.resource_mut::<ToolModes>().road = RoadMode::Junction;
                let f = town_focus(w);
                let j = super::tools::junction_near(sim(w), f, 2000.0).map(|j| j.1);
                if let Some(j) = j {
                    look(w, j, 160.0);
                }
                point(w, j);
            },
        },
        Step {
            name: "zone-paint",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some((p, d)) = street_spot(w, true) else { return };
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Residential));
                w.resource_mut::<ToolModes>().zone = ZoneMode::Paint;
                let r = right_of(d);
                let cells: Vec<_> = (0..12)
                    .filter_map(|k| sim(w).zoning.cell_at(add(add(p, d, -30.0 + k as f32 * 5.0), r, 10.0)))
                    .collect();
                press(w, add(p, r, 10.0));
                w.resource_mut::<ToolState>().stroke = cells;
                point(w, Some(add(add(p, d, 30.0), r, 10.0)));
                look(w, p, 160.0);
            },
        },
        Step {
            name: "zone-frontage-curve",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(c) = new_curve(w) else { return };
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Residential));
                w.resource_mut::<ToolModes>().zone = ZoneMode::Frontage;
                let mid = c.pose_at(c.length() * 0.5, 0.0);
                point(w, Some(add(mid.pos, right_of(mid.dir), 10.0)));
                look(w, mid.pos, 300.0);
            },
        },
        Step {
            name: "zone-fill",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some((p, d)) = street_spot(w, false) else { return };
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Industrial));
                w.resource_mut::<ToolModes>().zone = ZoneMode::Fill;
                point(w, Some(add(p, right_of(d), 12.0)));
                look(w, p, 260.0);
            },
        },
        Step {
            name: "zone-marquee",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(c) = new_curve(w) else { return };
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Commercial));
                w.resource_mut::<ToolModes>().zone = ZoneMode::Marquee;
                // Along the straight start of the new road, on its left.
                let p0 = c.pose_at(6.0, 0.0);
                let p1 = c.pose_at(60.0, 0.0);
                press(w, add(p0.pos, right_of(p0.dir), -6.0));
                point(w, Some(add(p1.pos, right_of(p1.dir), -18.0)));
                look(w, p1.pos, 220.0);
            },
        },
        Step {
            name: "dezone-frontage",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some((p, d)) = street_spot(w, true) else { return };
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(None);
                w.resource_mut::<ToolModes>().zone = ZoneMode::Frontage;
                point(w, Some(add(p, right_of(d), -10.0)));
                look(w, p, 260.0);
            },
        },
        Step {
            name: "bulldoze-road-part",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Bulldoze;
                w.resource_mut::<ToolModes>().bulldoze = BulldozeMode::Road;
                let Some((p, d)) = street_spot(w, false) else { return };
                press(w, add(p, d, -25.0));
                point(w, Some(add(p, d, 25.0)));
                look(w, p, 200.0);
            },
        },
        Step {
            name: "bulldoze-area",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Bulldoze;
                w.resource_mut::<ToolModes>().bulldoze = BulldozeMode::Area;
                let Some(b) = building_pos(w, building_where(w, |_| true)) else {
                    return;
                };
                press(w, add(b, [-1.0, -1.0], 18.0));
                point(w, Some(add(b, [1.0, 1.0], 14.0)));
                look(w, b, 180.0);
            },
        },
        Step {
            name: "demolish-switch",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Demolish);
            },
        },
        Step {
            name: "demolish-on",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Bulldoze;
                w.resource_mut::<ToolModes>().bulldoze = BulldozeMode::Road;
                let Some((p, d)) = street_spot(w, true) else { return };
                point(w, Some(p));
                look(w, add(p, d, 20.0), 240.0);
            },
        },
        Step {
            name: "demolish-building",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Bulldoze;
                w.resource_mut::<ToolModes>().bulldoze = BulldozeMode::Road;
                let Some(b) = building_pos(w, building_where(w, |b| b.kind == Zone::Residential && b.occupied >= 3))
                else {
                    return;
                };
                point(w, Some(b));
                look(w, b, 170.0);
            },
        },
        Step {
            name: "window-stats",
            wait: 1.5,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::Stats);
            },
        },
        Step {
            name: "window-people",
            wait: 1.5,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::CityPeople);
            },
        },
        Step {
            name: "window-budget",
            wait: 1.5,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::ToggleBudget);
                // A column of the day-by-day chart read out.
                w.resource_mut::<Pointer>().target = PointerTarget::None;
            },
        },
        Step {
            name: "route-car",
            wait: 2.0,
            setup: |w| {
                reset(w);
                // A car with a long way still to go, near the middle.
                let s = sim(w);
                let f = town_focus(w);
                let car = s
                    .vehicles
                    .keys()
                    .filter_map(|id| {
                        let pts = super::route::vehicle_path(s, id)?;
                        let len = super::route::path_length(&pts);
                        (len > 250.0).then(|| (id, pts))
                    })
                    .min_by(|a, b| dist(a.1[0], f).total_cmp(&dist(b.1[0], f)));
                w.resource_mut::<SimClock>().paused = true;
                if let Some((id, pts)) = car {
                    w.write_message(UiAction::Vehicle(id));
                    let (c, r) = extent(&pts);
                    look(w, c, (r * 2.2).clamp(160.0, 900.0));
                }
            },
        },
        Step {
            name: "route-person-home",
            wait: 2.0,
            setup: |w| {
                reset(w);
                // Someone at home whose next trip is known.
                let s = sim(w);
                let who = s
                    .citizens
                    .iter()
                    .filter(|(_, c)| c.job.is_some())
                    .filter_map(|(id, _)| {
                        let t = super::route::person_trip(s, id)?;
                        (t.state == crate::shared::RouteState::Planned && t.points.len() > 4).then_some((id, t.points))
                    })
                    .nth(2);
                w.resource_mut::<SimClock>().paused = true;
                if let Some((id, pts)) = who {
                    w.write_message(UiAction::Person(id));
                    let (c, r) = extent(&pts);
                    look(w, c, (r * 2.2).clamp(160.0, 900.0));
                }
            },
        },
        Step {
            name: "inspect-pinned",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let Some((p, _)) = street_spot(w, false) else { return };
                let t = super::tools::target_at(sim(w), p, 6.0);
                w.resource_mut::<WorldHighlight>().selected = t;
                look(w, p, 220.0);
            },
        },
        Step {
            name: "people-home",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let Some(id) = building_where(w, |b| b.kind == Zone::Residential && b.occupied >= 3) else {
                    return;
                };
                w.write_message(UiAction::Place(id));
                let p = building_pos(w, Some(id)).unwrap_or_default();
                look(w, p, 160.0);
            },
        },
        Step {
            name: "people-workplace",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let Some(id) = building_where(w, |b| b.kind != Zone::Residential && b.occupied >= 4) else {
                    return;
                };
                w.write_message(UiAction::Place(id));
                let p = building_pos(w, Some(id)).unwrap_or_default();
                look(w, p, 160.0);
            },
        },
        Step {
            name: "people-person",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let s = sim(w);
                let who = s
                    .citizens
                    .iter()
                    .filter(|(_, c)| c.job.is_some() && c.journal.len() >= 2)
                    .map(|(id, _)| id)
                    .next();
                if let Some(id) = who {
                    w.write_message(UiAction::Person(id));
                }
            },
        },
        Step {
            name: "people-follow-car",
            wait: 2.0,
            setup: |w| {
                reset(w);
                // Someone driving with the longest way still to go.
                let s = sim(w);
                let now = s.time.tick;
                let who = s
                    .citizens
                    .iter()
                    .filter_map(|(id, c)| match &c.location {
                        citysim_sim::citizens::Location::Travelling(j)
                            if j.vehicle.is_some_and(|v| s.vehicles.contains_key(v)) =>
                        {
                            Some((j.due_tick.saturating_sub(now), id))
                        }
                        _ => None,
                    })
                    .max_by_key(|x| x.0)
                    .map(|x| x.1);
                if let Some(id) = who {
                    w.write_message(UiAction::Person(id));
                    w.write_message(UiAction::Follow);
                    let at = super::people::person_position(sim(w), id).unwrap_or_default();
                    w.write_message(crate::shared::CameraGoTo {
                        focus: at,
                        distance: Some(110.0),
                    });
                }
                w.resource_mut::<CameraOverride>().0 = None;
                // Hold the moment: people get out on the calendar long
                // before their car finishes its drive.
                w.resource_mut::<SimClock>().paused = true;
            },
        },
        Step {
            name: "people-car",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let s = sim(w);
                let f = town_focus(w);
                let car = s
                    .vehicles
                    .iter()
                    .filter(|(id, v)| {
                        v.segment.is_some() && citysim_sim::inspect::vehicle(s, *id).is_some_and(|x| x.occupants >= 2)
                    })
                    .min_by(|a, b| dist(a.1.pos, f).total_cmp(&dist(b.1.pos, f)))
                    .map(|(id, v)| (id, citysim_sim::drive::vehicle_pose(s, v).0));
                w.resource_mut::<SimClock>().paused = true;
                if let Some((id, at)) = car {
                    w.write_message(UiAction::Vehicle(id));
                    look(w, add(at, [1.0, 0.0], 30.0), 140.0);
                }
            },
        },
        Step {
            name: "people-city",
            wait: 1.5,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::CityPeople);
            },
        },
        Step {
            name: "fiscal-deficit",
            wait: 1.2,
            setup: |w| {
                reset(w);
                stash_world(w);
                let mut sim = w.resource_mut::<SimWorld>();
                let f = &mut sim.world.economy.fiscal;
                f.state = citysim_sim::FiscalState::Deficit;
                f.reason = citysim_sim::FiscalReason::RunningAtALoss;
                f.days_in_deficit = 2;
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::ToggleBudget);
            },
        },
        Step {
            name: "fiscal-crisis",
            wait: 1.5,
            setup: |w| {
                reset(w);
                stash_world(w);
                let mut sim = w.resource_mut::<SimWorld>();
                sim.world.economy.money = -2_400;
                let f = &mut sim.world.economy.fiscal;
                f.state = citysim_sim::FiscalState::Crisis;
                f.reason = citysim_sim::FiscalReason::ResidentsLeaving;
                f.service_level = 0.55;
                f.days_in_debt = 4;
                f.left_last_night = 23;
                w.resource_mut::<UiState>().window = Some(CityPage::Budget);
            },
        },
        Step {
            name: "settings",
            wait: 1.2,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::OpenMenu);
                w.write_message(UiAction::OpenSettings);
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::UiScale(1));
            },
        },
        Step {
            name: "traffic-overlay",
            wait: 1.5,
            setup: |w| {
                reset(w);
                *w.resource_mut::<OverlayMode>() = OverlayMode::Traffic;
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Overlay(OverlayMode::Traffic));
            },
        },
        Step {
            name: "budget",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<UiState>().window = Some(CityPage::Budget);
            },
        },
        Step {
            name: "budget-day1",
            wait: 1.0,
            setup: |w| {
                reset(w);
                stash_world(w);
                // A fresh city half a day into day 1: one street, homes and
                // shops zoned beside it.
                let mut fresh = citysim_sim::World::new(1);
                let _ = fresh.apply(Command::BuildCurve {
                    curve: Curve::straight([92.0, 1028.0], [320.0, 1028.0]),
                    kind: RoadKind::Street,
                });
                let _ = fresh.apply(Command::ZoneArea {
                    quad: [[110.0, 1008.0], [310.0, 1008.0], [310.0, 1024.0], [110.0, 1024.0]],
                    zone: Some(Zone::Residential),
                });
                let _ = fresh.apply(Command::ZoneArea {
                    quad: [[110.0, 1032.0], [250.0, 1032.0], [250.0, 1048.0], [110.0, 1048.0]],
                    zone: Some(Zone::Commercial),
                });
                for _ in 0..citysim_sim::balance::TICKS_PER_DAY / 2 {
                    fresh.step();
                }
                w.resource_mut::<SimWorld>().replace(fresh);
                w.resource_mut::<UiState>().window = Some(CityPage::Budget);
            },
        },
        Step {
            name: "toasts",
            wait: 0.8,
            setup: |w| {
                reset(w);
                w.write_message(Toast::new(ToastKind::Success, "City saved"));
                // A real rejected command: a road through a building.
                if let Some(b) = building_pos(w, building_where(w, |_| true)) {
                    w.write_message(SimCommand(Command::BuildCurve {
                        curve: Curve::straight(add(b, [0.0, -1.0], 30.0), add(b, [0.0, 1.0], 30.0)),
                        kind: RoadKind::Street,
                    }));
                }
            },
        },
        Step {
            name: "save-load",
            wait: 1.2,
            setup: |w| {
                reset(w);
                // Only into a scratch slot: never over the player's save.
                if std::env::var_os("CITYSIM_SAVE_DIR").is_some_and(|d| !d.is_empty()) {
                    w.write_message(UiAction::Save);
                    w.write_message(UiAction::Load);
                }
            },
        },
        Step {
            name: "pause-menu",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::OpenMenu);
                // Keyboard focus on "Load game", the pointer over "Save game".
                w.resource_mut::<super::menu::MenuFocus>().0 = Some(2);
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Save);
            },
        },
        Step {
            name: "loading",
            wait: 1.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<super::loading::HoldLoading>().0 = true;
            },
        },
        Step {
            name: "first-run",
            wait: 1.0,
            setup: |w| {
                reset(w);
                stash_world(w);
                w.resource_mut::<SimWorld>().replace(citysim_sim::World::new(1));
                w.resource_mut::<UiState>().hint_visible = true;
                w.resource_mut::<SimClock>().paused = true;
            },
        },
        Step {
            name: "1440p",
            wait: 3.0,
            setup: |w| {
                reset(w);
                w.resource_mut::<UiState>().window = Some(CityPage::Budget);
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::ToggleBudget);
                if let Ok(mut win) = w.query_filtered::<&mut Window, With<PrimaryWindow>>().single_mut(w) {
                    win.resolution.set(2560.0, 1440.0);
                }
            },
        },
        Step {
            name: "1440p-road",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let Some((p, d)) = street_spot(w, false) else { return };
                let r = right_of(d);
                let a = add(p, d, 12.0);
                let corner = add(a, r, 100.0);
                drawing(w, RoadMode::Curve, Shape::Curve, &[a, corner], &[]);
                point(w, Some(add(corner, d, 90.0)));
                look(w, corner, 300.0);
            },
        },
        Step {
            name: "1440p-person",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let s = sim(w);
                let who = s
                    .citizens
                    .iter()
                    .filter(|(_, c)| c.job.is_some())
                    .map(|(id, _)| id)
                    .nth(3);
                if let Some(id) = who {
                    w.write_message(UiAction::Person(id));
                }
            },
        },
    ]
}

fn run_shots(world: &mut World) {
    let dt = world.resource::<Time<Real>>().delta_secs().min(0.25);
    let mut r = world.remove_resource::<Runner>().expect("runner");
    r.frames += 1;
    if r.frames == 1
        && let Some((x, y)) = std::env::var("CITYSIM_SHOT_SIZE").ok().and_then(|s| {
            let (a, b) = s.split_once('x')?;
            Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
        })
        && let Ok(mut win) = world
            .query_filtered::<&mut Window, With<PrimaryWindow>>()
            .single_mut(world)
    {
        // `CITYSIM_SHOT_SIZE=1920x1080`: the window size for the shots.
        win.resolution.set_physical_resolution(x, y);
    }
    // Count from the moment the scene really draws.
    if r.frames > 3 && world.resource::<crate::shared::SceneReady>().0 {
        r.t += dt;
    }
    if r.index >= r.steps.len() {
        if r.t > 1.0 {
            info!("ui shots done");
            world.write_message(AppExit::Success);
        }
        world.insert_resource(r);
        return;
    }
    let step = &r.steps[r.index];
    if !r.entered {
        r.entered = true;
        r.t = 0.0;
        (step.setup)(world);
    } else if !r.shot && r.t >= step.wait {
        r.shot = true;
        let path = r.dir.join(format!("{:02}-{}.png", r.index + 1, step.name));
        info!("ui shot {}", path.display());
        world.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
    } else if r.shot && r.t >= step.wait + 0.6 {
        r.index += 1;
        r.entered = false;
        r.shot = false;
        r.t = 0.0;
    }
    world.insert_resource(r);
}

#[allow(clippy::too_many_arguments)]
fn drive_pointer(
    time: Res<Time<Real>>,
    mut pointer: ResMut<Pointer>,
    data: Option<Res<TourData>>,
    mut tools: ResMut<ToolState>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    cams: Query<(&Camera, &GlobalTransform), With<WorldCamera>>,
    buttons: Query<(&UiButton, &UiGlobalTransform, &InheritedVisibility)>,
) {
    if let Some(d) = &pointer.draft
        && (tools.draft.points != d.points || tools.draft.pieces != d.pieces)
    {
        tools.draft = d.clone();
    }
    let forced_press = pointer.press.is_some();
    if let Some(p) = pointer.press
        && tools.press != Some(p)
    {
        tools.press = Some(p);
    }
    let Ok(mut win) = window.single_mut() else { return };
    let ground = |p: [f32; 2]| {
        cams.single()
            .ok()
            .and_then(|(cam, tf)| cam.world_to_viewport(tf, sim_to_world(p, 0.0)).ok())
    };
    let target = match pointer.target {
        // Park the pointer over open land at the right, away from every
        // panel (X ignores warps outside the window).
        PointerTarget::None => Some(Vec2::new(win.width() * 0.86, win.height() * 0.42)),
        PointerTarget::Ground(p) => ground(p),
        PointerTarget::Data(i) => data.as_ref().and_then(|d| d.points.get(i).copied()).and_then(ground),
        PointerTarget::Button(a) => buttons
            .iter()
            .find(|(b, _, v)| b.action == a && v.get())
            .map(|(_, tf, _)| tf.translation / win.scale_factor()),
    };
    let pos = match (pointer.smooth, pointer.current, target) {
        (true, Some(cur), Some(t)) if t.x >= 0.0 => {
            let k = super::widgets::approach(time.delta_secs().min(0.1), 0.16);
            let next = cur + (t - cur) * k;
            Some(if next.distance(t) < 0.5 { t } else { next })
        }
        _ => target,
    };
    pointer.current = pos;
    let pos = pos.map(|p| p.round());
    if win.cursor_position().map(|p| p.round()) != pos {
        win.set_cursor_position(pos);
    }
    if pointer.hold {
        if !mouse.pressed(MouseButton::Left) {
            mouse.press(MouseButton::Left);
            if forced_press {
                // The press is forced above, not a click at the cursor.
                mouse.clear_just_pressed(MouseButton::Left);
            }
        }
    } else if mouse.pressed(MouseButton::Left) {
        mouse.release(MouseButton::Left);
    }
}

// ---------------------------------------------------------------- tour (video)

/// Points the tour computed from the live town.
#[derive(Resource, Default)]
struct TourData {
    points: Vec<[f32; 2]>,
    focus: Option<[f32; 2]>,
}

#[derive(Clone, Copy)]
enum Op {
    /// Glide to a target over about this many seconds (waits for it).
    Go(PointerTarget, f32),
    Click,
    Up,
    Key(KeyCode),
    Wait(f32),
    Run(Setup),
}

#[derive(Resource)]
struct Tour {
    ops: Vec<Op>,
    index: usize,
    t: f32,
    total: f32,
    release_keys: Vec<KeyCode>,
}

pub struct TourPlugin;

impl Plugin for TourPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Tour {
            ops: tour_ops(),
            index: 0,
            t: 0.0,
            total: 0.0,
            release_keys: Vec::new(),
        })
        .init_resource::<TourData>()
        .init_resource::<SavedWorld>()
        .insert_resource(Pointer {
            smooth: true,
            ..default()
        })
        .add_systems(
            Update,
            run_tour.in_set(GameSet::Input).before(super::widgets::button_clicks),
        )
        .add_systems(
            Update,
            drive_pointer
                .in_set(GameSet::Input)
                .after(run_tour)
                .before(super::widgets::button_clicks),
        );
    }
}

/// The tour's points: a curved road across open land (0 start, 1 corner,
/// 2 end), a point beside it to zone (3), a building (4).
fn tour_setup(w: &mut World) {
    reset(w);
    w.resource_mut::<Pointer>().smooth = true;
    let o = open_land(w, 130.0).unwrap_or([600.0, 600.0]);
    let a = add(o, [-1.0, 0.2], 110.0);
    let corner = add(o, [0.3, -1.0], 40.0);
    let end = add(o, [1.0, 0.4], 110.0);
    let c = Curve::through_corner(a, corner, end);
    let mid = c.pose_at(c.length() * 0.5, 0.0);
    let beside = add(mid.pos, right_of(mid.dir), 12.0);
    let house = building_pos(w, building_where(w, |b| b.kind == Zone::Residential)).unwrap_or(o);
    let mut data = w.resource_mut::<TourData>();
    data.points = vec![a, corner, end, beside, house];
    data.focus = Some(o);
}

fn tour_ops() -> Vec<Op> {
    use Op::*;
    use PointerTarget::{Button, Data};
    vec![
        Run(tour_setup),
        Wait(1.0),
        // A curved avenue across open land.
        Go(Button(UiAction::Tool(ActiveTool::Road)), 0.65),
        Click,
        Wait(0.3),
        Go(Button(UiAction::RoadMode(RoadMode::Curve)), 0.65),
        Click,
        Go(Button(UiAction::RoadKind(RoadChoice::Avenue)), 0.65),
        Click,
        Go(Data(0), 0.9),
        Click,
        Go(Data(1), 0.9),
        Click,
        Go(Data(2), 1.2),
        Wait(0.6),
        Click,
        Key(KeyCode::Enter),
        Wait(1.0),
        // Zone along it.
        Go(Button(UiAction::ZoneTool), 0.65),
        Click,
        Go(Button(UiAction::ZoneMode(ZoneMode::Frontage)), 0.65),
        Click,
        Go(Data(3), 1.0),
        Wait(1.0),
        Click,
        Wait(0.8),
        // A house and its people.
        Go(Button(UiAction::Tool(ActiveTool::Inspect)), 0.65),
        Click,
        Go(Data(4), 0.9),
        Wait(0.6),
        Click,
        Wait(2.0),
        Key(KeyCode::Escape),
        Wait(0.5),
        Go(Button(UiAction::CityPeople), 0.65),
        Click,
        Wait(2.0),
        Click,
        Wait(0.5),
    ]
}

fn run_tour(world: &mut World) {
    let dt = world.resource::<Time<Real>>().delta_secs().min(0.1);
    let mut tour = world.remove_resource::<Tour>().expect("tour");
    tour.total += dt;
    // Slow orbit for some life in the picture.
    if let Some(f) = world.resource::<TourData>().focus {
        let mut p = pose_at(f, 380.0);
        p.yaw += tour.total * 0.012;
        world.resource_mut::<CameraOverride>().0 = Some(p);
    }
    {
        let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
        for k in tour.release_keys.drain(..) {
            keys.release(k);
        }
    }
    tour.t += dt;
    let mut guard = 0;
    while let Some(&op) = tour.ops.get(tour.index) {
        guard += 1;
        let done = match op {
            Op::Run(f) => {
                f(world);
                true
            }
            Op::Wait(s) => tour.t >= s,
            Op::Go(target, s) => {
                world.resource_mut::<Pointer>().target = target;
                tour.t >= s
            }
            Op::Click => {
                world
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .press(MouseButton::Left);
                world.resource_mut::<Pointer>().hold = true;
                // Release next frame.
                tour.ops[tour.index] = Op::Up;
                tour.t = 0.0;
                break;
            }
            Op::Up => {
                world.resource_mut::<Pointer>().hold = false;
                true
            }
            Op::Key(k) => {
                world.resource_mut::<ButtonInput<KeyCode>>().press(k);
                tour.release_keys.push(k);
                true
            }
        };
        if !done {
            break;
        }
        tour.index += 1;
        tour.t = 0.0;
        // One input event per frame; a release never shares a frame with a move.
        if matches!(op, Op::Up) || guard > 1 && !matches!(op, Op::Run(_)) {
            break;
        }
    }
    if tour.index >= tour.ops.len() {
        info!("ui tour finished after {:.1} s", tour.total);
        world.write_message(AppExit::Success);
    }
    world.insert_resource(tour);
}
