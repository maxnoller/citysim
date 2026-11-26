//! The bottom bar, flush with the screen edge: money with its daily change,
//! residents, jobless, the three demand meters (left); the tool slots
//! (centre); the day strip, the clock and the speed switch (right).
//! Separated by slanted rules. Narrow windows get a compact bar (no day
//! strip, no stat icons, narrower slots).

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;
use bevy::ui::{ColorStop, Gradient, LinearGradient};
use citysim_sim::{Zone, balance};

use super::UiState;
use super::format;
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts, icon};
use super::toasts::{Toast, ToastKind};
use super::widgets::{
    ActiveMark, ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, caps, figure, info_tip, label, layout,
    plate_node, slant_rule, spawn_button,
};
use crate::shared::{ActiveTool, GameSet, OverlayMode, RoadMode, SimClock, SimWorld};

pub struct BarPlugin;

impl Plugin for BarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MoneyShown>()
            .add_systems(Startup, spawn_bar)
            .add_systems(
                Update,
                (update_vitals, update_demand, update_clock, compact_bar, raise_slots).in_set(GameSet::Render),
            );
    }
}

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum BarText {
    Money,
    MoneyDelta,
    Residents,
    Jobless,
    JoblessCaption,
    DayLabel,
    Clock,
    ClockCaption,
}

#[derive(Component)]
struct MoneyIcon;

#[derive(Component)]
struct DayMarker;

#[derive(Component)]
struct DemandFill {
    zone: Zone,
    shown: f32,
}

/// Bottom tick of a meter: lit red while that zone has a surplus.
#[derive(Component)]
struct DemandSurplus(Zone);

/// Hidden in the compact bar.
#[derive(Component)]
struct Wide;

/// Shown only in the compact bar.
#[derive(Component)]
struct Narrow;

/// A tool slot (its height rises while active).
#[derive(Component)]
pub struct ToolSlot;

/// The bar's root.
#[derive(Component)]
pub struct BarRoot;

/// Money as displayed (rolls towards the real value; snaps when another
/// world is loaded, keyed by `SimWorld::generation`).
#[derive(Resource, Default)]
struct MoneyShown {
    value: Option<f64>,
    generation: u64,
    /// The broke warning was shown for the current spell of debt.
    warned_broke: bool,
}

const DEMAND_H: f32 = 46.0;

fn spawn_bar(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    let bar_mat = cache.get(
        &mut mats,
        PlateStyle {
            sheen: 0.006,
            ..PlateStyle::fill(t::BAR_TOP, t::BAR_BOTTOM, t::LINE)
        },
    );
    commands
        .spawn((
            Name::new("Bottom bar"),
            BarRoot,
            plate_node(
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    right: px(0),
                    bottom: px(0),
                    height: px(t::BAR_H),
                    ..default()
                },
                UiRect::top(px(t::LW)),
                [0.0; 4],
            ),
            bar_mat,
            GlobalZIndex(30),
        ))
        .with_children(|bar| {
            // The accent glow along the top edge.
            bar.spawn((
                layout(Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    right: px(0),
                    top: px(-t::LW),
                    height: px(3),
                    ..default()
                }),
                super::widgets::hgrad(&[
                    t::alpha(t::ACCENT, 0.0),
                    t::alpha(t::ACCENT, 0.85),
                    t::alpha(t::ACCENT, 0.85),
                    t::alpha(t::ACCENT, 0.85),
                    t::alpha(t::ACCENT, 0.0),
                ]),
            ));
            spawn_vitals(bar, fonts);
            spawn_tools(bar, fonts);
            spawn_time(bar, fonts);
        });
}

fn stat_column(p: &mut ChildSpawnerCommands, fonts: &UiFonts, big: bool, value: BarText, cap: BarText, cap_text: &str) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(5),
        ..default()
    }))
    .with_children(|col| {
        col.spawn((
            figure(fonts, "", if big { t::FONT_BIG } else { t::FONT_MID }, t::TEXT),
            value,
        ));
        col.spawn((caps(fonts, cap_text, t::FONT_CAP, t::TEXT_MUTED), cap));
    });
}

