//! Top bar: date / time with a day-progress bar, money (opens the budget),
//! population, jobs with the sim's unemployment, average commute; speed
//! controls on the right; a "Paused" pill below. Warns once (toast) when the
//! city's money goes negative.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;

use super::UiState;
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::toasts::{Toast, ToastKind};
use super::widgets::{
    ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, label, layout, number, panel, spawn_button,
    spawn_key_badge,
};
use crate::shared::{GameSet, SimClock, SimWorld};
use citysim_sim::balance;

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
    MoneyIcon,
    Population,
    Jobs,
    JobsCaption,
    Commute,
    CommuteCaption,
}

#[derive(Component)]
struct MoneyIconBox;

#[derive(Component)]
struct DayBar;

#[derive(Component)]
struct PausedPill;

/// Money as displayed (rolls towards the real value; snaps when another
/// world is loaded, keyed by `SimWorld::generation`).
#[derive(Resource, Default)]
struct MoneyShown {
    value: Option<f64>,
    generation: u64,
    /// The broke warning was shown for the current spell of debt.
    warned_broke: bool,
}

/// Everything the top bar shows; texts are rewritten only when it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HudKey {
    generation: u64,
    money_shown: i64,
    money: i64,
    day: u32,
    clock: u32,
    day_bar: u32,
    population: u32,
    jobs: u32,
    jobless_permille: u32,
    workforce: u32,
    commute: u32,
    booked: usize,
    paused: bool,
    menu: bool,
}

/// Rounded square holding a chip's icon.
fn spawn_icon_box(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    glyph: char,
    color: Color,
    marker: Option<HudText>,
) -> Entity {
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
    })
    .id()
}

