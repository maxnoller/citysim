//! The budget drawer of the tray: the balance with a chart of the daily
//! net over the last two weeks (`economy.history`), income and costs by
//! line for yesterday (`economy.last_day`) next to today so far
//! (`economy::budget_now`), the tax rate (steps and a slider), and the
//! fiscal state with what would fix it; and where the running costs go
//! (`economy::running_costs`: each kind of road, traffic lights, services
//! by what drives them) with the share of shift hours worked, which
//! business tax follows (`economy.attendance`).

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use citysim_sim::economy::{RunningCosts, budget_now, running_costs};
use citysim_sim::{Budget, Command, DaySummary, Zone, balance};

use super::UiState;
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::tray::Page;
use super::widgets::{Tooltip, UiAction, caps, figure, label, layout, paragraph, spawn_stepper, title_tick};
use crate::shared::{GameSet, SimCommand, SimWorld};

pub struct BudgetPlugin;

impl Plugin for BudgetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SliderDrag>()
            .init_resource::<BudgetCache>()
            .add_systems(PostStartup, spawn_budget)
            .add_systems(Update, tax_slider_input.in_set(GameSet::Input))
            .add_systems(Update, (update_budget, update_chart).in_set(GameSet::Render));
    }
}

/// Lines of the by-category table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Residential,
    Commercial,
    Industrial,
    Income,
    Upkeep,
    Services,
    Expenses,
    Net,
}

impl Line {
    const ALL: [Line; 8] = [
        Line::Residential,
        Line::Commercial,
        Line::Industrial,
        Line::Income,
        Line::Upkeep,
        Line::Services,
        Line::Expenses,
        Line::Net,
    ];

    fn name(self) -> &'static str {
        match self {
            Line::Residential => "Residential tax",
            Line::Commercial => "Commercial tax",
            Line::Industrial => "Industrial tax",
            Line::Income => "Income",
            Line::Upkeep => "Road upkeep",
            Line::Services => "City services",
            Line::Expenses => "Expenses",
            Line::Net => "Net",
        }
    }

    /// Signed amount (expenses negative).
    fn amount(self, b: &Budget) -> i64 {
        match self {
            Line::Residential => b.residential_tax,
            Line::Commercial => b.commercial_tax,
            Line::Industrial => b.industrial_tax,
            Line::Income => b.income(),
            Line::Upkeep => -b.road_upkeep,
            Line::Services => -b.services,
            Line::Expenses => -b.expenses(),
            Line::Net => b.net(),
        }
    }

    fn zone(self) -> Option<Zone> {
        match self {
            Line::Residential => Some(Zone::Residential),
            Line::Commercial => Some(Zone::Commercial),
            Line::Industrial => Some(Zone::Industrial),
            _ => None,
        }
    }

    /// Totals read stronger than the categories.
    fn is_total(self) -> bool {
        matches!(self, Line::Income | Line::Expenses | Line::Net)
    }
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum BudgetText {
    Balance,
    BalanceNote,
    YesterdayHeader,
    /// Amount of line `.0`, column `.1` (0 yesterday, 1 today).
    Amount(usize, usize),
    TaxValue,
    FiscalTitle,
    FiscalBody,
    FiscalFix,
    /// Label / amount of running-cost line `.0` (`COST_LINES`).
    CostLabel(usize),
    Cost(usize),
    Attendance,
}

/// The running-cost lines: name and amount (a cost, positive).
const COST_LINES: [&str; 7] = [
    "Streets",
    "Avenues",
    "Highway",
    "Traffic lights",
    "Services: residents",
    "Services: city size",
    "Services: dense homes",
];

fn cost_line(c: &RunningCosts, i: usize) -> (String, i64) {
    match i {
        0 => ("Streets".into(), c.streets),
        1 => ("Avenues".into(), c.avenues),
        2 => ("Highway".into(), c.highway),
        3 => (format!("Traffic lights · {}", c.signal_count), c.signals),
        4 => ("Services: residents".into(), c.services_base),
        5 => ("Services: city size".into(), c.services_city_size),
        _ => ("Services: dense homes".into(), c.services_density),
    }
}

