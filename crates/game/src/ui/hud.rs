//! Top bar: date / time with a day-progress bar, money (opens the budget),
//! population, jobs; speed controls on the right; a "Paused" pill below.

use bevy::prelude::*;

use super::UiState;
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, label, layout, panel, spawn_button, spawn_key_badge,
};
use crate::shared::{GameSet, SimClock, SimWorld};

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MoneyShown>()
            .add_systems(Startup, spawn_top_bar)
            .add_systems(Update, update_top_bar.in_set(GameSet::Render));
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum HudText {
    Day,
    Time,
    DayIcon,
    Money,
    MoneyDelta,
    MoneyDeltaIcon,
    Population,
    Jobs,
    JobsCaption,
}

#[derive(Component)]
struct DayBar;

#[derive(Component)]
struct PausedPill;

/// Money as displayed (rolls towards the real value).
#[derive(Resource, Default)]
struct MoneyShown(Option<f64>);

/// Rounded square holding a chip's icon.
fn spawn_icon_box(p: &mut ChildSpawnerCommands, fonts: &UiFonts, glyph: char, color: Color, marker: Option<HudText>) {
    p.spawn(layout(Node {
        width: px(30),
        height: px(30),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        border_radius: BorderRadius::all(px(t::RADIUS_SM)),
        flex_shrink: 0.0,
        ..default()
    }))
    .insert(BackgroundColor(t::alpha(color, 0.14)))
    .with_children(|b| {
        let mut e = b.spawn(super::widgets::icon(fonts, glyph, t::ICON_SM + 1.0, color));
        if let Some(m) = marker {
            e.insert(m);
        }
    });
}

fn chip_node() -> Node {
    Node {
        height: px(t::TOP_BAR_H),
        padding: UiRect::axes(px(t::S4), px(0)),
        align_items: AlignItems::Center,
        column_gap: px(t::S3 + 2.0),
        flex_shrink: 0.0,
        ..default()
    }
}

fn text_column() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        justify_content: JustifyContent::Center,
        row_gap: px(0),
        ..default()
    }
}

use bevy::ecs::hierarchy::ChildSpawnerCommands;

