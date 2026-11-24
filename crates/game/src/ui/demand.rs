//! Demand meter (bottom-left): residential, commercial and industrial
//! demand as bars signed around a centre line, eased towards the sim value.

use bevy::prelude::*;
use citysim_sim::Zone;

use super::theme::{self as t, UiFonts};
use super::widgets::{approach, label, layout, panel, zone_icon};
use crate::shared::{GameSet, SimWorld};

pub struct DemandPlugin;

impl Plugin for DemandPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_meter)
            .add_systems(Update, update_meter.in_set(GameSet::Render));
    }
}

/// Fill of one bar; `shown` eases towards the sim's demand.
#[derive(Component)]
struct DemandFill {
    zone: Zone,
    shown: f32,
}

const TRACK_W: f32 = 104.0;

fn spawn_meter(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Demand meter"),
            panel(Node {
                position_type: PositionType::Absolute,
                left: px(t::EDGE),
                bottom: px(t::EDGE),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(px(t::S4), px(t::S4), px(t::S3 + 2.0), px(t::S4)),
                row_gap: px(t::S3),
                ..default()
            }),
        ))
        .with_children(|p| {
            p.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            }))
            .with_children(|row| {
                row.spawn(label(fonts, "Demand", t::FONT_SMALL, t::W_SEMIBOLD, t::TEXT_MUTED));
            });
            for zone in Zone::ALL {
                let color = t::zone_color(Some(zone));
                p.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(t::S3),
                    ..default()
                }))
                .with_children(|row| {
                    row.spawn(layout(Node {
                        width: px(20),
                        height: px(20),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(px(t::RADIUS_XS + 1.0)),
                        ..default()
                    }))
                    .insert(BackgroundColor(t::alpha(color, 0.16)))
                    .with_child(super::widgets::icon(fonts, zone_icon(Some(zone)), 12.0, color));
                    row.spawn((
                        label(
                            fonts,
                            super::widgets::zone_name(Some(zone)),
                            t::FONT_SMALL,
                            t::W_MEDIUM,
                            t::TEXT,
                        ),
                        Node {
                            width: px(78),
                            ..default()
                        },
                    ));
                    row.spawn(layout(Node {
                        width: px(TRACK_W),
                        height: px(8),
                        border_radius: BorderRadius::all(px(4)),
                        ..default()
                    }))
                    .insert(BackgroundColor(t::WELL))
                    .with_children(|track| {
                        track.spawn((
                            DemandFill { zone, shown: 0.0 },
                            layout(Node {
                                position_type: PositionType::Absolute,
                                left: percent(50),
                                width: percent(0),
                                top: px(0),
                                bottom: px(0),
                                border_radius: BorderRadius::all(px(4)),
                                ..default()
                            }),
                            BackgroundColor(color),
                        ));
                        // Centre line.
                        track.spawn((
                            layout(Node {
                                position_type: PositionType::Absolute,
                                left: percent(50),
                                top: px(-3),
                                bottom: px(-3),
                                width: px(1),
                                margin: UiRect::left(px(-0.5)),
                                ..default()
                            }),
                            BackgroundColor(t::alpha(t::TEXT, 0.35)),
                        ));
                    });
                });
            }
        });
}

fn update_meter(
    time: Res<Time<Real>>,
    sim: Res<SimWorld>,
    mut fills: Query<(&mut DemandFill, &mut Node, &mut BackgroundColor)>,
) {
    let d = &sim.world.demand;
    let k = approach(time.delta_secs(), 0.35);
    for (mut fill, mut node, mut bg) in &mut fills {
        let target = match fill.zone {
            Zone::Residential => d.residential,
            Zone::Commercial => d.commercial,
            Zone::Industrial => d.industrial,
        }
        .clamp(-1.0, 1.0);
        fill.shown += (target - fill.shown) * k;
        let v = fill.shown;
        let half = v.abs() * 50.0;
        let (left, width) = if v >= 0.0 { (50.0, half) } else { (50.0 - half, half) };
        let (left, width) = (percent(left), percent(width));
        if node.left != left || node.width != width {
            node.left = left;
            node.width = width;
        }
        let color = t::zone_color(Some(fill.zone));
        let c = if v >= 0.0 { color } else { t::alpha(color, 0.45) };
        bg.set_if_neq(BackgroundColor(c));
    }
}
