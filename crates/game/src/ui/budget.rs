//! Budget panel (opens from the money chip): balance, yesterday's income
//! and expenses, and the tax-rate slider.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use citysim_sim::{Command, balance};

use super::UiState;
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, hdivider, label, layout, panel, spawn_button,
};
use crate::shared::{GameSet, SimCommand, SimWorld};

pub struct BudgetPlugin;

impl Plugin for BudgetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SliderDrag>()
            .add_systems(Startup, spawn_budget)
            .add_systems(Update, tax_slider_input.in_set(GameSet::Input))
            .add_systems(Update, update_budget.in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct BudgetRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum BudgetText {
    Balance,
    Income,
    Upkeep,
    UpkeepCaption,
    Net,
    TaxValue,
}

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

const PANEL_W: f32 = 304.0;
/// Tax slider step (fraction).
pub const TAX_STEP: f32 = 0.005;

fn row(p: &mut ChildSpawnerCommands, fonts: &UiFonts, name: &str, caption: Option<BudgetText>, value: BudgetText) {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        min_height: px(22),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(layout(Node {
            align_items: AlignItems::Baseline,
            column_gap: px(t::S3),
            ..default()
        }))
        .with_children(|l| {
            l.spawn(label(fonts, name, t::FONT_BODY, t::W_MEDIUM, t::TEXT));
            if let Some(c) = caption {
                l.spawn((label(fonts, "", t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED), c));
            }
        });
        r.spawn((label(fonts, "", t::FONT_BODY, t::W_SEMIBOLD, t::TEXT), value));
    });
}

fn section_title(p: &mut ChildSpawnerCommands, fonts: &UiFonts, s: &str) {
    p.spawn((
        label(fonts, s, t::FONT_CAPTION + 0.5, t::W_SEMIBOLD, t::TEXT_FAINT),
        Node {
            margin: UiRect::bottom(px(t::S2)),
            ..default()
        },
    ));
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
                margin: UiRect::bottom(px(t::S4)),
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
            section_title(p, fonts, "BALANCE");
            p.spawn((
                label(fonts, "$0", t::FONT_DISPLAY, t::W_BOLD, t::TEXT),
                BudgetText::Balance,
            ));
            p.spawn(hdivider()).insert(Node {
                height: px(1),
                width: percent(100),
                margin: UiRect::new(px(0), px(0), px(t::S4), px(t::S4)),
                ..default()
            });
            section_title(p, fonts, "YESTERDAY");
            row(p, fonts, "Tax income", None, BudgetText::Income);
            row(
                p,
                fonts,
                "Road upkeep",
                Some(BudgetText::UpkeepCaption),
                BudgetText::Upkeep,
            );
            p.spawn(hdivider());
            row(p, fonts, "Net", None, BudgetText::Net);
            p.spawn(hdivider()).insert(Node {
                height: px(1),
                width: percent(100),
                margin: UiRect::new(px(0), px(0), px(t::S3), px(t::S4)),
                ..default()
            });
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
                });
                r.spawn((
                    label(fonts, "9%", t::FONT_VALUE, t::W_SEMIBOLD, t::ACCENT),
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
            p.spawn((
                super::widgets::paragraph(
                    fonts,
                    "Higher taxes bring in more money but slow the city's growth.",
                    t::FONT_SMALL,
                    t::W_REGULAR,
                    t::TEXT_MUTED,
                ),
                Node {
                    margin: UiRect::top(px(t::S3)),
                    ..default()
                },
            ));
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

fn signed_color(v: i64) -> Color {
    match v.signum() {
        1 => t::POSITIVE,
        -1 => t::NEGATIVE,
        _ => t::TEXT_MUTED,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_budget(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    sim: Res<SimWorld>,
    mut root: Query<(&mut Node, &mut UiTransform), With<BudgetRoot>>,
    mut texts: Query<(&BudgetText, &mut Text, &mut TextColor)>,
    mut fill: Query<&mut Node, (With<TaxFill>, Without<BudgetRoot>, Without<TaxKnob>)>,
    mut knob: Query<&mut Node, (With<TaxKnob>, Without<BudgetRoot>, Without<TaxFill>)>,
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
    }
    if !state.budget_open {
        return;
    }
    *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.06);
    tf.translation = Val2::px(0.0, (1.0 - *anim) * -8.0);
    tf.scale = Vec2::splat(0.97 + 0.03 * *anim);

    let e = &sim.world.economy;
    let net = e.last_day_income - e.last_day_expenses;
    let road_cells = sim.world.stats.road_cells;
    for (which, mut text, mut color) in &mut texts {
        let (s, c) = match which {
            BudgetText::Balance => (format::money(e.money), if e.money < 0 { t::NEGATIVE } else { t::TEXT }),
            BudgetText::Income => (format::signed_money(e.last_day_income), signed_color(e.last_day_income)),
            BudgetText::Upkeep => (
                format::signed_money(-e.last_day_expenses),
                signed_color(-e.last_day_expenses),
            ),
            BudgetText::UpkeepCaption => (format!("{} cells", format::group(road_cells as u64)), t::TEXT_MUTED),
            BudgetText::Net => (format::signed_money(net), signed_color(net)),
            BudgetText::TaxValue => (format::percent(e.tax_rate), t::ACCENT),
        };
        if text.0 != s {
            text.0 = s;
        }
        color.set_if_neq(TextColor(c));
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
