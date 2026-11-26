//! The selection panel, attached to the right edge: whatever is inspected
//! or selected — a person (now and next, the trip, household, money,
//! happiness with its reasons, journal), a home with its households, a
//! workplace with its people, a car with its occupants, a road stretch, a
//! junction. One language: a header strip with the name, a portrait or
//! symbol with two lines, the trip box, label / value rows, meters, and an
//! action bar (follow, home, work). Back and links between views.
//!
//! Also the city's people (the People page of the city window) and following:
//! the camera stays on whoever is followed (`CameraFollow`).
//!
//! Thin widgets: everything shown comes from `people.rs` sheets and
//! `context.rs` cards.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::ComputedNode;
use citysim_sim::{Command, JunctionSetting, junction};

use super::people::{self, FoldKey, Item, Portrait, Sheet, SheetKind, Target};
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::readout::Tone;
use super::theme::{self as t, UiFonts, icon};
use super::toasts::{Toast, ToastKind};
use super::widgets::{
    ButtonFg, ButtonKind, Tooltip, UiAction, UiButton, approach, caps, figure, hrule, label, layout, paragraph,
    plate_node, plate_shadow, spawn_button, spawn_key,
};
use super::{CityPage, UiState};
use crate::shared::{ActiveTool, CameraFollow, GameSet, SimCommand, SimWorld, WorldHighlight, WorldTarget};

/// What the panel shows of people, places and cars: the last of `stack`
/// (earlier entries are "back").
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct PeopleView {
    pub stack: Vec<Target>,
}

impl PeopleView {
    pub fn current(&self) -> Option<Target> {
        self.stack.last().copied()
    }

    pub fn open(&mut self, t: Target) {
        if self.current() != Some(t) {
            self.stack.push(t);
            if self.stack.len() > 12 {
                self.stack.remove(0);
            }
        }
    }
}

/// Sections of the side panel the player folded or opened (others keep
/// `FoldKey::open_by_default`).
#[derive(Resource, Debug, Default, Clone, PartialEq)]
pub struct Folds(pub std::collections::BTreeMap<FoldKey, bool>);

impl Folds {
    pub fn open(&self, k: FoldKey) -> bool {
        self.0.get(&k).copied().unwrap_or(k.open_by_default())
    }
}

/// Where the side panel's plate is on screen (logical px), while it shows.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct PanelArea(pub Option<Rect>);

/// The plate of the side panel.
#[derive(Component)]
struct PanelPlate;

/// The line from what is selected to the panel, and its two ends.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Leader {
    Line,
    Dot,
    Tip,
}

pub struct PanelPlugin;

impl Plugin for PanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PeopleView>()
            .init_resource::<PanelArea>()
            .init_resource::<Folds>()
            .add_systems(Startup, (spawn_panel, spawn_leader))
            .add_systems(
                PostUpdate,
                (panel_area, leader).chain().after(bevy::ui::UiSystems::Layout),
            )
            .add_systems(
                Update,
                (
                    people_input,
                    follow,
                    draw_sheets,
                    add_ellipsis,
                    fit_ellipsis,
                    slide_panel,
                    scroll_bodies,
                )
                    .chain()
                    .in_set(GameSet::Render),
            );
    }
}

/// Which sheet a widget belongs to.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum Root {
    /// The right-edge panel.
    Panel,
    /// The city's people in the tray.
    City,
}

#[derive(Component)]
struct PanelRoot {
    t: f32,
    open: bool,
}

/// Where a sheet's widgets go (rebuilt when its shape changes).
#[derive(Component, Clone, Copy)]
struct Body(Root);

/// Which text of a sheet an entity shows.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum SheetText {
    Title,
    Headline,
    Subtitle,
    TripTitle,
    TripLeft,
    TripFrom,
    TripTo,
    MoodValue,
    MoodReason,
    Section(usize),
    Label(usize, usize),
    Value(usize, usize),
}

/// Which bar of a sheet an entity fills.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum SheetBar {
    Trip,
    Item(usize, usize),
}

/// The trip's position marker.
#[derive(Component)]
struct TripMark;

/// Mood pip `i` (of 10).
#[derive(Component, Clone, Copy)]
struct Pip(usize);

#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct Of(Root);

/// The kind of an item, for deciding whether to rebuild.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ItemShape {
    Line(bool),
    Link(Target),
    Bar,
    Jump,
}

/// What a sheet's widgets look like, without the texts.
#[derive(Debug, Clone, PartialEq)]
struct SheetShape {
    kind: SheetKind,
    portrait: Option<Portrait>,
    trip: bool,
    mood: bool,
    /// Per section: titled, folded, items (none while folded).
    sections: Vec<(bool, bool, Vec<ItemShape>)>,
    actions: Vec<Target>,
    /// Back, following, can follow, junction setting.
    controls: (bool, bool, bool, Option<JunctionSetting>),
    /// Laid out for a small window (the city drawer).
    compact: bool,
}

fn shape_of(s: &Sheet, controls: (bool, bool, bool, Option<JunctionSetting>), folds: &Folds) -> SheetShape {
    SheetShape {
        kind: s.kind,
        portrait: s.portrait,
        trip: s.trip.is_some(),
        mood: s.mood.is_some(),
        sections: s
            .sections
            .iter()
            .map(|sec| {
                let folded = sec.fold.is_some_and(|k| !folds.open(k));
                (
                    !sec.title.is_empty(),
                    folded,
                    if folded {
                        Vec::new()
                    } else {
                        sec.items
                            .iter()
                            .map(|it| match it {
                                Item::Line(l) => ItemShape::Line(l.label.is_empty()),
                                Item::Link(l) => ItemShape::Link(l.target),
                                Item::Bar { .. } => ItemShape::Bar,
                                Item::Jump { .. } => ItemShape::Jump,
                            })
                            .collect()
                    },
                )
            })
            .collect(),
        actions: s.actions.iter().map(|l| l.target).collect(),
        controls,
        compact: false,
    }
}

fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Normal => t::TEXT,
        Tone::Muted => t::TEXT_MUTED,
        Tone::Accent => t::ACCENT_TEXT,
        Tone::Good => t::POSITIVE,
        Tone::Warn => t::WARNING,
        Tone::Bad => t::NEGATIVE,
    }
}

fn tone_of(c: Color) -> Tone {
    if c == t::NEGATIVE {
        Tone::Bad
    } else if c == t::WARNING {
        Tone::Warn
    } else if c == t::POSITIVE {
        Tone::Good
    } else if c == t::TEXT_MUTED || c == t::TEXT_FAINT {
        Tone::Muted
    } else if c == t::ACCENT || c == t::ACCENT_TEXT {
        Tone::Accent
    } else {
        Tone::Normal
    }
}

/// A one-line text cut with "…" to the width its node gets (measured from
/// the laid-out text, so it does not depend on the font or the letters).
/// `full` is the whole text; the shown `Text` may be shorter.
#[derive(Component, Debug, Default)]
pub struct Ellipsis {
    pub full: String,
}