#[derive(Component)]
struct FiscalBar;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ChartText {
    Caption,
    AxisFirst,
    AxisLast,
    Empty,
}

#[derive(Component)]
struct ChartSlot(usize);

#[derive(Component)]
struct ChartBar(usize);

#[derive(Component)]
struct ZeroLine;

#[derive(Component)]
struct TaxTrack;

#[derive(Component)]
struct TaxFill;

#[derive(Component)]
struct TaxKnob;

/// Tax-rate slider drag in progress (with the last rate sent).
#[derive(Resource, Default)]
struct SliderDrag {
    active: bool,
    sent: Option<f32>,
}

/// Today's projection, refreshed a few times a second while open.
#[derive(Resource, Default)]
struct BudgetCache {
    today: Budget,
    costs: RunningCosts,
    age: f32,
    /// Inputs the texts were last written from.
    key: Option<BudgetKey>,
}

type BudgetKey = (
    u64,
    i64,
    u32,
    usize,
    Budget,
    u32,
    u32,
    (citysim_sim::FiscalState, citysim_sim::FiscalReason, u32),
    u32,
);

/// Width of the running-costs column.
const COSTS_W: f32 = 196.0;
/// Width of each amount column of the table.
const AMOUNT_W: f32 = 82.0;
/// Days in the chart.
pub const CHART_DAYS: usize = 14;
const CHART_H: f32 = 54.0;
const CHART_W: f32 = 236.0;
/// Seconds between refreshes of today's projection.
const PROJECTION_EVERY: f32 = 0.25;
/// Tax slider step (fraction).
pub const TAX_STEP: f32 = 0.005;
const TRACK_W: f32 = 220.0;

fn column(p: &mut ChildSpawnerCommands, width: f32, f: impl FnOnce(&mut ChildSpawnerCommands)) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Column,
        width: px(width),
        align_self: AlignSelf::Stretch,
        justify_content: JustifyContent::Center,
        ..default()
    }))
    .with_children(f);
}

