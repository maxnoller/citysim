//! The tool state machine. Reads `HoveredCell` and the mouse, writes
//! `ActiveTool`, `ToolPreview` (via `World::preview`) and `SimCommand`s.
//!
//! * Road: press on a cell to anchor; either drag and release on the end
//!   cell, or release in place and click the end cell. After a successful
//!   click-placed segment the next one chains from its end. The end snaps
//!   to the nearest straight / 45° direction.
//! * Zone / Bulldoze: press, drag a rectangle, release.
//! * Escape or right click cancels the action in progress (with nothing in
//!   progress Escape is handled by `actions.rs`: menus, then the tool).
//!
//! Keys: I inspect, R road, Z zone (cycles R/C/I/de-zone), B bulldoze.

use bevy::prelude::*;
use citysim_sim::{Cell, CellRect, Command, Zone};

use super::UiState;
use crate::shared::{ActiveTool, GameSet, HoveredCell, PreviewKind, SimCommand, SimWorld, ToolPreview};

/// What the current tool is in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    Idle,
    /// Road anchored at `start`; `held` while the button is still down
    /// from the anchoring press (drag mode until it is released elsewhere).
    Road { start: Cell, held: bool },
    /// Rectangle drag from `start` (button held).
    Rect { start: Cell },
}

#[derive(Resource, Debug, Default)]
pub struct ToolState {
    pub action: Action,
    /// Last on-map hovered cell (used while the pointer is over UI / off map).
    pub last_cell: Option<Cell>,
    /// Inputs the current preview was computed from.
    preview_key: Option<(Command, u64, u64, u64, u64, i64)>,
    /// Command the last click sent, so the result can be matched.
    pub sent: Option<Command>,
}

impl ToolState {
    pub fn busy(&self) -> bool {
        self.action != Action::Idle
    }
}

pub struct ToolsPlugin;

impl Plugin for ToolsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToolState>().add_systems(
            Update,
            (tool_keys, use_tool, update_preview).chain().in_set(GameSet::Tools),
        );
    }
}

fn tool_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ActiveTool>,
    mut state: ResMut<ToolState>,
    mut ui: ResMut<UiState>,
) {
    if !ui.menu_open {
        let new = if keys.just_pressed(KeyCode::KeyI) {
            Some(ActiveTool::Inspect)
        } else if keys.just_pressed(KeyCode::KeyR) {
            Some(ActiveTool::Road)
        } else if keys.just_pressed(KeyCode::KeyB) {
            Some(ActiveTool::Bulldoze)
        } else if keys.just_pressed(KeyCode::KeyZ) {
            Some(ActiveTool::Zone(match *tool {
                ActiveTool::Zone(Some(Zone::Residential)) => Some(Zone::Commercial),
                ActiveTool::Zone(Some(Zone::Commercial)) => Some(Zone::Industrial),
                ActiveTool::Zone(Some(Zone::Industrial)) => None,
                ActiveTool::Zone(None) => Some(Zone::Residential),
                _ => ui.last_zone,
            }))
        } else {
            None
        };
        if let Some(t) = new {
            tool.set_if_neq(t);
        }
    }
    // Any tool change (keys, buttons, scripts) drops the action.
    if tool.is_changed() {
        state.action = Action::Idle;
        if let ActiveTool::Zone(z) = *tool {
            ui.last_zone = z;
        }
    }
}

fn rect_command(tool: ActiveTool, a: Cell, b: Cell) -> Option<Command> {
    match tool {
        ActiveTool::Zone(zone) => Some(Command::SetZone { min: a, max: b, zone }),
        ActiveTool::Bulldoze => Some(Command::Bulldoze { min: a, max: b }),
        ActiveTool::Road | ActiveTool::Inspect => None,
    }
}

