//! Column charts in the HUD's language, built from plain nodes: a caps
//! title with the hovered (or newest) column's value on the right, a well
//! with faint guide lines, one column per day (stacked in colours, above
//! and below a zero line), the first and last labels under it and an
//! optional legend. The newest column is lit, the others a little dimmed;
//! hovering a column reads it out.
//!
//! A chart is spawned once with a fixed number of slots and filled from a
//! [`ChartData`] whenever its data changes (`fill`); nothing moves while
//! the data stays the same, so nothing jitters.

use bevy::ecs::hierarchy::ChildSpawnerCommands;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;

use super::theme::{self as t, UiFonts};
use super::widgets::{caps, figure, label, layout};

/// Stacked parts a column can have.
pub const STACK: usize = 5;

/// One column: stacked parts (value, colour), drawn from the zero line up
/// for positive values and down for negative ones, and what hovering it
/// reads ("Day 12", "4,993 residents").
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Column {
    pub parts: Vec<(f32, Color)>,
    pub label: String,
    pub value: String,
}

/// Everything a chart shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartData {
    pub columns: Vec<Column>,
    /// Under the first and the last column.
    pub first: String,
    pub last: String,
    /// The top of the scale, in words ("5,000").
    pub top: String,
    /// Shown instead of columns when there are none.
    pub empty: String,
    /// The newest column is a live "so far" value (drawn lighter).
    pub live_last: bool,
    /// The column read out while none is hovered (else the newest).
    pub pick: Option<usize>,
}

/// A chart's root (its id says what it shows).
#[derive(Component, Debug, Clone, Copy)]
pub struct Chart {
    pub id: u32,
    pub slots: usize,
}

#[derive(Component, Clone, Copy)]
struct Slot {
    chart: u32,
    i: usize,
}

#[derive(Component, Clone, Copy)]
struct Part {
    chart: u32,
    i: usize,
    k: usize,
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum ChartText {
    Value,
    Day,
    First,
    Last,
    Top,
    Empty,
}

#[derive(Component, Clone, Copy)]
struct TextOf(u32, ChartText);

#[derive(Component, Clone, Copy)]
struct ZeroLine(u32);

/// Spawn a chart: `title`, `slots` columns, `height` px plot, a legend.
pub fn spawn_chart(
    p: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    id: u32,
    title: &str,
    slots: usize,
    height: f32,
    legend: &[(&str, Color)],
) {
    p.spawn((
        Chart { id, slots },
        layout(Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            ..default()
        }),
    ))
    .with_children(|c| {
        // Title and the read-out.
        c.spawn(layout(Node {
            align_items: AlignItems::Center,
            column_gap: px(8),
            margin: UiRect::bottom(px(6)),
            ..default()
        }))
        .with_children(|h| {
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
                caps(fonts, title, t::FONT_CAP, t::TEXT),
                Node {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_width: px(0),
                    overflow: Overflow::clip(),
                    ..default()
                },
            ));
            h.spawn((
                caps(fonts, "", 10.5, t::TEXT_MUTED),
                TextOf(id, ChartText::Day),
                Node {
                    flex_shrink: 0.0,
                    ..default()
                },
            ));
            h.spawn((
                figure(fonts, "", 17.0, t::TEXT),
                TextOf(id, ChartText::Value),
                Node {
                    flex_shrink: 0.0,
                    min_width: px(64),
                    ..default()
                },
                super::widgets::Justified(Justify::Right),
            ));
        });
        // The plot.
        c.spawn((
            layout(Node {
                height: px(height),
                width: percent(100),
                border: UiRect::all(px(1)),
                padding: UiRect::axes(px(4), px(0)),
                ..default()
            }),
            BackgroundColor(t::alpha(t::WELL, 0.7)),
            BorderColor::all(t::RULE),
        ))
        .with_children(|plot| {
            // Guide lines at a quarter, half and three quarters.
            for k in 1..4 {
                plot.spawn((
                    layout(Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        right: px(0),
                        top: percent(k as f32 * 25.0),
                        height: px(1),
                        ..default()
                    }),
                    BackgroundColor(t::alpha(t::RULE, 0.6)),
                ));
            }
            plot.spawn((
                caps(fonts, "", 9.5, t::TEXT_FAINT),
                TextOf(id, ChartText::Top),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(5),
                    top: px(3),
                    ..default()
                },
            ));
            plot.spawn((
                layout(Node {
                    position_type: PositionType::Absolute,
                    left: px(0),
                    right: px(0),
                    height: px(1),
                    top: percent(100),
                    ..default()
                }),
                BackgroundColor(t::LINE),
                ZeroLine(id),
            ));
            // The columns.
            plot.spawn(layout(Node {
                width: percent(100),
                height: percent(100),
                column_gap: px(if slots > 24 { 2.0 } else { 3.0 }),
                ..default()
            }))
            .with_children(|row| {
                for i in 0..slots {
                    row.spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_basis: px(0),
                            height: percent(100),
                            ..default()
                        },
                        Slot { chart: id, i },
                        Hovered::default(),
                    ))
                    .with_children(|s| {
                        for k in 0..STACK {
                            s.spawn((
                                layout(Node {
                                    position_type: PositionType::Absolute,
                                    left: px(0),
                                    right: px(0),
                                    top: percent(100),
                                    height: px(0),
                                    ..default()
                                }),
                                BackgroundColor(Color::NONE),
                                Part { chart: id, i, k },
                            ));
                        }
                    });
                }
            });
            plot.spawn((
                label(fonts, "", 11.5, t::W_MEDIUM, t::TEXT_FAINT),
                TextOf(id, ChartText::Empty),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(10),
                    top: percent(40),
                    ..default()
                },
            ));
        });
        // Axis labels.
        c.spawn(layout(Node {
            justify_content: JustifyContent::SpaceBetween,
            margin: UiRect::top(px(4)),
            ..default()
        }))
        .with_children(|r| {
            r.spawn((caps(fonts, "", 9.5, t::TEXT_FAINT), TextOf(id, ChartText::First)));
            r.spawn((caps(fonts, "", 9.5, t::TEXT_FAINT), TextOf(id, ChartText::Last)));
        });
        if !legend.is_empty() {
            c.spawn(layout(Node {
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(14),
                row_gap: px(4),
                margin: UiRect::top(px(6)),
                ..default()
            }))
            .with_children(|l| {
                for (name, color) in legend {
                    l.spawn(layout(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(6),
                        ..default()
                    }))
                    .with_children(|e| {
                        e.spawn((
                            layout(Node {
                                width: px(10),
                                height: px(10),
                                flex_shrink: 0.0,
                                ..default()
                            }),
                            BackgroundColor(*color),
                        ));
                        e.spawn(label(fonts, *name, t::FONT_SMALL, t::W_MEDIUM, t::SLOT_INK));
                    });
                }
            });
        }
    });
}

