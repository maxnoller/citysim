//! Settings card (from the pause menu): graphics quality, interface scale,
//! edge scrolling, lot markers for zoned-but-empty land, and a key
//! reference. Display settings gfx honours live in `shared::DisplaySettings`.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::prelude::*;

use super::UiState;
use super::budget::close_button;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{
    ButtonFg, ButtonKind, UiAction, UiButton, approach, label, layout, panel, spawn_button, spawn_key_badge,
};
use crate::shared::{GameSet, GraphicsQuality};

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_settings)
            .add_systems(Update, show_settings.in_set(GameSet::Render));
    }
}

/// Interface scale steps offered (multiplied with the automatic scale).
pub const UI_SCALES: [(f32, &str); 4] = [(0.85, "85%"), (1.0, "100%"), (1.15, "115%"), (1.3, "130%")];

#[derive(Component)]
struct SettingsRoot;

#[derive(Component)]
struct SettingsCard;

/// Key reference, grouped.
const KEYS: [(&str, &[(&str, &str)]); 3] = [
    (
        "CAMERA",
        &[
            ("WASD", "Pan (Shift: faster)"),
            ("Q  E", "Rotate"),
            ("Wheel", "Zoom"),
            ("Middle drag", "Orbit and tilt"),
        ],
    ),
    (
        "BUILDING",
        &[
            ("I", "Inspect (click pins the card)"),
            ("R", "Road · again: street / avenue"),
            ("J", "Junctions: lights or priority"),
            ("Z", "Zone · again: next zone type"),
            ("B", "Bulldoze"),
            ("Tab", "Next mode of the tool"),
            ("Enter", "Build the path (or double-click)"),
            ("Backspace", "Remove the last corner"),
            ("Alt", "Hold: place without snapping"),
            ("[  ]", "Block size (block tool)"),
            ("Ctrl+Z", "Undo the last road or zoning"),
            ("Esc", "Cancel · back to Inspect · menu"),
        ],
    ),
    (
        "GAME",
        &[
            ("Space", "Pause"),
            ("1  2  3", "Speed"),
            ("T  O", "Traffic overlay · cycle overlays"),
            ("G", "Graphics quality"),
            ("F5  F9", "Save · load"),
        ],
    ),
];

fn heading(p: &mut ChildSpawnerCommands, fonts: &UiFonts, glyph: char, text: &str, key: Option<&str>) {
    p.spawn(layout(Node {
        align_items: AlignItems::Center,
        column_gap: px(t::S3 + 2.0),
        ..default()
    }))
    .with_children(|r| {
        r.spawn(super::widgets::icon(fonts, glyph, t::ICON_SM + 1.0, t::TEXT_MUTED));
        r.spawn((
            label(fonts, text, t::FONT_BODY, t::W_MEDIUM, t::TEXT),
            Node {
                flex_grow: 1.0,
                ..default()
            },
        ));
        if let Some(k) = key {
            spawn_key_badge(r, fonts, k);
        }
    });
}

