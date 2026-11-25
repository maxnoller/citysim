//! Budget panel (opens from the money chip): balance, a chart of the daily
//! net over the last two weeks (`economy.history`), the budget by category
//! for yesterday (`economy.last_day`) next to today's projection
//! (`economy::budget_now`), and the tax-rate slider.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use citysim_sim::economy::budget_now;
use citysim_sim::{Budget, Command, DaySummary, Zone, balance};

use super::UiState;
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, hdivider, label, layout, number, panel, paragraph,
    spawn_button,
};
use crate::shared::{GameSet, SimCommand, SimWorld};

pub struct BudgetPlugin;

impl Plugin for BudgetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SliderDrag>()
            .init_resource::<BudgetCache>()
            .add_systems(Startup, spawn_budget)
            .add_systems(Update, tax_slider_input.in_set(GameSet::Input))
            .add_systems(Update, (update_budget, update_chart).in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct BudgetRoot;

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
    UpkeepCaption,
    ServicesCaption,
    /// `(line, 0 = yesterday / 1 = today)`.
    Amount(usize, usize),
    YesterdayHeader,
    TaxValue,
    FiscalTitle,
    FiscalBody,
    FiscalFix,
    FiscalIcon,
}

/// The fiscal status box (shown when the budget is not healthy).
#[derive(Component)]
struct FiscalBox;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ChartText {
    Caption,
    AxisFirst,
    AxisLast,
    Empty,
}

/// One day's column in the chart (hoverable), counted from the left.
#[derive(Component, Clone, Copy)]
struct ChartSlot(usize);

#[derive(Component, Clone, Copy)]
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
);

const PANEL_W: f32 = 372.0;
/// Width of each amount column of the table.
const AMOUNT_W: f32 = 78.0;
/// Days in the chart.
pub const CHART_DAYS: usize = 14;
const CHART_H: f32 = 52.0;
/// Seconds between refreshes of today's projection.
const PROJECTION_EVERY: f32 = 0.25;
/// Tax slider step (fraction).
pub const TAX_STEP: f32 = 0.005;

fn divider(p: &mut ChildSpawnerCommands, top: f32, bottom: f32) {
    p.spawn(hdivider()).insert(Node {
        height: px(1),
        width: percent(100),
        margin: UiRect::new(px(0), px(0), px(top), px(bottom)),
        ..default()
    });
}

fn amount_cell(r: &mut ChildSpawnerCommands, fonts: &UiFonts, text: impl Bundle, s: &str, weight: u16, color: Color) {
    r.spawn(layout(Node {
        width: px(AMOUNT_W),
        justify_content: JustifyContent::End,
        flex_shrink: 0.0,
        ..default()
    }))
    .with_children(|c| {
        c.spawn((number(fonts, s, t::FONT_BODY, weight, color), text));
    });
}

