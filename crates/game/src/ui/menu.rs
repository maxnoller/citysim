//! Pause menu (Escape): resume, save, load, settings, keys, quit, with
//! arrow-key focus and Enter. Plus the first-run plate docked to the left
//! edge that ticks off the first three steps.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;
use citysim_sim::Command;

use super::UiState;
use super::format;
use super::plate::{PlateCache, PlateMaterial, PlateStyle};
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, Focused, Tooltip, UiAction, UiButton, approach, caps, label, layout, plate_node,
    plate_shadow, spawn_button, spawn_key, title_tick,
};
use crate::shared::{GameSet, LastCommand, SimClock, SimWorld};

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MenuFocus>()
            .add_systems(Startup, (spawn_menu, spawn_hint))
            .add_systems(PostStartup, start_fresh_game)
            .add_systems(Update, (update_menu, menu_keys, update_hint).in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct MenuRoot;

#[derive(Component)]
struct MenuCard;

#[derive(Component)]
struct MenuSubtitle;

/// The keyboard-focused row of the open menu (`MenuIndex`), if any.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq)]
pub struct MenuFocus(pub Option<usize>);

/// Position of a menu row for arrow-key focus.
#[derive(Component, Clone, Copy)]
pub struct MenuIndex(pub usize);

#[derive(Component)]
struct HintRoot;

#[derive(Component, Clone, Copy)]
struct HintStep(usize);

#[derive(Component, Clone, Copy)]
struct HintStepText(usize);

/// A full-width row of a menu: icon, caps label, key cap.
#[allow(clippy::too_many_arguments)]
pub fn menu_row(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    text: &str,
    key: Option<&str>,
    kind: ButtonKind,
    index: usize,
) {
    let id = spawn_button(
        p,
        UiButton::new(action, kind),
        Node {
            height: px(42),
            padding: UiRect::new(px(18), px(16), px(0), px(0)),
            align_items: AlignItems::Center,
            column_gap: px(14),
            border: UiRect::left(px(3)),
            ..default()
        },
        None,
        |b, id| {
            b.spawn((
                super::widgets::icon(fonts, glyph, t::ICON_MD, t::TEXT_MUTED),
                ButtonFg {
                    owner: id,
                    base: t::TEXT_MUTED,
                },
            ));
            b.spawn((
                caps(fonts, text, t::FONT_ROW, t::TEXT),
                ButtonFg {
                    owner: id,
                    base: t::TEXT,
                },
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
            ));
            if let Some(key) = key {
                spawn_key(b, fonts, key, t::TEXT_MUTED);
            }
        },
    );
    p.commands().entity(id).insert(MenuIndex(index));
}

/// The header strip of a modal plate: accent bar and caps title.
pub fn modal_header(p: &mut ChildSpawnerCommands, fonts: &UiFonts, title: &str) {
    p.spawn((
        layout(Node {
            height: px(48),
            padding: UiRect::new(px(26), px(14), px(0), px(0)),
            column_gap: px(10),
            align_items: AlignItems::Center,
            border: UiRect::bottom(px(t::LW)),
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
                ..default()
            }),
            BackgroundColor(t::ACCENT),
        ));
        h.spawn((
            Text::new(title.to_uppercase()),
            fonts.figures(21.0, t::W_BOLD),
            t::caps_spacing(21.0, 0.12),
            TextColor(t::TEXT),
            Pickable::IGNORE,
        ));
    });
}

