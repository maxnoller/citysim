//! The interface's building blocks in the HUD language: docked plates,
//! caps labels, figures, icons, key caps, slanted rules, and buttons
//! (tool slots, joined segments, lit switches, picture items, rows,
//! actions) with animated hover / press / selected / disabled / focus
//! looks that emit [`UiAction`]s.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::picking::Pickable;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::ui::{ColorStop, Gradient, LinearGradient};
use citysim_sim::Zone;

use super::UiState;
use super::plate::{PlateAnim, PlateStyle};
use super::theme::{self as t, UiFonts};
use crate::shared::{
    ActiveTool, BulldozeMode, DisplaySettings, GameSet, GraphicsQuality, OverlayMode, RoadChoice, RoadMode, SimClock,
    ToolModes, ZoneMode,
};

/// Everything a click on the interface can ask for. Handled in
/// `controls.rs` (and the panels for their own).
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub enum UiAction {
    /// Select a tool; selecting the active tool again returns to Inspect.
    Tool(ActiveTool),
    /// The Zones slot: opens zoning with the last zone type.
    ZoneTool,
    /// Pick an overlay (the maps tray).
    Overlay(OverlayMode),
    TogglePause,
    Speed(usize),
    ToggleBudget,
    /// Open / close the maps tray.
    Maps,
    /// Open / close the statistics tray.
    Stats,
    /// Change the tax rate by this many tenths of a percent.
    TaxStep(i32),
    OpenMenu,
    CloseMenu,
    Save,
    Load,
    Quality(GraphicsQuality),
    Quit,
    DismissHint,
    /// Undo the last road or zoning action (Ctrl+Z).
    Undo,
    /// Tool sub-modes and options (build trays).
    RoadMode(RoadMode),
    ZoneMode(ZoneMode),
    BulldozeMode(BulldozeMode),
    RoadKind(RoadChoice),
    /// Step the grid tool's block size by this many options.
    BlockSize(i32),
    /// Switch one snapping rule on or off.
    Snap(super::tools::SnapRule),
    /// Step the parallel-road distance by this many options.
    ParallelStep(i32),
    /// Frontage zoning depth in cells.
    ZoneDepth(u8),
    /// What a click with the junction tool does (None = next setting).
    JunctionBrush(Option<citysim_sim::JunctionSetting>),
    /// Set the control of the junction shown in the side panel.
    SetJunction(citysim_sim::JunctionSetting),
    /// People views: show a person, a building's people, a car.
    Person(citysim_sim::CitizenId),
    Place(citysim_sim::BuildingId),
    Vehicle(citysim_sim::VehicleId),
    /// Follow the person / car shown (again: stop).
    Follow,
    StopFollow,
    /// Back to the previous view of the side panel.
    PeopleBack,
    ClosePeople,
    /// Open / close the city-wide people view.
    CityPeople,
    OpenSettings,
    CloseSettings,
    /// Open / close the key reference (pause menu).
    Keys,
    /// Index into `settings::UI_SCALES`.
    UiScale(usize),
    EdgeScroll(bool),
    LotMarkers(bool),
    /// The performance panel's size.
    Perf(crate::shared::PerfDetail),
    /// Fold / unfold a section of the side panel.
    Fold(super::people::FoldKey),
    /// Does nothing (a locked item).
    None,
}

/// Visual family of a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Square tool slot in the bar; active = raised and filled with the accent.
    Slot,
    /// One of a row of joined switches; active = accent fill.
    Segment,
    /// A labelled switch with a lit marker (the marker lights when on).
    Toggle,
    /// Picture slot of a build tray; selected = accent frame and brackets.
    Item,
    /// Cell of a panel's action bar (flat, lighter on hover).
    Action,
    /// Always accent-filled (the main action of a panel or card).
    Primary,
    /// No surface until hovered (close buttons, links in text).
    Ghost,
    /// Full-width row (menus, links in a panel): lighter on hover, accent
    /// marker on the left.
    Row,
    /// Row that turns red on hover (Quit).
    Danger,
    /// Parallelogram of the speed switch (drawn as a plate).
    Speed,
    /// Small square step button.
    Stepper,
    /// Icon on the system plate (top right).
    Sys,
}