fn table(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    // Column headers.
    p.spawn(layout(Node {
        align_items: AlignItems::Center,
        margin: UiRect::bottom(px(t::S2)),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(layout(Node {
            flex_grow: 1.0,
            ..default()
        }))
        .with_child(label(
            fonts,
            "BY CATEGORY",
            t::FONT_CAPTION + 0.5,
            t::W_SEMIBOLD,
            t::TEXT_FAINT,
        ));
        for (i, h) in ["YESTERDAY", "TODAY"].into_iter().enumerate() {
            let mut col = r.spawn(layout(Node {
                width: px(AMOUNT_W),
                justify_content: JustifyContent::End,
                flex_shrink: 0.0,
                ..default()
            }));
            if i == 1 {
                col.insert(super::hud::info_tip(
                    "Today",
                    "What midnight would book if the day ended now: taxes from today's residents and \
                     workers, upkeep for today's roads, services for today's residents.",
                ));
            }
            col.with_children(|c| {
                let mut e = c.spawn(label(fonts, h, t::FONT_CAPTION + 0.5, t::W_SEMIBOLD, t::TEXT_FAINT));
                if i == 0 {
                    e.insert(BudgetText::YesterdayHeader);
                }
            });
        }
    });
    for (li, line) in Line::ALL.into_iter().enumerate() {
        if line == Line::Upkeep || line == Line::Net {
            divider(p, t::S1 + 1.0, t::S1 + 1.0);
        }
        let weight = if line.is_total() { t::W_SEMIBOLD } else { t::W_MEDIUM };
        p.spawn(layout(Node {
            align_items: AlignItems::Center,
            min_height: px(19),
            ..default()
        }))
        .with_children(|r| {
            r.spawn(layout(Node {
                flex_grow: 1.0,
                align_items: AlignItems::Center,
                column_gap: px(t::S3),
                ..default()
            }))
            .with_children(|l| {
                if let Some(z) = line.zone() {
                    l.spawn((
                        layout(Node {
                            width: px(7),
                            height: px(7),
                            border_radius: BorderRadius::MAX,
                            ..default()
                        }),
                        BackgroundColor(t::zone_color(Some(z))),
                    ));
                }
                let color = if line.is_total() { t::TEXT } else { t::TEXT_MUTED };
                l.spawn(label(fonts, line.name(), t::FONT_BODY, weight, color));
                let caption = match line {
                    Line::Upkeep => Some(BudgetText::UpkeepCaption),
                    Line::Services => Some(BudgetText::ServicesCaption),
                    _ => None,
                };
                if let Some(c) = caption {
                    l.spawn((label(fonts, "", t::FONT_SMALL, t::W_REGULAR, t::TEXT_FAINT), c));
                }
            });
            for col in 0..2 {
                amount_cell(r, fonts, BudgetText::Amount(li, col), "", weight, t::TEXT);
            }
        });
    }
}

fn chart(p: &mut ChildSpawnerCommands, fonts: &UiFonts) {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Baseline,
        margin: UiRect::bottom(px(t::S3)),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(label(
            fonts,
            "DAILY NET",
            t::FONT_CAPTION + 0.5,
            t::W_SEMIBOLD,
            t::TEXT_FAINT,
        ));
        r.spawn((
            number(fonts, "", t::FONT_CAPTION + 0.5, t::W_MEDIUM, t::TEXT_FAINT),
            ChartText::AxisFirst,
        ));
    });
    p.spawn(layout(Node {
        height: px(CHART_H),
        width: percent(100),
        ..default()
    }))
    .with_children(|area| {
        area.spawn((
            ZeroLine,
            layout(Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                top: percent(100),
                height: px(1),
                ..default()
            }),
            BackgroundColor(t::alpha(t::TEXT, 0.22)),
        ));
        for i in 0..CHART_DAYS {
            // Hover target: the whole column, bigger than the bar.
            area.spawn((
                ChartSlot(i),
                Hovered::default(),
                Node {
                    flex_grow: 1.0,
                    flex_basis: px(0),
                    height: percent(100),
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ))
            .with_child((
                ChartBar(i),
                layout(Node {
                    position_type: PositionType::Absolute,
                    left: px(2),
                    right: px(2),
                    top: percent(100),
                    height: px(0),
                    border_radius: BorderRadius::all(px(2)),
                    ..default()
                }),
                BackgroundColor(t::POSITIVE),
            ));
        }
        area.spawn((
            ChartText::Empty,
            paragraph(
                fonts,
                "The first day is booked at midnight: taxes in, upkeep and city services out.",
                t::FONT_SMALL,
                t::W_REGULAR,
                t::TEXT_FAINT,
            ),
            Node {
                position_type: PositionType::Absolute,
                left: px(t::S4),
                right: px(t::S4),
                top: px(CHART_H * 0.5 - 16.0),
                ..default()
            },
        ))
        .insert(TextLayout::justify(Justify::Center));
    });
    // The selected (hovered, else latest) day, labelled directly.
    p.spawn(layout(Node {
        align_items: AlignItems::Baseline,
        column_gap: px(t::S3),
        margin: UiRect::top(px(t::S3)),
        ..default()
    }))
    .with_children(|r| {
        r.spawn((
            number(fonts, "", t::FONT_SMALL, t::W_SEMIBOLD, t::TEXT),
            ChartText::AxisLast,
        ));
        r.spawn((
            number(fonts, "", t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED),
            ChartText::Caption,
        ));
    });
}