/// Vertical layout: `(zero line from the top, scale)` as fractions of the
/// plot height, for columns whose stacks reach `max` above and `min` below
/// zero.
pub fn scale(columns: &[Column]) -> (f32, f32) {
    let (mut max, mut min) = (0.0f32, 0.0f32);
    for c in columns {
        let up: f32 = c.parts.iter().map(|p| p.0.max(0.0)).sum();
        let down: f32 = c.parts.iter().map(|p| p.0.min(0.0)).sum();
        max = max.max(up);
        min = min.min(down);
    }
    let span = (max - min).max(1e-6);
    // A little headroom so the tallest column does not touch the top (and
    // the deepest the bottom).
    let zero = if max > 0.0 || min < 0.0 { max / span } else { 1.0 };
    (zero, 1.0 / (span * 1.08))
}

/// The chart queries `fill` needs (one parameter for systems).
#[derive(bevy::ecs::system::SystemParam)]
pub struct ChartNodes<'w, 's> {
    slots: Query<'w, 's, (&'static Slot, &'static Hovered)>,
    parts: Query<'w, 's, (&'static Part, &'static mut Node, &'static mut BackgroundColor), Without<ZeroLine>>,
    zero: Query<'w, 's, (&'static ZeroLine, &'static mut Node), Without<Part>>,
    texts: Query<
        'w,
        's,
        (
            &'static TextOf,
            &'static mut Text,
            &'static mut TextColor,
            &'static mut Node,
        ),
        (Without<Part>, Without<ZeroLine>),
    >,
    charts: Query<'w, 's, &'static Chart>,
}

impl ChartNodes<'_, '_> {
    /// The hovered column of chart `id`.
    pub fn hovered(&self, id: u32) -> Option<usize> {
        self.slots
            .iter()
            .find(|(s, h)| s.chart == id && h.get())
            .map(|(s, _)| s.i)
    }