#[derive(Component, Debug, Clone, Copy)]
#[require(Hovered, ButtonAnim, UiTransform)]
pub struct UiButton {
    pub action: UiAction,
    pub kind: ButtonKind,
}

impl UiButton {
    pub fn new(action: UiAction, kind: ButtonKind) -> UiButton {
        UiButton { action, kind }
    }
}

/// Smoothed 0..1 animation state of a button.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct ButtonAnim {
    pub hover: f32,
    pub press: f32,
    pub active: f32,
    pub focus: f32,
    pub disabled: f32,
}

/// The button can't be used right now (dimmed, ignores clicks).
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct Disabled;

/// The button has keyboard focus (pause menu, settings).
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct Focused;

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

/// A node inside a button that fills with the accent while the button is
/// active (a picture item's footer).
#[derive(Component, Debug, Clone, Copy)]
pub struct ActiveFill {
    pub owner: Entity,
}

/// The lit marker of a [`ButtonKind::Toggle`].
#[derive(Component, Debug, Clone, Copy)]
pub struct Led {
    pub owner: Entity,
}

/// Hover help for a button or readout. `price` is shown in the accent colour.
#[derive(Component, Debug, Clone)]
pub struct Tooltip {
    pub title: &'static str,
    pub desc: &'static str,
    pub key: Option<&'static str>,
    pub price: Option<String>,
}

impl Tooltip {
    pub fn new(title: &'static str, desc: &'static str) -> Tooltip {
        Tooltip {
            title,
            desc,
            key: None,
            price: None,
        }
    }

    pub fn key(mut self, key: &'static str) -> Tooltip {
        self.key = Some(key);
        self
    }

    pub fn price(mut self, price: impl Into<String>) -> Tooltip {
        self.price = Some(price.into());
        self
    }
}

/// Hover help for a non-button element.
pub fn info_tip(title: &'static str, desc: &'static str) -> impl Bundle {
    (Tooltip::new(title, desc), Hovered::default())
}

/// Text whose single line is aligned inside its (wider) box: becomes a
/// no-wrap `TextLayout` with this justification when spawned (the text
/// bundles already carry a `TextLayout`, so it can't go in the same bundle).
#[derive(Component, Debug, Clone, Copy)]
pub struct Justified(pub Justify);

fn apply_justified(mut q: Query<(&Justified, &mut TextLayout), Added<Justified>>) {
    for (j, mut l) in &mut q {
        l.justify = j.0;
    }
}

/// The button that received the current left-press.
#[derive(Resource, Default)]
pub(super) struct PressedButton(Option<Entity>);

pub struct WidgetsPlugin;

impl Plugin for WidgetsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PressedButton>()
            .add_message::<UiAction>()
            .add_systems(Update, (button_visuals, apply_justified).in_set(GameSet::Render));
    }
}

// ---------------------------------------------------------------- surfaces

/// A vertical two-colour gradient.
pub fn vgrad(top: Color, bottom: Color) -> BackgroundGradient {
    BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_bottom(vec![
        ColorStop::auto(top),
        ColorStop::auto(bottom),
    ]))])
}

/// A horizontal gradient through `stops` (evenly spaced).
pub fn hgrad(stops: &[Color]) -> BackgroundGradient {
    BackgroundGradient(vec![Gradient::Linear(LinearGradient::to_right(
        stops.iter().map(|c| ColorStop::auto(*c)).collect(),
    ))])
}

/// The soft shadow under docked plates (keeps them legible over a bright
/// scene).
pub fn plate_shadow() -> BoxShadow {
    BoxShadow(vec![ShadowStyle {
        color: t::SHADOW,
        x_offset: px(0),
        y_offset: px(6),
        spread_radius: px(0),
        blur_radius: px(18),
    }])
}