fn rule(p: &mut ChildSpawnerCommands) {
    p.spawn((
        Node {
            width: px(t::LW),
            height: px(150),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(t::LINE),
        UiTransform {
            rotation: Rot2::degrees(6.0),
            ..default()
        },
        Pickable::IGNORE,
    ));
}

fn spawn_budget(mut commands: Commands, fonts: Res<UiFonts>, pages: Query<(Entity, &Page)>) {
    let fonts = fonts.into_inner();
    let Some((page, _)) = pages.iter().find(|p| *p.1 == Page::Budget) else {
        return;
    };
    commands.entity(page).insert(Name::new("Budget")).with_children(|pg| {
        // Title.
        pg.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            min_width: px(98),
            align_self: AlignSelf::Center,
            ..default()
        }))
        .with_children(|c| {
            c.spawn(title_tick());
            c.spawn((
                Text::new("BUDGET"),
                fonts.figures(t::FONT_TITLE, t::W_BOLD),
                t::caps_spacing(t::FONT_TITLE, 0.12),
                TextColor(t::TEXT),
                Pickable::IGNORE,
            ));
            c.spawn((
                caps(fonts, "Booked at midnight", t::FONT_CAP, t::TEXT_MUTED),
                Node {
                    margin: UiRect::top(px(6)),
                    ..default()
                },
            ));
        });
        rule(pg);
        // Balance and chart.
        column(pg, CHART_W, |c| {
            c.spawn(caps(fonts, "Balance", t::FONT_CAP, t::TEXT_MUTED));
            c.spawn((
                figure(fonts, "", t::FONT_BIG, t::TEXT),
                BudgetText::Balance,
                Node {
                    margin: UiRect::vertical(px(4)),
                    ..default()
                },
            ));
            c.spawn((
                paragraph(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                BudgetText::BalanceNote,
                Node {
                    width: px(CHART_W),
                    height: px(34),
                    overflow: Overflow::clip(),
                    ..default()
                },
            ));
            chart(c, fonts);
        });
        rule(pg);
        // Table.
        column(pg, 150.0 + 2.0 * AMOUNT_W, |c| {
            c.spawn(layout(Node {
                justify_content: JustifyContent::End,
                margin: UiRect::bottom(px(4)),
                ..default()
            }))
            .with_children(|h| {
                for (i, txt) in ["", "Today, est."].into_iter().enumerate() {
                    let mut e = h.spawn((
                        caps(fonts, txt, 10.5, t::TEXT_MUTED),
                        Node {
                            width: px(AMOUNT_W),
                            justify_content: JustifyContent::End,
                            ..default()
                        },
                        super::widgets::Justified(Justify::Right),
                    ));
                    if i == 0 {
                        e.insert(BudgetText::YesterdayHeader);
                    }
                }
            });
            for (li, line) in Line::ALL.iter().enumerate() {
                let total = line.is_total();
                c.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    height: px(if total { 21.0 } else { 18.0 }),
                    border: UiRect::top(px(if total { 1.0 } else { 0.0 })),
                    margin: UiRect::bottom(px(if total && *line != Line::Net { 4.0 } else { 0.0 })),
                    ..default()
                }))
                .insert(BorderColor::all(t::RULE))
                .with_children(|r| {
                    r.spawn(layout(Node {
                        width: px(150),
                        align_items: AlignItems::Center,
                        column_gap: px(7),
                        ..default()
                    }))
                    .with_children(|n| {
                        if let Some(z) = line.zone() {
                            n.spawn((
                                layout(Node {
                                    width: px(7),
                                    height: px(7),
                                    ..default()
                                }),
                                BackgroundColor(t::zone_color(Some(z))),
                            ));
                        }
                        if total {
                            n.spawn(caps(fonts, line.name(), 11.0, t::TEXT));
                        } else {
                            n.spawn(label(fonts, line.name(), t::FONT_BODY, t::W_MEDIUM, t::SLOT_INK));
                        }
                    });
                    for col in 0..2 {
                        r.spawn((
                            figure(fonts, "", if total { 16.0 } else { 14.0 }, t::TEXT),
                            BudgetText::Amount(li, col),
                            Node {
                                width: px(AMOUNT_W),
                                justify_content: JustifyContent::End,
                                ..default()
                            },
                            super::widgets::Justified(Justify::Right),
                        ));
                    }
                });
            }
        });
        rule(pg);
        // Where the running costs go (today, if the day ended now).
        column(pg, COSTS_W, |c| {
            c.spawn((
                caps(fonts, "Running costs · today", 10.5, t::TEXT_MUTED),
                Node {
                    margin: UiRect::bottom(px(4)),
                    ..default()
                },
                Tooltip::new(
                    "Running costs",
                    "What the roads and city services cost per day at the city's size now.",
                ),
                Hovered::default(),
            ));
            for i in 0..COST_LINES.len() {
                c.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::SpaceBetween,
                    height: px(17),
                    border: UiRect::top(px(if i == 4 { 1.0 } else { 0.0 })),
                    ..default()
                }))
                .insert(BorderColor::all(t::RULE))
                .with_children(|r| {
                    r.spawn((
                        label(fonts, COST_LINES[i], t::FONT_SMALL, t::W_MEDIUM, t::SLOT_INK),
                        BudgetText::CostLabel(i),
                    ));
                    r.spawn((
                        figure(fonts, "", 13.0, t::TEXT),
                        BudgetText::Cost(i),
                        super::widgets::Justified(Justify::Right),
                    ));
                });
            }
            c.spawn((
                paragraph(fonts, "", t::FONT_SMALL - 0.5, t::W_MEDIUM, t::TEXT_MUTED),
                BudgetText::Attendance,
                Node {
                    width: px(COSTS_W),
                    margin: UiRect::top(px(6)),
                    ..default()
                },
            ));
        });
        rule(pg);
        // Taxes and the fiscal state.
        column(pg, 262.0, |c| {
            c.spawn(caps(fonts, "Tax rate", t::FONT_CAP, t::TEXT_MUTED));
            c.spawn(layout(Node {
                align_items: AlignItems::Center,
                column_gap: px(8),
                margin: UiRect::vertical(px(5)),
                ..default()
            }))
            .with_children(|r| {
                r.spawn((
                    figure(fonts, "", 30.0, t::ACCENT_TEXT),
                    BudgetText::TaxValue,
                    Node {
                        min_width: px(70),
                        ..default()
                    },
                ));
                spawn_stepper(
                    r,
                    fonts,
                    UiAction::TaxStep(-5),
                    icon::MINUS,
                    Tooltip::new("Lower taxes", "Half a percent less. Lower taxes make people happier."),
                );
                spawn_stepper(
                    r,
                    fonts,
                    UiAction::TaxStep(5),
                    icon::PLUS,
                    Tooltip::new("Raise taxes", "Half a percent more. Higher taxes cost happiness."),
                );
            });
            // Slider.
            c.spawn((
                Node {
                    width: px(TRACK_W),
                    height: px(18),
                    justify_content: JustifyContent::Center,
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                TaxTrack,
                Hovered::default(),
                Tooltip::new(
                    "Tax rate",
                    "Drag to set the tax on homes, shops and industry (0% to 20%).",
                ),
            ))
            .with_children(|tr| {
                tr.spawn((
                    layout(Node {
                        height: px(6),
                        width: percent(100),
                        border: UiRect::all(px(1)),
                        ..default()
                    }),
                    BackgroundColor(t::WELL),
                    BorderColor::all(t::LINE),
                ))
                .with_child((
                    layout(Node {
                        height: percent(100),
                        width: percent(0),
                        ..default()
                    }),
                    BackgroundColor(t::ACCENT),
                    TaxFill,
                ));
                tr.spawn((
                    layout(Node {
                        position_type: PositionType::Absolute,
                        top: px(0),
                        width: px(6),
                        height: px(18),
                        margin: UiRect::left(px(-3)),
                        left: percent(0),
                        ..default()
                    }),
                    BackgroundColor(t::TEXT),
                    TaxKnob,
                ));
            });
            c.spawn(layout(Node {
                width: px(TRACK_W),
                justify_content: JustifyContent::SpaceBetween,
                margin: UiRect::bottom(px(10)),
                ..default()
            }))
            .with_children(|r| {
                r.spawn(caps(fonts, "0%", 10.0, t::TEXT_FAINT));
                r.spawn(caps(fonts, format::percent(balance::MAX_TAX_RATE), 10.0, t::TEXT_FAINT));
            });
            // Fiscal state.
            c.spawn(layout(Node {
                column_gap: px(9),
                ..default()
            }))
            .with_children(|f| {
                f.spawn((
                    layout(Node {
                        width: px(4),
                        align_self: AlignSelf::Stretch,
                        ..default()
                    }),
                    BackgroundColor(t::POSITIVE),
                    FiscalBar,
                ));
                f.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(2),
                    width: px(248),
                    ..default()
                }))
                .with_children(|col| {
                    col.spawn((caps(fonts, "", t::FONT_CAP, t::POSITIVE), BudgetText::FiscalTitle));
                    col.spawn((
                        paragraph(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT),
                        BudgetText::FiscalBody,
                        Node {
                            width: px(248),
                            ..default()
                        },
                    ));
                    col.spawn((
                        paragraph(fonts, "", t::FONT_SMALL, t::W_SEMIBOLD, t::ACCENT_TEXT),
                        BudgetText::FiscalFix,
                        Node {
                            width: px(248),
                            ..default()
                        },
                    ));
                });
            });
        });
    });
}

