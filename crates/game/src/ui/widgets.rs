//! Reusable building blocks: panels, text, icons, key badges, and buttons
//! with animated hover / press / active states that emit [`UiAction`]s.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::picking::Pickable;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::LineHeight;
use citysim_sim::Zone;

use super::UiState;
use super::theme::{self as t, UiFonts};
use crate::shared::{ActiveTool, GameSet, GraphicsQuality, OverlayMode, SimClock};

/// Everything a click on the interface can ask for. Handled in `actions.rs`.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub enum UiAction {
    /// Select a tool; selecting the active tool again returns to Inspect.
    Tool(ActiveTool),
    /// The Zone toolbar button: opens zoning with the last zone type.
    ZoneTool,
    /// Toggle an overlay (selecting the active one turns overlays off).
    Overlay(OverlayMode),
    TogglePause,
    Speed(usize),
    ToggleBudget,
    /// Change the tax rate by this many tenths of a percent.
    TaxStep(i32),
    OpenMenu,
    CloseMenu,
    Save,
    Load,
    Quality(GraphicsQuality),
    Quit,
    DismissHint,
}

/// Visual family of a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Square toolbar / speed button; active = tinted background + border.
    Tool,
    /// Wide menu row; active is not shown.
    Menu,
    /// Menu row that turns red on hover (Quit).
    Danger,
    /// Segment of a segmented control.
    Segment,
    /// Flat, no background until hovered (close buttons, chips).
    Ghost,
    /// Like `Tool` but on a recessed background (small steppers).
    Filled,
}

#[derive(Component, Debug, Clone, Copy)]
#[require(Hovered, ButtonAnim)]
pub struct UiButton {
    pub action: UiAction,
    pub kind: ButtonKind,
    /// Colour used for the active state.
    pub accent: Color,
}

/// Smoothed 0..1 animation state of a button.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct ButtonAnim {
    pub hover: f32,
    pub press: f32,
    pub active: f32,
}

/// Text or icon inside a button that follows its state colour.
#[derive(Component, Debug, Clone, Copy)]
pub struct ButtonFg {
    pub owner: Entity,
    pub base: Color,
}

/// Node inside a button that is only visible while the button is active.
#[derive(Component, Debug, Clone, Copy)]
pub struct ActiveMark {
    pub owner: Entity,
}

/// Hover help for a button. `price` is shown in the accent colour.
#[derive(Component, Debug, Clone)]
pub struct Tooltip {
    pub title: &'static str,
    pub desc: &'static str,
    pub key: Option<&'static str>,
    pub price: Option<String>,
}

/// The button that received the current left-press.
#[derive(Resource, Default)]
pub(super) struct PressedButton(Option<Entity>);

pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PressedButton>()
            .add_message::<UiAction>()
            .add_systems(Update, button_visuals.in_set(GameSet::Render));
    }
}

// ---------------------------------------------------------------- bundles

/// Floating panel surface.
pub fn panel(node: Node) -> impl Bundle {
    (
        Node {
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(t::RADIUS)),
            ..node
        },
        BackgroundColor(t::PANEL),
        BorderColor::all(t::PANEL_BORDER),
        BoxShadow(vec![ShadowStyle {
            color: t::SHADOW,
            x_offset: px(0),
            y_offset: px(4),
            spread_radius: px(0),
            blur_radius: px(16),
        }]),
    )
}

/// Layout-only container that never catches the pointer.
pub fn layout(node: Node) -> impl Bundle {
    (node, Pickable::IGNORE)
}

pub fn label(fonts: &UiFonts, s: impl Into<String>, size: f32, weight: u16, color: Color) -> impl Bundle {
    (
        Text::new(s),
        fonts.text(size, weight),
        TextColor(color),
        LineHeight::RelativeToFont(1.25),
        Pickable::IGNORE,
    )
}

/// An icon glyph in a square box of `size`.
pub fn icon(fonts: &UiFonts, ch: char, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(ch.to_string()),
        fonts.icon(size),
        TextColor(color),
        LineHeight::RelativeToFont(1.0),
        TextLayout::justify(Justify::Center),
        Node {
            width: px(size),
            height: px(size),
            flex_shrink: 0.0,
            ..default()
        },
        Pickable::IGNORE,
    )
}