/// Layout-only container that never catches the pointer.
pub fn layout(node: Node) -> impl Bundle {
    (node, Pickable::IGNORE)
}

/// A plate's node: edge line `lw` on the given sides, corner cuts
/// (top-left, top-right, bottom-right, bottom-left).
pub fn plate_node(node: Node, line: UiRect, cuts: [f32; 4]) -> Node {
    Node {
        border: line,
        border_radius: BorderRadius {
            top_left: px(cuts[0]).into(),
            top_right: px(cuts[1]).into(),
            bottom_right: px(cuts[2]).into(),
            bottom_left: px(cuts[3]).into(),
        },
        ..node
    }
}

// ---------------------------------------------------------------- text

/// Single-line text (never wraps, so its box always fits the glyphs).
pub fn label(fonts: &UiFonts, s: impl Into<String>, size: f32, weight: u16, color: Color) -> impl Bundle {
    (
        Text::new(s),
        fonts.text(size, weight),
        TextColor(color),
        LineHeight::RelativeToFont(1.2),
        TextLayout::no_wrap(),
        Pickable::IGNORE,
    )
}

/// Caps label with letter spacing (captions, slot names, row labels).
pub fn caps(fonts: &UiFonts, s: impl AsRef<str>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s.as_ref().to_uppercase()),
        fonts.text(size, t::W_SEMIBOLD),
        t::caps_spacing(size, t::CAPS_EM),
        TextColor(color),
        LineHeight::RelativeToFont(1.15),
        TextLayout::no_wrap(),
        Pickable::IGNORE,
    )
}

/// A live figure: Barlow Condensed, tabular numerals.
pub fn figure(fonts: &UiFonts, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s),
        fonts.figures(size, t::W_BOLD),
        TextColor(color),
        LineHeight::RelativeToFont(1.0),
        TextLayout::no_wrap(),
        Pickable::IGNORE,
    )
}