fn spawn_vitals(bar: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    bar.spawn(layout(Node {
        position_type: PositionType::Absolute,
        left: px(30),
        top: px(0),
        bottom: px(0),
        align_items: AlignItems::Center,
        column_gap: px(t::S6),
        ..default()
    }))
    .with_children(|v| {
        // Money: opens the budget.
        spawn_button(
            v,
            UiButton::new(UiAction::ToggleBudget, ButtonKind::Ghost),
            Node {
                height: px(66),
                padding: UiRect::horizontal(px(8)),
                margin: UiRect::left(px(-8)),
                align_items: AlignItems::Center,
                column_gap: px(11),
                ..default()
            },
            Some(Tooltip::new("Budget", "Taxes, road upkeep and city services by day; the tax rate.").key("U")),
            |b, _| {
                b.spawn((
                    super::widgets::icon(fonts, icon::WALLET, 28.0, t::ACCENT_TEXT),
                    MoneyIcon,
                    Wide,
                ));
                b.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(5),
                    min_width: px(118),
                    ..default()
                }))
                .with_children(|col| {
                    col.spawn((figure(fonts, "$0", t::FONT_BIG, t::TEXT), BarText::Money));
                    col.spawn((caps(fonts, "", t::FONT_CAP, t::POSITIVE), BarText::MoneyDelta));
                });
            },
        );
        v.spawn(slant_rule(50.0));
        v.spawn((
            Node {
                align_items: AlignItems::Center,
                column_gap: px(11),
                ..default()
            },
            info_tip("Residents", "People living in your city. Zone homes to grow it."),
        ))
        .with_children(|r| {
            r.spawn((super::widgets::icon(fonts, icon::USERS, 26.0, t::ACCENT_TEXT), Wide));
            stat_column(
                r,
                fonts,
                false,
                BarText::Residents,
                BarText::JoblessCaption,
                "Residents",
            );
        });
        v.spawn((
            Node { ..default() },
            info_tip(
                "Jobless",
                "Share of the workforce without a job. Jobless households move away after a few days; \
                 zone shops and industry for work.",
            ),
        ))
        .with_children(|r| {
            r.spawn(layout(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(5),
                ..default()
            }))
            .with_children(|col| {
                col.spawn((figure(fonts, "", t::FONT_MID, t::TEXT), BarText::Jobless));
                col.spawn(caps(fonts, "Jobless", t::FONT_CAP, t::TEXT_MUTED));
            });
        });
        v.spawn(slant_rule(50.0));
        // Demand meters.
        v.spawn((
            Node {
                column_gap: px(7),
                align_items: AlignItems::End,
                ..default()
            },
            info_tip(
                "Demand",
                "How much the city wants more homes (R), shops and offices (C) and industry (I). \
                 A red foot: too many already.",
            ),
        ))
        .with_children(|d| {
            for (zone, letter) in [
                (Zone::Residential, "R"),
                (Zone::Commercial, "C"),
                (Zone::Industrial, "I"),
            ] {
                d.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: px(3),
                    ..default()
                }))
                .with_children(|m| {
                    m.spawn((
                        layout(Node {
                            width: px(13),
                            height: px(DEMAND_H),
                            border: UiRect::all(px(1)),
                            flex_direction: FlexDirection::Column,
                            justify_content: JustifyContent::End,
                            ..default()
                        }),
                        BackgroundColor(t::WELL),
                        BorderColor::all(t::LINE),
                    ))
                    .with_children(|w| {
                        w.spawn((
                            DemandFill { zone, shown: 0.0 },
                            layout(Node {
                                width: percent(100),
                                height: percent(0),
                                ..default()
                            }),
                            BackgroundColor(t::zone_color(Some(zone))),
                        ));
                        // Tick marks every 9 px.
                        for k in 1..5 {
                            w.spawn((
                                layout(Node {
                                    position_type: PositionType::Absolute,
                                    left: px(0),
                                    right: px(0),
                                    bottom: px(k as f32 * 9.0 - 1.0),
                                    height: px(1),
                                    ..default()
                                }),
                                BackgroundColor(t::alpha(t::LINE, 0.75)),
                            ));
                        }
                        w.spawn((
                            DemandSurplus(zone),
                            layout(Node {
                                position_type: PositionType::Absolute,
                                left: px(0),
                                right: px(0),
                                bottom: px(0),
                                height: px(4),
                                ..default()
                            }),
                            BackgroundColor(t::NEGATIVE),
                            Visibility::Hidden,
                        ));
                    });
                    m.spawn(label(fonts, letter, 11.0, t::W_BOLD, t::TEXT_MUTED));
                });
            }
        });
    });
}

