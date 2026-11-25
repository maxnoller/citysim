//! The people panels: a person, a building's people or a car (right side,
//! with a history to go back and a follow button), and the city-wide
//! people view (left side). Thin widgets: everything they show comes from
//! `people.rs` sheets; this file only lays them out, turns links into
//! buttons, and moves the camera after whoever is followed
//! (`CameraFollow`).
//!
//! Opening: a click with Inspect on a building or car, a link in a panel,
//! `UiAction::{Person, Place, Vehicle}`; P or the toolbar button for the
//! city view. Closing: the close button, Esc, a click on empty ground.
//! Following stops with the button, Esc, or a camera pan key.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;

use super::UiState;
use super::people::{self, Item, Portrait, Sheet, Target};
use super::readout::Tone;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{ButtonFg, ButtonKind, UiAction, UiButton, label, layout, panel, spawn_button};
use crate::shared::{ActiveTool, CameraFollow, GameSet, SimWorld, WorldHighlight, WorldTarget};

/// What the right-hand people panel shows: the last of `stack` (earlier
/// entries are "back").
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

pub struct PeoplePanelPlugin;

impl Plugin for PeoplePanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PeopleView>()
            .add_systems(Startup, spawn_panels)
            .add_systems(
                Update,
                (people_input, follow, draw_panels, scroll_panels)
                    .chain()
                    .in_set(GameSet::Render),
            );
    }
}

/// Which panel a root is.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum PanelRoot {
    Subject,
    City,
}

/// The scrolling body of a panel (rebuilt when its sheet's structure
/// changes; texts and bars are updated in place otherwise).
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct PanelBody(PanelRoot);

/// Which text of a sheet an entity shows.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum SheetText {
    Title,
    Subtitle,
    Section(usize),
    Label(usize, usize),
    Value(usize, usize),
}

/// Which bar of a sheet an entity fills.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum SheetBar {
    Mood,
    Item(usize, usize),
}

/// The part of a panel a text or bar belongs to.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
struct Of(PanelRoot);

/// The kind of an item, for deciding whether to rebuild.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ItemShape {
    Line,
    Link(Target),
    Bar,
}

/// What a sheet's widgets look like, without the texts.
#[derive(Debug, Clone, PartialEq)]
struct SheetShape {
    portrait: Option<Portrait>,
    mood: bool,
    sections: Vec<(bool, Vec<ItemShape>)>,
    /// Back, following, can follow (subject panel controls).
    controls: (bool, bool, bool),
}

fn shape_of(s: &Sheet, controls: (bool, bool, bool)) -> SheetShape {
    SheetShape {
        portrait: s.portrait,
        mood: s.mood.is_some(),
        sections: s
            .sections
            .iter()
            .map(|sec| {
                (
                    !sec.title.is_empty(),
                    sec.items
                        .iter()
                        .map(|it| match it {
                            Item::Line(_) => ItemShape::Line,
                            Item::Link(l) => ItemShape::Link(l.target),
                            Item::Bar { .. } => ItemShape::Bar,
                        })
                        .collect(),
                )
            })
            .collect(),
        controls,
    }
}

/// The text and colour a tagged entity shows in a sheet.
fn text_of(s: &Sheet, t: SheetText) -> Option<(String, Option<Color>)> {
    Some(match t {
        SheetText::Title => (s.title.clone(), None),
        SheetText::Subtitle => (s.subtitle.clone(), None),
        SheetText::Section(i) => (s.sections.get(i)?.title.to_uppercase(), None),
        SheetText::Label(i, j) => match s.sections.get(i)?.items.get(j)? {
            Item::Line(l) => (l.label.clone(), None),
            Item::Link(l) => (l.label.clone(), None),
            Item::Bar { label, .. } => (label.clone(), None),
        },
        SheetText::Value(i, j) => match s.sections.get(i)?.items.get(j)? {
            Item::Line(l) => (l.value.clone(), Some(tone_color(l.tone))),
            Item::Link(l) => (l.value.clone(), None),
            Item::Bar { value, .. } => (value.clone(), None),
        },
    })
}

