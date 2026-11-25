//! Loading card: covers the first seconds while the 3D scene's shaders are
//! built (`shared::SceneReady`), so a player never stares at an empty sky.
//! Fades out once the scene draws.

use bevy::prelude::*;

use super::theme::{self as t, UiFonts};
use super::widgets::{label, layout};
use crate::shared::{GameSet, SceneReady};

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_loading)
            .add_systems(Update, update_loading.in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct LoadingRoot;

/// The sliding highlight of the progress track.
#[derive(Component)]
struct LoadingBar;

/// Fade-out: text and panels that dim with the backdrop.
#[derive(Component)]
struct LoadingFade(Color);

/// Daytime sky behind the card (the scene fades in under it).
const SKY: Color = Color::srgb(0.53, 0.71, 0.87);
const FADE_S: f32 = 0.45;
const TRACK_W: f32 = 220.0;
const BAR_W: f32 = 70.0;

fn spawn_loading(mut commands: Commands, fonts: Res<UiFonts>, ready: Res<SceneReady>) {
    if ready.0 {
        return;
    }
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Loading"),
            LoadingRoot,
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                right: px(0),
                top: px(0),
                bottom: px(0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(SKY),
            LoadingFade(SKY),
            // Above everything, the menu included.
            GlobalZIndex(500),
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: px(t::S4),
                    padding: UiRect::axes(px(40), px(28)),
                    border_radius: BorderRadius::all(px(t::RADIUS + 4.0)),
                    ..default()
                },
                BackgroundColor(t::PANEL_RAISED),
                LoadingFade(t::PANEL_RAISED),
            ))
            .with_children(|card| {
                card.spawn((label(fonts, "CitySim", 30.0, t::W_BOLD, t::TEXT), LoadingFade(t::TEXT)));
                card.spawn((
                    label(
                        fonts,
                        "Laying out the town\u{2026}",
                        t::FONT_LABEL,
                        t::W_MEDIUM,
                        t::TEXT_MUTED,
                    ),
                    LoadingFade(t::TEXT_MUTED),
                ));
                card.spawn((
                    layout(Node {
                        width: px(TRACK_W),
                        height: px(4),
                        border_radius: BorderRadius::all(px(2)),
                        overflow: Overflow::clip(),
                        ..default()
                    }),
                    BackgroundColor(t::WELL),
                    LoadingFade(t::WELL),
                ))
                .with_children(|track| {
                    track.spawn((
                        layout(Node {
                            position_type: PositionType::Absolute,
                            width: px(BAR_W),
                            height: px(4),
                            left: px(0),
                            border_radius: BorderRadius::all(px(2)),
                            ..default()
                        }),
                        BackgroundColor(t::ACCENT),
                        LoadingFade(t::ACCENT),
                        LoadingBar,
                    ));
                });
            });
        });
}

#[allow(clippy::type_complexity)]
fn update_loading(
    mut commands: Commands,
    time: Res<Time<Real>>,
    ready: Res<SceneReady>,
    root: Query<Entity, With<LoadingRoot>>,
    mut bar: Query<&mut Node, With<LoadingBar>>,
    mut faded: Query<(&LoadingFade, Option<&mut BackgroundColor>, Option<&mut TextColor>)>,
    mut fade: Local<f32>,
) {
    let Ok(root) = root.single() else { return };
    // An indeterminate bar sliding back and forth.
    let s = (time.elapsed_secs() * 1.6).sin() * 0.5 + 0.5;
    for mut n in &mut bar {
        n.left = px(s * (TRACK_W - BAR_W));
    }
    if !ready.0 {
        return;
    }
    *fade += time.delta_secs() / FADE_S;
    if *fade >= 1.0 {
        commands.entity(root).despawn();
        return;
    }
    let a = 1.0 - *fade;
    for (base, bg, text) in &mut faded {
        let c = base.0.with_alpha(base.0.alpha() * a);
        if let Some(mut bg) = bg {
            bg.0 = c;
        }
        if let Some(mut tc) = text {
            tc.0 = c;
        }
    }
}