/// Small keyboard-key badge, e.g. `R` or `Space`.
pub fn spawn_key_badge(p: &mut ChildSpawnerCommands, fonts: &UiFonts, key: &str) {
    p.spawn(layout(Node {
        padding: UiRect::axes(px(5), px(1)),
        min_width: px(17),
        height: px(17),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        border: UiRect::all(px(1)),
        border_radius: BorderRadius::all(px(t::RADIUS_XS)),
        ..default()
    }))
    .insert((BackgroundColor(t::WELL), BorderColor::all(t::PANEL_BORDER)))
    .with_child(label(fonts, key, t::FONT_CAPTION, t::W_SEMIBOLD, t::TEXT_MUTED));
}

/// Thin vertical divider.
pub fn vdivider(height: f32) -> impl Bundle {
    (
        Node {
            width: px(1),
            height: px(height),
            margin: UiRect::horizontal(px(t::S2)),
            ..default()
        },
        BackgroundColor(t::DIVIDER),
        Pickable::IGNORE,
    )
}

pub fn hdivider() -> impl Bundle {
    (
        Node {
            height: px(1),
            width: percent(100),
            margin: UiRect::vertical(px(t::S3)),
            ..default()
        },
        BackgroundColor(t::DIVIDER),
        Pickable::IGNORE,
    )
}

/// Spawn a button as a child of `p`; `content` fills it. Returns its id.
pub fn spawn_button(
    p: &mut ChildSpawnerCommands,
    button: UiButton,
    node: Node,
    tooltip: Option<Tooltip>,
    content: impl FnOnce(&mut ChildSpawnerCommands, Entity),
) -> Entity {
    let radius = if button.kind == ButtonKind::Tool {
        t::RADIUS_SM
    } else {
        t::RADIUS_SM - 1.0
    };
    let mut e = p.spawn((
        button,
        Node {
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(radius)),
            ..node
        },
        BackgroundColor(t::BUTTON),
        BorderColor::all(Color::NONE),
        UiTransform::default(),
    ));
    if let Some(tip) = tooltip {
        e.insert(tip);
    }
    let id = e.id();
    e.with_children(|c| content(c, id));
    id
}

// ---------------------------------------------------------------- behaviour

/// Context needed to decide whether a button shows as active.
pub struct ActiveCtx<'a> {
    pub tool: &'a ActiveTool,
    pub overlay: &'a OverlayMode,
    pub clock: &'a SimClock,
    pub quality: &'a GraphicsQuality,
    pub state: &'a UiState,
}

pub fn is_active(action: UiAction, cx: &ActiveCtx<'_>) -> bool {
    match action {
        UiAction::Tool(tool) => *cx.tool == tool,
        UiAction::ZoneTool => matches!(cx.tool, ActiveTool::Zone(_)),
        UiAction::Overlay(o) => *cx.overlay == o,
        UiAction::TogglePause => cx.clock.paused,
        UiAction::Speed(i) => !cx.clock.paused && cx.clock.speed == i,
        UiAction::ToggleBudget => cx.state.budget_open,
        UiAction::Quality(q) => *cx.quality == q,
        _ => false,
    }
}

pub(super) fn button_clicks(
    mouse: Res<ButtonInput<MouseButton>>,
    buttons: Query<(Entity, &UiButton, &Hovered)>,
    mut pressed: ResMut<PressedButton>,
    mut out: MessageWriter<UiAction>,
) {
    if mouse.just_pressed(MouseButton::Left) {
        pressed.0 = buttons.iter().find(|(_, _, h)| h.get()).map(|(e, _, _)| e);
    }
    if mouse.just_released(MouseButton::Left) {
        if let Some(e) = pressed.0.take()
            && let Ok((_, b, h)) = buttons.get(e)
            && h.get()
        {
            out.write(b.action);
        }
    } else if !mouse.pressed(MouseButton::Left) {
        pressed.0 = None;
    }
}