/// Give one-line sheet texts an [`Ellipsis`].
fn add_ellipsis(mut commands: Commands, added: Query<(Entity, &SheetText, &TextLayout), Added<SheetText>>) {
    for (e, tag, layout) in &added {
        let measured = matches!(
            tag,
            SheetText::Title | SheetText::Headline | SheetText::Section(_) | SheetText::Value(..)
        );
        if measured && layout.linebreak == bevy::text::LineBreak::NoWrap {
            commands.entity(e).insert(Ellipsis::default());
        }
    }
}

/// Cut texts wider than their node: estimate from the measured widths,
/// then a letter at a time until it fits (each step is measured on the
/// next layout).
pub fn fit_ellipsis(mut texts: Query<(&mut Text, &Ellipsis, &ComputedNode, &bevy::text::TextLayoutInfo)>) {
    for (mut text, e, node, info) in &mut texts {
        let avail = node.size.x - node.padding.min_inset.x - node.padding.max_inset.x;
        if avail <= 1.0 || info.size.x <= avail + 0.5 || e.full.is_empty() {
            continue;
        }
        let shown: Vec<char> = text.0.trim_end_matches('\u{2026}').chars().collect();
        if shown.len() <= 1 {
            continue;
        }
        let guess = (shown.len() as f32 * avail / info.size.x).floor() as usize;
        let keep = guess.min(shown.len() - 1).max(1);
        let cut: String = shown[..keep].iter().collect();
        text.0 = format!("{}\u{2026}", cut.trim_end());
    }
}

/// The text and colour a tagged entity shows in a sheet.
fn text_of(s: &Sheet, tag: SheetText) -> Option<(String, Option<Color>)> {
    Some(match tag {
        SheetText::Title => (s.title.to_uppercase(), None),
        SheetText::Headline => (s.headline.clone(), None),
        SheetText::Subtitle => (s.subtitle.clone(), None),
        SheetText::TripTitle => (s.trip.as_ref()?.title.to_uppercase(), None),
        SheetText::TripLeft => {
            let trip = s.trip.as_ref()?;
            (
                trip.left.clone(),
                Some(if trip.late { t::WARNING } else { t::ACCENT_TEXT }),
            )
        }
        SheetText::TripFrom => (s.trip.as_ref()?.from.clone(), None),
        SheetText::TripTo => (s.trip.as_ref()?.to.clone(), None),
        SheetText::MoodValue => (format!("{}", (s.mood? * 100.0).round() as i64), None),
        SheetText::MoodReason => (s.mood_reason.clone().unwrap_or_default(), None),
        SheetText::Section(i) => (s.sections.get(i)?.title.to_uppercase(), None),
        SheetText::Label(i, j) => match s.sections.get(i)?.items.get(j)? {
            Item::Line(l) => (l.label.to_uppercase(), None),
            Item::Link(l) => (l.label.to_uppercase(), None),
            Item::Bar { label, .. } => (label.to_uppercase(), None),
            Item::Jump { label, .. } => (label.to_uppercase(), None),
        },
        SheetText::Value(i, j) => match s.sections.get(i)?.items.get(j)? {
            Item::Line(l) if l.label.is_empty() => (l.value.clone(), Some(tone_color(l.tone))),
            Item::Line(l) => (l.value.clone(), Some(tone_color(l.tone))),
            Item::Link(l) => (l.value.clone(), None),
            Item::Bar { value, .. } => (value.clone(), None),
            Item::Jump { value, .. } => (value.clone(), None),
        },
    })
}

fn bar_of(s: &Sheet, b: SheetBar) -> Option<f32> {
    match b {
        SheetBar::Trip => s.trip.as_ref().map(|t| t.progress),
        SheetBar::Item(i, j) => match s.sections.get(i)?.items.get(j)? {
            Item::Bar { share, .. } => Some(*share),
            _ => None,
        },
    }
}

/// Room above and below the panel (logical px).
const PANEL_MARGIN_TOP: f32 = 62.0;
const PANEL_MARGIN_BOTTOM: f32 = t::BAR_H + 14.0;