fn spawn_top_bar(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Top bar"),
            layout(Node {
                position_type: PositionType::Absolute,
                top: px(t::EDGE),
                left: px(t::EDGE),
                right: px(t::EDGE),
                height: px(t::TOP_BAR_H),
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Start,
                ..default()
            }),
        ))
        .with_children(|bar| {
            bar.spawn(layout(Node {
                column_gap: px(t::S3),
                ..default()
            }))
            .with_children(|left| {
                // Date & time.
                left.spawn(panel(chip_node())).with_children(|c| {
                    spawn_icon_box(c, fonts, icon::SUN, t::WARNING, Some(HudText::DayIcon));
                    c.spawn(layout(text_column())).with_children(|col| {
                        col.spawn((
                            label(fonts, "Day 1", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT),
                            HudText::Day,
                        ));
                        col.spawn(layout(Node {
                            align_items: AlignItems::Center,
                            column_gap: px(t::S3),
                            ..default()
                        }))
                        .with_children(|row| {
                            row.spawn((
                                label(fonts, "06:00", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                                HudText::Time,
                                Node {
                                    min_width: px(34),
                                    ..default()
                                },
                            ));
                            row.spawn(layout(Node {
                                width: px(48),
                                height: px(4),
                                border_radius: BorderRadius::all(px(2)),
                                overflow: Overflow::clip(),
                                ..default()
                            }))
                            .insert(BackgroundColor(t::WELL))
                            .with_child((
                                DayBar,
                                layout(Node {
                                    width: percent(25),
                                    height: percent(100),
                                    border_radius: BorderRadius::all(px(2)),
                                    ..default()
                                }),
                                BackgroundColor(t::alpha(t::TEXT_MUTED, 0.9)),
                            ));
                        });
                    });
                });

                // Money: a button opening the budget.
                left.spawn(panel(Node {
                    padding: UiRect::all(px(t::S1)),
                    flex_shrink: 0.0,
                    ..default()
                }))
                .with_children(|c| {
                    spawn_button(
                        c,
                        UiButton {
                            action: UiAction::ToggleBudget,
                            kind: ButtonKind::Ghost,
                            accent: t::ACCENT,
                        },
                        Node {
                            height: px(t::TOP_BAR_H - 2.0 * t::S1 - 2.0),
                            padding: UiRect::new(px(t::S4 - t::S1), px(t::S3), px(0), px(0)),
                            align_items: AlignItems::Center,
                            column_gap: px(t::S3 + 2.0),
                            ..default()
                        },
                        Some(Tooltip {
                            title: "Budget",
                            desc: "Balance, yesterday's income and expenses, tax rate.",
                            key: None,
                            price: None,
                        }),
                        |b, id| {
                            spawn_icon_box(b, fonts, icon::WALLET, t::ACCENT, None);
                            b.spawn(layout(Node {
                                min_width: px(92),
                                ..text_column()
                            }))
                            .with_children(|col| {
                                col.spawn((
                                    label(fonts, "$0", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT),
                                    HudText::Money,
                                ));
                                col.spawn(layout(Node {
                                    align_items: AlignItems::Center,
                                    column_gap: px(t::S2),
                                    ..default()
                                }))
                                .with_children(|row| {
                                    row.spawn((
                                        super::widgets::icon(fonts, icon::TRENDING_UP, 12.0, t::TEXT_MUTED),
                                        HudText::MoneyDeltaIcon,
                                    ));
                                    row.spawn((
                                        label(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                                        HudText::MoneyDelta,
                                    ));
                                });
                            });
                            b.spawn((
                                super::widgets::icon(fonts, icon::CHEVRON_DOWN, t::ICON_SM, t::TEXT_FAINT),
                                ButtonFg {
                                    owner: id,
                                    base: t::TEXT_FAINT,
                                },
                            ));
                        },
                    );
                });

                // Population.
                left.spawn(panel(chip_node())).with_children(|c| {
                    spawn_icon_box(c, fonts, icon::USERS, t::ACCENT, None);
                    c.spawn(layout(text_column())).with_children(|col| {
                        col.spawn((
                            label(fonts, "0", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT),
                            HudText::Population,
                            Node {
                                min_width: px(44),
                                ..default()
                            },
                        ));
                        col.spawn(label(fonts, "Residents", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED));
                    });
                });

                // Jobs.
                left.spawn(panel(chip_node())).with_children(|c| {
                    spawn_icon_box(c, fonts, icon::BRIEFCASE, t::ACCENT, None);
                    c.spawn(layout(text_column())).with_children(|col| {
                        col.spawn((label(fonts, "0", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT), HudText::Jobs));
                        col.spawn((
                            label(fonts, "Jobs", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                            HudText::JobsCaption,
                        ));
                    });
                });
            });

            // Speed controls.
            bar.spawn(panel(Node {
                height: px(t::TOP_BAR_H),
                padding: UiRect::all(px(t::S2)),
                align_items: AlignItems::Center,
                column_gap: px(t::S1),
                ..default()
            }))
            .with_children(|c| {
                spawn_speed_button(
                    c,
                    fonts,
                    UiAction::TogglePause,
                    0,
                    "Pause",
                    "Stop the clock. Build freely while paused.",
                    "Space",
                );
                c.spawn(super::widgets::vdivider(22.0));
                spawn_speed_button(
                    c,
                    fonts,
                    UiAction::Speed(0),
                    1,
                    "Normal speed",
                    "One in-game day every two minutes.",
                    "1",
                );
                spawn_speed_button(
                    c,
                    fonts,
                    UiAction::Speed(1),
                    2,
                    "Fast",
                    "Three times normal speed.",
                    "2",
                );
                spawn_speed_button(
                    c,
                    fonts,
                    UiAction::Speed(2),
                    3,
                    "Fastest",
                    "Six times normal speed.",
                    "3",
                );
            });
        });

    // "Paused" pill under the top bar.
    commands
        .spawn((
            Name::new("Paused pill"),
            PausedPill,
            layout(Node {
                position_type: PositionType::Absolute,
                top: px(t::BELOW_TOP_BAR),
                left: px(0),
                right: px(0),
                justify_content: JustifyContent::Center,
                display: Display::None,
                ..default()
            }),
        ))
        .with_children(|p| {
            p.spawn(panel(Node {
                padding: UiRect::new(px(t::S4), px(t::S3), px(t::S2 + 1.0), px(t::S2 + 1.0)),
                align_items: AlignItems::Center,
                column_gap: px(t::S3),
                border_radius: BorderRadius::all(px(999)),
                ..default()
            }))
            .with_children(|pill| {
                pill.spawn(super::widgets::icon(fonts, icon::PAUSE, t::ICON_SM, t::WARNING));
                pill.spawn(label(fonts, "Paused", t::FONT_BODY, t::W_SEMIBOLD, t::WARNING));
                pill.spawn(super::widgets::vdivider(14.0));
                pill.spawn(label(fonts, "Resume with", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED));
                spawn_key_badge(pill, fonts, "Space");
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn spawn_speed_button(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    arrows: usize,
    title: &'static str,
    desc: &'static str,
    key: &'static str,
) {
    let accent = if arrows == 0 { t::WARNING } else { t::ACCENT };
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Tool,
            accent,
        },
        Node {
            width: px(if arrows == 3 { 46.0 } else { 40.0 }),
            height: px(t::TOP_BAR_H - 2.0 * t::S2 - 2.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Some(Tooltip {
            title,
            desc,
            key: Some(key),
            price: None,
        }),
        |b, id| {
            let fg = ButtonFg {
                owner: id,
                base: t::TEXT_MUTED,
            };
            if arrows == 0 {
                b.spawn((super::widgets::icon(fonts, icon::PAUSE, 16.0, t::TEXT_MUTED), fg));
            }
            for i in 0..arrows {
                b.spawn((super::widgets::icon(fonts, icon::PLAY, 13.0, t::TEXT_MUTED), fg))
                    .insert(Node {
                        width: px(13),
                        height: px(13),
                        margin: UiRect::left(px(if i == 0 { 0.0 } else { -4.0 })),
                        ..default()
                    });
            }
        },
    );
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_top_bar(
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    state: Res<UiState>,
    time: Res<Time<Real>>,
    mut shown: ResMut<MoneyShown>,
    mut texts: Query<(&HudText, &mut Text, &mut TextColor)>,
    mut bar: Query<(&mut Node, &mut BackgroundColor), With<DayBar>>,
    mut pill: Query<&mut Node, (With<PausedPill>, Without<DayBar>)>,
) {
    let w = &sim.world;
    let hour = w.time.hour();
    let day = (6.0..18.0).contains(&hour);
    let money = w.economy.money as f64;
    let m = shown.0.get_or_insert(money);
    if (*m - money).abs() < 1.0 {
        *m = money;
    } else {
        *m += (money - *m) * approach(time.delta_secs(), 0.12) as f64;
    }
    let money_shown = m.round() as i64;
    let net = w.economy.last_day_income - w.economy.last_day_expenses;
    let has_history = w.economy.last_day_income != 0 || w.economy.last_day_expenses != 0;
    let s = &w.stats;
    let unemployed = if s.population > 0 {
        s.population.saturating_sub(s.employed) as f32 / s.population as f32
    } else {
        0.0
    };
    let net_color = if !has_history {
        t::TEXT_MUTED
    } else if net >= 0 {
        t::POSITIVE
    } else {
        t::NEGATIVE
    };
    for (which, mut text, mut color) in &mut texts {
        let (s, c): (String, Color) = match which {
            HudText::Day => (format!("Day {}", w.time.day), t::TEXT),
            HudText::Time => (format::clock(hour), t::TEXT_MUTED),
            HudText::DayIcon => {
                let glyph = match hour as u32 {
                    5 | 6 => icon::SUNRISE,
                    18 | 19 => icon::SUNSET,
                    _ if day => icon::SUN,
                    _ => icon::MOON,
                };
                (glyph.to_string(), if day { t::WARNING } else { t::ACCENT })
            }
            HudText::Money => (
                format::money(money_shown),
                if w.economy.money < 0 { t::NEGATIVE } else { t::TEXT },
            ),
            HudText::MoneyDelta => (
                if has_history {
                    format!("{} / day", format::signed_money(net))
                } else {
                    "No taxes yet".to_string()
                },
                net_color,
            ),
            HudText::MoneyDeltaIcon => (
                if net >= 0 {
                    icon::TRENDING_UP
                } else {
                    icon::TRENDING_DOWN
                }
                .to_string(),
                net_color,
            ),
            HudText::Population => (format::group(s.population as u64), t::TEXT),
            HudText::Jobs => (format::group(s.jobs as u64), t::TEXT),
            HudText::JobsCaption => {
                if s.population == 0 {
                    ("Jobs".to_string(), t::TEXT_MUTED)
                } else {
                    let c = if unemployed > 0.15 {
                        t::NEGATIVE
                    } else if unemployed > 0.08 {
                        t::WARNING
                    } else {
                        t::TEXT_MUTED
                    };
                    (
                        format!(
                            "Jobs · {} jobless",
                            format::percent((unemployed * 100.0).round() / 100.0)
                        ),
                        c,
                    )
                }
            }
        };
        if text.0 != s {
            text.0 = s;
        }
        if color.0 != c {
            color.0 = c;
        }
    }
    if let Ok((mut node, mut bg)) = bar.single_mut() {
        let w = percent(hour / 24.0 * 100.0);
        if node.width != w {
            node.width = w;
        }
        bg.set_if_neq(BackgroundColor(if day { t::WARNING } else { t::ACCENT }));
    }
    if let Ok(mut node) = pill.single_mut() {
        let d = if clock.paused && !state.menu_open {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != d {
            node.display = d;
        }
    }
}
