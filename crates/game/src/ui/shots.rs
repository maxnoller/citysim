//! `CITYSIM_UI_SHOTS=dir` (best with `CITYSIM_DEMO=town`): walks through
//! every interface state and saves `dir/NN-name.png` for each, then exits.
//! States: idle HUD, inspect a home, a shop, an empty building and the
//! busiest road, a tooltip, road preview (valid, blocked, and while broke),
//! zone drag, bulldoze drag, round-2 tools (multi-point road path with
//! snapping, T-junction snap, avenue upgrade, block tool, frontage and fill
//! zoning, road bulldozer, junction tool), a pinned inspect card, the
//! deficit and debt-crisis states, settings, traffic overlay with its
//! legend, budget (with history and on day 1), toasts from real rejected
//! commands, pause menu, first-run hint, and the HUD, a road path and the
//! settings at 2560x1440.
//! `CITYSIM_UI_SHOTS_ONLY=name,name` limits it to some states.
//!
//! Drives the real ui through synthetic cursor / mouse input and resources.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::ui::UiGlobalTransform;
use bevy::window::PrimaryWindow;
use citysim_sim::{Building, Cell, Command, Occupant, Zone};

use super::UiState;
use super::toasts::{Toast, ToastKind};
use super::tools::{Action, ToolState};
use super::widgets::{UiAction, UiButton};
use crate::shared::{
    ActiveTool, BulldozeMode, CameraOverride, CameraPose, GameSet, OverlayMode, RoadChoice, RoadMode, SimClock,
    SimCommand, SimWorld, ToolModes, WorldCamera, WorldHighlight, WorldTarget, ZoneMode, cell_center,
};

/// The scenario's world while a step shows a modified copy (broke, day 1);
/// `reset` puts it back.
#[derive(Resource, Default)]
struct SavedWorld(Option<citysim_sim::World>);

/// Keep the current world for `reset` and return a mutable handle to it.
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