    /// Show `data` in chart `id`; `hover` is the hovered slot.
    pub fn fill(&mut self, id: u32, data: &ChartData, hover: Option<usize>) {
        let Some(slots) = self.charts.iter().find(|c| c.id == id).map(|c| c.slots) else {
            return;
        };
        let cols = &data.columns[data.columns.len().saturating_sub(slots)..];
        let offset = slots - cols.len();
        let (zero, k) = scale(cols);
        let selected = hover
            .and_then(|h| h.checked_sub(offset))
            .filter(|&h| h < cols.len())
            .or(data.pick.filter(|&p| p < cols.len()))
            .or(cols.len().checked_sub(1));
        for (p, mut node, mut bg) in &mut self.parts {
            if p.chart != id {
                continue;
            }
            let col = p.i.checked_sub(offset).and_then(|i| cols.get(i).map(|c| (i, c)));
            let (top, h, color) = match col {
                Some((i, c)) => {
                    // Stack this part on the ones before it, on its side
                    // of the zero line.
                    let v = c.parts.get(p.k).map_or(0.0, |x| x.0);
                    let before: f32 = c.parts[..p.k.min(c.parts.len())]
                        .iter()
                        .map(|x| x.0)
                        .filter(|x| (x.signum() == v.signum()) && *x != 0.0)
                        .sum();
                    let color = c.parts.get(p.k).map_or(Color::NONE, |x| x.1);
                    let lit = Some(i) == selected;
                    let a = if lit {
                        1.0
                    } else if data.live_last && i + 1 == cols.len() {
                        0.45
                    } else {
                        0.62
                    };
                    if v >= 0.0 {
                        (zero - (before + v) * k, v * k, t::alpha(color, a * color.alpha()))
                    } else {
                        (zero - before * k, -v * k, t::alpha(color, a * color.alpha()))
                    }
                }
                None => (zero, 0.0, Color::NONE),
            };
            let (top, h) = (
                percent((top * 100.0).clamp(0.0, 100.0)),
                percent((h * 100.0).clamp(0.0, 100.0)),
            );
            if node.top != top || node.height != h {
                node.top = top;
                node.height = h;
            }
            bg.set_if_neq(BackgroundColor(color));
        }
        for (z, mut n) in &mut self.zero {
            if z.0 == id {
                let top = percent(zero * 100.0);
                if n.top != top {
                    n.top = top;
                }
            }
        }
        let sel = selected.map(|i| &cols[i]);
        for (tag, mut text, mut color, mut node) in &mut self.texts {
            if tag.0 != id {
                continue;
            }
            let (s, c, show) = match tag.1 {
                ChartText::Value => (sel.map(|c| c.value.clone()).unwrap_or_default(), t::TEXT, true),
                ChartText::Day => (
                    sel.map(|c| c.label.to_uppercase()).unwrap_or_default(),
                    t::TEXT_MUTED,
                    true,
                ),
                ChartText::First => (data.first.to_uppercase(), t::TEXT_FAINT, true),
                ChartText::Last => (data.last.to_uppercase(), t::TEXT_FAINT, true),
                ChartText::Top => (data.top.to_uppercase(), t::TEXT_FAINT, !cols.is_empty()),
                ChartText::Empty => (data.empty.clone(), t::TEXT_FAINT, cols.is_empty()),
            };
            if text.0 != s {
                text.0 = s;
            }
            color.set_if_neq(TextColor(c));
            let d = if show { Display::Flex } else { Display::None };
            if node.display != d {
                node.display = d;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(v: &[f32]) -> Column {
        Column {
            parts: v.iter().map(|&x| (x, Color::WHITE)).collect(),
            ..default()
        }
    }

    #[test]
    fn positive_columns_stand_on_the_bottom() {
        let (zero, k) = scale(&[col(&[50.0, 50.0]), col(&[20.0])]);
        assert!((zero - 1.0).abs() < 1e-4);
        // The tallest stack leaves a little headroom.
        assert!(100.0 * k < 1.0 && 100.0 * k > 0.9);
    }

    #[test]
    fn negative_parts_hang_below_zero() {
        let (zero, k) = scale(&[col(&[300.0, -100.0])]);
        assert!(zero > 0.7 && zero < 0.8, "{zero}");
        assert!(zero + 100.0 * k <= 1.0 + 1e-4);
        assert!(zero - 300.0 * k >= -1e-4);
    }

    #[test]
    fn empty_and_flat_charts_do_not_divide_by_zero() {
        let (zero, k) = scale(&[]);
        assert!(zero.is_finite() && k.is_finite());
        let (zero, k) = scale(&[col(&[0.0])]);
        assert!(zero.is_finite() && k.is_finite());
    }
}
