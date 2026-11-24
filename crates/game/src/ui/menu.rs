//! Pause menu (Escape): resume, save, load, graphics quality, quit. Plus
//! the first-run hint card that ticks off the first three steps.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;
use citysim_sim::Command;

use super::UiState;
use super::budget::close_button;
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, UiAction, UiButton, approach, hdivider, label, layout, panel, spawn_button, spawn_key_badge,
};
use crate::shared::{GameSet, GraphicsQuality, LastCommand, SimClock, SimWorld};

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (spawn_menu, spawn_hint.after(super::demand::spawn_meter)))
            .add_systems(PostStartup, start_fresh_game)
            .add_systems(Update, (update_menu, update_hint).in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct MenuRoot;

#[derive(Component)]
struct MenuCard;

#[derive(Component)]
struct MenuSubtitle;

#[derive(Component)]
struct HintRoot;

#[derive(Component, Clone, Copy)]
struct HintStep(usize);

#[derive(Component, Clone, Copy)]
struct HintStepText(usize);

fn menu_row(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    action: UiAction,
    glyph: char,
    text: &str,
    key: Option<&str>,
    kind: ButtonKind,
) {
    spawn_button(
        p,
        UiButton {
            action,
            kind,
            accent: t::ACCENT,
        },
        Node {
            height: px(40),
            padding: UiRect::axes(px(t::S4), px(0)),
            align_items: AlignItems::Center,
            column_gap: px(t::S4),
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
                label(fonts, text, t::FONT_LABEL, t::W_MEDIUM, t::TEXT),
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
                spawn_key_badge(b, fonts, key);
            }
        },
    );
}

fn spawn_menu(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
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
                panel(Node {
                    width: px(340),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(px(t::S4)),
                    row_gap: px(t::S1),
                    ..default()
                }),
                UiTransform::default(),
            ))
            .insert(BackgroundColor(t::PANEL_RAISED))
            .with_children(|c| {
                c.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(px(t::S4), px(t::S4), px(t::S3), px(t::S4)),
                    row_gap: px(t::S2),
                    ..default()
                }))
                .with_children(|h| {
                    h.spawn(label(fonts, "Paused", 22.0, t::W_BOLD, t::TEXT));
                    h.spawn((
                        label(fonts, "", t::FONT_BODY, t::W_REGULAR, t::TEXT_MUTED),
                        MenuSubtitle,
                    ));
                });
                menu_row(
                    c,
                    fonts,
                    UiAction::CloseMenu,
                    icon::PLAY,
                    "Resume",
                    Some("Esc"),
                    ButtonKind::Menu,
                );
                menu_row(
                    c,
                    fonts,
                    UiAction::Save,
                    icon::SAVE,
                    "Save game",
                    Some("F5"),
                    ButtonKind::Menu,
                );
                menu_row(
                    c,
                    fonts,
                    UiAction::Load,
                    icon::LOAD,
                    "Load game",
                    Some("F9"),
                    ButtonKind::Menu,
                );
                c.spawn(hdivider()).insert(Node {
                    height: px(1),
                    margin: UiRect::axes(px(t::S4), px(t::S3)),
                    ..default()
                });
                // Graphics quality.
                c.spawn(layout(Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::axes(px(t::S4), px(t::S2)),
                    row_gap: px(t::S3),
                    ..default()
                }))
                .with_children(|g| {
                    g.spawn(layout(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(t::S4),
                        ..default()
                    }))
                    .with_children(|r| {
                        r.spawn(super::widgets::icon(fonts, icon::MONITOR, t::ICON_MD, t::TEXT_MUTED));
                        r.spawn((
                            label(fonts, "Graphics quality", t::FONT_LABEL, t::W_MEDIUM, t::TEXT),
                            Node {
                                flex_grow: 1.0,
                                ..default()
                            },
                        ));
                        spawn_key_badge(r, fonts, "G");
                    });
                    g.spawn(layout(Node {
                        padding: UiRect::all(px(3)),
                        column_gap: px(3),
                        border_radius: BorderRadius::all(px(t::RADIUS_SM + 1.0)),
                        ..default()
                    }))
                    .insert(BackgroundColor(t::WELL))
                    .with_children(|seg| {
                        for (q, name) in [
                            (GraphicsQuality::Low, "Low"),
                            (GraphicsQuality::Medium, "Medium"),
                            (GraphicsQuality::High, "High"),
                        ] {
                            spawn_button(
                                seg,
                                UiButton {
                                    action: UiAction::Quality(q),
                                    kind: ButtonKind::Segment,
                                    accent: t::ACCENT,
                                },
                                Node {
                                    flex_grow: 1.0,
                                    flex_basis: px(0),
                                    height: px(30),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                                None,
                                |b, id| {
                                    b.spawn((
                                        label(fonts, name, t::FONT_BODY, t::W_MEDIUM, t::TEXT_MUTED),
                                        ButtonFg {
                                            owner: id,
                                            base: t::TEXT_MUTED,
                                        },
                                    ));
                                },
                            );
                        }
                    });
                });
                c.spawn(hdivider()).insert(Node {
                    height: px(1),
                    margin: UiRect::axes(px(t::S4), px(t::S3)),
                    ..default()
                });
                menu_row(
                    c,
                    fonts,
                    UiAction::Quit,
                    icon::QUIT,
                    "Quit to desktop",
                    None,
                    ButtonKind::Danger,
                );
            });
        });
}