/// The chart of the daily net: one bar per day, the newest on the right.
fn chart(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        margin: UiRect::top(px(8)),
        ..default()
    }))
    .with_children(|r| {
        r.spawn((caps(fonts, "", 10.0, t::TEXT_FAINT), ChartText::AxisFirst));
        r.spawn((caps(fonts, "", 10.0, t::TEXT), ChartText::AxisLast));
    });
    p.spawn((
        layout(Node {
            height: px(CHART_H),
            width: px(CHART_W),
            margin: UiRect::vertical(px(3)),
            column_gap: px(3),
            border: UiRect::bottom(px(1)),
            ..default()
        }),
        BorderColor::all(t::RULE),
    ))
    .with_children(|c| {
        c.spawn((
            layout(Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                height: px(1),
                top: percent(100),
                ..default()
            }),
            BackgroundColor(t::LINE),
            ZeroLine,
        ));
        for i in 0..CHART_DAYS {
            c.spawn((
                Node {
                    flex_grow: 1.0,
                    height: percent(100),
                    ..default()
                },
                ChartSlot(i),
                Hovered::default(),
            ))
            .with_child((
                layout(Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    right: px(0),
                    top: percent(100),
                    height: px(0),
                    ..default()
                }),
                BackgroundColor(Color::NONE),
                ChartBar(i),
            ));
        }
        c.spawn((
            label(
                fonts,
                "The first day's figures come at midnight",
                11.5,
                t::W_MEDIUM,
                t::TEXT_FAINT,
            ),
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(18),
                ..default()
            },
            ChartText::Empty,
        ));
    });
    p.spawn((label(fonts, "", 11.5, t::W_MEDIUM, t::TEXT_MUTED), ChartText::Caption));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_budget(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    sim: Res<SimWorld>,
    mut cache: ResMut<BudgetCache>,
    mut texts: Query<(&BudgetText, &mut Text, &mut TextColor)>,
    mut fill: Query<&mut Node, (With<TaxFill>, Without<TaxKnob>)>,
    mut knob: Query<&mut Node, (With<TaxKnob>, Without<TaxFill>)>,
    mut bar: Query<&mut BackgroundColor, With<FiscalBar>>,
) {
    if !state.budget_open() {
        cache.key = None;
        cache.age = f32::INFINITY;
        return;
    }
    let w = &sim.world;
    let e = &w.economy;
    cache.age += time.delta_secs();
    if cache.age >= PROJECTION_EVERY || cache.key.is_some_and(|k| k.0 != sim.generation) {
        cache.age = 0.0;
        cache.today = budget_now(w);
        cache.costs = running_costs(w);
    }
    let key = (
        sim.generation,
        e.money,
        e.tax_rate.to_bits(),
        e.history.len(),
        cache.today,
        w.stats.road_cells,
        w.stats.population,
        (e.fiscal.state, e.fiscal.reason, e.fiscal.left_last_night),
        e.attendance.share().to_bits(),
    );
    if cache.key == Some(key) {
        return;
    }
    cache.key = Some(key);
    let booked = !e.history.is_empty();
    let note = super::fiscal::fiscal_note(w);
    let note_color = note.as_ref().map_or(t::POSITIVE, |n| n.color);
    if let Ok(mut b) = bar.single_mut() {
        b.set_if_neq(BackgroundColor(note_color));
    }
    for (which, mut text, mut color) in &mut texts {
        let (s, c) = match *which {
            BudgetText::Balance => (format::money(e.money), if e.money < 0 { t::NEGATIVE } else { t::TEXT }),
            BudgetText::BalanceNote => balance_note(e.money, booked.then_some(e.last_day.net()), cache.today.net()),
            BudgetText::FiscalTitle => (
                note.as_ref().map_or("Healthy".to_string(), |n| n.title.to_uppercase()),
                note_color,
            ),
            BudgetText::FiscalBody => (
                note.as_ref().map(|n| n.body.clone()).unwrap_or_else(|| {
                    format!(
                        "City services at {}. Taxes cover the roads and services.",
                        format::percent(e.fiscal.service_level)
                    )
                }),
                t::TEXT,
            ),
            BudgetText::FiscalFix => (
                note.as_ref().map(|n| format!("Fix: {}", n.fix)).unwrap_or_default(),
                t::ACCENT_TEXT,
            ),
            BudgetText::YesterdayHeader => (
                match e.history.last() {
                    Some(d) => format!("DAY {}", d.day),
                    None => "YESTERDAY".into(),
                },
                t::TEXT_MUTED,
            ),
            BudgetText::Amount(li, col) => {
                let line = Line::ALL[li];
                if col == 0 && !booked {
                    ("\u{2014}".to_string(), t::TEXT_FAINT)
                } else {
                    let b = if col == 0 { &e.last_day } else { &cache.today };
                    let v = line.amount(b);
                    let c = if line.is_total() {
                        signed_color(v)
                    } else if col == 1 {
                        t::TEXT_MUTED
                    } else {
                        t::TEXT
                    };
                    (format::signed_money(v), c)
                }
            }
            BudgetText::TaxValue => (format::percent(e.tax_rate), t::ACCENT_TEXT),
            BudgetText::CostLabel(i) => (cost_line(&cache.costs, i).0, t::SLOT_INK),
            BudgetText::Cost(i) => {
                let v = cost_line(&cache.costs, i).1;
                (
                    if v == 0 {
                        "\u{2014}".to_string()
                    } else {
                        format::signed_money(-v)
                    },
                    if v == 0 { t::TEXT_FAINT } else { t::TEXT },
                )
            }
            BudgetText::Attendance => {
                let share = e.attendance.share();
                (
                    format!(
                        "Shops and works pay tax on the hours their staff are at work: {} of shift hours lately{}.",
                        format::percent((share * 100.0).round() / 100.0),
                        if share < 0.9 {
                            " (late commutes cost the rest)"
                        } else {
                            ""
                        }
                    ),
                    if share < 0.8 { t::WARNING } else { t::TEXT_MUTED },
                )
            }
        };
        set_text(&mut text, &mut color, s, c);
    }
    let frac = (e.tax_rate / balance::MAX_TAX_RATE).clamp(0.0, 1.0) * 100.0;
    if let Ok(mut n) = fill.single_mut()
        && n.width != percent(frac)
    {
        n.width = percent(frac);
    }
    if let Ok(mut n) = knob.single_mut()
        && n.left != percent(frac)
    {
        n.left = percent(frac);
    }
}