fn spawn_budget(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Budget panel"),
            BudgetRoot,
            panel(Node {
                position_type: PositionType::Absolute,
                top: px(t::BELOW_TOP_BAR),
                left: px(t::EDGE),
                width: px(PANEL_W),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(px(t::S5), px(t::S5), px(t::S4), px(t::S5)),
                display: Display::None,
                ..default()
            }),
            UiTransform::default(),
            GlobalZIndex(20),
        ))
        .with_children(|p| {
            // Header.
            p.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                margin: UiRect::bottom(px(t::S3)),
                ..default()
            }))
            .with_children(|h| {
                h.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(t::S3),
                    ..default()
                }))
                .with_children(|l| {
                    l.spawn(super::widgets::icon(fonts, icon::LANDMARK, t::ICON_MD, t::ACCENT));
                    l.spawn(label(fonts, "Budget", t::FONT_TITLE, t::W_SEMIBOLD, t::TEXT));
                });
                close_button(h, fonts, UiAction::CloseBudget);
            });
            p.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::End,
                column_gap: px(t::S4),
                ..default()
            }))
            .with_children(|r| {
                r.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    ..default()
                }))
                .with_children(|c| {
                    c.spawn(label(
                        fonts,
                        "BALANCE",
                        t::FONT_CAPTION + 0.5,
                        t::W_SEMIBOLD,
                        t::TEXT_FAINT,
                    ));
                    c.spawn((
                        number(fonts, "$0", t::FONT_DISPLAY, t::W_BOLD, t::TEXT),
                        BudgetText::Balance,
                    ));
                });
                r.spawn((
                    paragraph(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                    BudgetText::BalanceNote,
                    Node {
                        max_width: px(150),
                        margin: UiRect::bottom(px(t::S2)),
                        ..default()
                    },
                ))
                .insert(TextLayout::justify(Justify::Right));
            });
            // Fiscal status: what is wrong and what would fix it.
            p.spawn((
                FiscalBox,
                layout(Node {
                    margin: UiRect::top(px(t::S3 + 2.0)),
                    padding: UiRect::new(px(t::S3 + 2.0), px(t::S3 + 2.0), px(t::S3), px(t::S3 + 1.0)),
                    column_gap: px(t::S3 + 1.0),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(px(t::RADIUS_SM)),
                    display: Display::None,
                    ..default()
                }),
                BackgroundColor(t::alpha(t::WARNING, 0.10)),
                BorderColor::all(t::alpha(t::WARNING, 0.35)),
            ))
            .with_children(|b| {
                b.spawn((
                    super::widgets::icon(fonts, icon::ALERT, t::ICON_MD, t::WARNING),
                    BudgetText::FiscalIcon,
                ));
                b.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(t::S1 + 1.0),
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    ..default()
                }))
                .with_children(|c| {
                    c.spawn((
                        label(fonts, "", t::FONT_BODY, t::W_SEMIBOLD, t::WARNING),
                        BudgetText::FiscalTitle,
                    ));
                    c.spawn((
                        paragraph(fonts, "", t::FONT_SMALL, t::W_REGULAR, t::TEXT),
                        BudgetText::FiscalBody,
                    ));
                    c.spawn((
                        paragraph(fonts, "", t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                        BudgetText::FiscalFix,
                    ));
                });
            });
            divider(p, t::S3 + 2.0, t::S3 + 2.0);
            chart(p, fonts);
            divider(p, t::S3 + 2.0, t::S3 + 2.0);
            table(p, fonts);
            divider(p, t::S3 + 2.0, t::S3 + 2.0);
            // Tax rate.
            p.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                margin: UiRect::bottom(px(t::S3)),
                ..default()
            }))
            .with_children(|r| {
                r.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(t::S3),
                    ..default()
                }))
                .with_children(|l| {
                    l.spawn(super::widgets::icon(fonts, icon::PERCENT, t::ICON_SM, t::TEXT_MUTED));
                    l.spawn(label(fonts, "Tax rate", t::FONT_BODY, t::W_MEDIUM, t::TEXT));
                    l.spawn(label(
                        fonts,
                        "Higher taxes slow growth",
                        t::FONT_SMALL,
                        t::W_REGULAR,
                        t::TEXT_FAINT,
                    ));
                });
                r.spawn((
                    number(fonts, "9%", t::FONT_VALUE, t::W_SEMIBOLD, t::ACCENT),
                    BudgetText::TaxValue,
                ));
            });
            p.spawn(layout(Node {
                align_items: AlignItems::Center,
                column_gap: px(t::S3),
                ..default()
            }))
            .with_children(|r| {
                small_button(r, fonts, UiAction::TaxStep(-5), icon::MINUS, "Lower taxes");
                r.spawn((
                    TaxTrack,
                    Hovered::default(),
                    Node {
                        flex_grow: 1.0,
                        height: px(22),
                        justify_content: JustifyContent::Center,
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                ))
                .with_children(|track| {
                    track
                        .spawn(layout(Node {
                            height: px(6),
                            width: percent(100),
                            border_radius: BorderRadius::all(px(3)),
                            ..default()
                        }))
                        .insert(BackgroundColor(t::WELL))
                        .with_children(|bar| {
                            bar.spawn((
                                TaxFill,
                                layout(Node {
                                    height: percent(100),
                                    width: percent(30),
                                    border_radius: BorderRadius::all(px(3)),
                                    ..default()
                                }),
                                BackgroundColor(t::ACCENT),
                            ));
                            bar.spawn((
                                TaxKnob,
                                layout(Node {
                                    position_type: PositionType::Absolute,
                                    left: percent(30),
                                    top: px(-5),
                                    width: px(16),
                                    height: px(16),
                                    margin: UiRect::left(px(-8)),
                                    border: UiRect::all(px(3)),
                                    border_radius: BorderRadius::MAX,
                                    ..default()
                                }),
                                BackgroundColor(t::TEXT),
                                BorderColor::all(t::ACCENT),
                                BoxShadow(vec![ShadowStyle {
                                    color: t::alpha(Color::BLACK, 0.4),
                                    x_offset: px(0),
                                    y_offset: px(1),
                                    spread_radius: px(0),
                                    blur_radius: px(4),
                                }]),
                            ));
                        });
                });
                small_button(r, fonts, UiAction::TaxStep(5), icon::PLUS, "Raise taxes");
            });
        });
}

