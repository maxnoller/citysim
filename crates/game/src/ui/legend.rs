//! Traffic overlay legend (bottom-right, only while `OverlayMode::Traffic`
//! is on): the three congestion bands from `shared` and live network
//! figures from `world.stats`.

use bevy::prelude::*;

use super::context::{Band, JAM_CRAWL_MPS, pct};
use super::format;
use super::theme::{self as t, UiFonts, icon};
use super::widgets::{approach, hdivider, label, layout, number, panel};
use crate::shared::{CONGESTION_BUSY, CONGESTION_JAM, GameSet, OverlayMode, SimWorld};

pub struct LegendPlugin;

impl Plugin for LegendPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_legend)
            .add_systems(Update, update_legend.in_set(GameSet::Render));
    }
}

#[derive(Component)]
struct LegendRoot;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum LegendText {
    Jammed,
    Speed,
}

/// Legend rows: band, name, what it means.
fn bands() -> [(Band, &'static str, String); 3] {
    [
        (Band::Free, "Free", format!("under {} full", pct(CONGESTION_BUSY))),
        (
            Band::Busy,
            "Busy",
            format!(
                "{}\u{2013}{} full",
                (CONGESTION_BUSY * 100.0).round() as i64,
                pct(CONGESTION_JAM)
            ),
        ),
        (
            Band::Jammed,
            "Jammed",
            format!(
                "over {} · < {} km/h",
                pct(CONGESTION_JAM),
                (JAM_CRAWL_MPS * 3.6).round() as i64
            ),
        ),
    ]
}

fn spawn_legend(mut commands: Commands, fonts: Res<UiFonts>) {
    let fonts = fonts.into_inner();
    commands
        .spawn((
            Name::new("Traffic legend"),
            LegendRoot,
            super::hud::info_tip(
                "Traffic overlay",
                "Each side of a road is coloured by how full it is: cars on it compared with the cars it holds.",
            ),
            panel(Node {
                position_type: PositionType::Absolute,
                right: px(t::EDGE),
                bottom: px(t::EDGE),
                width: px(256),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(px(t::S4), px(t::S4), px(t::S3 + 2.0), px(t::S4)),
                row_gap: px(t::S2 + 1.0),
                display: Display::None,
                ..default()
            }),
            UiTransform::default(),
        ))
        .with_children(|p| {
            p.spawn(layout(Node {
                align_items: AlignItems::Center,
                column_gap: px(t::S3),
                margin: UiRect::bottom(px(t::S2)),
                ..default()
            }))
            .with_children(|h| {
                h.spawn(super::widgets::icon(fonts, icon::TRAFFIC, t::ICON_SM, t::TEXT_MUTED));
                h.spawn(label(fonts, "Traffic", t::FONT_SMALL, t::W_SEMIBOLD, t::TEXT_MUTED));
            });
            for (band, name, desc) in bands() {
                p.spawn(layout(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(t::S3 + 2.0),
                    min_height: px(20),
                    ..default()
                }))
                .with_children(|r| {
                    // A short road strip in the band colour.
                    r.spawn((
                        layout(Node {
                            width: px(18),
                            height: px(5),
                            border_radius: BorderRadius::all(px(2.5)),
                            flex_shrink: 0.0,
                            ..default()
                        }),
                        BackgroundColor(band.color()),
                    ));
                    r.spawn((
                        label(fonts, name, t::FONT_BODY, t::W_MEDIUM, t::TEXT),
                        Node {
                            width: px(58),
                            ..default()
                        },
                    ));
                    r.spawn(label(fonts, desc, t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED));
                });
            }
            p.spawn(hdivider()).insert(Node {
                height: px(1),
                width: percent(100),
                margin: UiRect::vertical(px(t::S2)),
                ..default()
            });
            for (name, which) in [
                ("Jammed now", LegendText::Jammed),
                ("Average car speed", LegendText::Speed),
            ] {
                p.spawn(layout(Node {
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    min_height: px(18),
                    ..default()
                }))
                .with_children(|r| {
                    r.spawn(label(fonts, name, t::FONT_SMALL, t::W_REGULAR, t::TEXT_MUTED));
                    r.spawn((number(fonts, "", t::FONT_SMALL, t::W_SEMIBOLD, t::TEXT), which));
                });
            }
        });
}

#[allow(clippy::type_complexity)]
fn update_legend(
    time: Res<Time<Real>>,
    overlay: Res<OverlayMode>,
    sim: Res<SimWorld>,
    mut root: Query<(&mut Node, &mut UiTransform), With<LegendRoot>>,
    mut texts: Query<(&LegendText, &mut Text, &mut TextColor)>,
    mut anim: Local<f32>,
    mut last: Local<Option<(u64, u32, usize, i64, bool)>>,
) {
    let Ok((mut node, mut tf)) = root.single_mut() else {
        return;
    };
    let show = *overlay == OverlayMode::Traffic;
    let d = if show { Display::Flex } else { Display::None };
    if node.display != d {
        node.display = d;
        *anim = 0.0;
        *last = None;
    }
    if !show {
        return;
    }
    if *anim < 1.0 {
        *anim += (1.0 - *anim) * approach(time.delta_secs(), 0.06);
        if *anim > 0.995 {
            *anim = 1.0;
        }
        tf.translation = Val2::px(0.0, (1.0 - *anim) * 8.0);
    }
    let s = &sim.world.stats;
    let key = (
        sim.generation,
        s.congested_segments,
        sim.world.roads.segments.len(),
        (s.avg_speed * 3.6).round() as i64,
        s.vehicles_on_road > 0,
    );
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    for (which, mut text, mut color) in &mut texts {
        let (v, c) = match which {
            LegendText::Jammed => {
                let band = if s.congested_segments == 0 {
                    t::POSITIVE
                } else {
                    t::WARNING
                };
                (
                    format!(
                        "{} of {} stretches",
                        format::group(s.congested_segments as u64),
                        format::group(sim.world.roads.segments.len() as u64)
                    ),
                    band,
                )
            }
            LegendText::Speed => (
                if s.vehicles_on_road > 0 {
                    format!("{} km/h", (s.avg_speed * 3.6).round() as i64)
                } else {
                    "no cars".to_string()
                },
                t::TEXT,
            ),
        };
        if text.0 != v {
            text.0 = v;
        }
        if color.0 != c {
            color.0 = c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legend_labels_follow_the_shared_bands() {
        let b = bands();
        assert_eq!(b[0].2, "under 50% full");
        assert_eq!(b[1].2, "50\u{2013}80% full");
        assert_eq!(b[2].2, "over 80% · < 12 km/h");
    }
}
