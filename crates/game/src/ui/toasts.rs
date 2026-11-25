//! Toasts: short stacked messages under the top-right speed controls that
//! slide in and auto-dismiss. Rejected commands become error toasts.

use bevy::prelude::*;

use super::context::error_text;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{approach, label, layout, panel};
use crate::shared::{GameSet, LastCommand};

pub struct ToastsPlugin;

impl Plugin for ToastsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Toast>()
            .add_systems(Startup, spawn_stack)
            .add_systems(
                Update,
                (command_toasts, spawn_toasts, animate_toasts)
                    .chain()
                    .in_set(GameSet::Render),
            );
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
struct ToastStack;

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

fn spawn_stack(mut commands: Commands) {
    commands.spawn((
        Name::new("Toasts"),
        ToastStack,
        layout(Node {
            position_type: PositionType::Absolute,
            top: px(t::BELOW_TOP_BAR),
            right: px(t::EDGE),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::End,
            row_gap: px(t::S3),
            ..default()
        }),
        GlobalZIndex(300),
    ));
}

fn kind_style(kind: ToastKind) -> (char, Color) {
    match kind {
        ToastKind::Success => (icon::CHECK_CIRCLE, t::POSITIVE),
        ToastKind::Info => (icon::INFO, t::ACCENT),
        ToastKind::Warning => (icon::ALERT, t::WARNING),
        ToastKind::Error => (icon::ERROR, t::NEGATIVE),
    }
}

/// Every rejected command as a toast in the sim's own terms (the tools
/// check their plans first and say themselves when a plan changes nothing).
fn command_toasts(last: Res<LastCommand>, mut out: MessageWriter<Toast>) {
    if !last.is_changed() {
        return;
    }
    if let Some((_, Err(e))) = &last.result {
        out.write(Toast::new(ToastKind::Error, error_text(e)));
    }
}

fn spawn_toasts(
    mut commands: Commands,
    fonts: Res<UiFonts>,
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
        commands.entity(stack).with_children(|s| {
            s.spawn((
                ToastItem {
                    text: toast.text.clone(),
                    age: 0.0,
                    life,
                    t: 0.0,
                    closing: false,
                },
                panel(Node {
                    padding: UiRect::new(px(t::S4), px(t::S5), px(t::S3 + 2.0), px(t::S3 + 2.0)),
                    align_items: AlignItems::Center,
                    column_gap: px(t::S3 + 2.0),
                    ..default()
                }),
                UiTransform::from_translation(Val2::px(40.0, 0.0)),
            ))
            .insert(BorderColor::all(t::alpha(color, 0.35)))
            .with_children(|row| {
                row.spawn(super::widgets::icon(&fonts, glyph, t::ICON_MD, color));
                row.spawn(label(&fonts, toast.text.clone(), t::FONT_BODY, t::W_MEDIUM, t::TEXT));
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
        // Ease: enter from the right, leave to the right.
        let x = (1.0 - s) * if item.closing { 420.0 } else { 40.0 };
        tf.translation = Val2::px(x, 0.0);
        tf.scale = Vec2::splat(0.96 + 0.04 * s);
    }
}
