//! One shared tooltip: name + hotkey, one-line description, price. Appears
//! after a short hover delay next to the hovered button, kept on screen.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;

use super::UiState;
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts};
use super::widgets::{
    Tooltip, UiAction, UiButton, approach, figure, label, layout, paragraph, plate_node, plate_shadow,
};
use crate::shared::GameSet;

pub struct TooltipPlugin;

impl Plugin for TooltipPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TipState>()
            .add_systems(Startup, spawn_tooltip)
            .add_systems(Update, update_tooltip.in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct TooltipRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum TipPart {
    Title,
    Desc,
    Price,
    Key,
    KeyBadge,
}

#[derive(Resource, Default)]
struct TipState {
    target: Option<Entity>,
    hover_time: f32,
    /// Seconds since a tooltip was last visible (short = show the next fast).
    since_visible: f32,
    /// Entity whose content is currently in the tooltip.
    content: Option<Entity>,
    /// Frames since the content changed (layout needs one to measure).
    settle: u32,
    slide: f32,
}

/// Hover delay before a tooltip appears, and when moving between buttons.
const DELAY: f32 = 0.45;
const DELAY_WARM: f32 = 0.05;
/// Width of the tooltip's description column.
const TIP_TEXT_W: f32 = 250.0;

fn spawn_tooltip(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    let mat = cache.get(&mut mats, PlateStyle::fill(t::PLATE_TOP, t::PLATE_BOTTOM, t::LINE));
    commands
        .spawn((
            Name::new("Tooltip"),
            TooltipRoot,
            plate_node(
                Node {
                    position_type: PositionType::Absolute,
                    padding: UiRect::new(px(14), px(14), px(9), px(11)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(t::S2),
                    ..default()
                },
                UiRect::new(px(3), px(1), px(1), px(1)),
                [0.0, 0.0, 10.0, 0.0],
            ),
            mat,
            plate_shadow(),
            GlobalZIndex(100),
            UiTransform::default(),
            Visibility::Hidden,
            Pickable::IGNORE,
        ))
        .with_children(|tip| {
            tip.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                column_gap: px(t::S5),
                ..default()
            }))
            .with_children(|row| {
                row.spawn((
                    Text::new(""),
                    fonts.text(t::FONT_ROW, t::W_BOLD),
                    t::caps_spacing(t::FONT_ROW, 0.05),
                    TextColor(t::TEXT),
                    TextLayout::no_wrap(),
                    Pickable::IGNORE,
                    TipPart::Title,
                ));
                row.spawn((
                    TipPart::KeyBadge,
                    layout(Node {
                        padding: UiRect::axes(px(5), px(0)),
                        min_width: px(17),
                        height: px(17),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border: UiRect::all(px(1)),
                        ..default()
                    }),
                    BorderColor::all(t::LINE),
                    BackgroundColor(t::WELL),
                ))
                .with_child((label(fonts, "", t::FONT_KEY, t::W_BOLD, t::TEXT_MUTED), TipPart::Key));
            });
            // Fixed width: wrapped text inside an auto-width box is measured
            // as one line by the layout, which clipped the price row.
            tip.spawn((
                paragraph(fonts, "", t::FONT_BODY, t::W_REGULAR, t::TEXT_MUTED),
                TipPart::Desc,
                Node {
                    width: px(TIP_TEXT_W),
                    ..default()
                },
            ));
            tip.spawn((figure(fonts, "", 16.0, t::ACCENT_TEXT), TipPart::Price));
        });
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_tooltip(
    time: Res<Time<Real>>,
    mouse: Res<ButtonInput<MouseButton>>,
    ui_scale: Res<UiScale>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut state: ResMut<TipState>,
    buttons: Query<(
        Entity,
        &Tooltip,
        &Hovered,
        &ComputedNode,
        &UiGlobalTransform,
        &InheritedVisibility,
        Option<&UiButton>,
    )>,
    ui: Res<UiState>,
    mut root: Query<
        (&mut Node, &mut Visibility, &ComputedNode, &mut UiTransform),
        (With<TooltipRoot>, Without<TipPart>),
    >,
    mut parts: Query<(&TipPart, Option<&mut Text>, Option<&mut Node>), Without<TooltipRoot>>,
) {
    let dt = time.delta_secs();
    let Ok((mut node, mut vis, computed, mut tf)) = root.single_mut() else {
        return;
    };
    let hovered = buttons
        .iter()
        .find(|(_, _, h, _, _, v, b)| {
            // No help for a toggle whose panel is already open (it would cover it).
            let redundant = b.is_some_and(|b| b.action == UiAction::ToggleBudget && ui.budget_open());
            h.get() && v.get() && !redundant
        })
        .map(|(e, ..)| e);
    if hovered != state.target {
        state.target = hovered;
        state.hover_time = 0.0;
    } else {
        state.hover_time += dt;
    }
    if mouse.pressed(MouseButton::Left) {
        state.hover_time = 0.0;
    }
    let delay = if state.since_visible < 0.3 { DELAY_WARM } else { DELAY };
    let show = hovered.filter(|_| state.hover_time >= delay);
    let Some(target) = show else {
        state.since_visible += dt;
        state.content = None;
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    state.since_visible = 0.0;
    let Ok((_, tip, _, bn, bt, _, _)) = buttons.get(target) else {
        return;
    };
    if state.content != Some(target) {
        state.content = Some(target);
        state.settle = 0;
        state.slide = 0.0;
        for (part, text, pnode) in &mut parts {
            let (value, shown) = match part {
                TipPart::Title => (tip.title.to_uppercase(), true),
                TipPart::Desc => (tip.desc.to_string(), !tip.desc.is_empty()),
                TipPart::Price => (tip.price.clone().unwrap_or_default(), tip.price.is_some()),
                TipPart::Key => (tip.key.unwrap_or("").to_string(), tip.key.is_some()),
                TipPart::KeyBadge => (String::new(), tip.key.is_some()),
            };
            if let Some(mut text) = text {
                text.0 = value;
            }
            if let Some(mut n) = pnode {
                n.display = if shown { Display::Flex } else { Display::None };
            }
        }
        vis.set_if_neq(Visibility::Hidden);
        return;
    }
    // Wait for layout to measure the new content before showing it.
    state.settle += 1;
    if state.settle < 2 {
        return;
    }
    let inv = bn.inverse_scale_factor;
    let center = bt.translation * inv;
    let size = bn.size * inv;
    let tip_size = computed.size * computed.inverse_scale_factor;
    let Ok(win) = window.single() else { return };
    let screen = Vec2::new(win.width(), win.height()) / ui_scale.0.max(0.01);
    let gap = 10.0;
    let below = center.y < screen.y * 0.5;
    let top = if below {
        center.y + size.y * 0.5 + gap
    } else {
        center.y - size.y * 0.5 - gap - tip_size.y
    };
    let left = (center.x - tip_size.x * 0.5).clamp(8.0, (screen.x - 8.0 - tip_size.x).max(8.0));
    let (left, top) = (px(left.round()), px(top.round()));
    if node.left != left || node.top != top {
        node.left = left;
        node.top = top;
    }
    if state.slide < 1.0 {
        state.slide += (1.0 - state.slide) * approach(dt, 0.05);
        if state.slide > 0.995 {
            state.slide = 1.0;
        }
        let offset = (1.0 - state.slide) * if below { -4.0 } else { 4.0 };
        tf.translation = Val2::px(0.0, offset);
    }
    vis.set_if_neq(Visibility::Inherited);
}
