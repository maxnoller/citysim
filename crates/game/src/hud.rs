//! On-screen text: money, day/time, speed, tool, ghost cost / last error.

use bevy::prelude::*;
use citysim_sim::Command;

use crate::GameSet;
use crate::sim_driver::{LastCommand, SPEEDS, SimClock, SimWorld};
use crate::tools::{Ghost, ToolState};

#[derive(Component)]
struct HudText;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(Update, update_hud.in_set(GameSet::Render));
    }
}

fn spawn_hud(mut commands: Commands) {
    commands
        .spawn((
            Name::new("HUD"),
            Node {
                position_type: PositionType::Absolute,
                top: px(8),
                left: px(8),
                padding: UiRect::axes(px(10), px(6)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ))
        .with_child((
            HudText,
            Text::new("CitySim"),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(Color::WHITE),
        ));
    commands.spawn((
        Name::new("Help"),
        Text::new(
            "WASD pan  Q/E or middle-drag rotate  wheel zoom  |  R road  Z zone  B bulldoze  Esc cancel  |  Space pause  1/2/3 speed  F5/F9 save/load",
        ),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.85)),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(6),
            left: px(8),
            ..default()
        },
    ));
}

pub fn format_money(v: i64) -> String {
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if v < 0 { format!("-${out}") } else { format!("${out}") }
}

fn command_label(cmd: &Command) -> &'static str {
    match cmd {
        Command::BuildRoad { .. } => "road",
        Command::Bulldoze { .. } => "bulldoze",
        Command::SetZone { .. } => "zone",
        Command::SetTaxRate(_) => "tax",
    }
}

fn update_hud(
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    tools: Res<ToolState>,
    ghost: Res<Ghost>,
    last: Res<LastCommand>,
    time: Res<Time<Real>>,
    mut q: Query<&mut Text, With<HudText>>,
) {
    let w = &sim.world;
    let hour = w.time.hour();
    let speed = if clock.paused {
        "Paused".to_string()
    } else {
        format!("Speed {} ({}x)", clock.speed + 1, SPEEDS[clock.speed])
    };
    let mut line = format!(
        "{}   Day {}  {:02}:{:02}   {}   Tool: {}",
        format_money(w.economy.money),
        w.time.day,
        hour as u32,
        ((hour.fract()) * 60.0) as u32,
        speed,
        tools.tool.label(),
    );
    match &ghost.result {
        Some(Ok(p)) if p.cost > 0 => line.push_str(&format!("   cost {}", format_money(p.cost))),
        Some(Ok(p)) if matches!(ghost.cmd, Some(Command::SetZone { .. })) => {
            line.push_str(&format!("   {} cells", p.cells.len()))
        }
        Some(Err(e)) => line.push_str(&format!("   ({e})")),
        _ => {}
    }
    if let Some((cmd, Err(e))) = &last.result
        && time.elapsed_secs() - last.at < 4.0
    {
        line.push_str(&format!("\nLast {}: {e}", command_label(cmd)));
    }
    for mut text in &mut q {
        if text.0 != line {
            text.0.clone_from(&line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::format_money;

    #[test]
    fn money_formatting() {
        assert_eq!(format_money(0), "$0");
        assert_eq!(format_money(999), "$999");
        assert_eq!(format_money(50_000), "$50,000");
        assert_eq!(format_money(1_234_567), "$1,234,567");
        assert_eq!(format_money(-4_200), "-$4,200");
    }
}
