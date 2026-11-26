//! Loading card: covers the first seconds while the 3D scene's shaders are
//! built (`shared::SceneReady`), so a player never stares at an empty sky.
//! Fades out once the scene draws.

use bevy::prelude::*;

use super::theme::{self as t, UiFonts};
use super::widgets::layout;
use crate::shared::{GameSet, SceneReady};

pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoldLoading>()
            .add_systems(Startup, spawn_loading)
            .add_systems(Update, update_loading.in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct LoadingRoot;

/// Show the loading plate again and keep it (screenshots of it; dev only).
#[derive(Resource, Default)]
pub struct HoldLoading(pub bool);

/// The sliding highlight of the progress track.
#[derive(Component)]
struct LoadingBar;

/// Fade-out: text and panels that dim with the backdrop.
#[derive(Component)]
struct LoadingFade(Color);

/// Behind the plate (the scene fades in under it).
const SKY: Color = Color::srgb(0.075, 0.083, 0.098);
const FADE_S: f32 = 0.45;
const TRACK_W: f32 = 220.0;
const BAR_W: f32 = 70.0;

fn spawn_loading(mut commands: Commands, fonts: Res<UiFonts>, ready: Res<SceneReady>) {
    if ready.0 {
        return;
    }
    spawn_plate(&mut commands, fonts.into_inner());
}

fn spawn_plate(commands: &mut Commands, fonts: &UiFonts) {
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
                    align_items: AlignItems::Start,
                    row_gap: px(t::S3),
                    padding: UiRect::new(px(40), px(48), px(30), px(30)),
                    border: UiRect::new(px(4), px(0), px(t::LW), px(t::LW)),
                    ..default()
                },
                BackgroundColor(t::PLATE_BOTTOM),
                LoadingFade(t::PLATE_BOTTOM),
                BorderColor::all(t::LINE),
            ))
            .with_children(|card| {
                card.spawn((
                    layout(Node {
                        width: px(34),
                        height: px(4),
                        margin: UiRect::bottom(px(6)),
                        ..default()
                    }),
                    BackgroundColor(t::ACCENT),
                    LoadingFade(t::ACCENT),
                ));
                card.spawn((
                    Text::new("CITYSIM"),
                    fonts.figures(44.0, t::W_BOLD),
                    t::caps_spacing(44.0, 0.16),
                    TextColor(t::TEXT),
                    LoadingFade(t::TEXT),
                ));
                card.spawn((
                    super::widgets::caps(fonts, "Laying out the town\u{2026}", t::FONT_CAP, t::TEXT_MUTED),
                    LoadingFade(t::TEXT_MUTED),
                ));
                card.spawn((
                    layout(Node {
                        width: px(TRACK_W),
                        height: px(5),
                        margin: UiRect::top(px(10)),
                        overflow: Overflow::clip(),
                        ..default()
                    }),
                    BackgroundColor(t::LINE),
                    LoadingFade(t::LINE),
                ))
                .with_children(|track| {
                    track.spawn((
                        layout(Node {
                            position_type: PositionType::Absolute,
                            width: px(BAR_W),
                            height: px(5),
                            left: px(0),
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
#[allow(clippy::too_many_arguments)]
fn update_loading(
    mut commands: Commands,
    time: Res<Time<Real>>,
    ready: Res<SceneReady>,
    root: Query<Entity, With<LoadingRoot>>,
    mut bar: Query<&mut Node, With<LoadingBar>>,
    mut faded: Query<(&LoadingFade, Option<&mut BackgroundColor>, Option<&mut TextColor>)>,
    mut fade: Local<f32>,
    hold: Res<HoldLoading>,
    fonts: Res<UiFonts>,
) {
    if hold.0 && root.is_empty() && hold.is_changed() {
        *fade = 0.0;
        spawn_plate(&mut commands, &fonts);
        return;
    }
    let Ok(root) = root.single() else { return };
    // An indeterminate bar sliding back and forth.
    let s = (time.elapsed_secs() * 1.6).sin() * 0.5 + 0.5;
    for mut n in &mut bar {
        n.left = px(s * (TRACK_W - BAR_W));
    }
    if !ready.0 || hold.0 {
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