/// Snap `to` onto the nearest of the 8 straight / diagonal rays from `from`.
pub fn snap_line(from: Cell, to: Cell) -> Cell {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    if dx == 0 || dy == 0 || dx.abs() == dy.abs() {
        return to;
    }
    let (ax, ay) = (dx.abs(), dy.abs());
    // Candidates: horizontal, vertical, diagonal (with the larger extent
    // projected onto it); pick the one closest to the pointer.
    let diag = (ax + ay + 1) / 2;
    let candidates = [
        Cell::new(to.x, from.y),
        Cell::new(from.x, to.y),
        Cell::new(from.x + diag * dx.signum(), from.y + diag * dy.signum()),
    ];
    let dist = |c: &Cell| {
        let (ex, ey) = ((c.x - to.x) as f32, (c.y - to.y) as f32);
        ex * ex + ey * ey
    };
    candidates
        .into_iter()
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))
        .unwrap_or(to)
}

/// Road end for the current pointer cell: snapped and clamped to the map.
fn road_end(start: Cell, cell: Cell) -> Cell {
    let end = snap_line(start, cell);
    if end.in_bounds() {
        return end;
    }
    // Pull a diagonal back inside the map along the same direction.
    let (sx, sy) = ((end.x - start.x).signum(), (end.y - start.y).signum());
    let mut c = start;
    while c.offset(sx, sy).in_bounds() && c != end {
        c = c.offset(sx, sy);
    }
    c
}

#[allow(clippy::too_many_arguments)]
fn use_tool(
    hovered: Res<HoveredCell>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    tool: Res<ActiveTool>,
    preview: Res<ToolPreview>,
    ui: Res<UiState>,
    mut state: ResMut<ToolState>,
    mut out: MessageWriter<SimCommand>,
) {
    if hovered.0.is_some() {
        state.last_cell = hovered.0;
    }
    if ui.menu_open {
        state.action = Action::Idle;
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right) {
        state.action = Action::Idle;
        return;
    }
    let pressed = mouse.just_pressed(MouseButton::Left);
    let cell = hovered.0;
    match *tool {
        ActiveTool::Inspect => {}
        ActiveTool::Road => match state.action {
            Action::Idle => {
                if pressed && let Some(c) = cell {
                    // A click can press and release within one frame.
                    let held = mouse.pressed(MouseButton::Left);
                    state.action = Action::Road { start: c, held };
                }
            }
            Action::Road { start, held: true } => {
                if !mouse.pressed(MouseButton::Left) {
                    let end = state.last_cell.map(|c| road_end(start, c));
                    match end {
                        Some(end) if end != start => {
                            // Drag finished: build, then stop (drag = one segment).
                            let cmd = Command::BuildRoad { from: start, to: end };
                            state.sent = Some(cmd);
                            out.write(SimCommand(cmd));
                            state.action = Action::Idle;
                        }
                        // Released in place: wait for the end click.
                        _ => state.action = Action::Road { start, held: false },
                    }
                }
            }
            Action::Road { start, held: false } => {
                if pressed && let Some(c) = cell {
                    let end = road_end(start, c);
                    if end == start {
                        state.action = Action::Idle;
                    } else {
                        let cmd = Command::BuildRoad { from: start, to: end };
                        state.sent = Some(cmd);
                        out.write(SimCommand(cmd));
                        // Chain from the end when the preview said it works.
                        let ok = preview.line == Some((start, end)) && preview.is_valid();
                        state.action = if ok {
                            Action::Road {
                                start: end,
                                held: false,
                            }
                        } else {
                            Action::Road { start, held: false }
                        };
                    }
                }
            }
            Action::Rect { .. } => state.action = Action::Idle,
        },
        t => match state.action {
            Action::Idle => {
                if pressed && let Some(c) = cell {
                    if mouse.pressed(MouseButton::Left) {
                        state.action = Action::Rect { start: c };
                    } else if let Some(cmd) = rect_command(t, c, c) {
                        // Pressed and released within one frame: a click.
                        state.sent = Some(cmd);
                        out.write(SimCommand(cmd));
                    }
                }
            }
            Action::Rect { start } => {
                if !mouse.pressed(MouseButton::Left) {
                    if let Some(end) = state.last_cell
                        && let Some(cmd) = rect_command(t, start, end)
                    {
                        state.sent = Some(cmd);
                        out.write(SimCommand(cmd));
                    }
                    state.action = Action::Idle;
                }
            }
            Action::Road { .. } => state.action = Action::Idle,
        },
    }
}