fn spawn_menu(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    let mat = cache.get(&mut mats, PlateStyle::PANEL);
    commands
        .spawn((
            Name::new("Pause menu"),
            MenuRoot,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                top: px(0),
                bottom: px(0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::NONE),
            GlobalZIndex(200),
        ))
        .with_children(|root| {
            root.spawn((
                MenuCard,
                plate_node(
                    Node {
                        width: px(380),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::bottom(px(12)),
                        ..default()
                    },
                    UiRect::all(px(t::LW)),
                    [t::CUT, 0.0, t::CUT, 0.0],
                ),
                mat,
                plate_shadow(),
                UiTransform::default(),
            ))
            .with_children(|c| {
                modal_header(c, fonts, "Paused");
                c.spawn((
                    caps(fonts, "", t::FONT_CAP, t::TEXT_MUTED),
                    Node {
                        margin: UiRect::new(px(26), px(14), px(12), px(8)),
                        ..default()
                    },
                    MenuSubtitle,
                ));
                let rows: [(UiAction, char, &str, Option<&str>, ButtonKind); 7] = [
                    (UiAction::CloseMenu, icon::PLAY, "Resume", Some("Esc"), ButtonKind::Row),
                    (UiAction::Save, icon::SAVE, "Save game", Some("F5"), ButtonKind::Row),
                    (UiAction::Load, icon::LOAD, "Load game", Some("F9"), ButtonKind::Row),
                    (
                        UiAction::OpenSettings,
                        icon::SETTINGS,
                        "Settings",
                        None,
                        ButtonKind::Row,
                    ),
                    (UiAction::Keys, icon::KEYBOARD, "Keys", None, ButtonKind::Row),
                    (UiAction::Quit, icon::QUIT, "Quit to desktop", None, ButtonKind::Danger),
                    (UiAction::None, icon::QUIT, "", None, ButtonKind::Row),
                ];
                for (i, (action, glyph, text, key, kind)) in rows.into_iter().enumerate() {
                    if text.is_empty() {
                        continue;
                    }
                    if i == 3 || i == 5 {
                        c.spawn((
                            layout(Node {
                                height: px(1),
                                margin: UiRect::new(px(20), px(20), px(6), px(6)),
                                ..default()
                            }),
                            BackgroundColor(t::RULE),
                        ));
                    }
                    menu_row(c, fonts, action, glyph, text, key, kind, i);
                }
                c.spawn((
                    caps(fonts, "Arrow keys choose · Enter", 10.0, t::TEXT_FAINT),
                    Node {
                        margin: UiRect::new(px(26), px(14), px(10), px(0)),
                        ..default()
                    },
                ));
            });
        });
}