#[allow(clippy::too_many_arguments)]
fn tax_slider_input(
    mouse: Res<ButtonInput<MouseButton>>,
    state: Res<UiState>,
    sim: Res<SimWorld>,
    ui_scale: Res<UiScale>,
    window: Query<&Window, With<PrimaryWindow>>,
    track: Query<(&Hovered, &ComputedNode, &UiGlobalTransform), With<TaxTrack>>,
    mut drag: ResMut<SliderDrag>,
    mut out: MessageWriter<SimCommand>,
) {
    let Ok((hovered, node, tf)) = track.single() else {
        return;
    };
    if !state.budget_open() {
        drag.active = false;
        return;
    }
    if mouse.just_pressed(MouseButton::Left) && hovered.get() {
        drag.active = true;
        drag.sent = None;
    }
    if !mouse.pressed(MouseButton::Left) {
        drag.active = false;
    }
    if !drag.active {
        return;
    }
    let Some(cursor) = window.single().ok().and_then(|w| w.cursor_position()) else {
        return;
    };
    let inv = node.inverse_scale_factor;
    let cursor = cursor / ui_scale.0.max(0.01);
    let (center, width) = (tf.translation.x * inv, node.size.x * inv);
    let frac = ((cursor.x - (center - width * 0.5)) / width.max(1.0)).clamp(0.0, 1.0);
    let rate = ((frac * balance::MAX_TAX_RATE / TAX_STEP).round() * TAX_STEP).clamp(0.0, balance::MAX_TAX_RATE);
    if (rate - sim.world.economy.tax_rate).abs() > 1e-4 && drag.sent != Some(rate) {
        drag.sent = Some(rate);
        out.write(SimCommand(Command::SetTaxRate(rate)));
    }
}