/// (action, icon, label, key, tooltip title, description)
type SlotDef = (UiAction, char, &'static str, &'static str, &'static str, &'static str);

const SLOTS: [&[SlotDef]; 3] = [
    &[(
        UiAction::Tool(ActiveTool::Inspect),
        icon::INSPECT,
        "Inspect",
        "I",
        "Inspect",
        "Point at buildings, roads and cars; click a home, workplace, road or car to see it in the side panel.",
    )],
    &[
        (
            UiAction::Tool(ActiveTool::Road),
            icon::ROAD,
            "Roads",
            "R",
            "Roads",
            "Straight, curved and continuous roads and street grids at any angle. R again: street / avenue.",
        ),
        (
            UiAction::ZoneTool,
            icon::GRID_ZONE,
            "Zones",
            "Z",
            "Zones",
            "Zone lots for homes, shops and industry; buildings grow by demand. Z again: next zone type.",
        ),
        (
            UiAction::Tool(ActiveTool::Bulldoze),
            icon::BULLDOZE,
            "Clear",
            "B",
            "Clear",
            "Remove roads (between junctions, or part of one), buildings, or everything in an area.",
        ),
        (
            UiAction::RoadMode(RoadMode::Junction),
            icon::WAYPOINTS,
            "Junctions",
            "J",
            "Junctions",
            "Traffic lights or priority signs: see a junction's phases and queues, click to change it.",
        ),
    ],
    &[
        (
            UiAction::CityPeople,
            icon::USER,
            "People",
            "P",
            "People",
            "Who lives in the city: ages, households, work, what everyone is doing, why people are unhappy.",
        ),
        (
            UiAction::ToggleBudget,
            icon::WALLET,
            "Budget",
            "U",
            "Budget",
            "Income and costs by line, the tax rate, and what would fix a deficit.",
        ),
        (
            UiAction::Maps,
            icon::LAYERS,
            "Maps",
            "O",
            "Maps",
            "Traffic and zone maps over the city. O cycles them, T toggles traffic.",
        ),
    ],
];