/// A new game starts paused, with the getting-started plate.
fn start_fresh_game(sim: Res<SimWorld>, mut clock: ResMut<SimClock>, mut state: ResMut<UiState>) {
    let w = &sim.world;
    if w.roads.segments.len() <= 1 && w.buildings.is_empty() {
        clock.paused = true;
        state.hint_visible = true;
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn update_menu(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    sim: Res<SimWorld>,
    mut root: Query<(&mut Node, &mut BackgroundColor), With<MenuRoot>>,
    mut card: Query<&mut UiTransform, With<MenuCard>>,
    mut subtitle: Query<&mut Text, With<MenuSubtitle>>,
    mut anim: Local<f32>,
    mut last: Local<Option<(u64, u32, i64, u32)>>,
) {
    let Ok((mut node, mut bg)) = root.single_mut() else {
        return;
    };
    let sub_open = state.settings_open || state.keys_open;
    let d = if state.menu_open && !sub_open {
        Display::Flex
    } else {
        Display::None
    };
    if node.display != d {
        node.display = d;
        *anim = 0.0;
        *last = None;
    }
    if !state.menu_open || sub_open {
        return;
    }
    if *anim < 1.0 {
        *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.07);
        if *anim > 0.995 {
            *anim = 1.0;
        }
        let a = *anim;
        bg.0 = t::alpha(t::BACKDROP, t::BACKDROP.alpha() * a);
        if let Ok(mut tf) = card.single_mut() {
            tf.translation = Val2::px(0.0, (1.0 - a) * 14.0);
        }
    }
    let w = &sim.world;
    let key = (sim.generation, w.time.day, w.economy.money, w.stats.population);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let s = format!(
        "{} · Day {} · {} · {} residents",
        super::top::city_name(w.seed),
        w.time.day,
        format::money(w.economy.money),
        format::group(w.stats.population as u64)
    )
    .to_uppercase();
    if let Ok(mut text) = subtitle.single_mut()
        && text.0 != s
    {
        text.0 = s;
    }
}

/// Arrow keys move the focus over the visible rows of the open menu
/// (pause menu or settings); Enter presses the focused one.
#[allow(clippy::type_complexity)]
fn menu_keys(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<UiState>,
    rows: Query<(Entity, &MenuIndex, &UiButton, &InheritedVisibility, Has<Focused>)>,
    mut focus: ResMut<MenuFocus>,
    mut out: MessageWriter<UiAction>,
) {
    let visible: Vec<(Entity, usize, UiAction, bool)> = {
        let mut v: Vec<_> = rows
            .iter()
            .filter(|r| r.3.get())
            .map(|(e, i, b, _, f)| (e, i.0, b.action, f))
            .collect();
        v.sort_by_key(|r| r.1);
        v
    };
    if !state.menu_open || visible.is_empty() {
        if state.is_changed() {
            focus.0 = None;
        }
        for (e, .., f) in &visible {
            if *f {
                commands.entity(*e).remove::<Focused>();
            }
        }
        return;
    }
    let n = visible.len();
    let mut cur = focus.0.and_then(|i| visible.iter().position(|r| r.1 == i));
    if keys.just_pressed(KeyCode::ArrowDown) {
        cur = Some(cur.map_or(0, |c| (c + 1) % n));
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        cur = Some(cur.map_or(n - 1, |c| (c + n - 1) % n));
    }
    let f = cur.map(|c| visible[c].1);
    if focus.0 != f {
        focus.0 = f;
    }
    if (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
        && let Some(c) = cur
    {
        out.write(visible[c].2);
    }
    for (i, (e, .., f)) in visible.iter().enumerate() {
        let want = cur == Some(i);
        if want && !f {
            commands.entity(*e).insert(Focused);
        } else if !want && *f {
            commands.entity(*e).remove::<Focused>();
        }
    }
}

// ---------------------------------------------------------------- first-run plate

const STEPS: [(&str, &str); 3] = [
    ("Build a road from the highway", "R"),
    ("Zone land beside the road", "Z"),
    ("Unpause to let the city grow", "Space"),
];

fn spawn_hint(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    mut cache: ResMut<PlateCache>,
    mut mats: ResMut<Assets<PlateMaterial>>,
) {
    let fonts = fonts.into_inner();
    let mat = cache.get(&mut mats, PlateStyle::PANEL.slanted([0.0, 0.0, 0.0, 0.12]));
    commands
        .spawn((
            Name::new("Getting started"),
            HintRoot,
            plate_node(
                Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    bottom: px(t::BAR_H + 150.0),
                    width: px(360),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(px(26), px(40), px(16), px(16)),
                    row_gap: px(4),
                    display: Display::None,
                    ..default()
                },
                UiRect::new(px(0), px(t::LW), px(t::LW), px(t::LW)),
                [0.0; 4],
            ),
            mat,
            plate_shadow(),
            UiTransform::default(),
            GlobalZIndex(21),
        ))
        .with_children(|p| {
            p.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Start,
                margin: UiRect::bottom(px(6)),
                ..default()
            }))
            .with_children(|h| {
                h.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    ..default()
                }))
                .with_children(|l| {
                    l.spawn(title_tick());
                    l.spawn((
                        Text::new("GETTING STARTED"),
                        fonts.figures(t::FONT_HEAD, t::W_BOLD),
                        t::caps_spacing(t::FONT_HEAD, 0.1),
                        TextColor(t::TEXT),
                        Pickable::IGNORE,
                    ));
                });
                spawn_button(
                    h,
                    UiButton::new(UiAction::DismissHint, ButtonKind::Ghost),
                    Node {
                        width: px(28),
                        height: px(28),
                        margin: UiRect::right(px(-24)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    Some(Tooltip::new("Hide", "Hide these steps.")),
                    |b, id| {
                        b.spawn((
                            super::widgets::icon(fonts, icon::CLOSE, 16.0, t::TEXT_MUTED),
                            ButtonFg {
                                owner: id,
                                base: t::TEXT_MUTED,
                            },
                        ));
                    },
                );
            });
            for (i, (text, key)) in STEPS.iter().enumerate() {
                p.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(12),
                    min_height: px(30),
                    border: UiRect::bottom(px(if i < 2 { 1.0 } else { 0.0 })),
                    ..default()
                }))
                .insert(BorderColor::all(t::RULE))
                .with_children(|r| {
                    r.spawn((
                        layout(Node {
                            width: px(16),
                            height: px(16),
                            border: UiRect::all(px(2)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        }),
                        BorderColor::all(t::LINE),
                        BackgroundColor(Color::NONE),
                        HintStep(i),
                    ))
                    .with_child((super::widgets::icon(fonts, icon::CHECK, 11.0, Color::NONE), HintStep(i)));
                    r.spawn((
                        label(fonts, *text, t::FONT_ROW, t::W_MEDIUM, t::TEXT),
                        HintStepText(i),
                        Node {
                            flex_grow: 1.0,
                            ..default()
                        },
                    ));
                    spawn_key(r, fonts, key, t::TEXT_MUTED);
                });
            }
        });
}