pub fn close_button(p: &mut ChildSpawnerCommands, fonts: &UiFonts, action: UiAction) {
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Ghost,
            accent: t::ACCENT,
        },
        Node {
            width: px(28),
            height: px(28),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        None,
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, icon::CLOSE, t::ICON_SM + 1.0, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
        },
    );
}

fn small_button(p: &mut ChildSpawnerCommands, fonts: &UiFonts, action: UiAction, glyph: char, title: &'static str) {
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Filled,
            accent: t::ACCENT,
        },
        Node {
            width: px(26),
            height: px(26),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Some(Tooltip {
            title,
            desc: "Changes the tax rate by half a percent.",
            key: None,
            price: None,
        }),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, t::ICON_SM, t::TEXT),
                ButtonFg {
                    owner: id,
                    base: t::TEXT,
                },
            ));
        },
    );
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
    if !state.budget_open {
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

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_budget(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    sim: Res<SimWorld>,
    mut cache: ResMut<BudgetCache>,
    mut root: Query<(&mut Node, &mut UiTransform), With<BudgetRoot>>,
    mut texts: Query<(&BudgetText, &mut Text, &mut TextColor)>,
    mut fill: Query<&mut Node, (With<TaxFill>, Without<BudgetRoot>, Without<TaxKnob>)>,
    mut knob: Query<&mut Node, (With<TaxKnob>, Without<BudgetRoot>, Without<TaxFill>)>,
    mut fiscal_box: Query<
        (&mut Node, &mut BackgroundColor, &mut BorderColor),
        (With<FiscalBox>, Without<BudgetRoot>, Without<TaxFill>, Without<TaxKnob>),
    >,
    mut anim: Local<f32>,
) {
    let Ok((mut node, mut tf)) = root.single_mut() else {
        return;
    };
    let d = if state.budget_open {
        Display::Flex
    } else {
        Display::None
    };
    if node.display != d {
        node.display = d;
        *anim = 0.0;
        cache.age = f32::INFINITY;
        cache.key = None;
    }
    if !state.budget_open {
        return;
    }
    if *anim < 1.0 {
        *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.06);
        if *anim > 0.995 {
            *anim = 1.0;
        }
        tf.translation = Val2::px(0.0, (1.0 - *anim) * -8.0);
        tf.scale = Vec2::splat(0.97 + 0.03 * *anim);
    }
    let w = &sim.world;
    let e = &w.economy;
    // Texts change only with the data: refresh today's projection (a pass
    // over the buildings) a few times a second, and the table when it or
    // the booked days / balance / tax rate changed.
    cache.age += time.delta_secs();
    if cache.age >= PROJECTION_EVERY || cache.key.is_some_and(|k| k.0 != sim.generation) {
        cache.age = 0.0;
        cache.today = budget_now(w);
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
    );
    if cache.key == Some(key) {
        return;
    }
    cache.key = Some(key);
    let booked = !e.history.is_empty();
    let note = super::fiscal::fiscal_note(w);
    let note_color = note.as_ref().map_or(t::WARNING, |n| n.color);
    if let Ok((mut n, mut bg, mut border)) = fiscal_box.single_mut() {
        let d = if note.is_some() { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
        bg.set_if_neq(BackgroundColor(t::alpha(note_color, 0.10)));
        border.set_if_neq(BorderColor::all(t::alpha(note_color, 0.35)));
    }
    for (which, mut text, mut color) in &mut texts {
        let (s, c) = match *which {
            BudgetText::Balance => (format::money(e.money), if e.money < 0 { t::NEGATIVE } else { t::TEXT }),
            // With a fiscal note below, the line beside the balance stays short.
            BudgetText::BalanceNote if note.is_some() => match booked {
                true => (
                    format!("{} yesterday", format::signed_money(e.last_day.net())),
                    signed_color(e.last_day.net()),
                ),
                false => (String::new(), t::TEXT_MUTED),
            },
            BudgetText::BalanceNote => balance_note(e.money, booked.then_some(e.last_day.net()), cache.today.net()),
            // (Short: a longer caption pushes the numbers out of the panel.)
            BudgetText::UpkeepCaption => (
                format!("{} cells", format::group(w.stats.road_cells as u64)),
                t::TEXT_FAINT,
            ),
            BudgetText::FiscalTitle => (note.as_ref().map_or("", |n| n.title).to_string(), note_color),
            BudgetText::FiscalBody => (note.as_ref().map(|n| n.body.clone()).unwrap_or_default(), t::TEXT),
            BudgetText::FiscalFix => (
                note.as_ref().map(|n| format!("Fix: {}", n.fix)).unwrap_or_default(),
                t::TEXT_MUTED,
            ),
            BudgetText::FiscalIcon => (icon::ALERT.to_string(), note_color),
            BudgetText::ServicesCaption => (
                format!("{} people", format::group(w.stats.population as u64)),
                t::TEXT_FAINT,
            ),
            BudgetText::YesterdayHeader => (
                match e.history.last() {
                    Some(d) => format!("DAY {}", d.day),
                    None => "YESTERDAY".into(),
                },
                t::TEXT_FAINT,
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
            BudgetText::TaxValue => (format::percent(e.tax_rate), t::ACCENT),
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
    if !state.budget_open {
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