fn spawn_panel(mut commands: Commands, mut cache: ResMut<PlateCache>, mut mats: ResMut<Assets<PlateMaterial>>) {
    let mat = cache.get(&mut mats, PlateStyle::PANEL);
    commands
        .spawn((
            Name::new("Selection panel"),
            PanelRoot { t: 0.0, open: false },
            layout(Node {
                position_type: PositionType::Absolute,
                right: px(0),
                top: px(PANEL_MARGIN_TOP),
                bottom: px(PANEL_MARGIN_BOTTOM),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::End,
                ..default()
            }),
            UiTransform::default(),
            GlobalZIndex(22),
            Visibility::Hidden,
        ))
        .with_children(|r| {
            r.spawn((
                PanelPlate,
                plate_node(
                    Node {
                        width: px(t::PANEL_W),
                        max_height: percent(100),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    UiRect::new(px(t::LW), px(0), px(t::LW), px(t::LW)),
                    [t::CUT, 0.0, 0.0, t::CUT],
                ),
                mat,
                plate_shadow(),
            ))
            .with_child((
                Body(Root::Panel),
                layout(Node {
                    flex_direction: FlexDirection::Column,
                    max_height: percent(100),
                    width: percent(100),
                    ..default()
                }),
            ));
        });
}

fn target_of(t: &WorldTarget) -> Option<Target> {
    match t {
        WorldTarget::Building(b) => Some(Target::Place(*b)),
        WorldTarget::Vehicle(v) => Some(Target::Vehicle(*v)),
        _ => None,
    }
}

/// Clicks on links, panel buttons and the map.
#[allow(clippy::too_many_arguments)]
fn people_input(
    keys: Res<ButtonInput<KeyCode>>,
    tool: Res<ActiveTool>,
    mut actions: MessageReader<UiAction>,
    mut view: ResMut<PeopleView>,
    mut ui: ResMut<UiState>,
    mut highlight: ResMut<WorldHighlight>,
    mouse: Res<ButtonInput<MouseButton>>,
    over_ui: Res<crate::shared::PointerOverUi>,
    sim: Res<SimWorld>,
    mut cmds: MessageWriter<SimCommand>,
    mut toasts: MessageWriter<Toast>,
    mut folds: ResMut<Folds>,
) {
    for a in actions.read() {
        match *a {
            UiAction::Person(c) => view.open(Target::Person(c)),
            UiAction::Place(b) => view.open(Target::Place(b)),
            UiAction::Vehicle(v) => view.open(Target::Vehicle(v)),
            UiAction::Follow => ui.following = !ui.following && view.current().is_some(),
            UiAction::StopFollow => ui.following = false,
            UiAction::PeopleBack => {
                view.stack.pop();
                if view.stack.is_empty() {
                    ui.following = false;
                }
            }
            UiAction::ClosePeople => {
                view.stack.clear();
                ui.following = false;
                highlight.selected = None;
            }
            UiAction::Fold(k) => {
                let open = folds.open(k);
                folds.0.insert(k, !open);
            }
            UiAction::SetJunction(s) => {
                if let Some(WorldTarget::Junction { node, .. }) = highlight.selected {
                    let cmd = Command::SetNodeControl { node, control: s };
                    if sim.world.preview(&cmd).is_ok() {
                        cmds.write(SimCommand(cmd));
                    } else {
                        toasts.write(Toast::new(ToastKind::Error, "This junction can't be changed"));
                    }
                }
            }
            _ => {}
        }
    }
    // F follows whoever the panel shows.
    if keys.just_pressed(KeyCode::KeyF) && !ui.menu_open && view.current().is_some() {
        ui.following = !ui.following;
    }
    // A camera pan key stops following.
    if ui.following
        && keys.any_just_pressed([
            KeyCode::KeyW,
            KeyCode::KeyA,
            KeyCode::KeyS,
            KeyCode::KeyD,
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
        ])
    {
        ui.following = false;
    }
    // A click with Inspect (the tools pinned what it hit this frame): a
    // building or car opens its people; a road or junction shows itself;
    // empty ground closes the panel (unless following).
    if mouse.just_pressed(MouseButton::Left) && !over_ui.0 && *tool == ActiveTool::Inspect {
        match highlight.selected.as_ref() {
            Some(sel) => match target_of(sel) {
                Some(t) => view.open(t),
                None => {
                    view.stack.clear();
                    ui.following = false;
                }
            },
            None if !ui.following => view.stack.clear(),
            None => {}
        }
    }
    if view.stack.is_empty() {
        ui.following = false;
    }
}

/// Keep the camera on whoever is followed, and highlight where they are.
fn follow(
    sim: Res<SimWorld>,
    ui: Res<UiState>,
    mut view: ResMut<PeopleView>,
    mut cam: ResMut<CameraFollow>,
    mut highlight: ResMut<WorldHighlight>,
) {
    let w = &sim.world;
    // A car that arrived: carry on with its owner.
    if let Some(Target::Vehicle(v)) = view.current()
        && !w.vehicles.contains_key(v)
    {
        let owner = w.traffic.trips.get(v).and_then(|t| t.owner);
        view.stack.pop();
        if let Some(o) = owner {
            view.open(Target::Person(o));
        }
    }
    let at = match view.current() {
        Some(Target::Person(c)) => people::person_position(w, c),
        Some(Target::Place(b)) => w.buildings.get(b).map(|b| b.center()),
        Some(Target::Vehicle(v)) => w.vehicles.get(v).map(|veh| citysim_sim::drive::vehicle_pose(w, veh).0),
        None => None,
    };
    cam.set_if_neq(CameraFollow {
        focus: at.filter(|_| ui.following),
    });
    // Where a person is shows in the world (their car or building).
    if let Some(Target::Person(c)) = view.current()
        && let Some(v) = citysim_sim::inspect::citizen(w, c)
    {
        let car = v.trip.and_then(|t| t.vehicle).filter(|c| w.vehicles.contains_key(*c));
        let sel = match (car, v.trip, v.inside) {
            (Some(car), _, _) => Some(WorldTarget::Vehicle(car)),
            // On the way where the map draws nothing: mark where they go.
            (None, Some(t), _) => Some(WorldTarget::Building(t.to)),
            (None, None, Some(b)) => Some(WorldTarget::Building(b)),
            _ => None,
        };
        if highlight.selected != sel {
            highlight.selected = sel;
        }
    }
    // A car shown in the panel stays marked.
    if let Some(Target::Vehicle(v)) = view.current() {
        let sel = Some(WorldTarget::Vehicle(v));
        if highlight.selected != sel {
            highlight.selected = sel;
        }
    }
}

/// The sheet the panel shows now: a person / place / car, else the
/// selected road or junction.
fn panel_sheet(world: &citysim_sim::World, view: &PeopleView, hl: &WorldHighlight) -> Option<Sheet> {
    if let Some(t) = view.current() {
        return match t {
            Target::Person(c) => people::person_sheet(world, c),
            Target::Place(b) => people::place_sheet(world, b),
            Target::Vehicle(v) => people::vehicle_sheet(world, v),
        };
    }
    let target = hl.selected.as_ref()?;
    if !matches!(target, WorldTarget::Road(_) | WorldTarget::Junction { .. }) {
        return None;
    }
    let card = super::context::inspect_card(world, target)?;
    let mut s = Sheet {
        kind: if matches!(target, WorldTarget::Junction { .. }) {
            SheetKind::Junction
        } else {
            SheetKind::Road
        },
        ..Default::default()
    };
    // "Avenue · 2 lanes each way · 152 m" -> title and headline.
    let mut parts = card.title.splitn(2, " · ");
    s.title = parts.next().unwrap_or_default().to_string();
    s.headline = parts.next().unwrap_or_default().to_string();
    s.subtitle = card.hint.clone();
    let mut sec = people::Section::new("");
    for r in &card.rows {
        sec.line(r.label.clone(), r.value.clone(), tone_of(r.color));
    }
    if let Some((f, _)) = card.bar {
        sec.bar("Load", format!("{}%", (f * 100.0).round() as i64), f);
    }
    s.sections.push(sec);
    Some(s)
}

#[derive(Default)]
struct Shown {
    panel: Option<SheetShape>,
    city: Option<SheetShape>,
    age: f32,
}

type TextQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static SheetText,
        &'static Of,
        &'static mut Text,
        &'static mut TextColor,
        Option<&'static mut Ellipsis>,
    ),