/// Where the synthetic pointer is and whether the left button is held.
#[derive(Resource, Default, Clone)]
struct Pointer {
    /// Road path corners to force with `Action::Path`.
    path: Vec<Cell>,
    target: PointerTarget,
    hold: bool,
    /// Glide towards the target instead of jumping (tour mode).
    smooth: bool,
    /// Current smoothed position.
    current: Option<Vec2>,
    /// Tool action to force (re-applied every frame: a tool change resets it).
    action: Option<Action>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum PointerTarget {
    #[default]
    None,
    Cell(Cell),
    Button(UiAction),
    /// A cell stored in `TourData`.
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

fn pose() -> CameraPose {
    CameraPose {
        focus: cell_center(Cell::new(48, 130), 0.0),
        yaw: 0.35,
        pitch: 0.92,
        distance: 360.0,
    }
}

fn reset(world: &mut World) {
    if let Some(saved) = world.resource_mut::<SavedWorld>().0.take() {
        world.resource_mut::<SimWorld>().replace(saved);
    }
    world.resource_mut::<CameraOverride>().0 = Some(pose());
    *world.resource_mut::<ActiveTool>() = ActiveTool::Inspect;
    *world.resource_mut::<OverlayMode>() = OverlayMode::None;
    world.resource_mut::<ToolState>().action = Action::Idle;
    world.resource_mut::<ToolState>().path.clear();
    *world.resource_mut::<ToolModes>() = ToolModes::default();
    *world.resource_mut::<RoadChoice>() = RoadChoice::Street;
    world.resource_mut::<WorldHighlight>().selected = None;
    let mut ui = world.resource_mut::<UiState>();
    ui.budget_open = false;
    ui.hint_visible = false;
    ui.menu_open = false;
    ui.settings_open = false;
    world.resource_mut::<SimClock>().paused = false;
    *world.resource_mut::<Pointer>() = Pointer::default();
}

/// A building cell (prefers one near the camera focus) and an empty cell
/// a few cells west of it on the same row.
fn find_building(world: &World) -> Option<Cell> {
    find_building_near(world, Cell::new(48, 130))
}

fn find_building_near(world: &World, focus: Cell) -> Option<Cell> {
    find_building_where(world, focus, |_| true)
}

/// Building nearest to `focus` that matches `pred`.
fn find_building_where(world: &World, focus: Cell, pred: impl Fn(&Building) -> bool) -> Option<Cell> {
    let sim = world.resource::<SimWorld>();
    sim.world
        .buildings
        .values()
        .filter(|b| pred(b))
        .map(|b| Cell::from_pos(b.center()))
        .min_by_key(|c| c.chebyshev(focus))
}

/// Force a road path in progress with these placed corners.
fn force_path(w: &mut World, corners: Vec<Cell>) {
    let mut p = w.resource_mut::<Pointer>();
    p.path = corners;
    p.action = Some(Action::Path {
        held: false,
        dragged: false,
    });
}

fn point_at(w: &mut World, c: Option<Cell>) {
    if let Some(c) = c {
        w.resource_mut::<Pointer>().target = PointerTarget::Cell(c);
    }
}

fn find_road(world: &World) -> Option<Cell> {
    find_road_near(world, Cell::new(44, 128))
}

fn find_road_near(world: &World, focus: Cell) -> Option<Cell> {
    let sim = &world.resource::<SimWorld>().world;
    let grid = sim.grid_view();
    let mut best: Option<(i32, Cell)> = None;
    for y in focus.y - 20..focus.y + 20 {
        for x in focus.x - 20..focus.x + 20 {
            let c = Cell::new(x, y);
            if grid.is_road(c)
                && sim.roads.segment_through(c.center()).is_some()
            {
                let d = c.chebyshev(focus);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, c));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

/// A buildable new road from an existing road cell across empty land.
fn find_new_road(world: &World) -> Option<(Cell, Cell)> {
    let sim = &world.resource::<SimWorld>().world;
    let focus = Cell::new(46, 126);
    let grid = sim.grid_view();
    let mut best: Option<(i32, Cell, Cell)> = None;
    for y in focus.y - 60..focus.y + 60 {
        for x in focus.x - 60..focus.x + 60 {
            let start = Cell::new(x, y);
            if !grid.is_road(start) {
                continue;
            }
            for (dx, dy) in [(1, 1), (1, -1), (0, 1), (1, 0)] {
                let end = start.offset(dx * 9, dy * 9);
                let cmd = citysim_sim::Command::BuildRoad {
                    from: start,
                    to: end,
                    kind: citysim_sim::RoadKind::Street,
                };
                // Only across unzoned empty land, so the demo town never
                // grows a building into it before the click.
                let clear = start.straight_line_to(end).is_some_and(|cells| {
                    cells[1..].iter().all(|&c| {
                        grid.get(c)
                            .is_some_and(|cd| cd.zone.is_none() && cd.occupant == Occupant::Empty)
                    })
                });
                if clear
                    && let Ok(p) = sim.preview(&cmd)
                    && p.cost >= 8 * citysim_sim::balance::ROAD_COST_PER_CELL
                {
                    let d = start.chebyshev(focus);
                    if best.is_none_or(|(bd, ..)| d < bd) {
                        best = Some((d, start, end));
                    }
                }
            }
        }
    }
    best.map(|(_, a, b)| (a, b))
}

/// Look at `c` from the default angle at `distance`.
fn look_at(w: &mut World, c: Cell, distance: f32) {
    let mut p = pose();
    p.focus = cell_center(c, 0.0);
    p.distance = distance;
    w.resource_mut::<CameraOverride>().0 = Some(p);
}

/// Top-left cell of a `wdt` x `hgt` patch of empty, unzoned land (with a
/// one-cell margin) near the town, if any.
fn find_empty_area(world: &World, wdt: i32, hgt: i32) -> Option<Cell> {
    let sim = &world.resource::<SimWorld>().world;
    let focus = Cell::new(46, 126);
    let grid = sim.grid_view();
    let free = |c: Cell| {
        grid.get(c)
            .is_some_and(|cd| cd.zone.is_none() && cd.occupant == Occupant::Empty)
    };
    let mut best: Option<(i32, Cell)> = None;
    for y in (focus.y - 60..focus.y + 60).step_by(2) {
        for x in (focus.x - 60..focus.x + 60).step_by(2) {
            let min = Cell::new(x, y);
            let d = min.chebyshev(focus);
            if best.is_some_and(|(bd, _)| d >= bd) {
                continue;
            }
            let ok = (-1..=hgt).all(|dy| (-1..=wdt).all(|dx| free(min.offset(dx, dy))));
            if ok {
                best = Some((d, min));
            }
        }
    }
    best.map(|(_, c)| c)
}

/// Keep the scenario's world for `reset`, then apply `cmds` to the live
/// one (with money to spare), for states that need a fresh road.
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

fn street(from: Cell, to: Cell) -> Command {
    Command::BuildRoad {
        from,
        to,
        kind: citysim_sim::RoadKind::Street,
    }
}

/// A signalled junction near the town centre (else any junction).
fn find_junction(world: &World) -> Option<Cell> {
    let sim = &world.resource::<SimWorld>().world;
    let focus = Cell::new(46, 126);
    citysim_sim::junction::junction_nodes(sim)
        .map(|n| {
            let c = Cell::from_pos(sim.roads.node_pos(n));
            let signal = citysim_sim::junction::junction_control(sim, n) == Some(citysim_sim::JunctionControl::Signal);
            (!signal, c.chebyshev(focus), c)
        })
        .min_by_key(|&(ns, d, c)| (ns, d, c.x, c.y))
        .map(|(.., c)| c)
}

/// The two ends of the straight street through road cell `c`.
fn street_ends(world: &World, c: Cell) -> Option<(Cell, Cell)> {
    let sim = &world.resource::<SimWorld>().world;
    let s = sim.roads.segment_through(c.center())?;
    let (a, b) = sim.roads.segment_ends(s);
    Some((Cell::from_pos(a), Cell::from_pos(b)))
}

fn steps() -> Vec<Step> {
    vec![
        Step {
            name: "idle",
            wait: 6.0,
            setup: reset,
        },
        Step {
            name: "inspect-home",
            wait: 1.0,
            setup: |w| {
                reset(w);
                let c = find_building_where(w, Cell::new(48, 130), |b| {
                    b.kind == Zone::Residential && b.level > 1 && b.occupied > 0
                });
                point_at(w, c);
            },
        },
        Step {
            name: "inspect-shop",
            wait: 1.0,
            setup: |w| {
                reset(w);
                let c = find_building_where(w, Cell::new(48, 130), |b| b.kind == Zone::Commercial);
                point_at(w, c);
            },
        },
        Step {
            name: "inspect-empty",
            wait: 1.0,
            setup: |w| {
                reset(w);
                let c = find_building_where(w, Cell::new(48, 130), |b| b.occupied == 0).or_else(|| find_building(w));
                point_at(w, c);
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
                let c = find_building_where(w, Cell::new(48, 130), |b| b.kind == Zone::Residential && b.occupied > 0);
                let world = &mut w.resource_mut::<SimWorld>().into_inner().world;
                if let Some(c) = c
                    && let Some(Occupant::Building(id)) = world.grid_view().get(c).map(|cd| cd.occupant)
                {
                    let hour = citysim_sim::balance::TICKS_PER_HOUR as u64;
                    let since = (world.time.tick / hour).saturating_sub(7) * hour;
                    world.growth.meta.entry(id).unreachable_since = Some(since);
                }
                point_at(w, c);
            },
        },
        Step {
            name: "zone-hover-zoned",
            wait: 1.0,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Residential));
                // An empty lot that is already residential (the old bug said
                // "needs a road" here).
                let sim = &w.resource::<SimWorld>().world;
                let focus = Cell::new(48, 130);
                let grid = sim.grid_view();
                let lot = (0..citysim_sim::MAP_CELLS * citysim_sim::MAP_CELLS)
                    .map(|i| Cell::new(i % citysim_sim::MAP_CELLS, i / citysim_sim::MAP_CELLS))
                    .filter(|&c| {
                        grid.get(c)
                            .is_some_and(|cd| cd.zone == Some(Zone::Residential) && cd.occupant == Occupant::Empty)
                    })
                    .min_by_key(|c| c.chebyshev(focus));
                point_at(w, lot);
            },
        },
        Step {
            name: "inspect-road",
            wait: 1.0,
            setup: |w| {
                reset(w);
                let c = busiest_road_near(w, Cell::new(46, 128)).or_else(|| find_road(w));
                point_at(w, c);
            },
        },
        Step {
            name: "inspect-jam",
            wait: 1.2,
            setup: |w| {
                reset(w);
                // The most congested street of the whole map, centred.
                let sim = &w.resource::<SimWorld>().world;
                let worst = sim
                    .roads
                    .segments
                    .values()
                    .filter(|s| s.kind == citysim_sim::RoadKind::Street)
                    .max_by(|a, b| a.congestion().total_cmp(&b.congestion()))
                    .map(|s| {
                        let (pa, pb) = (sim.roads.node_pos(s.a), sim.roads.node_pos(s.b));
                        Cell::from_pos([(pa[0] + pb[0]) * 0.5, (pa[1] + pb[1]) * 0.5])
                    });
                if let Some(c) = worst {
                    let mut p = pose();
                    p.focus = cell_center(c, 0.0);
                    p.distance = 220.0;
                    w.resource_mut::<CameraOverride>().0 = Some(p);
                    *w.resource_mut::<OverlayMode>() = OverlayMode::Traffic;
                }
                point_at(w, worst);
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
            name: "road-preview",
            wait: 1.0,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                let Some((start, end)) = find_new_road(w) else { return };
                let mid = (cell_center(start, 0.0) + cell_center(end, 0.0)) * 0.5;
                let mut p = pose();
                p.focus = mid;
                p.distance = 260.0;
                w.resource_mut::<CameraOverride>().0 = Some(p);
                force_path(w, vec![start]);
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(end);
            },
        },
        Step {
            name: "road-blocked",
            wait: 1.0,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                let Some(b) = find_building(w) else { return };
                let start = Cell::new(b.x, b.y - 8);
                force_path(w, vec![start]);
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(Cell::new(b.x, b.y + 3));
            },
        },
        Step {
            name: "road-broke",
            wait: 1.2,
            setup: |w| {
                reset(w);
                stash_world(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                // Find a buildable road first, then go broke.
                let found = find_new_road(w);
                w.resource_mut::<SimWorld>().world.economy.money = -420;
                let Some((start, end)) = found else { return };
                let mid = (cell_center(start, 0.0) + cell_center(end, 0.0)) * 0.5;
                let mut p = pose();
                p.focus = mid;
                p.distance = 260.0;
                w.resource_mut::<CameraOverride>().0 = Some(p);
                force_path(w, vec![start]);
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(end);
            },
        },
        Step {
            name: "zone-drag",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Commercial));
                w.resource_mut::<Pointer>().action = Some(Action::Rect {
                    start: Cell::new(36, 112),
                });
                let mut p = w.resource_mut::<Pointer>();
                p.target = PointerTarget::Cell(Cell::new(46, 118));
                p.hold = true;
            },
        },
        Step {
            name: "bulldoze-drag",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Bulldoze;
                let b = find_building(w).unwrap_or(Cell::new(48, 130));
                w.resource_mut::<Pointer>().action = Some(Action::Rect {
                    start: b.offset(-3, -3),
                });
                let mut p = w.resource_mut::<Pointer>();
                p.target = PointerTarget::Cell(b.offset(3, 2));
                p.hold = true;
            },
        },
        Step {
            name: "road-path",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                let Some((a, b)) = find_new_road(w) else { return };
                let (dx, dy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
                let third = Cell::new(b.x - dy * 7, b.y + dx * 7);
                force_path(w, vec![a, b]);
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(third);
                look_at(w, b, 240.0);
            },
        },
        Step {
            name: "road-t-junction",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                let Some((a, b)) = find_new_road(w) else { return };
                force_path(w, vec![b]);
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(a);
                look_at(w, b, 220.0);
            },
        },
        Step {
            name: "road-avenue-upgrade",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                *w.resource_mut::<RoadChoice>() = RoadChoice::Avenue;
                let Some((a, b)) = find_road(w).and_then(|r| street_ends(w, r)) else {
                    return;
                };
                force_path(w, vec![a]);
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(b);
                look_at(w, Cell::new((a.x + b.x) / 2, (a.y + b.y) / 2), 240.0);
            },
        },
        Step {
            name: "block-tool",
            wait: 1.4,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                w.resource_mut::<ToolModes>().road = RoadMode::Block;
                let Some(min) = find_empty_area(w, 16, 12) else { return };
                look_at(w, min.offset(8, 6), 280.0);
                let mut p = w.resource_mut::<Pointer>();
                p.action = Some(Action::Rect { start: min });
                p.target = PointerTarget::Cell(min.offset(15, 10));
                p.hold = true;
            },
        },
        Step {
            name: "zone-frontage",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(min) = find_empty_area(w, 16, 8) else { return };
                build_now(w, &[street(min.offset(0, 4), min.offset(15, 4))]);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Residential));
                w.resource_mut::<ToolModes>().zone = ZoneMode::Frontage;
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(min.offset(7, 3));
                look_at(w, min.offset(8, 4), 220.0);
            },
        },
        Step {
            name: "zone-fill",
            wait: 1.2,
            setup: |w| {
                reset(w);
                let Some(min) = find_empty_area(w, 12, 12) else { return };
                let cmds: Vec<Command> = super::plan::block_legs(min, min.offset(10, 10), None)
                    .into_iter()
                    .map(|(a, b)| street(a, b))
                    .collect();
                build_now(w, &cmds);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Commercial));
                w.resource_mut::<ToolModes>().zone = ZoneMode::Fill;
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(min.offset(5, 6));
                look_at(w, min.offset(5, 5), 230.0);
            },
        },
        Step {
            name: "bulldoze-road",
            wait: 1.2,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Bulldoze;
                w.resource_mut::<ToolModes>().bulldoze = BulldozeMode::Road;
                let Some(r) = find_road(w) else { return };
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(r);
                look_at(w, r, 240.0);
            },
        },
        Step {
            name: "junction-tool",
            wait: 1.5,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                w.resource_mut::<ToolModes>().road = RoadMode::Junction;
                let Some(j) = find_junction(w) else { return };
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(j);
                look_at(w, j, 200.0);
            },
        },
        Step {
            name: "inspect-pinned",
            wait: 1.5,
            setup: |w| {
                reset(w);
                let Some(c) = find_building(w) else { return };
                let id = match w.resource::<SimWorld>().world.grid_view().get(c).map(|cd| cd.occupant) {
                    Some(Occupant::Building(id)) => id,
                    _ => return,
                };
                w.resource_mut::<WorldHighlight>().selected = Some(WorldTarget::Building(id));
                look_at(w, c, 200.0);
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
                w.resource_mut::<UiState>().budget_open = true;
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
                w.resource_mut::<UiState>().budget_open = true;
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
                let _ = fresh.apply(Command::BuildRoad {
                    from: Cell::new(11, 128),
                    to: Cell::new(40, 128),
                    kind: citysim_sim::RoadKind::Street,
                });
                let _ = fresh.apply(Command::SetZone {
                    min: Cell::new(14, 126),
                    max: Cell::new(38, 127),
                    zone: Some(Zone::Residential),
                });
                let _ = fresh.apply(Command::SetZone {
                    min: Cell::new(14, 129),
                    max: Cell::new(30, 130),
                    zone: Some(Zone::Commercial),
                });
                for _ in 0..citysim_sim::balance::TICKS_PER_DAY / 2 {
                    fresh.step();
                }
                w.resource_mut::<SimWorld>().replace(fresh);
                w.resource_mut::<UiState>().budget_open = true;
            },
        },
        Step {
            name: "toasts",
            wait: 0.8,
            setup: |w| {
                reset(w);
                w.write_message(Toast::new(ToastKind::Success, "City saved"));
                // A real rejected command: a road through a building.
                if let Some(b) = find_building(w) {
                    w.write_message(SimCommand(Command::BuildRoad {
                        from: Cell::new(b.x, b.y - 6),
                        to: Cell::new(b.x, b.y + 2),
                        kind: citysim_sim::RoadKind::Street,
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
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Save);
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
                w.resource_mut::<UiState>().budget_open = true;
                *w.resource_mut::<ActiveTool>() = ActiveTool::Zone(Some(Zone::Residential));
                // The X screen is 1920x1080: keep the pointer in its top-left part.
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::ToggleBudget);
                if let Ok(mut win) = w.query_filtered::<&mut Window, With<PrimaryWindow>>().single_mut(w) {
                    win.resolution.set(2560.0, 1440.0);
                }
            },
        },
        Step {
            name: "1440p-menu",
            wait: 1.2,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::OpenMenu);
                w.write_message(Toast::new(ToastKind::Success, "City saved"));
                w.resource_mut::<Pointer>().target = PointerTarget::Button(UiAction::Load);
            },
        },
        Step {
            name: "1440p-road",
            wait: 1.5,
            setup: |w| {
                reset(w);
                *w.resource_mut::<ActiveTool>() = ActiveTool::Road;
                let Some((a, b)) = find_new_road(w) else { return };
                let (dx, dy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
                force_path(w, vec![a, b]);
                w.resource_mut::<Pointer>().target = PointerTarget::Cell(Cell::new(b.x - dy * 6, b.y + dx * 6));
                look_at(w, b, 240.0);
            },
        },
        Step {
            name: "1440p-settings",
            wait: 1.2,
            setup: |w| {
                reset(w);
                w.write_message(UiAction::OpenMenu);
                w.write_message(UiAction::OpenSettings);
            },
        },
    ]
}