fn segmented(p: &mut ChildSpawnerCommands, fonts: &UiFonts, items: &[(UiAction, &'static str)]) {
    p.spawn(layout(Node {
        padding: UiRect::all(px(3)),
        column_gap: px(3),
        margin: UiRect::bottom(px(t::S4)),
        border_radius: BorderRadius::all(px(t::RADIUS_SM + 1.0)),
        ..default()
    }))
    .insert(BackgroundColor(t::WELL))
    .with_children(|seg| {
        for &(action, name) in items {
            spawn_button(
                seg,
                UiButton {
                    action,
                    kind: ButtonKind::Segment,
                    accent: t::ACCENT,
                },
                Node {
                    flex_grow: 1.0,
                    flex_basis: px(0),
                    height: px(28),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                None,
                |b, id| {
                    b.spawn((
                        label(fonts, name, t::FONT_SMALL + 0.5, t::W_MEDIUM, t::TEXT_MUTED),
                        ButtonFg {
                            owner: id,
                            base: t::TEXT_MUTED,
                        },
                    ));
                },
            );
        }
    });
}

fn spawn_settings(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Settings"),
            SettingsRoot,
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
            BackgroundColor(t::BACKDROP),
            // Above toasts (300): the settings are modal.
            GlobalZIndex(320),
        ))
        .with_children(|root| {
            root.spawn((
                SettingsCard,
                panel(Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(px(t::S5 + 4.0), px(t::S5 + 4.0), px(t::S4 + 2.0), px(t::S5 + 2.0)),
                    ..default()
                }),
                UiTransform::default(),
            ))
            .insert(BackgroundColor(t::PANEL_RAISED))
            .with_children(|c| {
                c.spawn(layout(Node {
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    margin: UiRect::bottom(px(t::S4)),
                    ..default()
                }))
                .with_children(|h| {
                    h.spawn(label(fonts, "Settings", 22.0, t::W_BOLD, t::TEXT));
                    close_button(h, fonts, UiAction::CloseSettings);
                });
                c.spawn(layout(Node {
                    column_gap: px(t::S5 + 12.0),
                    ..default()
                }))
                .with_children(|cols| {
                    // Left: settings.
                    cols.spawn(layout(Node {
                        flex_direction: FlexDirection::Column,
                        width: px(268),
                        row_gap: px(t::S3),
                        ..default()
                    }))
                    .with_children(|l| {
                        heading(l, fonts, icon::MONITOR, "Graphics quality", Some("G"));
                        segmented(
                            l,
                            fonts,
                            &[
                                (UiAction::Quality(GraphicsQuality::Low), "Low"),
                                (UiAction::Quality(GraphicsQuality::Medium), "Medium"),
                                (UiAction::Quality(GraphicsQuality::High), "High"),
                            ],
                        );
                        heading(l, fonts, icon::SCALING, "Interface size", None);
                        let scales: Vec<(UiAction, &'static str)> = UI_SCALES
                            .iter()
                            .enumerate()
                            .map(|(i, &(_, n))| (UiAction::UiScale(i), n))
                            .collect();
                        segmented(l, fonts, &scales);
                        heading(l, fonts, icon::MOVE, "Edge scrolling", None);
                        segmented(
                            l,
                            fonts,
                            &[(UiAction::EdgeScroll(true), "On"), (UiAction::EdgeScroll(false), "Off")],
                        );
                        heading(l, fonts, icon::ZONE, "Mark empty zoned lots", None);
                        segmented(
                            l,
                            fonts,
                            &[
                                (UiAction::LotMarkers(true), "Always"),
                                (UiAction::LotMarkers(false), "While zoning"),
                            ],
                        );
                        l.spawn(layout(Node {
                            flex_grow: 1.0,
                            ..default()
                        }));
                        spawn_button(
                            l,
                            UiButton {
                                action: UiAction::CloseSettings,
                                kind: ButtonKind::Menu,
                                accent: t::ACCENT,
                            },
                            Node {
                                height: px(38),
                                padding: UiRect::axes(px(t::S3), px(0)),
                                align_items: AlignItems::Center,
                                column_gap: px(t::S3 + 2.0),
                                ..default()
                            },
                            None,
                            |b, id| {
                                b.spawn((
                                    super::widgets::icon(fonts, icon::CHEVRON_LEFT, t::ICON_MD, t::TEXT_MUTED),
                                    ButtonFg {
                                        owner: id,
                                        base: t::TEXT_MUTED,
                                    },
                                ));
                                b.spawn((
                                    label(fonts, "Back to the menu", t::FONT_LABEL, t::W_MEDIUM, t::TEXT),
                                    ButtonFg {
                                        owner: id,
                                        base: t::TEXT,
                                    },
                                    Node {
                                        flex_grow: 1.0,
                                        ..default()
                                    },
                                ));
                                spawn_key_badge(b, fonts, "Esc");
                            },
                        );
                    });
                    // Right: keys.
                    cols.spawn(layout(Node {
                        flex_direction: FlexDirection::Column,
                        width: px(318),
                        row_gap: px(5),
                        ..default()
                    }))
                    .with_children(|r| {
                        for (gi, (group, keys)) in KEYS.iter().enumerate() {
                            r.spawn((
                                label(fonts, *group, t::FONT_CAPTION + 0.5, t::W_SEMIBOLD, t::TEXT_FAINT),
                                Node {
                                    margin: UiRect::new(px(0), px(0), px(if gi == 0 { 0.0 } else { t::S3 }), px(1)),
                                    ..default()
                                },
                            ));
                            for (k, what) in keys.iter() {
                                r.spawn(layout(Node {
                                    align_items: AlignItems::Center,
                                    column_gap: px(t::S3),
                                    ..default()
                                }))
                                .with_children(|row| {
                                    row.spawn(layout(Node {
                                        width: px(92),
                                        flex_shrink: 0.0,
                                        ..default()
                                    }))
                                    .with_children(|kc| spawn_key_badge(kc, fonts, k));
                                    row.spawn(label(fonts, *what, t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED));
                                });
                            }
                        }
                    });
                });
            });
        });
}

fn show_settings(
    time: Res<Time<Real>>,
    state: Res<UiState>,
    mut root: Query<&mut Node, With<SettingsRoot>>,
    mut card: Query<&mut UiTransform, With<SettingsCard>>,
    mut anim: Local<f32>,
) {
    let Ok(mut node) = root.single_mut() else { return };
    let open = state.menu_open && state.settings_open;
    let d = if open { Display::Flex } else { Display::None };
    if node.display != d {
        node.display = d;
        *anim = 0.0;
    }
    if open && *anim < 1.0 {
        *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.07);
        if *anim > 0.995 {
            *anim = 1.0;
        }
        if let Ok(mut tf) = card.single_mut() {
            tf.scale = Vec2::splat(0.96 + 0.04 * *anim);
            tf.translation = Val2::px(0.0, (1.0 - *anim) * 8.0);
        }
    }
}