/// The line beside the balance: calm, and only what helps the player now.
fn balance_note(money: i64, yesterday: Option<i64>, today: i64) -> (String, Color) {
    if money < 0 {
        (
            "In debt: new roads wait until taxes lift it above zero. Bulldozing still works.".into(),
            t::NEGATIVE,
        )
    } else if yesterday.is_none() {
        ("Taxes and costs are booked at midnight".into(), t::TEXT_MUTED)
    } else if today < 0 {
        (
            "Today's costs outrun taxes: grow the city or raise taxes".into(),
            t::WARNING,
        )
    } else {
        let net = yesterday.unwrap_or(0);
        (format!("{} yesterday", format::signed_money(net)), signed_color(net))
    }
}

fn signed_color(v: i64) -> Color {
    match v.signum() {
        1 => t::POSITIVE,
        -1 => t::NEGATIVE,
        _ => t::TEXT_MUTED,
    }
}

fn set_text(text: &mut Mut<Text>, color: &mut Mut<TextColor>, s: String, c: Color) {
    if text.0 != s {
        text.0 = s;
    }
    color.set_if_neq(TextColor(c));
}

/// Vertical layout of the chart: `(zero line from the top, scale)` as
/// fractions of the chart height, for the days shown.
pub fn chart_scale(nets: &[i64]) -> (f32, f32) {
    let max = nets.iter().copied().max().unwrap_or(0).max(0) as f32;
    let min = nets.iter().copied().min().unwrap_or(0).min(0) as f32;
    let range = (max - min).max(1.0);
    (max / range, 1.0 / range)
}