/// A new game starts paused, with the getting-started card.
fn start_fresh_game(sim: Res<SimWorld>, mut clock: ResMut<SimClock>, mut state: ResMut<UiState>) {
    let w = &sim.world;
    if w.roads.segments.len() <= 1 && w.buildings.is_empty() {
        clock.paused = true;
        state.hint_visible = true;
    }
}

#[allow(clippy::type_complexity)]
fn update_menu(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    sim: Res<SimWorld>,
    mut root: Query<(&mut Node, &mut BackgroundColor), With<MenuRoot>>,
    mut card: Query<&mut UiTransform, With<MenuCard>>,
    mut subtitle: Query<&mut Text, With<MenuSubtitle>>,
    mut anim: Local<f32>,
) {
    let Ok((mut node, mut bg)) = root.single_mut() else {
        return;
    };
    let d = if state.menu_open { Display::Flex } else { Display::None };
    if node.display != d {
        node.display = d;
        *anim = 0.0;
    }
    if !state.menu_open {
        return;
    }
    *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.07);
    let a = *anim;
    bg.0 = t::alpha(t::BACKDROP, t::BACKDROP.alpha() * a);
    if let Ok(mut tf) = card.single_mut() {
        tf.scale = Vec2::splat(0.95 + 0.05 * a);
        tf.translation = Val2::px(0.0, (1.0 - a) * 10.0);
    }
    let w = &sim.world;
    let s = format!(
        "Day {} · {} · {} residents",
        w.time.day,
        format::money(w.economy.money),
        format::group(w.stats.population as u64)
    );
    if let Ok(mut text) = subtitle.single_mut()
        && text.0 != s
    {
        text.0 = s;
    }
}

// ---------------------------------------------------------------- hint card

const STEPS: [(&str, &str); 3] = [
    ("Build a road from the highway", "R"),
    ("Zone land beside the road", "Z"),
    ("Unpause to let the city grow", "Space"),
];

fn spawn_hint(mut commands: Commands, fonts: Res<UiFonts>, stack: Query<Entity, With<super::demand::BottomLeftStack>>) {
    let fonts = fonts.into_inner();
    let Ok(stack) = stack.single() else { return };
    let hint = commands
        .spawn((
            Name::new("Hint card"),
            HintRoot,
            panel(Node {
                width: px(318),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(px(t::S4 + 2.0), px(t::S3), px(t::S3), px(t::S4)),
                row_gap: px(t::S2),
                display: Display::None,
                ..default()
            }),
            UiTransform::default(),
        ))
        .with_children(|p| {
            p.spawn(layout(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                margin: UiRect::bottom(px(t::S2)),
                ..default()
            }))
            .with_children(|h| {
                h.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(t::S3),
                    ..default()
                }))
                .with_children(|l| {
                    l.spawn(super::widgets::icon(fonts, icon::LIGHTBULB, t::ICON_MD, t::WARNING));
                    l.spawn(label(fonts, "Getting started", t::FONT_LABEL, t::W_SEMIBOLD, t::TEXT));
                });
                close_button(h, fonts, UiAction::DismissHint);
            });
            for (i, (text, key)) in STEPS.iter().enumerate() {
                p.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(t::S3 + 2.0),
                    min_height: px(26),
                    padding: UiRect::right(px(t::S2)),
                    ..default()
                }))
                .with_children(|r| {
                    r.spawn((
                        super::widgets::icon(fonts, icon::CIRCLE, t::ICON_MD, t::TEXT_FAINT),
                        HintStep(i),
                    ));
                    r.spawn((
                        label(fonts, *text, t::FONT_BODY, t::W_MEDIUM, t::TEXT),
                        HintStepText(i),
                        Node {
                            flex_grow: 1.0,
                            ..default()
                        },
                    ));
                    spawn_key_badge(r, fonts, key);
                });
            }
        })
        .id();
    commands.entity(stack).insert_children(0, &[hint]);
}

#[derive(Default)]
struct HintProgress {
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
    mut steps: Query<(&HintStep, &mut Text, &mut TextColor), Without<HintStepText>>,
    mut texts: Query<(&HintStepText, &mut TextColor), Without<HintStep>>,
    mut progress: Local<HintProgress>,
) {
    let Ok((mut node, mut tf)) = root.single_mut() else {
        return;
    };
    let w = &sim.world;
    if w.roads.segments.len() > 1 {
        progress.done[0] = true;
    }
    let key = sim.key(w.versions.grid);
    if progress.grid_version != Some(key) || last.is_changed() {
        progress.grid_version = Some(key);
        if !progress.done[1] {
            progress.done[1] = w.grid.cells.iter().any(|c| c.zone.is_some())
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
    let show = state.hint_visible && !state.budget_open && !state.menu_open;
    let d = if show { Display::Flex } else { Display::None };
    if node.display != d {
        node.display = d;
        progress.anim = 0.0;
    }
    if !show {
        return;
    }
    progress.anim += (1.0 - progress.anim) * approach(time.delta_secs(), 0.08);
    tf.translation = Val2::px((1.0 - progress.anim) * -12.0, 0.0);
    for (HintStep(i), mut text, mut color) in &mut steps {
        let (g, c) = if progress.done[*i] {
            (icon::CHECK_CIRCLE, t::POSITIVE)
        } else {
            (icon::CIRCLE, t::TEXT_FAINT)
        };
        let g = g.to_string();
        if text.0 != g {
            text.0 = g;
        }
        color.set_if_neq(TextColor(c));
    }
    for (HintStepText(i), mut color) in &mut texts {
        color.set_if_neq(TextColor(if progress.done[*i] { t::TEXT_MUTED } else { t::TEXT }));
    }
}