/// The command the current tool would send if the player clicked now.
pub fn pending_command(tool: ActiveTool, state: &ToolState, hovered: Option<Cell>) -> Option<Command> {
    let cell = hovered.or(state.last_cell.filter(|_| state.busy()));
    match (tool, state.action, cell) {
        (ActiveTool::Road, Action::Road { start, .. }, Some(c)) => {
            let end = road_end(start, c);
            (end != start).then_some(Command::BuildRoad { from: start, to: end })
        }
        (ActiveTool::Road | ActiveTool::Inspect, _, _) | (_, _, None) => None,
        (t, Action::Rect { start }, Some(c)) => rect_command(t, start, c),
        (t, _, Some(c)) => rect_command(t, c, c),
    }
}

fn update_preview(
    hovered: Res<HoveredCell>,
    tool: Res<ActiveTool>,
    sim: Res<SimWorld>,
    ui: Res<UiState>,
    mut state: ResMut<ToolState>,
    mut preview: ResMut<ToolPreview>,
) {
    let cmd = if ui.menu_open {
        None
    } else {
        pending_command(*tool, &state, hovered.0)
    };
    let Some(cmd) = cmd else {
        state.preview_key = None;
        preview.set_if_neq(ToolPreview::default());
        return;
    };
    let w = &sim.world;
    let key = (
        cmd,
        sim.generation,
        w.versions.roads,
        w.versions.grid,
        w.versions.buildings,
        w.economy.money,
    );
    if state.preview_key == Some(key) {
        return;
    }
    state.preview_key = Some(key);
    preview.set_if_neq(compute_preview(&sim.world, cmd));
}

pub fn compute_preview(w: &citysim_sim::World, cmd: Command) -> ToolPreview {
    let result = w.preview(&cmd);
    let mut p = ToolPreview::default();
    match cmd {
        Command::BuildRoad { from, to } => {
            p.kind = PreviewKind::Road;
            p.line = Some((from, to));
        }
        Command::SetZone { min, max, zone } => {
            p.kind = PreviewKind::Zone(zone);
            p.rect = Some(CellRect::from_corners(min, max));
        }
        Command::Bulldoze { min, max } => {
            p.kind = PreviewKind::Bulldoze;
            p.rect = Some(CellRect::from_corners(min, max));
        }
        Command::SetTaxRate(_) => {}
    }
    match result {
        Ok(r) => {
            p.cells = r.cells;
            p.cost = r.cost;
        }
        Err(e) => {
            if let Some((from, to)) = p.line {
                p.cells = from.straight_line_to(to).unwrap_or_default();
            }
            p.error = Some(e);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_keeps_straight_and_diagonal() {
        let o = Cell::new(10, 10);
        assert_eq!(snap_line(o, Cell::new(20, 10)), Cell::new(20, 10));
        assert_eq!(snap_line(o, Cell::new(15, 15)), Cell::new(15, 15));
        assert_eq!(snap_line(o, Cell::new(20, 11)), Cell::new(20, 10));
        assert_eq!(snap_line(o, Cell::new(11, 3)), Cell::new(10, 3));
        assert_eq!(snap_line(o, Cell::new(16, 15)), Cell::new(16, 16));
        assert_eq!(snap_line(o, Cell::new(4, 15)), Cell::new(4, 16));
        for to in [Cell::new(17, 13), Cell::new(3, 22), Cell::new(12, 40)] {
            let end = snap_line(o, to);
            assert!(o.straight_line_to(end).is_some(), "{to:?} -> {end:?}");
        }
    }

    #[test]
    fn road_end_stays_on_map() {
        let start = Cell::new(1, 1);
        let end = road_end(start, Cell::new(-5, -6));
        assert!(end.in_bounds());
        assert!(start == end || start.straight_line_to(end).is_some());
    }
}