/// Exponential approach factor for `dt` with time constant `tau` seconds.
pub fn approach(dt: f32, tau: f32) -> f32 {
    1.0 - (-dt / tau).exp()
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn button_visuals(
    time: Res<Time<Real>>,
    pressed: Res<PressedButton>,
    tool: Res<ActiveTool>,
    overlay: Res<OverlayMode>,
    clock: Res<SimClock>,
    quality: Res<GraphicsQuality>,
    state: Res<UiState>,
    mut buttons: Query<(
        Entity,
        &UiButton,
        &Hovered,
        &mut ButtonAnim,
        &mut BackgroundColor,
        &mut BorderColor,
        &mut UiTransform,
    )>,
    mut fgs: Query<(&ButtonFg, &mut TextColor)>,
    mut marks: Query<(&ActiveMark, &mut Visibility)>,
) {
    let cx = ActiveCtx {
        tool: &tool,
        overlay: &overlay,
        clock: &clock,
        quality: &quality,
        state: &state,
    };
    let k = approach(time.delta_secs(), 0.06);
    for (e, b, hovered, mut anim, mut bg, mut border, mut tf) in &mut buttons {
        let target_press = (pressed.0 == Some(e) && hovered.get()) as u8 as f32;
        let target_hover = hovered.get() as u8 as f32;
        let target_active = is_active(b.action, &cx) as u8 as f32;
        anim.hover += (target_hover - anim.hover) * k;
        anim.press += (target_press - anim.press) * approach(time.delta_secs(), 0.03);
        anim.active += (target_active - anim.active) * k;
        let anim = anim.as_mut();
        for v in [&mut anim.hover, &mut anim.press, &mut anim.active] {
            if (*v - v.round()).abs() < 0.002 {
                *v = v.round();
            }
        }
        let show_active = !matches!(b.kind, ButtonKind::Menu | ButtonKind::Danger);
        let active = if show_active { anim.active } else { 0.0 };
        let base = if b.kind == ButtonKind::Filled {
            t::WELL
        } else {
            t::BUTTON
        };
        let mut c = t::mix(base, t::BUTTON_HOVER, anim.hover);
        c = t::mix(c, t::BUTTON_PRESS, anim.press);
        if b.kind == ButtonKind::Danger {
            c = t::mix(c, t::alpha(t::NEGATIVE, 0.16), anim.hover);
        }
        let active_bg = t::alpha(b.accent, t::ACTIVE_TINT + 0.06 * anim.hover);
        bg.set_if_neq(BackgroundColor(t::mix(c, active_bg, active)));
        let border_c = BorderColor::all(t::mix(Color::NONE, t::alpha(b.accent, t::ACTIVE_BORDER), active));
        border.set_if_neq(border_c);
        let scale = Vec2::splat(1.0 - 0.05 * anim.press);
        if tf.scale != scale {
            tf.scale = scale;
        }
    }
    for (fg, mut color) in &mut fgs {
        let Ok((_, b, _, anim, ..)) = buttons.get(fg.owner) else {
            continue;
        };
        let show_active = !matches!(b.kind, ButtonKind::Menu | ButtonKind::Danger | ButtonKind::Ghost);
        let mut c = fg.base;
        if b.kind == ButtonKind::Ghost || b.kind == ButtonKind::Menu {
            c = t::mix(c, t::TEXT, anim.hover);
        }
        if b.kind == ButtonKind::Danger {
            c = t::mix(c, t::NEGATIVE, anim.hover);
        }
        if show_active {
            c = t::mix(c, b.accent, anim.active);
        }
        if color.0 != c {
            color.0 = c;
        }
    }
    for (mark, mut vis) in &mut marks {
        let Ok((_, b, ..)) = buttons.get(mark.owner) else {
            continue;
        };
        let v = if is_active(b.action, &cx) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        vis.set_if_neq(v);
    }
}

/// Icon for a zone type (None = de-zone).
pub fn zone_icon(zone: Option<Zone>) -> char {
    match zone {
        Some(Zone::Residential) => t::icon::HOUSE,
        Some(Zone::Commercial) => t::icon::STORE,
        Some(Zone::Industrial) => t::icon::FACTORY,
        None => t::icon::ERASER,
    }
}

pub fn zone_name(zone: Option<Zone>) -> &'static str {
    match zone {
        Some(Zone::Residential) => "Residential",
        Some(Zone::Commercial) => "Commercial",
        Some(Zone::Industrial) => "Industrial",
        None => "De-zone",
    }
}