>;
type BarQuery<'w, 's> = Query<'w, 's, (&'static SheetBar, &'static Of, &'static mut Node), Without<TripMark>>;

/// Rebuild a sheet's widgets when its shape changed; otherwise update its
/// texts and bars in place (a few times a second).
#[allow(clippy::too_many_arguments)]
fn draw_sheets(
    mut commands: Commands,
    time: Res<Time<Real>>,
    fonts: Res<UiFonts>,
    sim: Res<SimWorld>,
    ui: Res<UiState>,
    (view, folds): (Res<PeopleView>, Res<Folds>),
    hl: Res<WorldHighlight>,
    mut roots: Query<&mut PanelRoot>,
    bodies: Query<(Entity, &Body)>,
    city_body: Query<Entity, With<super::city::CitySheetBody>>,
    mut texts: TextQuery,
    mut bars: BarQuery,
    mut marks: Query<(&Of, &mut Node), With<TripMark>>,
    mut pips: Query<(&Pip, &Of, &mut BackgroundColor)>,
    mut shown: Local<Shown>,
) {
    shown.age += time.delta_secs();
    if shown.age < 0.25 && !view.is_changed() && !ui.is_changed() && !hl.is_changed() && !folds.is_changed() {
        return;
    }
    shown.age = 0.0;
    let w = &sim.world;
    let panel = panel_sheet(w, &view, &hl).filter(|_| !ui.menu_open);
    let city = (ui.window == Some(CityPage::People) && !ui.menu_open).then(|| people::city_sheet(w));
    if let Ok(mut r) = roots.single_mut() {
        r.open = panel.is_some();
    }
    let fonts = fonts.into_inner();
    let setting = match (&panel, &hl.selected) {
        (Some(s), Some(WorldTarget::Junction { node, .. })) if s.kind == SheetKind::Junction => {
            junction::junction(w, *node).map(|j| j.setting)
        }
        _ => None,
    };
    let can_follow = matches!(view.current(), Some(Target::Person(_) | Target::Vehicle(_)));
    let controls = (view.stack.len() > 1, ui.following, can_follow, setting);
    for (root, sheet) in [(Root::Panel, &panel), (Root::City, &city)] {
        let shape = sheet.as_ref().map(|s| SheetShape {
            compact: root == Root::City && ui.compact,
            ..shape_of(s, controls, &folds)
        });
        let old = match root {
            Root::Panel => &mut shown.panel,
            Root::City => &mut shown.city,
        };
        // A closing panel keeps its content while it slides out.
        if shape.is_none() && root == Root::Panel {
            continue;
        }
        if shape != *old {
            let target = match root {
                Root::Panel => bodies.iter().find(|b| b.1.0 == Root::Panel).map(|b| b.0),
                Root::City => city_body.single().ok(),
            };
            if let Some(body) = target {
                commands.entity(body).despawn_children();
                if let Some(s) = sheet {
                    commands.entity(body).with_children(|p| match root {
                        Root::Panel => spawn_panel_sheet(p, fonts, s, controls, &folds),
                        Root::City => spawn_city_sheet(p, fonts, s, ui.compact),
                    });
                }
            }
            *old = shape;
            // Fill the new widgets' texts on the next frame (they exist then).
            shown.age = f32::INFINITY;
            continue;
        }
        let Some(s) = sheet else { continue };
        for (tag, of, mut text, mut color, ellipsis) in &mut texts {
            if of.0 != root {
                continue;
            }
            if let Some((v, c)) = text_of(s, *tag) {
                match ellipsis {
                    // Measured texts: start again from the whole text when
                    // it changes (`fit_ellipsis` cuts it to fit).
                    Some(mut e) => {
                        if e.full != v {
                            text.0 = v.clone();
                            e.full = v;
                        }
                    }
                    None => {
                        if text.0 != v {
                            text.0 = v;
                        }
                    }
                }
                if let Some(c) = c {
                    color.set_if_neq(TextColor(c));
                }
            }
        }
        for (tag, of, mut node) in &mut bars {
            if of.0 != root {
                continue;
            }
            if let Some(share) = bar_of(s, *tag) {
                let wv = percent((share.clamp(0.0, 1.0) * 100.0).max(1.5));
                if node.width != wv {
                    node.width = wv;
                }
            }
        }
        if let Some(trip) = &s.trip {
            for (of, mut n) in &mut marks {
                if of.0 == root {
                    let l = percent(trip.progress.clamp(0.0, 1.0) * 100.0);
                    if n.left != l {
                        n.left = l;
                    }
                }
            }
        }
        if let Some(m) = s.mood {
            for (p, of, mut bg) in &mut pips {
                if of.0 == root {
                    bg.set_if_neq(BackgroundColor(pip_color(m, p.0)));
                }
            }
        }
    }
}

fn mood_color(h: f32) -> Color {
    if h >= 0.7 {
        t::POSITIVE
    } else if h >= citysim_sim::balance::UNHAPPY_BELOW {
        t::WARNING
    } else {
        t::NEGATIVE
    }
}

fn pip_color(mood: f32, i: usize) -> Color {
    if (i as f32) < (mood * 10.0).round() {
        mood_color(mood)
    } else {
        t::LINE
    }
}

fn action_of(t: Target) -> UiAction {
    match t {
        Target::Person(c) => UiAction::Person(c),
        Target::Place(b) => UiAction::Place(b),
        Target::Vehicle(v) => UiAction::Vehicle(v),
    }
}

fn kind_icon(k: SheetKind) -> char {
    match k {
        SheetKind::Person | SheetKind::City => icon::USER,
        SheetKind::Home => icon::HOUSE,
        SheetKind::Workplace => icon::BRIEFCASE,
        SheetKind::Vehicle => icon::CAR,
        SheetKind::Road => icon::ROAD,
        SheetKind::Junction => icon::WAYPOINTS,
    }
}

/// Header strip: accent bar, name, back and close.
fn header(p: &mut ChildSpawnerCommands, fonts: &UiFonts, back: bool) {
    let of = Of(Root::Panel);
    p.spawn((
        layout(Node {
            height: px(44),
            padding: UiRect::new(px(24), px(8), px(0), px(0)),
            column_gap: px(10),
            align_items: AlignItems::Center,
            border: UiRect::bottom(px(t::LW)),
            flex_shrink: 0.0,
            ..default()
        }),
        BackgroundColor(t::HEAD),
        BorderColor::all(t::LINE),
    ))
    .with_children(|h| {
        h.spawn((
            layout(Node {
                width: px(5),
                height: px(22),
                flex_shrink: 0.0,
                ..default()
            }),
            BackgroundColor(t::ACCENT),
        ));
        h.spawn((
            Text::new(""),
            fonts.figures(t::FONT_HEAD, t::W_BOLD),
            t::caps_spacing(t::FONT_HEAD, 0.1),
            TextColor(t::TEXT),
            TextLayout::no_wrap(),
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: px(0),
                overflow: Overflow::clip(),
                ..default()
            },
            Pickable::IGNORE,
            SheetText::Title,
            of,
        ));
        if back {
            small(
                h,
                fonts,
                UiAction::PeopleBack,
                icon::CHEVRON_LEFT,
                "Back",
                "Back to the previous view",
            );
        }
        small(h, fonts, UiAction::ClosePeople, icon::CLOSE, "Close", "Close (Esc)");
    });
}

fn small(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    name: &'static str,
    tip: &'static str,
) {
    let id = spawn_button(
        p,
        UiButton::new(action, ButtonKind::Ghost),
        Node {
            width: px(30),
            height: px(30),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            ..default()
        },
        Some(Tooltip::new(name, tip)),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, 18.0, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
        },
    );
    p.commands().entity(id).insert(Name::new(name));
}

