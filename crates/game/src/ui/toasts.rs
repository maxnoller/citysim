//! Toasts: short messages docked to the left edge under the city plate,
//! sliding in from the edge and away again. (Refused commands go to the
//! alert ribbon, `top.rs`.)

use bevy::prelude::*;

use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{approach, label, layout, plate_node, plate_shadow};
use crate::shared::GameSet;

pub struct ToastsPlugin;

impl Plugin for ToastsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Toast>()
            .add_systems(Startup, spawn_stack)
            .add_systems(Update, (spawn_toasts, animate_toasts).chain().in_set(GameSet::Render));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

/// Show a toast.
#[derive(Message, Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
}

impl Toast {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Toast {
        Toast {
            kind,
            text: text.into(),
        }
    }
}

#[derive(Component)]
pub(super) struct ToastStack;

#[derive(Component)]
struct ToastItem {
    text: String,
    age: f32,
    life: f32,
    /// 0 -> 1 entering; once closing, counts down 1 -> 0.
    t: f32,
    closing: bool,
}

const MAX_TOASTS: usize = 4;

/// Top of the stack (the performance panel moves it while it shows).
pub(super) const STACK_TOP: f32 = 72.0;

fn spawn_stack(mut commands: Commands) {
    commands.spawn((
        Name::new("Toasts"),
        ToastStack,
        layout(Node {
            position_type: PositionType::Absolute,
            top: px(STACK_TOP),
            left: px(0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Start,
            row_gap: px(t::S2 + 2.0),
            ..default()
        }),
        GlobalZIndex(300),
    ));
}

fn kind_style(kind: ToastKind) -> (char, Color) {
    match kind {
        ToastKind::Success => (icon::CHECK_CIRCLE, t::POSITIVE),
        ToastKind::Info => (icon::INFO, t::ACCENT_TEXT),
        ToastKind::Warning => (icon::ALERT, t::WARNING),
        ToastKind::Error => (icon::ERROR, t::NEGATIVE),
    }
}

fn spawn_toasts(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
    mut incoming: MessageReader<Toast>,
    stack: Query<Entity, With<ToastStack>>,
    mut items: Query<(Entity, &mut ToastItem)>,
) {
    let Ok(stack) = stack.single() else { return };
    for toast in incoming.read() {
        // Same message again: refresh the visible one instead of stacking.
        if let Some((_, mut item)) = items.iter_mut().find(|(_, i)| i.text == toast.text && !i.closing) {
            item.age = 0.0;
            continue;
        }
        let mut alive: Vec<_> = items.iter_mut().filter(|(_, i)| !i.closing).collect();
        if alive.len() >= MAX_TOASTS {
            alive.sort_by(|a, b| b.1.age.total_cmp(&a.1.age));
            alive[0].1.closing = true;
        }
        let (glyph, color) = kind_style(toast.kind);
        let life = if toast.kind == ToastKind::Error { 4.5 } else { 3.2 };
        let mat = cache.get(&mut mats, PlateStyle::PANEL.slanted([0.0, 0.0, 0.0, 0.5]));
        commands.entity(stack).with_children(|s| {
            s.spawn((
                ToastItem {
                    text: toast.text.clone(),
                    age: 0.0,
                    life,
                    t: 0.0,
                    closing: false,
                },
                plate_node(
                    Node {
                        height: px(38),
                        padding: UiRect::new(px(0), px(34), px(0), px(0)),
                        align_items: AlignItems::Center,
                        column_gap: px(10),
                        ..default()
                    },
                    UiRect::new(px(0), px(t::LW), px(t::LW), px(t::LW)),
                    [0.0; 4],
                ),
                mat,
                plate_shadow(),
                UiTransform::from_translation(Val2::px(-60.0, 0.0)),
            ))
            .with_children(|row| {
                row.spawn((
                    layout(Node {
                        width: px(5),
                        height: percent(100),
                        margin: UiRect::right(px(10)),
                        ..default()
                    }),
                    BackgroundColor(color),
                ));
                row.spawn(super::widgets::icon(&fonts, glyph, t::ICON_SM + 1.0, color));
                row.spawn(label(&fonts, toast.text.clone(), t::FONT_BODY, t::W_SEMIBOLD, t::TEXT));
            });
        });
    }
}

fn animate_toasts(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut items: Query<(Entity, &mut ToastItem, &mut UiTransform)>,
) {
    // Clamp: one long frame (a load rebuilding the whole scene) must not
    // eat the "City loaded" toast before anyone saw it.
    let dt = time.delta_secs().min(0.1);
    for (e, mut item, mut tf) in &mut items {
        item.age += dt;
        if item.age > item.life {
            item.closing = true;
        }
        if item.closing {
            item.t -= dt / 0.22;
            if item.t <= 0.0 {
                commands.entity(e).despawn();
                continue;
            }
        } else {
            item.t += (1.0 - item.t) * approach(dt, 0.07);
        }
        let s = item.t.clamp(0.0, 1.0);
        // Slide out of the left edge and back into it.
        let x = -(1.0 - s) * if item.closing { 480.0 } else { 60.0 };
        tf.translation = Val2::px(x, 0.0);
    }
}