fn run_shots(world: &mut World) {
    let dt = world.resource::<Time<Real>>().delta_secs().min(0.25);
    let mut r = world.remove_resource::<Runner>().expect("runner");
    r.frames += 1;
    if r.frames > 3 {
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
    if let Some(a) = pointer.action
        && tools.action != a
    {
        tools.action = a;
        if matches!(a, Action::Path { .. }) {
            tools.path = pointer.path.clone();
        }
    }
    let Ok(mut win) = window.single_mut() else { return };
    let cell_pos = |c: Cell| {
        cams.single()
            .ok()
            .and_then(|(cam, tf)| cam.world_to_viewport(tf, cell_center(c, 0.0)).ok())
    };
    let target = match pointer.target {
        // Park the pointer over open land at the right, away from every
        // panel (X ignores warps outside the window).
        PointerTarget::None => Some(Vec2::new(win.width() * 0.86, win.height() * 0.42)),
        PointerTarget::Cell(c) => cell_pos(c),
        PointerTarget::Data(i) => data.as_ref().and_then(|d| d.cells.get(i).copied()).and_then(cell_pos),
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
        }
    } else if mouse.pressed(MouseButton::Left) {
        mouse.release(MouseButton::Left);
    }
}

// ---------------------------------------------------------------- tour (video)