/// The whole right-edge sheet.
fn spawn_panel_sheet(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    s: &Sheet,
    controls: (bool, bool, bool, Option<JunctionSetting>),
    folds: &Folds,
) {
    let of = Of(Root::Panel);
    header(p, fonts, controls.0);
    // Scrolling body.
    p.spawn((
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::new(px(22), px(16), px(14), px(12)),
            overflow: Overflow::scroll_y(),
            flex_shrink: 1.0,
            min_height: px(0),
            ..default()
        },
        ScrollPosition::default(),
        Hovered::default(),
    ))
    .with_children(|b| {
        // Who / what.
        b.spawn(layout(Node {
            column_gap: px(13),
            align_items: AlignItems::Center,
            ..default()
        }))
        .with_children(|w| {
            frame(w, fonts, s);
            w.spawn(layout(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(3),
                flex_shrink: 1.0,
                min_width: px(0),
                ..default()
            }))
            .with_children(|c| {
                c.spawn((
                    label(fonts, "", t::FONT_LEAD, t::W_SEMIBOLD, t::TEXT),
                    Node {
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    SheetText::Headline,
                    of,
                ));
                c.spawn((
                    paragraph(fonts, "", t::FONT_BODY, t::W_REGULAR, t::TEXT_MUTED),
                    SheetText::Subtitle,
                    of,
                ));
            });
        });
        if s.trip.is_some() {
            trip_box(b, fonts, of);
        }
        if s.mood.is_some() {
            b.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                height: px(31),
                margin: UiRect::top(px(6)),
                ..default()
            }))
            .with_children(|r| {
                r.spawn(caps(fonts, "Happiness", t::FONT_CAP, t::TEXT_MUTED));
                r.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    ..default()
                }))
                .with_children(|v| {
                    v.spawn(layout(Node {
                        column_gap: px(2),
                        margin: UiRect::right(px(9)),
                        ..default()
                    }))
                    .with_children(|pp| {
                        for i in 0..10 {
                            pp.spawn((
                                layout(Node {
                                    width: px(9),
                                    height: px(13),
                                    ..default()
                                }),
                                BackgroundColor(pip_color(s.mood.unwrap_or(0.0), i)),
                                Pip(i),
                                of,
                            ));
                        }
                    });
                    v.spawn((figure(fonts, "", 15.0, t::TEXT), SheetText::MoodValue, of));
                    v.spawn((
                        label(fonts, "", 12.5, t::W_SEMIBOLD, t::WARNING),
                        Node {
                            margin: UiRect::left(px(8)),
                            ..default()
                        },
                        SheetText::MoodReason,
                        of,
                    ));
                });
            });
            b.spawn(hrule());
        }
        if let Some(setting) = controls.3 {
            junction_switch(b, fonts, setting);
        }
        sections(b, fonts, s, of, false, folds);
    });
    // Action bar.
    let follow = controls.2;
    if follow || !s.actions.is_empty() {
        p.spawn((
            layout(Node {
                border: UiRect::top(px(t::LW)),
                flex_shrink: 0.0,
                ..default()
            }),
            BorderColor::all(t::LINE),
        ))
        .with_children(|a| {
            if follow {
                let following = controls.1;
                let id = spawn_button(
                    a,
                    UiButton::new(
                        if following {
                            UiAction::StopFollow
                        } else {
                            UiAction::Follow
                        },
                        ButtonKind::Primary,
                    ),
                    action_node(true),
                    Some(
                        Tooltip::new(
                            if following { "Stop following" } else { "Follow" },
                            "The camera stays with them; a pan key or Esc lets go.",
                        )
                        .key("F"),
                    ),
                    |b, id| {
                        let fg = ButtonFg {
                            owner: id,
                            base: t::ACCENT_INK,
                        };
                        b.spawn((super::widgets::icon(fonts, icon::EYE, 15.0, t::ACCENT_INK), fg));
                        b.spawn((
                            caps(fonts, if following { "Stop" } else { "Follow" }, 12.5, t::ACCENT_INK),
                            fg,
                        ));
                        spawn_key(b, fonts, "F", t::ACCENT_INK);
                    },
                );
                a.commands().entity(id).insert(Name::new("Follow"));
            }
            for (i, l) in s.actions.iter().enumerate() {
                let glyph = match l.target {
                    Target::Place(_) if l.label == "Home" => icon::HOUSE,
                    Target::Place(_) => icon::BRIEFCASE,
                    Target::Person(_) => icon::USER,
                    Target::Vehicle(_) => icon::CAR,
                };
                let first = i == 0 && !follow;
                spawn_button(
                    a,
                    UiButton::new(action_of(l.target), ButtonKind::Action),
                    Node {
                        border: UiRect::left(px(if first { 0.0 } else { 1.0 })),
                        ..action_node(false)
                    },
                    Some(Tooltip::new(
                        match l.label.as_str() {
                            "Home" => "Home",
                            "Work" => "Work",
                            _ => "Driver",
                        },
                        "",
                    )),
                    |b, id| {
                        let fg = ButtonFg {
                            owner: id,
                            base: t::SLOT_INK,
                        };
                        b.spawn((super::widgets::icon(fonts, glyph, 15.0, t::SLOT_INK), fg));
                        b.spawn((caps(fonts, &l.label, 12.5, t::SLOT_INK), fg));
                    },
                );
            }
        });
    }
}

fn action_node(_primary: bool) -> Node {
    Node {
        flex_grow: 1.0,
        flex_basis: px(0),
        height: px(40),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        column_gap: px(7),
        ..default()
    }
}