/// Bar `(top, height)` as fractions of the chart height.
pub fn bar_extent(net: i64, zero: f32, scale: f32) -> (f32, f32) {
    let h = net.unsigned_abs() as f32 * scale;
    if net >= 0 { (zero - h, h) } else { (zero, h) }
}

fn plural_days(n: usize) -> String {
    if n == 1 { "DAY".into() } else { format!("{n} DAYS") }
}

/// The days the chart shows, oldest first, right-aligned in its slots.
fn shown_days(history: &[DaySummary]) -> &[DaySummary] {
    &history[history.len().saturating_sub(CHART_DAYS)..]
}

#[allow(clippy::type_complexity)]
fn update_chart(
    state: Res<UiState>,
    sim: Res<SimWorld>,
    slots: Query<(&ChartSlot, &Hovered)>,
    mut bars: Query<(&ChartBar, &mut Node, &mut BackgroundColor), Without<ZeroLine>>,
    mut zero_line: Query<&mut Node, (With<ZeroLine>, Without<ChartBar>)>,
    mut texts: Query<(&ChartText, &mut Text, &mut TextColor, &mut Node), (Without<ChartBar>, Without<ZeroLine>)>,
    mut last: Local<Option<(u64, usize, Option<usize>, bool)>>,
) {
    if !state.budget_open() {
        *last = None;
        return;
    }
    let history = &sim.world.economy.history;
    let days = shown_days(history);
    let offset = CHART_DAYS - days.len();
    let hovered = slots
        .iter()
        .find(|(s, h)| h.get() && s.0 >= offset)
        .map(|(s, _)| s.0 - offset);
    // Only touch nodes when the data or the hovered day changed.
    let key = (sim.generation, history.len(), hovered, sim.world.economy.money < 0);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let nets: Vec<i64> = days.iter().map(|d| d.income - d.expenses).collect();
    let (zero, scale) = chart_scale(&nets);
    let selected = hovered.or(days.len().checked_sub(1));
    for (bar, mut node, mut bg) in &mut bars {
        let (top, height, color) = match bar.0.checked_sub(offset).map(|i| (i, nets[i])) {
            Some((i, net)) => {
                let (top, h) = bar_extent(net, zero, scale);
                let base = if net >= 0 { t::POSITIVE } else { t::NEGATIVE };
                let a = if Some(i) == selected { 1.0 } else { 0.55 };
                (top, h, t::alpha(base, a))
            }
            None => (zero, 0.0, Color::NONE),
        };
        let (top, height) = (percent(top * 100.0), px((height * CHART_H).max(2.0)));
        if node.top != top || node.height != height {
            node.top = top;
            node.height = height;
        }
        bg.set_if_neq(BackgroundColor(color));
    }
    if let Ok(mut n) = zero_line.single_mut() {
        let top = percent(zero * 100.0);
        if n.top != top {
            n.top = top;
        }
    }
    let caption = selected.map(|i| {
        let d = &days[i];
        (
            format!("Day {}", d.day),
            format!(
                "{} net · balance {}",
                format::signed_money(nets[i]),
                format::money(d.money)
            ),
        )
    });
    for (which, mut text, mut color, mut node) in &mut texts {
        let (s, c, show) = match which {
            ChartText::AxisLast => (
                caption.as_ref().map(|c| c.0.clone()).unwrap_or_default(),
                t::TEXT,
                caption.is_some(),
            ),
            ChartText::Caption => (
                caption.as_ref().map(|c| c.1.clone()).unwrap_or_default(),
                t::TEXT_MUTED,
                caption.is_some(),
            ),
            ChartText::AxisFirst => (
                if days.is_empty() {
                    String::new()
                } else {
                    format!("LAST {}", plural_days(days.len()))
                },
                t::TEXT_FAINT,
                true,
            ),
            ChartText::Empty => (text.0.clone(), t::TEXT_FAINT, days.is_empty()),
        };
        set_text(&mut text, &mut color, s, c);
        let d = if show { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_scale_puts_zero_where_it_belongs() {
        assert_eq!(chart_scale(&[100, 50]), (1.0, 0.01));
        let (zero, scale) = chart_scale(&[300, -100]);
        assert!((zero - 0.75).abs() < 1e-6);
        assert_eq!(bar_extent(300, zero, scale), (0.0, 0.75));
        let (top, h) = bar_extent(-100, zero, scale);
        assert!((top - 0.75).abs() < 1e-6 && (h - 0.25).abs() < 1e-6);
        // No data or all zero: a flat line at the bottom, no division by 0.
        assert_eq!(chart_scale(&[]), (0.0, 1.0));
        assert_eq!(chart_scale(&[0, 0]).0, 0.0);
    }

    #[test]
    fn lines_add_up_like_the_sim() {
        let b = Budget {
            residential_tax: 100,
            commercial_tax: 50,
            industrial_tax: 25,
            road_upkeep: 40,
            services: 30,
        };
        let sum: i64 = [
            Line::Residential,
            Line::Commercial,
            Line::Industrial,
            Line::Upkeep,
            Line::Services,
        ]
        .iter()
        .map(|l| l.amount(&b))
        .sum();
        assert_eq!(sum, Line::Net.amount(&b));
        assert_eq!(Line::Income.amount(&b), b.income());
        assert_eq!(Line::Expenses.amount(&b), -b.expenses());
    }
}