fn bar_of(s: &Sheet, b: SheetBar) -> Option<(f32, Option<Color>)> {
    match b {
        SheetBar::Mood => s.mood.map(|m| (m, Some(mood_color(m)))),
        SheetBar::Item(i, j) => match s.sections.get(i)?.items.get(j)? {
            Item::Bar { share, .. } => Some((*share, None)),
            _ => None,
        },
    }
}

const PANEL_W: f32 = 340.0;
/// The city view: two columns of sections.
const CITY_W: f32 = 600.0;

fn spawn_panels(mut commands: Commands) {
    for (root, name) in [
        (PanelRoot::Subject, "People panel"),
        (PanelRoot::City, "City people panel"),
    ] {
        let mut node = Node {
            position_type: PositionType::Absolute,
            top: px(t::BELOW_TOP_BAR),
            width: px(if root == PanelRoot::City { CITY_W } else { PANEL_W }),
            max_height: Val::Vh(68.0),
            flex_direction: FlexDirection::Column,
            padding: UiRect::new(px(t::S4), px(t::S4), px(t::S4), px(t::S4)),
            display: Display::None,
            ..default()
        };
        match root {
            PanelRoot::Subject => node.right = px(t::EDGE),
            PanelRoot::City => node.left = px(t::EDGE),
        }
        commands
            .spawn((Name::new(name), root, panel(node), GlobalZIndex(15)))
            .with_children(|p| {
                let city = root == PanelRoot::City;
                p.spawn((
                    PanelBody(root),
                    Node {
                        flex_direction: if city {
                            FlexDirection::Row
                        } else {
                            FlexDirection::Column
                        },
                        flex_wrap: if city { FlexWrap::Wrap } else { FlexWrap::NoWrap },
                        align_items: if city {
                            AlignItems::FlexStart
                        } else {
                            AlignItems::Stretch
                        },
                        align_content: AlignContent::FlexStart,
                        overflow: Overflow::scroll_y(),
                        row_gap: px(t::S2),
                        column_gap: px(t::S5),
                        ..default()
                    },
                    ScrollPosition::default(),
                    Hovered::default(),
                ));
            });
    }
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
            UiAction::CityPeople => {
                ui.city_people_open = !ui.city_people_open;
                if ui.city_people_open {
                    ui.budget_open = false;
                }
            }
            UiAction::ToggleBudget if ui.budget_open => ui.city_people_open = false,
            _ => {}
        }
    }
    if keys.just_pressed(KeyCode::KeyP) && !ui.menu_open {
        ui.city_people_open = !ui.city_people_open;
        if ui.city_people_open {
            ui.budget_open = false;
        }
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
    // building or car opens its people, empty ground closes the panel
    // (unless following).
    if mouse.just_pressed(MouseButton::Left) && !over_ui.0 && *tool == ActiveTool::Inspect {
        match highlight.selected.as_ref().and_then(target_of) {
            Some(t) => view.open(t),
            None if highlight.selected.is_none() && !ui.following => view.stack.clear(),
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
    // Where a followed person is shows in the world.
    if let Some(Target::Person(c)) = view.current()
        && let Some(v) = citysim_sim::inspect::citizen(w, c)
    {
        let sel = match (v.trip.and_then(|t| t.vehicle), v.inside) {
            (Some(car), _) => Some(WorldTarget::Vehicle(car)),
            (None, Some(b)) => Some(WorldTarget::Building(b)),
            _ => None,
        };
        if highlight.selected != sel {
            highlight.selected = sel;
        }
    }
}

/// The sheet a panel shows now.
fn sheet_for(world: &citysim_sim::World, t: Target) -> Option<Sheet> {
    match t {
        Target::Person(c) => people::person_sheet(world, c),
        Target::Place(b) => people::place_sheet(world, b),
        Target::Vehicle(v) => people::vehicle_sheet(world, v),
    }
}

#[derive(Default)]
struct Shown {
    subject: Option<SheetShape>,
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
    ),
>;
type BarQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static SheetBar,
        &'static Of,
        &'static mut Node,
        &'static mut BackgroundColor,
    ),
>;

/// Rebuild a panel's body when the shape of what it shows changed;
/// otherwise update its texts and bars in place (a few times a second).
#[allow(clippy::too_many_arguments)]
fn draw_panels(
    mut commands: Commands,
    time: Res<Time<Real>>,
    fonts: Res<UiFonts>,
    sim: Res<SimWorld>,
    ui: Res<UiState>,
    view: Res<PeopleView>,
    mut roots: Query<(&PanelRoot, &mut Node), Without<SheetBar>>,
    bodies: Query<(Entity, &PanelBody)>,
    mut texts: TextQuery,
    mut bars: BarQuery,
    mut shown: Local<Shown>,
) {
    shown.age += time.delta_secs();
    if shown.age < 0.3 && !view.is_changed() && !ui.is_changed() {
        return;
    }
    shown.age = 0.0;
    let w = &sim.world;
    let subject = view.current().filter(|_| !ui.menu_open).and_then(|t| sheet_for(w, t));
    let city = (ui.city_people_open && !ui.menu_open).then(|| people::city_sheet(w));
    for (root, mut node) in &mut roots {
        let show = match root {
            PanelRoot::Subject => subject.is_some(),
            PanelRoot::City => city.is_some(),
        };
        let d = if show { Display::Flex } else { Display::None };
        if node.display != d {
            node.display = d;
        }
    }
    let fonts = fonts.into_inner();
    let can_follow = matches!(view.current(), Some(Target::Person(_) | Target::Vehicle(_)));
    let controls = (view.stack.len() > 1, ui.following, can_follow);
    for (root, sheet) in [(PanelRoot::Subject, &subject), (PanelRoot::City, &city)] {
        let shape = sheet.as_ref().map(|s| shape_of(s, controls));
        let old = match root {
            PanelRoot::Subject => &mut shown.subject,
            PanelRoot::City => &mut shown.city,
        };
        if shape != *old {
            if let Some((body, _)) = bodies.iter().find(|b| b.1.0 == root) {
                commands.entity(body).despawn_children();
                if let Some(s) = sheet {
                    commands.entity(body).with_children(|p| {
                        match root {
                            PanelRoot::Subject => subject_controls(p, fonts, controls.0, controls.1, controls.2),
                            PanelRoot::City => {
                                p.spawn(layout(Node {
                                    justify_content: JustifyContent::End,
                                    width: percent(100),
                                    ..default()
                                }))
                                .with_children(|r| {
                                    small_button(r, fonts, UiAction::CityPeople, icon::CLOSE, "", "Close")
                                });
                            }
                        }
                        spawn_sheet(p, fonts, s, root);
                    });
                }
            }
            *old = shape;
            continue;
        }
        let Some(s) = sheet else { continue };
        for (tag, of, mut text, mut color) in &mut texts {
            if of.0 != root {
                continue;
            }
            if let Some((v, c)) = text_of(s, *tag) {
                if text.0 != v {
                    text.0 = v;
                }
                if let Some(c) = c {
                    color.set_if_neq(TextColor(c));
                }
            }
        }
        for (tag, of, mut node, mut bg) in &mut bars {
            if of.0 != root {
                continue;
            }
            if let Some((share, c)) = bar_of(s, *tag) {
                let wv = percent((share.clamp(0.0, 1.0) * 100.0).max(1.5));
                if node.width != wv {
                    node.width = wv;
                }
                if let Some(c) = c {
                    bg.set_if_neq(BackgroundColor(c));
                }
            }
        }
    }
}

/// Mouse wheel over a panel scrolls it.
fn scroll_panels(scroll: Res<AccumulatedMouseScroll>, mut bodies: Query<(&Hovered, &mut ScrollPosition)>) {
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

/// Back · Follow · Close.
fn subject_controls(p: &mut ChildSpawnerCommands, fonts: &UiFonts, back: bool, following: bool, can_follow: bool) {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        ..default()
    }))
    .with_children(|r| {
        r.spawn(layout(Node {
            column_gap: px(t::S2),
            ..default()
        }))
        .with_children(|l| {
            if back {
                small_button(l, fonts, UiAction::PeopleBack, icon::CHEVRON_LEFT, "Back", "Back");
            }
            if can_follow {
                if following {
                    small_button(
                        l,
                        fonts,
                        UiAction::StopFollow,
                        icon::FOLLOW,
                        "Stop following",
                        "Stop following",
                    );
                } else {
                    small_button(
                        l,
                        fonts,
                        UiAction::Follow,
                        icon::FOLLOW,
                        "Follow",
                        "Follow with the camera",
                    );
                }
            }
        });
        small_button(r, fonts, UiAction::ClosePeople, icon::CLOSE, "", "Close");
    });
}