/// Portrait (people) or symbol (places, cars, roads) in a square well with
/// an accent corner.
fn frame(p: &mut ChildSpawnerCommands, fonts: &UiFonts, s: &Sheet) {
    p.spawn((
        layout(Node {
            width: px(62),
            height: px(62),
            border: UiRect::all(px(1)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            ..default()
        }),
        BackgroundColor(t::WELL),
        BorderColor::all(t::LINE),
    ))
    .with_children(|f| {
        match &s.portrait {
            Some(face) => spawn_portrait(f, face),
            None => {
                f.spawn(super::widgets::icon(fonts, kind_icon(s.kind), 32.0, t::ACCENT_TEXT));
            }
        }
        f.spawn((
            layout(Node {
                position_type: PositionType::Absolute,
                left: px(-1),
                bottom: px(-1),
                width: px(12),
                height: px(12),
                border: UiRect::new(px(3), px(0), px(0), px(3)),
                ..default()
            }),
            BorderColor::all(t::ACCENT),
        ));
    });
}

/// "Driving to work 0:40", a progress line with a marker, from and to.
fn trip_box(p: &mut ChildSpawnerCommands, fonts: &UiFonts, of: Of) {
    p.spawn((
        layout(Node {
            flex_direction: FlexDirection::Column,
            margin: UiRect::top(px(13)),
            padding: UiRect::new(px(11), px(11), px(9), px(10)),
            border: UiRect::all(px(1)),
            ..default()
        }),
        BackgroundColor(t::WELL),
        BorderColor::all(t::LINE),
    ))
    .with_children(|b| {
        b.spawn(layout(Node {
            align_items: AlignItems::Center,
            column_gap: px(8),
            ..default()
        }))
        .with_children(|r| {
            r.spawn(super::widgets::icon(fonts, icon::CAR, 16.0, t::TEXT));
            r.spawn((
                Text::new(""),
                fonts.text(t::FONT_BODY, t::W_BOLD),
                t::caps_spacing(t::FONT_BODY, t::CAPS_EM),
                TextColor(t::TEXT),
                TextLayout::no_wrap(),
                Pickable::IGNORE,
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
                SheetText::TripTitle,
                of,
            ));
            r.spawn((
                figure(fonts, "", 19.0, t::ACCENT_TEXT),
                SheetText::TripLeft,
                of,
                Tooltip::new("Still to go", "Expected time to arrive, in game hours and minutes."),
                Hovered::default(),
            ));
        });
        b.spawn((
            layout(Node {
                height: px(5),
                margin: UiRect::new(px(0), px(0), px(9), px(6)),
                ..default()
            }),
            BackgroundColor(t::LINE),
        ))
        .with_children(|bar| {
            bar.spawn((
                layout(Node {
                    width: percent(0),
                    height: percent(100),
                    ..default()
                }),
                BackgroundColor(t::ACCENT),
                SheetBar::Trip,
                of,
            ));
            bar.spawn((
                layout(Node {
                    position_type: PositionType::Absolute,
                    top: px(-4),
                    width: px(5),
                    height: px(13),
                    margin: UiRect::left(px(-2)),
                    left: percent(0),
                    ..default()
                }),
                BackgroundColor(t::TEXT),
                TripMark,
                of,
            ));
        });
        b.spawn(layout(Node {
            justify_content: JustifyContent::SpaceBetween,
            column_gap: px(10),
            ..default()
        }))
        .with_children(|r| {
            r.spawn((
                label(fonts, "", t::FONT_SMALL - 0.5, t::W_MEDIUM, t::TEXT_MUTED),
                SheetText::TripFrom,
                of,
            ));
            r.spawn((
                label(fonts, "", t::FONT_SMALL - 0.5, t::W_MEDIUM, t::TEXT_MUTED),
                SheetText::TripTo,
                of,
            ));
        });
    });
}

/// The junction's control as a switch (Auto / Lights / Priority).
fn junction_switch(p: &mut ChildSpawnerCommands, fonts: &UiFonts, current: JunctionSetting) {
    p.spawn(layout(Node {
        align_items: AlignItems::Center,
        justify_content: JustifyContent::SpaceBetween,
        margin: UiRect::vertical(px(10)),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(caps(fonts, "Control", t::FONT_CAP, t::TEXT_MUTED));
        r.spawn(layout(Node::default())).with_children(|g| {
            for (i, (s, text)) in [
                (JunctionSetting::Auto, "Auto"),
                (JunctionSetting::Signal, "Lights"),
                (JunctionSetting::Priority, "Priority"),
            ]
            .into_iter()
            .enumerate()
            {
                let on = s == current;
                spawn_button(
                    g,
                    UiButton::new(
                        UiAction::SetJunction(s),
                        if on { ButtonKind::Primary } else { ButtonKind::Segment },
                    ),
                    Node {
                        height: px(30),
                        padding: UiRect::horizontal(px(12)),
                        margin: UiRect::left(px(if i == 0 { 0.0 } else { -1.0 })),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    Some(Tooltip::new(
                        super::context::setting_name(s),
                        "How this junction is controlled.",
                    )),
                    |b, id| super::widgets::button_face(b, fonts, id, None, text),
                );
            }
        });
    });
}

/// Sections: caps titles, label / value rows, links, meters.
fn sections(p: &mut ChildSpawnerCommands, fonts: &UiFonts, s: &Sheet, of: Of, city: bool, folds: &Folds) {
    for (si, sec) in s.sections.iter().enumerate() {
        section(p, fonts, si, sec, of, city, folds);
    }
}

/// One section: its title (a fold button when foldable) and its items.
fn section(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    si: usize,
    sec: &people::Section,
    of: Of,
    city: bool,
    folds: &Folds,
) {
    {
        p.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            margin: UiRect::top(px(if city || si == 0 && sec.title.is_empty() {
                4.0
            } else {
                10.0
            })),
            ..default()
        }))
        .with_children(|c| {
            let folded = sec.fold.is_some_and(|k| !folds.open(k));
            if let Some(k) = sec.fold {
                // A foldable title: the whole row is a button with a chevron.
                let id = spawn_button(
                    c,
                    UiButton::new(UiAction::Fold(k), ButtonKind::Row),
                    Node {
                        align_items: AlignItems::Center,
                        column_gap: px(8),
                        min_height: px(26),
                        margin: UiRect::bottom(px(if folded { 0.0 } else { 3.0 })),
                        padding: UiRect::right(px(2)),
                        border: UiRect::bottom(px(if folded { 1.0 } else { 0.0 })),
                        ..default()
                    },
                    Some(Tooltip::new(
                        if folded { "Show" } else { "Hide" },
                        "Fold this part of the panel away; it stays folded for everyone you look at.",
                    )),
                    |h, id| {
                        h.spawn((
                            layout(Node {
                                width: px(3),
                                height: px(11),
                                flex_shrink: 0.0,
                                ..default()
                            }),
                            BackgroundColor(t::ACCENT),
                        ));
                        h.spawn((
                            caps(fonts, "", t::FONT_CAP, t::TEXT),
                            Node {
                                flex_grow: 1.0,
                                flex_shrink: 1.0,
                                min_width: px(0),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            SheetText::Section(si),
                            of,
                        ));
                        h.spawn((
                            super::widgets::icon(
                                fonts,
                                if folded {
                                    icon::CHEVRON_RIGHT
                                } else {
                                    icon::CHEVRON_DOWN
                                },
                                14.0,
                                t::TEXT_MUTED,
                            ),
                            ButtonFg {
                                owner: id,
                                base: t::TEXT_MUTED,
                            },
                        ));
                    },
                );
                c.commands().entity(id).insert(Name::new("Fold section"));
            } else if !sec.title.is_empty() {
                c.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    margin: UiRect::bottom(px(3)),
                    ..default()
                }))
                .with_children(|h| {
                    h.spawn((
                        layout(Node {
                            width: px(3),
                            height: px(11),
                            ..default()
                        }),
                        BackgroundColor(t::ACCENT),
                    ));
                    h.spawn((
                        caps(fonts, "", t::FONT_CAP, t::TEXT),
                        Node {
                            flex_shrink: 1.0,
                            min_width: px(0),
                            overflow: Overflow::clip(),
                            ..default()
                        },
                        SheetText::Section(si),
                        of,
                    ));
                });
            }
            let n = sec.items.len();
            for (ii, it) in sec.items.iter().enumerate() {
                if folded {
                    break;
                }
                let last = ii + 1 == n;
                let tags = (SheetText::Label(si, ii), SheetText::Value(si, ii), of);
                match it {
                    Item::Line(l) if l.label.is_empty() => {
                        c.spawn((
                            paragraph(fonts, "", t::FONT_BODY, t::W_MEDIUM, tone_color(l.tone)),
                            Node {
                                margin: UiRect::vertical(px(4)),
                                ..default()
                            },
                            tags.1,
                            tags.2,
                        ));
                    }
                    Item::Line(l) => row(c, fonts, tags, tone_color(l.tone), last, city),
                    Item::Link(l) => link_row(c, fonts, action_of(l.target), tags, last),
                    Item::Jump { .. } => link_row(c, fonts, UiAction::JumpToDestination, tags, last),
                    Item::Bar { share, .. } => {
                        row(c, fonts, tags, t::TEXT, true, city);
                        c.spawn((
                            layout(Node {
                                height: px(4),
                                margin: UiRect::bottom(px(5)),
                                ..default()
                            }),
                            BackgroundColor(t::RULE),
                        ))
                        .with_child((
                            layout(Node {
                                width: percent((share.clamp(0.0, 1.0) * 100.0).max(1.5)),
                                height: percent(100),
                                ..default()
                            }),
                            BackgroundColor(t::ACCENT),
                            SheetBar::Item(si, ii),
                            tags.2,
                        ));
                    }
                }
            }
        });
    }
}