/// Wrapping text; give it (or its parent) a definite width.
pub fn paragraph(fonts: &UiFonts, s: impl Into<String>, size: f32, weight: u16, color: Color) -> impl Bundle {
    (
        Text::new(s),
        fonts.text(size, weight),
        TextColor(color),
        LineHeight::RelativeToFont(1.32),
        TextLayout::default(),
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

/// Small key cap, e.g. `F` or `Space` (thin frame in the text colour).
pub fn spawn_key(p: &mut ChildSpawnerCommands, fonts: &UiFonts, key: &str, color: Color) -> Entity {
    p.spawn(layout(Node {
        padding: UiRect::axes(px(4), px(0)),
        min_width: px(14),
        height: px(15),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        border: UiRect::all(px(1)),
        flex_shrink: 0.0,
        ..default()
    }))
    .insert(BorderColor::all(t::alpha(color, 0.6)))
    .with_child(label(fonts, key, t::FONT_KEY, t::W_BOLD, t::alpha(color, 0.85)))
    .id()
}

/// A slanted rule between groups of readouts (`height` tall).
pub fn slant_rule(height: f32) -> impl Bundle {
    (
        Node {
            width: px(t::LW),
            height: px(height),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(t::LINE),
        UiTransform {
            rotation: Rot2::degrees(14.0),
            ..default()
        },
        Pickable::IGNORE,
    )
}

/// Thin horizontal rule between rows.
pub fn hrule() -> impl Bundle {
    (
        Node {
            height: px(1),
            width: percent(100),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(t::RULE),
        Pickable::IGNORE,
    )
}

/// The short accent bar above a title.
pub fn title_tick() -> impl Bundle {
    (
        Node {
            width: px(34),
            height: px(4),
            margin: UiRect::bottom(px(9)),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(t::ACCENT),
        Pickable::IGNORE,
    )
}

// ---------------------------------------------------------------- buttons

/// Spawn a button as a child of `p`; `content` fills it. Returns its id.
pub fn spawn_button(
    p: &mut ChildSpawnerCommands,
    button: UiButton,
    node: Node,
    tooltip: Option<Tooltip>,
    content: impl FnOnce(&mut ChildSpawnerCommands, Entity),
) -> Entity {
    let look = base_look(button.kind);
    let border = match button.kind {
        ButtonKind::Ghost | ButtonKind::Sys | ButtonKind::Row | ButtonKind::Danger | ButtonKind::Action => node.border,
        ButtonKind::Item => UiRect::all(px(1)),
        _ if node.border == UiRect::DEFAULT => UiRect::all(px(1)),
        _ => node.border,
    };
    let mut e = p.spawn((button, Node { border, ..node }, BorderColor::all(look.border)));
    if button.kind == ButtonKind::Speed {
        e.insert(PlateAnim::new(
            PlateStyle::fill(look.top, look.bottom, look.border).slanted(SPEED_SLANT),
        ));
    } else {
        e.insert(vgrad(look.top, look.bottom));
    }
    if let Some(tip) = tooltip {
        e.insert(tip);
    }
    let id = e.id();
    e.with_children(|c| content(c, id));
    id
}

/// Slant of the speed switch's parallelograms (left side leans right).
pub const SPEED_SLANT: [f32; 4] = [t::SLANT, 0.0, 0.0, t::SLANT];

/// Icon + optional label inside a button, both following its state.
pub fn button_face(b: &mut ChildSpawnerCommands, fonts: &UiFonts, id: Entity, glyph: Option<char>, text: &str) {
    let base = t::SLOT_INK;
    if let Some(g) = glyph {
        b.spawn((icon(fonts, g, t::ICON_SM, base), ButtonFg { owner: id, base }));
    }
    if !text.is_empty() {
        b.spawn((
            label(fonts, text, t::FONT_BODY, t::W_SEMIBOLD, base),
            ButtonFg { owner: id, base },
        ));
    }
}

/// A row of joined segments (`items`: action, icon, label, tooltip).
pub fn spawn_segments(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    items: &[(UiAction, Option<char>, &str, Option<Tooltip>)],
) {
    p.spawn(layout(Node {
        flex_direction: FlexDirection::Row,
        ..default()
    }))
    .with_children(|row| {
        for (i, (action, glyph, text, tip)) in items.iter().enumerate() {
            spawn_button(
                row,
                UiButton::new(*action, ButtonKind::Segment),
                Node {
                    height: px(32),
                    padding: UiRect::horizontal(px(if text.is_empty() { 9.0 } else { 12.0 })),
                    margin: UiRect::left(px(if i == 0 { 0.0 } else { -1.0 })),
                    column_gap: px(7),
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                tip.clone(),
                |b, id| button_face(b, fonts, id, *glyph, text),
            );
        }
    });
}

/// A labelled switch with a lit marker.
pub fn spawn_toggle(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    text: &str,
    tip: Tooltip,
    first: bool,
) {
    spawn_button(
        p,
        UiButton::new(action, ButtonKind::Toggle),
        Node {
            height: px(32),
            padding: UiRect::horizontal(px(11)),
            margin: UiRect::left(px(if first { 0.0 } else { -1.0 })),
            column_gap: px(7),
            align_items: AlignItems::Center,
            ..default()
        },
        Some(tip),
        |b, id| {
            b.spawn((
                Led { owner: id },
                layout(Node {
                    width: px(7),
                    height: px(7),
                    flex_shrink: 0.0,
                    ..default()
                }),
                BackgroundColor(t::alpha(t::TEXT_MUTED, 0.5)),
                BoxShadow(Vec::new()),
            ));
            b.spawn((
                label(fonts, text, t::FONT_BODY, t::W_SEMIBOLD, t::SLOT_INK),
                ButtonFg {
                    owner: id,
                    base: t::SLOT_INK,
                },
            ));
        },
    );
}

/// Small square step button (`-` / `+`).
pub fn spawn_stepper(p: &mut ChildSpawnerCommands, fonts: &UiFonts, action: UiAction, glyph: char, tip: Tooltip) {
    spawn_button(
        p,
        UiButton::new(action, ButtonKind::Stepper),
        Node {
            width: px(28),
            height: px(28),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Some(tip),
        |b, id| {
            b.spawn((
                icon(fonts, glyph, 14.0, t::SLOT_INK),
                ButtonFg {
                    owner: id,
                    base: t::SLOT_INK,
                },
            ));
        },
    );
}

/// The corner brackets of a selected picture item (shown while active).
pub fn spawn_brackets(b: &mut ChildSpawnerCommands, id: Entity) {
    for (top, left) in [(true, true), (false, false)] {
        let mut n = Node {
            position_type: PositionType::Absolute,
            width: px(10),
            height: px(10),
            border: UiRect {
                left: px(if left { 3.0 } else { 0.0 }),
                right: px(if left { 0.0 } else { 3.0 }),
                top: px(if top { 3.0 } else { 0.0 }),
                bottom: px(if top { 0.0 } else { 3.0 }),
            },
            ..default()
        };
        if top {
            n.top = px(-6);
            n.left = px(-6);
        } else {
            n.bottom = px(-6);
            n.right = px(-6);
        }
        b.spawn((
            ActiveMark { owner: id },
            layout(n),
            BorderColor::all(t::ACCENT),
            Visibility::Hidden,
            ZIndex(2),
        ));
    }
}

// ---------------------------------------------------------------- behaviour

/// Context needed to decide whether a button shows as active.
pub struct ActiveCtx<'a> {
    pub tool: &'a ActiveTool,
    pub overlay: &'a OverlayMode,
    pub clock: &'a SimClock,
    pub quality: &'a GraphicsQuality,
    pub state: &'a UiState,
    pub modes: &'a ToolModes,
    pub kind: &'a RoadChoice,
    pub display: &'a DisplaySettings,
    pub opts: &'a super::tools::ToolOptions,
}

pub fn is_active(action: UiAction, cx: &ActiveCtx<'_>) -> bool {
    use super::InfoTray;
    match action {
        UiAction::Tool(ActiveTool::Road) => *cx.tool == ActiveTool::Road && cx.modes.road != RoadMode::Junction,
        UiAction::Tool(ActiveTool::Inspect) => *cx.tool == ActiveTool::Inspect && cx.state.tray.is_none(),
        UiAction::Tool(tool) => *cx.tool == tool,
        UiAction::ZoneTool => matches!(cx.tool, ActiveTool::Zone(_)),
        UiAction::Overlay(o) => *cx.overlay == o,
        UiAction::TogglePause => cx.clock.paused,
        UiAction::Speed(i) => !cx.clock.paused && cx.clock.speed == i,
        UiAction::ToggleBudget => cx.state.tray == Some(InfoTray::Budget),
        UiAction::Maps => cx.state.tray == Some(InfoTray::Maps),
        UiAction::Stats => cx.state.tray == Some(InfoTray::Stats),
        UiAction::CityPeople => cx.state.tray == Some(InfoTray::People),
        UiAction::Quality(q) => *cx.quality == q,
        UiAction::RoadMode(m) => *cx.tool == ActiveTool::Road && cx.modes.road == m,
        UiAction::ZoneMode(m) => matches!(cx.tool, ActiveTool::Zone(_)) && cx.modes.zone == m,
        UiAction::BulldozeMode(m) => *cx.tool == ActiveTool::Bulldoze && cx.modes.bulldoze == m,
        UiAction::RoadKind(k) => *cx.kind == k,
        UiAction::UiScale(i) => cx.state.ui_scale == i,
        UiAction::EdgeScroll(on) => cx.display.edge_scroll == on,
        UiAction::LotMarkers(on) => cx.display.lot_markers == on,
        UiAction::Perf(d) => cx.state.perf == d,
        UiAction::Snap(r) => cx.opts.rule(r),
        UiAction::ZoneDepth(d) => matches!(cx.tool, ActiveTool::Zone(_)) && cx.opts.zone_depth == d,
        UiAction::JunctionBrush(b) => cx.opts.junction_brush == b,
        UiAction::Follow => cx.state.following,
        UiAction::Keys => cx.state.keys_open,
        UiAction::OpenSettings => cx.state.settings_open,
        _ => false,
    }
}

pub(super) fn button_clicks(
    mouse: Res<ButtonInput<MouseButton>>,
    buttons: Query<(Entity, &UiButton, &Hovered, Has<Disabled>)>,
    mut pressed: ResMut<PressedButton>,
    mut out: MessageWriter<UiAction>,
) {
    if mouse.just_pressed(MouseButton::Left) {
        pressed.0 = buttons.iter().find(|(_, _, h, d)| h.get() && !d).map(|(e, ..)| e);
    }
    if mouse.just_released(MouseButton::Left) {
        if let Some(e) = pressed.0.take()
            && let Ok((_, b, h, d)) = buttons.get(e)
            && h.get()
            && !d
            && b.action != UiAction::None
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

/// Surface colours of a button in one state.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    top: Color,
    bottom: Color,
    border: Color,
}

fn blend(a: Look, b: Look, k: f32) -> Look {
    Look {
        top: t::mix(a.top, b.top, k),
        bottom: t::mix(a.bottom, b.bottom, k),
        border: t::mix(a.border, b.border, k),
    }
}

fn base_look(kind: ButtonKind) -> Look {
    let clear = Look {
        top: Color::NONE,
        bottom: Color::NONE,
        border: Color::NONE,
    };
    match kind {
        ButtonKind::Ghost | ButtonKind::Sys | ButtonKind::Danger => clear,
        ButtonKind::Row => Look {
            border: t::RULE,
            ..clear
        },
        ButtonKind::Action => Look {
            border: t::LINE,
            ..clear
        },
        ButtonKind::Primary => Look {
            top: t::ACCENT_SET.fill_top,
            bottom: t::ACCENT_SET.fill_bottom,
            border: t::ACCENT,
        },
        _ => Look {
            top: t::SLOT_TOP,
            bottom: t::SLOT_BOTTOM,
            border: t::LINE,
        },
    }
}

fn hover_look(kind: ButtonKind) -> Look {
    match kind {
        ButtonKind::Ghost | ButtonKind::Sys => Look {
            top: t::alpha(Color::WHITE, 0.07),
            bottom: t::alpha(Color::WHITE, 0.07),
            border: Color::NONE,
        },
        ButtonKind::Row | ButtonKind::Action => Look {
            top: t::alpha(Color::WHITE, 0.06),
            bottom: t::alpha(Color::WHITE, 0.03),
            border: if kind == ButtonKind::Action { t::LINE } else { t::RULE },
        },
        ButtonKind::Danger => Look {
            top: t::alpha(t::NEGATIVE, 0.18),
            bottom: t::alpha(t::NEGATIVE, 0.12),
            border: Color::NONE,
        },
        ButtonKind::Primary => Look {
            top: t::mix(t::ACCENT_SET.fill_top, Color::WHITE, 0.12),
            bottom: t::mix(t::ACCENT_SET.fill_bottom, Color::WHITE, 0.08),
            border: t::ACCENT_SET.fill_top,
        },
        _ => Look {
            top: t::SLOT_HOVER_TOP,
            bottom: t::SLOT_HOVER_BOTTOM,
            border: t::mix(t::LINE, t::TEXT_MUTED, 0.45),
        },
    }
}

fn active_look(kind: ButtonKind) -> Option<Look> {
    match kind {
        ButtonKind::Slot | ButtonKind::Segment | ButtonKind::Speed | ButtonKind::Primary | ButtonKind::Action => {
            Some(Look {
                top: t::ACCENT_SET.fill_top,
                bottom: t::ACCENT_SET.fill_bottom,
                border: t::ACCENT,
            })
        }
        ButtonKind::Item => Some(Look {
            top: t::SLOT_TOP,
            bottom: t::SLOT_BOTTOM,
            border: t::ACCENT,
        }),
        ButtonKind::Row => Some(Look {
            top: t::alpha(t::ACCENT, 0.16),
            bottom: t::alpha(t::ACCENT, 0.10),
            border: t::RULE,
        }),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn button_visuals(
    (time, pressed): (Res<Time<Real>>, Res<PressedButton>),
    (tool, overlay, clock, quality): (Res<ActiveTool>, Res<OverlayMode>, Res<SimClock>, Res<GraphicsQuality>),
    (state, modes, kind, display, opts): (
        Res<UiState>,
        Res<ToolModes>,
        Res<RoadChoice>,
        Res<DisplaySettings>,
        Res<super::tools::ToolOptions>,
    ),
    mut buttons: Query<(
        Entity,
        &UiButton,
        Ref<Hovered>,
        &mut ButtonAnim,
        Option<&mut BackgroundGradient>,
        Option<&mut PlateAnim>,
        &mut BorderColor,
        &mut UiTransform,
        Has<Disabled>,
        Has<Focused>,
        Option<&mut Outline>,
    )>,
    mut fgs: Query<(&ButtonFg, &mut TextColor)>,
    mut marks: Query<(&ActiveMark, &mut Visibility)>,
    mut leds: Query<(&Led, &mut BackgroundColor, Option<&mut BoxShadow>)>,
    mut fills: Query<(&ActiveFill, &mut BackgroundGradient, &mut BorderColor), Without<UiButton>>,
    mut commands: Commands,
) {
    let cx = ActiveCtx {
        tool: &tool,
        overlay: &overlay,
        clock: &clock,
        quality: &quality,
        state: &state,
        modes: &modes,
        kind: &kind,
        display: &display,
        opts: &opts,
    };
    let dt = time.delta_secs();
    let k = approach(dt, 0.07);
    for (e, b, hovered, mut anim, grad, plate, mut border, mut tf, disabled, focused, outline) in &mut buttons {
        let target_press = (pressed.0 == Some(e) && hovered.get() && !disabled) as u8 as f32;
        let target_hover = (hovered.get() && !disabled) as u8 as f32;
        let target_active = is_active(b.action, &cx) as u8 as f32;
        let before = *anim;
        anim.hover += (target_hover - anim.hover) * k;
        anim.press += (target_press - anim.press) * approach(dt, 0.03);
        anim.active += (target_active - anim.active) * approach(dt, 0.09);
        anim.focus += ((focused as u8 as f32) - anim.focus) * k;
        anim.disabled += ((disabled as u8 as f32) - anim.disabled) * k;
        {
            let a = anim.bypass_change_detection();
            for v in [&mut a.hover, &mut a.press, &mut a.active, &mut a.focus, &mut a.disabled] {
                if (*v - v.round()).abs() < 0.003 {
                    *v = v.round();
                }
            }
        }
        let a = *anim;
        let settled = a.hover == before.hover
            && a.press == before.press
            && a.active == before.active
            && a.focus == before.focus
            && a.disabled == before.disabled;
        if settled && !hovered.is_changed() && !anim.is_added() {
            continue;
        }
        let mut look = blend(base_look(b.kind), hover_look(b.kind), a.hover);
        if let Some(act) = active_look(b.kind) {
            // Hovering an active button lifts it a touch.
            let act = blend(act, hover_look(ButtonKind::Primary), a.hover * 0.35);
            look = blend(look, act, a.active);
        }
        if a.disabled > 0.0 {
            look.border = t::mix(look.border, t::alpha(t::LINE, 0.5), a.disabled);
        }
        if a.press > 0.0 {
            look.top = t::mix(look.top, look.bottom, 0.6 * a.press);
        }
        match (grad, plate) {
            (_, Some(mut p)) => {
                let style = PlateStyle::fill(look.top, look.bottom, look.border).slanted(SPEED_SLANT);
                if p.style != style {
                    p.style = style;
                }
            }
            (Some(mut g), None) => {
                let new = vgrad(look.top, look.bottom);
                if *g != new {
                    *g = new;
                }
            }
            (None, None) => {}
        }
        border.set_if_neq(BorderColor::all(look.border));
        let scale = Vec2::splat(1.0 - 0.04 * a.press);
        if tf.scale != scale {
            tf.scale = scale;
        }
        // Keyboard focus: an accent outline just outside the button.
        let want = (a.focus > 0.01).then(|| Outline {
            width: px(2),
            offset: px(2),
            color: t::alpha(t::ACCENT, a.focus),
        });
        match (outline, want) {
            (Some(mut o), Some(w)) => {
                if *o != w {
                    *o = w;
                }
            }
            (None, Some(w)) => {
                commands.entity(e).insert(w);
            }
            (Some(_), None) => {
                commands.entity(e).remove::<Outline>();
            }
            (None, None) => {}
        }
    }
    for (fg, mut color) in &mut fgs {
        let Ok((_, b, _, anim, ..)) = buttons.get(fg.owner) else {
            continue;
        };
        let mut c = fg.base;
        match b.kind {
            ButtonKind::Ghost | ButtonKind::Row | ButtonKind::Sys | ButtonKind::Action => {
                c = t::mix(c, t::TEXT, anim.hover);
            }
            ButtonKind::Danger => c = t::mix(c, t::NEGATIVE, anim.hover),
            ButtonKind::Toggle => c = t::mix(c, t::TEXT, anim.active.max(anim.hover)),
            _ => c = t::mix(c, t::TEXT, anim.hover * 0.6),
        }
        match b.kind {
            ButtonKind::Slot
            | ButtonKind::Segment
            | ButtonKind::Speed
            | ButtonKind::Primary
            | ButtonKind::Action
            | ButtonKind::Item => {
                c = t::mix(c, t::ACCENT_INK, anim.active);
            }
            ButtonKind::Row | ButtonKind::Sys | ButtonKind::Ghost => c = t::mix(c, t::ACCENT_TEXT, anim.active),
            _ => {}
        }
        if b.kind == ButtonKind::Primary {
            c = t::ACCENT_INK;
        }
        c = t::mix(c, t::TEXT_FAINT, anim.disabled * 0.8);
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
    for (f, mut g, mut border) in &mut fills {
        let Ok((_, _, _, anim, ..)) = buttons.get(f.owner) else {
            continue;
        };
        let k = anim.active;
        let new = vgrad(
            t::alpha(t::ACCENT_SET.fill_top, k),
            t::alpha(t::ACCENT_SET.fill_bottom, k),
        );
        if *g != new {
            *g = new;
        }
        border.set_if_neq(BorderColor::all(t::mix(t::LINE, t::ACCENT, k)));
    }
    for (led, mut bg, shadow) in &mut leds {
        let Ok((_, _, _, anim, ..)) = buttons.get(led.owner) else {
            continue;
        };
        let c = t::mix(t::alpha(t::TEXT_MUTED, 0.5), t::ACCENT_TEXT, anim.active);
        bg.set_if_neq(BackgroundColor(c));
        let glow = BoxShadow(vec![ShadowStyle {
            color: t::alpha(t::ACCENT, 0.9 * anim.active),
            x_offset: px(0),
            y_offset: px(0),
            spread_radius: px(0),
            blur_radius: px(8),
        }]);
        if let Some(mut s) = shadow
            && *s != glow
        {
            *s = glow;
        }
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