fn small_button(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    text: &'static str,
    tip: &'static str,
) {
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Ghost,
            accent: t::ACCENT,
        },
        Node {
            height: px(28),
            padding: UiRect::horizontal(px(t::S2 + 1.0)),
            column_gap: px(t::S2),
            align_items: AlignItems::Center,
            ..default()
        },
        Some(super::widgets::Tooltip {
            title: tip,
            desc: "",
            key: None,
            price: None,
        }),
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, t::ICON_SM, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
            if !text.is_empty() {
                b.spawn((
                    label(fonts, text, t::FONT_SMALL, t::W_MEDIUM, t::TEXT),
                    ButtonFg {
                        owner: id,
                        base: t::TEXT,
                    },
                ));
            }
        },
    );
}

fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Normal => t::TEXT,
        Tone::Muted => t::TEXT_MUTED,
        Tone::Accent => t::ACCENT,
        Tone::Good => t::POSITIVE,
        Tone::Warn => t::WARNING,
        Tone::Bad => t::NEGATIVE,
    }
}

fn srgb(c: [f32; 3]) -> Color {
    Color::srgb(c[0], c[1], c[2])
}

/// A small face: shirt, head, hair, drawn with rounded boxes.
fn spawn_portrait(p: &mut ChildSpawnerCommands, f: &Portrait) {
    let size = 52.0;
    let child = matches!(
        f.stage,
        citysim_sim::citizens::LifeStage::Child | citysim_sim::citizens::LifeStage::Teen
    );
    let head = if child { 20.0 } else { 23.0 };
    p.spawn((
        layout(Node {
            width: px(size),
            height: px(size),
            border_radius: BorderRadius::all(px(size * 0.5)),
            overflow: Overflow::clip(),
            flex_shrink: 0.0,
            ..default()
        }),
        BackgroundColor(t::alpha(srgb(f.shirt), 0.25)),
    ))
    .with_children(|b| {
        // Shoulders.
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
        // Long hair behind the head.
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
        // Head.
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
        // Hair on top.
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

fn action_of(t: Target) -> UiAction {
    match t {
        Target::Person(c) => UiAction::Person(c),
        Target::Place(b) => UiAction::Place(b),
        Target::Vehicle(v) => UiAction::Vehicle(v),
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

/// A sheet: heading (portrait, title, subtitle, mood) and its sections,
/// every text and bar tagged for updates in place.
fn spawn_sheet(p: &mut ChildSpawnerCommands, fonts: &UiFonts, s: &Sheet, root: PanelRoot) {
    let of = Of(root);
    let city = root == PanelRoot::City;
    p.spawn(layout(Node {
        column_gap: px(t::S3),
        align_items: AlignItems::Center,
        margin: UiRect::bottom(px(t::S1)),
        width: percent(100),
        ..default()
    }))
    .with_children(|h| {
        if let Some(f) = &s.portrait {
            spawn_portrait(h, f);
        }
        h.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            flex_grow: 1.0,
            ..default()
        }))
        .with_children(|c| {
            c.spawn((
                label(fonts, s.title.clone(), t::FONT_TITLE, t::W_SEMIBOLD, t::TEXT),
                SheetText::Title,
                of,
            ));
            c.spawn((
                label(fonts, s.subtitle.clone(), t::FONT_SMALL, t::W_MEDIUM, t::TEXT_MUTED),
                SheetText::Subtitle,
                of,
            ));
            if let Some(m) = s.mood {
                bar(c, m, mood_color(m), 4.0, (SheetBar::Mood, of));
            }
        });
    });
    for (si, sec) in s.sections.iter().enumerate() {
        p.spawn(layout(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(3),
            width: if city {
                px((CITY_W - 2.0 * t::S4 - t::S5) * 0.5 - 1.0)
            } else {
                percent(100)
            },
            ..default()
        }))
        .with_children(|c| {
            if !sec.title.is_empty() {
                c.spawn((
                    label(
                        fonts,
                        sec.title.to_uppercase(),
                        t::FONT_CAPTION,
                        t::W_SEMIBOLD,
                        t::TEXT_FAINT,
                    ),
                    Node {
                        margin: UiRect::top(px(t::S2)),
                        ..default()
                    },
                    SheetText::Section(si),
                    of,
                ));
            }
            for (ii, it) in sec.items.iter().enumerate() {
                let tags = (SheetText::Label(si, ii), SheetText::Value(si, ii), of);
                match it {
                    Item::Line(l) => row(c, fonts, &l.label, &l.value, tone_color(l.tone), tags),
                    Item::Link(l) => link_row(c, fonts, &l.label, &l.value, action_of(l.target), tags),
                    Item::Bar {
                        label: lb,
                        value,
                        share,
                    } => {
                        row(c, fonts, lb, value, t::TEXT, tags);
                        bar(c, *share, t::ACCENT, 3.0, (SheetBar::Item(si, ii), of));
                    }
                }
            }
        });
    }
}

fn row(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    l: &str,
    v: &str,
    color: Color,
    tags: (SheetText, SheetText, Of),
) {
    p.spawn(layout(Node {
        justify_content: JustifyContent::SpaceBetween,
        column_gap: px(t::S3),
        ..default()
    }))
    .with_children(|r| {
        if !l.is_empty() {
            r.spawn((
                label(fonts, l.to_string(), t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED),
                tags.0,
                tags.2,
            ));
        }
        if l.is_empty() {
            // A sentence: wraps.
            r.spawn((
                super::widgets::paragraph(fonts, v.to_string(), t::FONT_SMALL, t::W_MEDIUM, color),
                tags.1,
                tags.2,
            ));
        } else {
            r.spawn((
                label(fonts, v.to_string(), t::FONT_SMALL, t::W_MEDIUM, color),
                tags.1,
                tags.2,
            ));
        }
    });
}

fn link_row(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    l: &str,
    v: &str,
    action: UiAction,
    tags: (SheetText, SheetText, Of),
) {
    let (l, v) = (l.to_string(), v.to_string());
    spawn_button(
        p,
        UiButton {
            action,
            kind: ButtonKind::Ghost,
            accent: t::ACCENT,
        },
        Node {
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            column_gap: px(t::S3),
            padding: UiRect::axes(px(t::S1 + 1.0), px(2)),
            margin: UiRect::horizontal(px(-(t::S1 + 1.0))),
            ..default()
        },
        None,
        move |b, id| {
            b.spawn((
                label(fonts, l, t::FONT_SMALL, t::W_MEDIUM, t::ACCENT),
                ButtonFg {
                    owner: id,
                    base: t::ACCENT,
                },
                tags.0,
                tags.2,
            ));
            b.spawn(layout(Node {
                column_gap: px(t::S1 + 1.0),
                align_items: AlignItems::Center,
                ..default()
            }))
            .with_children(|r| {
                r.spawn((label(fonts, v, t::FONT_SMALL, t::W_REGULAR, t::TEXT), tags.1, tags.2));
                r.spawn(super::widgets::icon(
                    fonts,
                    icon::ARROW_RIGHT,
                    t::ICON_SM - 2.0,
                    t::TEXT_FAINT,
                ));
            });
        },
    );
}

fn bar(p: &mut ChildSpawnerCommands, share: f32, color: Color, h: f32, tag: (SheetBar, Of)) {
    p.spawn((
        layout(Node {
            height: px(h),
            border_radius: BorderRadius::all(px(h * 0.5)),
            overflow: Overflow::clip(),
            ..default()
        }),
        BackgroundColor(t::alpha(t::TEXT, 0.08)),
    ))
    .with_children(|b| {
        b.spawn((
            layout(Node {
                width: percent((share.clamp(0.0, 1.0) * 100.0).max(1.5)),
                height: percent(100),
                border_radius: BorderRadius::all(px(h * 0.5)),
                ..default()
            }),
            BackgroundColor(color),
            tag.0,
            tag.1,
        ));
    });
}
