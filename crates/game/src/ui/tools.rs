//! The tool state machine. Keys: I inspect, R road, Z zone (cycles
//! R/C/I/remove), B bulldoze; Escape or right click cancels.
//! Road: click start, click end (chains from the end). Zone / bulldoze:
//! drag a rectangle. Reads `HoveredCell`, writes `ActiveTool`,
//! `ToolPreview` and `SimCommand`s.

use bevy::prelude::*;
use citysim_sim::{Cell, CellRect, Command, Zone};

use crate::shared::{ActiveTool, GameSet, HoveredCell, PreviewKind, SimCommand, SimWorld, ToolPreview};

#[derive(Resource, Debug, Default)]
struct ToolState {
    /// Road start cell, or rectangle drag start.
    anchor: Option<Cell>,
    /// Last on-map hovered cell (used while dragging off the map).
    last_cell: Option<Cell>,
    /// Inputs the current preview was computed from.
    preview_key: Option<(Command, u64, u64, u64, i64)>,
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

fn tool_keys(keys: Res<ButtonInput<KeyCode>>, mut tool: ResMut<ActiveTool>, mut state: ResMut<ToolState>) {
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
            _ => Some(Zone::Residential),
        }))
    } else {
        None
    };
    if let Some(t) = new {
        *tool = t;
    }
    // Any tool change (from keys, other UI, or scripts) drops the anchor.
    if tool.is_changed() {
        state.anchor = None;
    }
}

fn rect_command(tool: ActiveTool, a: Cell, b: Cell) -> Option<Command> {
    match tool {
        ActiveTool::Zone(zone) => Some(Command::SetZone { min: a, max: b, zone }),
        ActiveTool::Bulldoze => Some(Command::Bulldoze { min: a, max: b }),
        ActiveTool::Road | ActiveTool::Inspect => None,
    }
}

fn use_tool(
    hovered: Res<HoveredCell>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    tool: Res<ActiveTool>,
    preview: Res<ToolPreview>,
    mut state: ResMut<ToolState>,
    mut out: MessageWriter<SimCommand>,
) {
    if hovered.0.is_some() {
        state.last_cell = hovered.0;
    }
    if keys.just_pressed(KeyCode::Escape) || mouse.just_pressed(MouseButton::Right) {
        state.anchor = None;
        return;
    }
    let pressed = mouse.just_pressed(MouseButton::Left);
    let released = mouse.just_released(MouseButton::Left);
    match *tool {
        ActiveTool::Inspect => {}
        ActiveTool::Road => {
            let Some(cell) = hovered.0 else { return };
            if !pressed {
                return;
            }
            match state.anchor {
                None => state.anchor = Some(cell),
                Some(start) if start == cell => state.anchor = None,
                Some(start) => {
                    out.write(SimCommand(Command::BuildRoad { from: start, to: cell }));
                    // Chain from the end if the preview said it would work.
                    if preview.line == Some((start, cell)) && preview.is_valid() {
                        state.anchor = Some(cell);
                    }
                }
            }
        }
        t => {
            if pressed && hovered.0.is_some() {
                state.anchor = hovered.0;
            }
            if released && let (Some(a), Some(b)) = (state.anchor, state.last_cell) {
                if let Some(cmd) = rect_command(t, a, b) {
                    out.write(SimCommand(cmd));
                }
                state.anchor = None;
            }
        }
    }
}

fn update_preview(
    hovered: Res<HoveredCell>,
    tool: Res<ActiveTool>,
    sim: Res<SimWorld>,
    mut state: ResMut<ToolState>,
    mut preview: ResMut<ToolPreview>,
) {
    let cell = hovered.0.or(state.last_cell.filter(|_| state.anchor.is_some()));
    let cmd = match (*tool, state.anchor, cell) {
        (ActiveTool::Road, Some(a), Some(c)) if a != c => Some(Command::BuildRoad { from: a, to: c }),
        (ActiveTool::Road | ActiveTool::Inspect, _, _) | (_, _, None) => None,
        (t, anchor, Some(c)) => rect_command(t, anchor.unwrap_or(c), c),
    };
    let Some(cmd) = cmd else {
        state.preview_key = None;
        preview.set_if_neq(ToolPreview::default());
        return;
    };
    let w = &sim.world;
    let key = (cmd, sim.generation, w.versions.roads, w.versions.grid, w.economy.money);
    if state.preview_key == Some(key) {
        return;
    }
    state.preview_key = Some(key);
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
    preview.set_if_neq(p);
}