fn row(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    tags: (SheetText, SheetText, Of),
    color: Color,
    last: bool,
    city: bool,
) {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        column_gap: px(10),
        min_height: px(if city { 21.0 } else { 29.0 }),
        border: UiRect::bottom(px(if last { 0.0 } else { 1.0 })),
        ..default()
    }))
    .insert(BorderColor::all(t::RULE))
    .with_children(|r| {
        r.spawn((
            caps(fonts, "", 10.5, t::TEXT_MUTED),
            tags.0,
            tags.2,
            Node {
                flex_shrink: 0.0,
                ..default()
            },
        ));
        r.spawn((
            label(
                fonts,
                "",
                if city { t::FONT_BODY } else { t::FONT_ROW },
                t::W_MEDIUM,
                color,
            ),
            Node {
                flex_shrink: 1.0,
                min_width: px(0),
                overflow: Overflow::clip(),
                ..default()
            },
            tags.1,
            tags.2,
        ));
    });
}

fn link_row(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    tags: (SheetText, SheetText, Of),
    last: bool,
) {
    let name = match action {
        UiAction::Person(_) => "Person link",
        UiAction::Place(_) => "Place link",
        UiAction::JumpToDestination => "Destination link",
        _ => "Vehicle link",
    };
    let glyph = if action == UiAction::JumpToDestination {
        icon::MAP_PIN
    } else {
        icon::CHEVRON_RIGHT
    };
    let id = spawn_button(
        p,
        UiButton::new(action, ButtonKind::Row),
        Node {
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            column_gap: px(10),
            min_height: px(29),
            padding: UiRect::horizontal(px(6)),
            margin: UiRect::horizontal(px(-6)),
            border: UiRect::bottom(px(if last { 0.0 } else { 1.0 })),
            ..default()
        },
        None,
        move |b, id| {
            b.spawn((
                caps(fonts, "", 10.5, t::ACCENT_TEXT),
                ButtonFg {
                    owner: id,
                    base: t::ACCENT_TEXT,
                },
                tags.0,
                tags.2,
                Node {
                    flex_shrink: 0.0,
                    ..default()
                },
            ));
            b.spawn(layout(Node {
                column_gap: px(5),
                align_items: AlignItems::Center,
                flex_shrink: 1.0,
                min_width: px(0),
                ..default()
            }))
            .with_children(|r| {
                r.spawn((
                    label(fonts, "", t::FONT_ROW, t::W_MEDIUM, t::TEXT),
                    Node {
                        flex_shrink: 1.0,
                        min_width: px(0),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    tags.1,
                    tags.2,
                ));
                r.spawn((
                    super::widgets::icon(fonts, glyph, 13.0, t::TEXT_MUTED),
                    ButtonFg {
                        owner: id,
                        base: t::TEXT_MUTED,
                    },
                ));
            });
        },
    );
    // Rows keep the rule between them (the button's border colour animates
    // only for slots; give it the rule colour).
    p.commands()
        .entity(id)
        .insert((Name::new(name), BorderColor::all(t::RULE)));
}

/// The city's people in the city window: the sections in columns that
/// wrap to the window's width (which share a column, by index: ages;
/// households and money; work; right now; trips and the unhappy;
/// happiness by part).
const CITY_COLUMNS: [&[usize]; 6] = [&[0], &[1, 6], &[2], &[3], &[4, 5], &[7]];

/// Width of a column of the city's people.
const CITY_COL: f32 = 236.0;

fn spawn_city_sheet(p: &mut ChildSpawnerCommands, fonts: &UiFonts, s: &Sheet, _compact: bool) {
    let of = Of(Root::City);
    let folds = Folds::default();
    for group in CITY_COLUMNS {
        p.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            width: px(CITY_COL),
            flex_shrink: 0.0,
            ..default()
        }))
        .with_children(|c| {
            for &si in group {
                if let Some(sec) = s.sections.get(si) {
                    section(c, fonts, si, sec, of, true, &folds);
                }
            }
        });
    }
}

fn slide_panel(
    time: Res<Time<Real>>,
    tool: Res<ActiveTool>,
    modes: Res<crate::shared::ToolModes>,
    ui: Res<UiState>,
    extent: Res<super::tray::TrayExtent>,
    mut q: Query<(&mut PanelRoot, &mut UiTransform, &mut Visibility, &mut Node)>,
) {
    let Ok((mut r, mut tf, mut vis, mut node)) = q.single_mut() else {
        return;
    };
    // Keep above the tray (as tall as its page) while one is up.
    let tray = super::tray::current_page(*tool, *modes, &ui).is_some();
    let bottom = px(PANEL_MARGIN_BOTTOM + if tray { extent.0.max(t::TRAY_H) + 8.0 } else { 0.0 });
    if node.bottom != bottom {
        node.bottom = bottom;
    }
    let target = if r.open { 1.0 } else { 0.0 };
    let tau = if r.open { 0.07 } else { 0.04 };
    r.t += (target - r.t) * approach(time.delta_secs(), tau);
    if (target - r.t).abs() < 0.003 {
        r.t = target;
    }
    let e = 1.0 - (1.0 - r.t).powi(2);
    let x = Val2::px((1.0 - e) * (t::PANEL_W + 24.0), 0.0);
    if tf.translation != x {
        tf.translation = x;
    }
    vis.set_if_neq(if r.t > 0.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    });
}

/// Mouse wheel over a scrolling body scrolls it.
fn scroll_bodies(scroll: Res<AccumulatedMouseScroll>, mut bodies: Query<(&Hovered, &mut ScrollPosition)>) {
    let dy = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y * 40.0,
        MouseScrollUnit::Pixel => scroll.delta.y,
    };
    if dy == 0.0 {
        return;
    }
    for (h, mut pos) in &mut bodies {
        if h.get() {
            pos.0.y = (pos.0.y - dy).max(0.0);
        }
    }
}

fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

/// A small face: shirt, head, hair (an illustration, so its shapes are
/// round; the frame around it is square).
fn spawn_portrait(p: &mut ChildSpawnerCommands, f: &Portrait) {
    let size = 56.0;
    let child = matches!(
        f.stage,
        citysim_sim::citizens::LifeStage::Child | citysim_sim::citizens::LifeStage::Teen
    );
    let head = if child { 20.0 } else { 23.0 };
    p.spawn((layout(Node {
        width: px(size),
        height: px(size),
        overflow: Overflow::clip(),
        flex_shrink: 0.0,
        ..default()
    }),))
        .with_children(|b| {
            b.spawn((
                layout(Node {
                    position_type: PositionType::Absolute,
                    left: px(size * 0.14),
                    right: px(size * 0.14),
                    bottom: px(-6),
                    height: px(size * 0.36),
                    border_radius: BorderRadius::top(px(14)),
                    ..default()
                }),
                BackgroundColor(srgb(f.shirt)),
            ));
            let top = size * 0.2;
            let left = (size - head) * 0.5;
            if f.hair_style == 1 {
                b.spawn((
                    layout(Node {
                        position_type: PositionType::Absolute,
                        left: px(left - 3.0),
                        top: px(top - 2.0),
                        width: px(head + 6.0),
                        height: px(head + 10.0),
                        border_radius: BorderRadius::all(px(10)),
                        ..default()
                    }),
                    BackgroundColor(srgb(f.hair)),
                ));
            }
            b.spawn((
                layout(Node {
                    position_type: PositionType::Absolute,
                    left: px(left),
                    top: px(top),
                    width: px(head),
                    height: px(head * 1.1),
                    border_radius: BorderRadius::all(px(head * 0.5)),
                    ..default()
                }),
                BackgroundColor(srgb(f.skin)),
            ));
            if f.hair_style != 3 || f.stage != citysim_sim::citizens::LifeStage::Senior {
                b.spawn((
                    layout(Node {
                        position_type: PositionType::Absolute,
                        left: px(left - 1.0),
                        top: px(top - 2.0),
                        width: px(head + 2.0),
                        height: px(head * 0.42),
                        border_radius: BorderRadius::top(px(head * 0.5)),
                        ..default()
                    }),
                    BackgroundColor(srgb(f.hair)),
                ));
            }
            if f.hair_style == 2 {
                b.spawn((
                    layout(Node {
                        position_type: PositionType::Absolute,
                        left: px(size * 0.5 - 5.0),
                        top: px(top - 8.0),
                        width: px(10),
                        height: px(9),
                        border_radius: BorderRadius::all(px(5)),
                        ..default()
                    }),
                    BackgroundColor(srgb(f.hair)),
                ));
            }
        });
}

fn spawn_leader(mut commands: Commands) {
    commands.spawn((
        Name::new("Leader line"),
        layout(Node {
            position_type: PositionType::Absolute,
            left: px(0),
            top: px(0),
            width: px(0),
            height: px(2),
            ..default()
        }),
        BackgroundColor(t::TEXT),
        Outline {
            width: px(1),
            offset: px(0),
            color: t::alpha(Color::BLACK, 0.35),
        },
        UiTransform::default(),
        Leader::Line,
        GlobalZIndex(21),
        Visibility::Hidden,
    ));
    for (which, size) in [(Leader::Dot, 7.0), (Leader::Tip, 10.0)] {
        commands.spawn((
            layout(Node {
                position_type: PositionType::Absolute,
                width: px(size),
                height: px(size),
                ..default()
            }),
            BackgroundColor(if which == Leader::Tip { t::ACCENT } else { t::TEXT }),
            UiTransform {
                rotation: Rot2::degrees(45.0),
                ..default()
            },
            which,
            GlobalZIndex(23),
            Visibility::Hidden,
        ));
    }
}

/// Record where the panel's plate is (the cursor readout keeps clear).
fn panel_area(
    roots: Query<(&PanelRoot, &InheritedVisibility)>,
    plate: Query<(&ComputedNode, &bevy::ui::UiGlobalTransform), With<PanelPlate>>,
    mut area: ResMut<PanelArea>,
) {
    let open = roots.iter().any(|(r, v)| v.get() && r.t > 0.5);
    let rect = plate.single().ok().filter(|_| open).map(|(n, tf)| {
        let inv = n.inverse_scale_factor;
        Rect::from_center_size(tf.translation * inv, n.size * inv)
    });
    if area.0 != rect {
        area.0 = rect;
    }
}

/// A line from the selected car, person, building or road to the panel,
/// with a dot at the thing and an accent diamond at the panel.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn leader(
    area: Res<PanelArea>,
    view: Res<PeopleView>,
    hl: Res<WorldHighlight>,
    sim: Res<SimWorld>,
    ui_scale: Res<UiScale>,
    cameras: Query<(&Camera, &GlobalTransform), With<crate::shared::WorldCamera>>,
    mut parts: Query<(&Leader, &mut Node, &mut UiTransform, &mut Visibility)>,
) {
    let w = &sim.world;
    let at = match view.current() {
        Some(Target::Person(c)) => people::person_position(w, c),
        Some(Target::Place(b)) => w.buildings.get(b).map(|b| b.center()),
        Some(Target::Vehicle(v)) => w.vehicles.get(v).map(|veh| citysim_sim::drive::vehicle_pose(w, veh).0),
        None => match &hl.selected {
            Some(WorldTarget::Junction { pos, .. }) => Some(*pos),
            Some(WorldTarget::Road(s)) => s.points.get(s.points.len() / 2).copied(),
            _ => None,
        },
    };
    let scale = ui_scale.0.max(0.01);
    let ends = (|| {
        let rect = area.0?;
        let (cam, tf) = cameras.single().ok()?;
        let p = cam.world_to_viewport(tf, crate::shared::sim_to_world(at?, 1.5)).ok()? / scale;
        if p.x > rect.min.x - 30.0 || p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let q = Vec2::new(rect.min.x - 2.0, p.y.clamp(rect.min.y + 40.0, rect.max.y - 40.0));
        Some((p, q))
    })();
    for (which, mut node, mut tf, mut vis) in &mut parts {
        let Some((p, q)) = ends else {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        };
        vis.set_if_neq(Visibility::Inherited);
        match which {
            Leader::Line => {
                let d = q - p;
                let len = d.length();
                let mid = (p + q) * 0.5;
                node.left = px(mid.x - len * 0.5);
                node.top = px(mid.y - 1.0);
                node.width = px(len);
                tf.rotation = Rot2::radians(d.y.atan2(d.x));
            }
            Leader::Dot => {
                node.left = px(p.x - 3.5);
                node.top = px(p.y - 3.5);
            }
            Leader::Tip => {
                node.left = px(q.x - 5.0);
                node.top = px(q.y - 5.0);
            }
        }
    }
}