fn spawn_tools(bar: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    bar.spawn(layout(Node {
        position_type: PositionType::Absolute,
        left: px(0),
        right: px(0),
        top: px(0),
        bottom: px(0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::End,
        padding: UiRect::bottom(px(9)),
        column_gap: px(5),
        ..default()
    }))
    .with_children(|row| {
        for (g, group) in SLOTS.iter().enumerate() {
            if g > 0 {
                row.spawn((layout(Node {
                    width: px(13),
                    ..default()
                }),));
            }
            for &(action, glyph, text, key, title, desc) in group.iter() {
                spawn_slot(row, fonts, action, glyph, text, key, Tooltip::new(title, desc).key(key));
            }
        }
    });
}

fn spawn_slot(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    text: &str,
    key: &str,
    tip: Tooltip,
) {
    let id = spawn_button(
        p,
        UiButton::new(action, ButtonKind::Slot),
        Node {
            width: px(t::SLOT_W),
            height: px(t::SLOT_H),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: px(5),
            flex_shrink: 0.0,
            ..default()
        },
        Some(tip),
        |b, id| {
            let base = t::SLOT_INK;
            b.spawn((
                label(fonts, key, t::FONT_KEY, t::W_SEMIBOLD, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
                Node {
                    position_type: PositionType::Absolute,
                    right: px(5),
                    top: px(3),
                    ..default()
                },
            ));
            if action == UiAction::RoadMode(RoadMode::Junction) {
                signal_glyph(b, id);
            } else {
                b.spawn((
                    super::widgets::icon(fonts, glyph, t::ICON_LG, base),
                    ButtonFg { owner: id, base },
                ));
            }
            b.spawn((caps(fonts, text, t::FONT_CAP, base), ButtonFg { owner: id, base }, Wide));
            // The caret above the raised slot.
            b.spawn((
                ActiveMark { owner: id },
                layout(Node {
                    position_type: PositionType::Absolute,
                    top: px(-7),
                    width: px(12),
                    height: px(12),
                    ..default()
                }),
                BackgroundColor(t::ACCENT),
                UiTransform {
                    rotation: Rot2::degrees(45.0),
                    ..default()
                },
                Visibility::Hidden,
                ZIndex(-1),
            ));
        },
    );
    p.commands().entity(id).insert(ToolSlot);
}

/// A small traffic signal drawn with nodes (Lucide has none).
fn signal_glyph(b: &mut ChildSpawnerCommands, owner: Entity) {
    b.spawn((
        layout(Node {
            width: px(13),
            height: px(26),
            border: UiRect::all(px(2)),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::SpaceEvenly,
            align_items: AlignItems::Center,
            ..default()
        }),
        BorderColor::all(t::SLOT_INK),
        SignalGlyph(owner, false),
    ))
    .with_children(|s| {
        for _ in 0..3 {
            s.spawn((
                layout(Node {
                    width: px(4),
                    height: px(4),
                    ..default()
                }),
                BackgroundColor(t::SLOT_INK),
                SignalGlyph(owner, true),
            ));
        }
    });
}

/// Parts of the drawn signal glyph (they follow the slot's text colour).
#[derive(Component)]
struct SignalGlyph(Entity, bool);

fn spawn_time(bar: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    bar.spawn(layout(Node {
        position_type: PositionType::Absolute,
        right: px(30),
        top: px(0),
        bottom: px(0),
        align_items: AlignItems::Center,
        column_gap: px(t::S6),
        ..default()
    }))
    .with_children(|tm| {
        // Day strip.
        tm.spawn((
            Node {
                width: px(168),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            },
            Wide,
            info_tip(
                "Day",
                "A day lasts 16 minutes at normal speed. Taxes and upkeep are booked at midnight.",
            ),
        ))
        .with_children(|d| {
            d.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            }))
            .with_children(|r| {
                r.spawn(super::widgets::icon(fonts, icon::SUN, 14.0, t::TEXT_MUTED));
                r.spawn((caps(fonts, "", t::FONT_CAP, t::TEXT_MUTED), BarText::DayLabel));
                r.spawn(super::widgets::icon(fonts, icon::MOON, 14.0, t::TEXT_MUTED));
            });
            d.spawn((
                layout(Node {
                    height: px(9),
                    border: UiRect::all(px(1)),
                    ..default()
                }),
                BorderColor::all(t::LINE),
                BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_right(vec![
                    ColorStop::new(t::rgba(0x1b2440, 1.0), percent(0)),
                    ColorStop::new(t::rgba(0x1b2440, 1.0), percent(22)),
                    ColorStop::new(t::rgba(0xe9a03b, 1.0), percent(27)),
                    ColorStop::new(t::rgba(0x9ed2f0, 1.0), percent(36)),
                    ColorStop::new(t::rgba(0x9ed2f0, 1.0), percent(70)),
                    ColorStop::new(t::rgba(0xe9793b, 1.0), percent(78)),
                    ColorStop::new(t::rgba(0x1b2440, 1.0), percent(84)),
                    ColorStop::new(t::rgba(0x1b2440, 1.0), percent(100)),
                ]))]),
            ))
            .with_child((
                DayMarker,
                layout(Node {
                    position_type: PositionType::Absolute,
                    top: px(-5),
                    bottom: px(-5),
                    width: px(3),
                    margin: UiRect::left(px(-1.5)),
                    left: percent(50),
                    ..default()
                }),
                BackgroundColor(t::TEXT),
                Outline {
                    width: px(1),
                    offset: px(0),
                    color: t::alpha(Color::BLACK, 0.5),
                },
            ));
        });
        // Clock (with the day under it in the compact bar).
        tm.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::End,
            row_gap: px(4),
            min_width: px(88),
            ..default()
        }))
        .with_children(|c| {
            c.spawn((figure(fonts, "", t::FONT_CLOCK, t::TEXT), BarText::Clock));
            c.spawn((
                caps(fonts, "", 10.5, t::TEXT_MUTED),
                BarText::ClockCaption,
                Narrow,
                Node {
                    display: Display::None,
                    ..default()
                },
            ));
        });
        // Speed switch.
        tm.spawn(layout(Node {
            align_items: AlignItems::Center,
            ..default()
        }))
        .with_children(|s| {
            let defs: [(UiAction, usize, &str, &str, &str); 5] = [
                (
                    UiAction::TogglePause,
                    0,
                    "Pause",
                    "Stop the clock. Build freely while paused.",
                    "Space",
                ),
                (
                    UiAction::Speed(0),
                    1,
                    "Normal speed",
                    "One in-game day every 16 minutes.",
                    "1",
                ),
                (UiAction::Speed(1), 2, "Fast", "Three times normal speed.", "2"),
                (UiAction::Speed(2), 3, "Faster", "Six times normal speed.", "3"),
                (
                    UiAction::Speed(3),
                    4,
                    "Fastest",
                    "Twelve times normal speed: a day in 80 seconds.",
                    "4",
                ),
            ];
            for (i, (action, arrows, title, desc, key)) in defs.into_iter().enumerate() {
                spawn_button(
                    s,
                    UiButton::new(action, ButtonKind::Speed),
                    Node {
                        width: px(46.0 + 10.0),
                        height: px(40),
                        margin: UiRect::left(px(if i == 0 { 0.0 } else { -(40.0 * t::SLANT) - 1.0 })),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border: UiRect::all(px(1)),
                        ..default()
                    },
                    Some(Tooltip::new(title, desc).key(key)),
                    |b, id| {
                        let fg = ButtonFg {
                            owner: id,
                            base: t::SLOT_INK,
                        };
                        if arrows == 0 {
                            b.spawn((super::widgets::icon(fonts, icon::PAUSE, 17.0, t::SLOT_INK), fg));
                        }
                        for k in 0..arrows {
                            b.spawn((super::widgets::icon(fonts, icon::PLAY, 15.0, t::SLOT_INK), fg))
                                .insert(Node {
                                    width: px(15),
                                    height: px(15),
                                    margin: UiRect::left(px(if k == 0 { 0.0 } else { -8.0 })),
                                    ..default()
                                });
                        }
                    },
                );
            }
        });
    });
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_vitals(
    sim: Res<SimWorld>,
    time: Res<Time<Real>>,
    mut shown: ResMut<MoneyShown>,
    mut toasts: MessageWriter<Toast>,
    mut texts: Query<(&BarText, &mut Text, &mut TextColor)>,
    mut money_icon: Query<&mut TextColor, (With<MoneyIcon>, Without<BarText>)>,
    mut last: Local<Option<(u64, i64, i64, u32, u32, u32, usize, (citysim_sim::FiscalState, u32))>>,
) {
    let w = &sim.world;
    let money = w.economy.money as f64;
    if shown.generation != sim.generation {
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
    let s = &w.stats;
    let key = (
        sim.generation,
        money_shown,
        w.economy.money,
        s.population,
        (s.unemployment * 1000.0) as u32,
        s.workforce,
        w.economy.history.len(),
        (w.economy.fiscal.state, (w.economy.fiscal.service_level * 100.0) as u32),
    );
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let broke = w.economy.money < 0;
    if broke && !shown.warned_broke {
        shown.warned_broke = true;
        toasts.write(Toast::new(
            ToastKind::Warning,
            "Money is below zero: new roads wait, and at midnight city services are cut",
        ));
    } else if !broke {
        shown.warned_broke = false;
    }
    let booked = !w.economy.history.is_empty();
    let net = w.economy.last_day.income() - w.economy.last_day.expenses();
    let note = super::fiscal::fiscal_note(w);
    let delta_color = if let Some(n) = &note {
        n.color
    } else if !booked {
        t::TEXT_MUTED
    } else if net >= 0 {
        t::POSITIVE
    } else {
        t::NEGATIVE
    };
    let alarm = broke || note.as_ref().is_some_and(|n| n.color == t::NEGATIVE);
    for (which, mut text, mut color) in &mut texts {
        let (v, c): (String, Color) = match which {
            BarText::Money => (format::money(money_shown), if broke { t::NEGATIVE } else { t::TEXT }),
            BarText::MoneyDelta => (
                match (&note, booked) {
                    (Some(n), _) => n.short.to_uppercase(),
                    (None, true) => format!("{} / day", format::signed_money(net)).to_uppercase(),
                    (None, false) => "BOOKED AT MIDNIGHT".to_string(),
                },
                delta_color,
            ),
            BarText::Residents => (format::group(s.population as u64), t::TEXT),
            BarText::Jobless => {
                if s.workforce == 0 || s.jobs == 0 {
                    ("\u{2014}".to_string(), t::TEXT_MUTED)
                } else {
                    let u = s.unemployment;
                    let c = if u > 0.15 {
                        t::NEGATIVE
                    } else if u > 0.08 {
                        t::WARNING
                    } else {
                        t::TEXT
                    };
                    (format!("{}%", (u * 100.0).round() as i64), c)
                }
            }
            _ => continue,
        };
        if text.0 != v {
            text.0 = v;
        }
        color.set_if_neq(TextColor(c));
    }
    for mut c in &mut money_icon {
        c.set_if_neq(TextColor(if alarm { t::NEGATIVE } else { t::ACCENT_TEXT }));
    }
}

fn update_demand(
    time: Res<Time<Real>>,
    sim: Res<SimWorld>,
    mut fills: Query<(&mut DemandFill, &mut Node)>,
    mut surplus: Query<(&DemandSurplus, &mut Visibility)>,
) {
    let d = &sim.world.demand;
    let value = |z: Zone| {
        match z {
            Zone::Residential => d.residential,
            Zone::Commercial => d.commercial,
            Zone::Industrial => d.industrial,
        }
        .clamp(-1.0, 1.0)
    };
    let k = approach(time.delta_secs(), 0.35);
    for (mut fill, mut node) in &mut fills {
        let target = value(fill.zone).max(0.0);
        if fill.shown == target {
            continue;
        }
        fill.shown += (target - fill.shown) * k;
        if (target - fill.shown).abs() < 0.002 {
            fill.shown = target;
        }
        let h = percent(fill.shown * 100.0);
        if node.height != h {
            node.height = h;
        }
    }
    for (DemandSurplus(z), mut vis) in &mut surplus {
        vis.set_if_neq(if value(*z) < -0.15 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

/// "Tue · Day 12".
pub fn day_label(day: u32) -> String {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    format!("{} · Day {day}", DAYS[(day.max(1) - 1) as usize % 7])
}

fn update_clock(
    sim: Res<SimWorld>,
    mut texts: Query<(&BarText, &mut Text)>,
    mut marker: Query<&mut Node, With<DayMarker>>,
    mut last: Local<Option<(u32, u32)>>,
) {
    let w = &sim.world;
    let hour = w.time.hour();
    // The clock moves in 10-minute steps (a game minute is 1/6 s at 1x).
    let step = ((hour * 6.0) as u32).min(143);
    let key = (w.time.day, step);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let day = day_label(w.time.day);
    for (which, mut text) in &mut texts {
        let v = match which {
            BarText::Clock => format!("{:02}:{:02}", step / 6, step % 6 * 10),
            BarText::DayLabel | BarText::ClockCaption => day.to_uppercase(),
            _ => continue,
        };
        if text.0 != v {
            text.0 = v;
        }
    }
    if let Ok(mut n) = marker.single_mut() {
        let l = percent(hour / 24.0 * 100.0);
        if n.left != l {
            n.left = l;
        }
    }
}

/// Compact bar: hide the wide-only parts, show the narrow-only ones, make
/// the slots narrower.
#[allow(clippy::type_complexity)]
fn compact_bar(
    ui: Res<UiState>,
    mut wide: Query<&mut Node, (With<Wide>, Without<Narrow>, Without<ToolSlot>)>,
    mut narrow: Query<&mut Node, (With<Narrow>, Without<Wide>, Without<ToolSlot>)>,
    mut slots: Query<&mut Node, (With<ToolSlot>, Without<Wide>, Without<Narrow>)>,
    mut first: Local<bool>,
) {
    if !ui.is_changed() && *first {
        return;
    }
    *first = true;
    let c = ui.compact;
    for mut n in &mut wide {
        let d = if c { Display::None } else { Display::Flex };
        if n.display != d {
            n.display = d;
        }
    }
    for mut n in &mut narrow {
        let d = if c { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    }
    for mut n in &mut slots {
        let w = px(if c { 62.0 } else { t::SLOT_W });
        if n.width != w {
            n.width = w;
        }
    }
}

/// The active slot stands 10 px taller than the others; its drawn signal
/// glyph follows the slot's text colour.
#[allow(clippy::type_complexity)]
fn raise_slots(
    ui: Res<UiState>,
    mut slots: Query<(&super::widgets::ButtonAnim, &mut Node), With<ToolSlot>>,
    fgs: Query<(&ButtonFg, &TextColor)>,
    mut glyph: Query<(&SignalGlyph, Option<&mut BorderColor>, Option<&mut BackgroundColor>)>,
    children: Query<&Children>,
) {
    let base = if ui.compact { 64.0 } else { t::SLOT_H };
    let raised = if ui.compact { 72.0 } else { t::SLOT_ACTIVE_H };
    for (anim, mut node) in &mut slots {
        let h = px((base + (raised - base) * anim.active).round());
        if node.height != h {
            node.height = h;
        }
    }
    for (g, border, bg) in &mut glyph {
        // The colour of the slot's caps label.
        let Some(c) = children
            .get(g.0)
            .ok()
            .and_then(|ch| {
                ch.iter()
                    .find_map(|c| fgs.get(c).ok().filter(|(f, _)| f.base == t::SLOT_INK))
            })
            .map(|(_, c)| c.0)
        else {
            continue;
        };
        if let Some(mut b) = border {
            b.set_if_neq(BorderColor::all(c));
        }
        if let Some(mut b) = bg {
            b.set_if_neq(BackgroundColor(if g.1 { c } else { Color::NONE }));
        }
    }
}

/// Commute colour (the statistics drawer uses it): fine up to `COMMUTE_OK_S`,
/// red from `HOPELESS_COMMUTE_S`.
pub fn commute_color(secs: f32) -> Color {
    if secs >= balance::HOPELESS_COMMUTE_S {
        t::NEGATIVE
    } else if secs > balance::COMMUTE_OK_S {
        t::WARNING
    } else {
        t::TEXT
    }
}

/// Average commute: real driving seconds at 1x ("—" before the first
/// commute finished).
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

/// Is any overlay map on (for the maps slot's look)?
pub fn overlay_on(o: OverlayMode) -> bool {
    o != OverlayMode::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_texts_match_the_sim_clock() {
        // The speed tooltips say "16 minutes" and "a day in 80 seconds".
        assert_eq!(balance::TICKS_PER_DAY, 16 * 60 * citysim_sim::TICKS_PER_SECOND);
        assert_eq!(
            balance::TICKS_PER_DAY / crate::shared::SPEEDS[3] / citysim_sim::TICKS_PER_SECOND,
            80
        );
        assert_eq!(balance::COMMUTE_OK_S, 130.0);
    }

    #[test]
    fn commute_formatting() {
        assert_eq!(commute_text(0.0), "\u{2014}");
        assert_eq!(commute_text(42.4), "42 s");
        assert_eq!(commute_text(59.6), "1 min 00 s");
        assert_eq!(commute_text(125.0), "2 min 05 s");
    }

    #[test]
    fn day_labels_cycle_the_week() {
        assert_eq!(day_label(1), "Mon · Day 1");
        assert_eq!(day_label(12), "Fri · Day 12");
        assert_eq!(day_label(8), "Mon · Day 8");
    }
}