/// Hover help for a non-button element.
pub fn info_tip(title: &'static str, desc: &'static str) -> impl Bundle {
    (
        Tooltip {
            title,
            desc,
            key: None,
            price: None,
        },
        Hovered::default(),
    )
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
                left.spawn((
                    panel(chip_node()),
                    info_tip(
                        "Date & time",
                        "One in-game day lasts four minutes at normal speed. Taxes and upkeep are booked at midnight.",
                    ),
                ))
                .with_children(|c| {
                    spawn_icon_box(c, fonts, icon::SUN, t::WARNING, Some(HudText::DayIcon));
                    c.spawn(layout(text_column())).with_children(|col| {
                        col.spawn((
                            number(fonts, "Day 1", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT),
                            HudText::Day,
                        ));
                        col.spawn(layout(Node {
                            align_items: AlignItems::Center,
                            column_gap: px(t::S3),
                            ..default()
                        }))
                        .with_children(|row| {
                            row.spawn((
                                number(fonts, "06:00", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
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
                            desc: "Balance, taxes, road upkeep and city services by day; the tax rate.",
                            key: None,
                            price: None,
                        }),
                        |b, id| {
                            let icon_box = spawn_icon_box(b, fonts, icon::WALLET, t::ACCENT, Some(HudText::MoneyIcon));
                            b.commands().entity(icon_box).insert(MoneyIconBox);
                            b.spawn(layout(Node {
                                min_width: px(92),
                                ..text_column()
                            }))
                            .with_children(|col| {
                                col.spawn((
                                    number(fonts, "$0", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT),
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
                                        number(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
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
                left.spawn((
                    panel(chip_node()),
                    info_tip("Residents", "People living in your city. Zone homes to grow it."),
                ))
                .with_children(|c| {
                    spawn_icon_box(c, fonts, icon::USERS, t::ACCENT, None);
                    c.spawn(layout(text_column())).with_children(|col| {
                        col.spawn((
                            number(fonts, "0", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT),
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
                left.spawn((
                    panel(chip_node()),
                    info_tip(
                        "Jobs",
                        "Workplaces in shops, offices and industry. Jobless: share of the workforce (one worker \
                         per household) without a job. Jobless households move away after a few days.",
                    ),
                ))
                .with_children(|c| {
                    spawn_icon_box(c, fonts, icon::BRIEFCASE, t::ACCENT, None);
                    c.spawn(layout(text_column())).with_children(|col| {
                        col.spawn((number(fonts, "0", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT), HudText::Jobs));
                        col.spawn((
                            label(fonts, "Jobs", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                            HudText::JobsCaption,
                        ));
                    });
                });

                // Average commute.
                left.spawn((
                    panel(chip_node()),
                    info_tip(
                        "Average commute",
                        "Average door-to-door drive to work, in real seconds at normal speed (the game \
                         clock runs faster). Over a minute and fewer people want to move in.",
                    ),
                ))
                .with_children(|c| {
                    spawn_icon_box(c, fonts, icon::TRAFFIC, t::ACCENT, None);
                    c.spawn(layout(text_column())).with_children(|col| {
                        col.spawn((
                            number(fonts, "\u{2014}", t::FONT_VALUE, t::W_SEMIBOLD, t::TEXT),
                            HudText::Commute,
                            Node {
                                min_width: px(44),
                                ..default()
                            },
                        ));
                        col.spawn((
                            label(fonts, "Commute", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                            HudText::CommuteCaption,
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
                    "One in-game day every four minutes.",
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

/// Average commute for the HUD: real driving seconds at 1x ("—" before
/// the first commute finished).
pub fn commute_text(secs: f32) -> String {
    if secs <= 0.0 || !secs.is_finite() {
        return "\u{2014}".to_string();
    }
    let s = secs.round() as u32;
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min {:02} s", s / 60, s % 60)
    }
}

/// Colour for the average commute: fine up to `COMMUTE_OK_S` (where it
/// starts to cut residential demand), red from `HOPELESS_COMMUTE_S` (where
/// households start giving up).
fn commute_color(secs: f32) -> Color {
    if secs >= balance::HOPELESS_COMMUTE_S {
        t::NEGATIVE
    } else if secs > balance::COMMUTE_OK_S {
        t::WARNING
    } else {
        t::TEXT
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_top_bar(
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    state: Res<UiState>,
    time: Res<Time<Real>>,
    mut shown: ResMut<MoneyShown>,
    mut toasts: MessageWriter<Toast>,
    mut texts: Query<(&HudText, &mut Text, &mut TextColor)>,
    mut bar: Query<(&mut Node, &mut BackgroundColor), With<DayBar>>,
    mut pill: Query<&mut Node, (With<PausedPill>, Without<DayBar>)>,
    mut money_box: Query<&mut BackgroundColor, (With<MoneyIconBox>, Without<DayBar>)>,
    mut last: Local<Option<HudKey>>,
) {
    let w = &sim.world;
    let hour = w.time.hour();
    let day = (6.0..18.0).contains(&hour);
    let money = w.economy.money as f64;
    if shown.generation != sim.generation {
        // Another world was loaded: show it as it is, no roll.
        shown.generation = sim.generation;
        shown.value = None;
    }
    let m = shown.value.get_or_insert(money);
    if (*m - money).abs() < 1.0 {
        *m = money;
    } else {
        *m += (money - *m) * approach(time.delta_secs(), 0.12) as f64;
    }
    let money_shown = m.round() as i64;
    // The clock moves in 10-minute steps (a game minute is 1/6 s at 1x).
    let s = &w.stats;
    let key = HudKey {
        generation: sim.generation,
        money_shown,
        money: w.economy.money,
        day: w.time.day,
        clock: ((hour * 6.0) as u32).min(143),
        day_bar: (hour / 24.0 * 96.0) as u32,
        population: s.population,
        jobs: s.jobs,
        jobless_permille: (s.unemployment * 1000.0) as u32,
        workforce: s.workforce,
        commute: s.avg_commute_s.round() as u32,
        booked: w.economy.history.len(),
        paused: clock.paused,
        menu: state.menu_open,
    };
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let broke = w.economy.money < 0;
    if broke && !shown.warned_broke {
        shown.warned_broke = true;
        toasts.write(Toast::new(
            ToastKind::Warning,
            "Money is below zero. New roads wait until taxes bring it back; bulldozing still works.",
        ));
    } else if !broke {
        shown.warned_broke = false;
    }
    // Booked at midnight; nothing to show before the first one.
    let booked = !w.economy.history.is_empty();
    let net = w.economy.last_day.income() - w.economy.last_day.expenses();
    let net_color = if !booked {
        t::TEXT_MUTED
    } else if net >= 0 {
        t::POSITIVE
    } else {
        t::NEGATIVE
    };
    for (which, mut text, mut color) in &mut texts {
        let (s, c): (String, Color) = match which {
            HudText::Day => (format!("Day {}", w.time.day), t::TEXT),
            HudText::Time => (format!("{:02}:{:02}", key.clock / 6, key.clock % 6 * 10), t::TEXT_MUTED),
            HudText::DayIcon => {
                let glyph = match hour as u32 {
                    5 | 6 => icon::SUNRISE,
                    18 | 19 => icon::SUNSET,
                    _ if day => icon::SUN,
                    _ => icon::MOON,
                };
                (glyph.to_string(), if day { t::WARNING } else { t::ACCENT })
            }
            HudText::Money => (format::money(money_shown), if broke { t::NEGATIVE } else { t::TEXT }),
            HudText::MoneyIcon => (
                if broke { icon::ALERT } else { icon::WALLET }.to_string(),
                if broke { t::NEGATIVE } else { t::ACCENT },
            ),
            HudText::MoneyDelta => (
                if booked {
                    format!("{} / day", format::signed_money(net))
                } else {
                    "Booked at midnight".to_string()
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
                if s.workforce == 0 {
                    ("Jobs".to_string(), t::TEXT_MUTED)
                } else {
                    // The sim's own figure: unemployed / workforce.
                    let u = s.unemployment;
                    let c = if u > 0.15 {
                        t::NEGATIVE
                    } else if u > 0.08 {
                        t::WARNING
                    } else {
                        t::TEXT_MUTED
                    };
                    (
                        format!("Jobs · {} jobless", format::percent((u * 100.0).round() / 100.0)),
                        c,
                    )
                }
            }
            HudText::Commute => (commute_text(s.avg_commute_s), commute_color(s.avg_commute_s)),
            HudText::CommuteCaption => (
                if s.avg_commute_s > 0.0 {
                    "Avg. drive to work"
                } else {
                    "No commutes yet"
                }
                .to_string(),
                t::TEXT_MUTED,
            ),
        };
        if text.0 != s {
            text.0 = s;
        }
        if color.0 != c {
            color.0 = c;
        }
    }
    if let Ok(mut bg) = money_box.single_mut() {
        bg.set_if_neq(BackgroundColor(t::alpha(
            if broke { t::NEGATIVE } else { t::ACCENT },
            0.14,
        )));
    }
    if let Ok((mut node, mut bg)) = bar.single_mut() {
        let w = percent(key.day_bar as f32 / 96.0 * 100.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_texts_match_the_sim_clock() {
        // The tooltips say "four minutes" and "over a minute".
        assert_eq!(balance::TICKS_PER_DAY, 4 * 60 * citysim_sim::TICKS_PER_SECOND);
        assert_eq!(balance::COMMUTE_OK_S, 60.0);
    }

    #[test]
    fn commute_formatting() {
        assert_eq!(commute_text(0.0), "\u{2014}");
        assert_eq!(commute_text(42.4), "42 s");
        assert_eq!(commute_text(59.6), "1 min 00 s");
        assert_eq!(commute_text(125.0), "2 min 05 s");
    }
}