/// Cells the tour computed from the live town: road start/end, zone rect,
/// bulldoze rect, a building and a road to inspect.
#[derive(Resource, Default)]
struct TourData {
    cells: Vec<Cell>,
    focus: Option<Vec3>,
}

#[derive(Clone, Copy)]
enum Op {
    /// Glide to a target over about this many seconds (waits for it).
    Go(PointerTarget, f32),
    Click,
    Down,
    Up,
    RightClick,
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
    release_right: bool,
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
            release_right: false,
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

fn tour_setup(w: &mut World) {
    reset(w);
    w.resource_mut::<Pointer>().smooth = true;
    w.resource_mut::<Pointer>().target = PointerTarget::Cell(Cell::new(60, 120));
    let mut cells = Vec::new();
    let (a, b) = find_new_road(w).unwrap_or((Cell::new(40, 100), Cell::new(49, 100)));
    cells.extend([a, b]);
    let (dx, dy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
    // Zone beside the new road (one side, two cells deep).
    let (px, py) = (-dy, dx);
    cells.push(Cell::new(a.x + px + dx, a.y + py + dy));
    cells.push(Cell::new(b.x + 2 * px - dx, b.y + 2 * py - dy));
    // Frame the new road with the town behind it.
    let mid = Cell::new((a.x + b.x) / 2, (a.y + b.y) / 2);
    let toward = Cell::new(mid.x + (46 - mid.x).signum() * 12, mid.y + (128 - mid.y).signum() * 12);
    let mut data = w.resource_mut::<TourData>();
    data.cells = cells;
    data.focus = Some((cell_center(mid, 0.0) + cell_center(toward, 0.0)) * 0.5);
}

/// Bulldoze / inspect targets, chosen once the town has grown in.
fn tour_targets(w: &mut World) {
    let Some(focus) = w.resource::<TourData>().focus else {
        return;
    };
    let fc = crate::shared::world_to_cell(focus);
    let bld = find_building_near(w, fc).unwrap_or(fc);
    let road = busiest_road_near(w, fc).unwrap_or(fc);
    let junction = find_junction(w).unwrap_or(road);
    let mut data = w.resource_mut::<TourData>();
    data.cells.truncate(4);
    data.cells.extend([bld.offset(-2, -3), bld.offset(3, 2), bld, road]);
    // 8: third corner of the road path; 9: beside its last leg; 10: a junction.
    let (a, b) = (data.cells[0], data.cells[1]);
    let (dx, dy) = ((b.x - a.x).signum(), (b.y - a.y).signum());
    let third = Cell::new(b.x - dy * 7, b.y + dx * 7);
    let beside = Cell::new(b.x - dy * 4 + dx, b.y + dx * 4 + dy);
    data.cells.extend([third, beside, junction]);
}

fn busiest_road_near(world: &World, focus: Cell) -> Option<Cell> {
    let sim = &world.resource::<SimWorld>().world;
    let mut best: Option<(f32, Cell)> = None;
    for y in focus.y - 12..focus.y + 12 {
        for x in focus.x - 12..focus.x + 12 {
            let c = Cell::new(x, y);
            if let Some(s) = sim
                .roads
                .segment_through(c.center())
                .and_then(|s| sim.roads.segments.get(s))
            {
                let score = s.congestion() - c.chebyshev(focus) as f32 * 0.01;
                if best.is_none_or(|(b, _)| score > b) {
                    best = Some((score, c));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

fn tour_ops() -> Vec<Op> {
    use Op::*;
    use PointerTarget::{Button, Data};
    vec![
        Run(tour_setup),
        Run(tour_targets),
        Wait(1.5),
        // A three-corner road, built with a double-click.
        Go(Button(UiAction::Tool(ActiveTool::Road)), 0.65),
        Wait(0.6),
        Click,
        Go(Data(0), 0.65),
        Click,
        Go(Data(1), 0.9),
        Wait(0.4),
        Click,
        Go(Data(8), 0.9),
        Wait(0.9),
        Click,
        Wait(0.15),
        Click,
        Wait(0.8),
        // Zone along the new road.
        Go(Button(UiAction::ZoneTool), 0.65),
        Click,
        Wait(0.3),
        Go(Button(UiAction::ZoneMode(ZoneMode::Frontage)), 0.65),
        Wait(0.6),
        Click,
        Go(Data(9), 0.9),
        Wait(1.2),
        Click,
        Wait(0.6),
        // Switch a junction's control.
        Go(Button(UiAction::RoadMode(RoadMode::Junction)), 0.65),
        Click,
        Go(Data(10), 0.9),
        Wait(1.6),
        Click,
        Wait(1.4),
        // Bulldozer preview (cancelled with a right click).
        Go(Button(UiAction::Tool(ActiveTool::Bulldoze)), 0.65),
        Click,
        Go(Data(4), 0.65),
        Down,
        Go(Data(5), 0.9),
        Wait(0.7),
        RightClick,
        Up,
        Wait(0.2),
        // Inspect and pin a building, then a road.
        Go(Button(UiAction::Tool(ActiveTool::Inspect)), 0.65),
        Click,
        Go(Data(6), 0.65),
        Wait(0.6),
        Click,
        Wait(1.0),
        Go(Data(7), 0.65),
        Wait(1.3),
        Go(Button(UiAction::Overlay(OverlayMode::Traffic)), 0.65),
        Click,
        Wait(1.3),
        Click,
        Go(Button(UiAction::ToggleBudget), 0.65),
        Click,
        Wait(1.2),
        Go(Button(UiAction::CloseBudget), 0.65),
        Click,
        Key(KeyCode::Escape),
        Wait(0.6),
        Go(Button(UiAction::OpenSettings), 0.65),
        Click,
        Wait(1.6),
        Go(Button(UiAction::CloseSettings), 0.65),
        Click,
        Wait(0.4),
        Go(Button(UiAction::CloseMenu), 0.65),
        Click,
        Wait(1.0),
    ]
}

fn run_tour(world: &mut World) {
    let dt = world.resource::<Time<Real>>().delta_secs().min(0.1);
    let mut tour = world.remove_resource::<Tour>().expect("tour");
    tour.total += dt;
    // Slow orbit for some life in the picture.
    let mut p = pose();
    p.distance = 330.0;
    if let Some(f) = world.resource::<TourData>().focus {
        p.focus = f;
    }
    p.yaw += tour.total * 0.012;
    world.resource_mut::<CameraOverride>().0 = Some(p);
    {
        let mut keys = world.resource_mut::<ButtonInput<KeyCode>>();
        for k in tour.release_keys.drain(..) {
            keys.release(k);
        }
    }
    if std::mem::take(&mut tour.release_right) {
        world
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Right);
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
            Op::Down | Op::Click => {
                let mut mouse = world.resource_mut::<ButtonInput<MouseButton>>();
                mouse.press(MouseButton::Left);
                let mut p = world.resource_mut::<Pointer>();
                p.hold = true;
                if matches!(op, Op::Click) {
                    // Release next frame.
                    tour.ops[tour.index] = Op::Up;
                    tour.t = 0.0;
                    break;
                }
                true
            }
            Op::Up => {
                world.resource_mut::<Pointer>().hold = false;
                true
            }
            Op::RightClick => {
                world
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .press(MouseButton::Right);
                tour.release_right = true;
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