#[derive(Default)]
struct HintProgress {
    generation: Option<u64>,
    done: [bool; 3],
    grid_version: Option<(u64, u64)>,
    all_done_for: f32,
    anim: f32,
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_hint(
    time: Res<Time<Real>>,
    sim: Res<SimWorld>,
    clock: Res<SimClock>,
    last: Res<LastCommand>,
    mut state: ResMut<UiState>,
    mut root: Query<(&mut Node, &mut UiTransform), With<HintRoot>>,
    mut boxes: Query<(
        &HintStep,
        Option<&mut BorderColor>,
        Option<&mut BackgroundColor>,
        Option<&mut TextColor>,
    )>,
    mut texts: Query<(&HintStepText, &mut TextColor), Without<HintStep>>,
    mut progress: Local<HintProgress>,
) {
    let Ok((mut node, mut tf)) = root.single_mut() else {
        return;
    };
    let w = &sim.world;
    if progress.generation != Some(sim.generation) {
        progress.generation = Some(sim.generation);
        progress.done = [false; 3];
        progress.all_done_for = 0.0;
    }
    if w.roads.segments.len() > 1 {
        progress.done[0] = true;
    }
    let key = sim.key(w.versions.grid);
    if progress.grid_version != Some(key) || last.is_changed() {
        progress.grid_version = Some(key);
        if !progress.done[1] {
            progress.done[1] = w.zoning.cells().any(|(_, c)| c.zone.is_some())
                || matches!(&last.result, Some((Command::SetZone { zone: Some(_), .. }, Ok(o))) if !o.cells.is_empty());
        }
    }
    if !clock.paused && progress.done[0] {
        progress.done[2] = true;
    }
    if progress.done.iter().all(|&d| d) {
        progress.all_done_for += time.delta_secs();
        if progress.all_done_for > 3.0 {
            state.hint_visible = false;
        }
    }
    let show = state.hint_visible && state.tray.is_none() && !state.menu_open;
    let d = if show { Display::Flex } else { Display::None };
    if node.display != d {
        node.display = d;
        progress.anim = 0.0;
    }
    if !show {
        return;
    }
    if progress.anim < 1.0 {
        progress.anim += (1.0 - progress.anim) * approach(time.delta_secs(), 0.08);
        if progress.anim > 0.995 {
            progress.anim = 1.0;
        }
        tf.translation = Val2::px((1.0 - progress.anim) * -40.0, 0.0);
    }
    for (HintStep(i), border, bg, color) in &mut boxes {
        let done = progress.done[*i];
        if let Some(mut b) = border {
            b.set_if_neq(BorderColor::all(if done { t::POSITIVE } else { t::LINE }));
        }
        if let Some(mut b) = bg {
            b.set_if_neq(BackgroundColor(if done { t::POSITIVE } else { Color::NONE }));
        }
        if let Some(mut c) = color {
            c.set_if_neq(TextColor(if done { t::WARNING_INK } else { Color::NONE }));
        }
    }
    for (HintStepText(i), mut color) in &mut texts {
        color.set_if_neq(TextColor(if progress.done[*i] { t::TEXT_MUTED } else { t::TEXT }));
    }
}
