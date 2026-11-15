//! Small line charts of a measurement's recent values, newest at the right.

use crate::dashboard::{HISTORY_POINTS, Series};
use crate::theme::scaled;
use b3313_protocol::metrics::REPORT_INTERVAL;
use gpui::{Bounds, Hsla, PathBuilder, Pixels, Point, canvas, div, point, prelude::*, px};
use std::time::Duration;

/// Points a chart spans at least: five minutes of reports. A new session's first points fill
/// part of it, so a short history still reads as a line; past this the chart widens with the
/// history, up to `HISTORY_POINTS`.
pub const MIN_SPAN: usize = 30;

/// The value at a chart's top: the smallest of 1, 2 or 5 times a power of ten that holds its
/// largest value, so its scale reads as a round number. A chart of zeros goes up to 1.
#[must_use]
pub fn top(values: impl IntoIterator<Item = f64>) -> f64 {
    let largest = values.into_iter().fold(0.0_f64, f64::max);
    if largest <= 0.0 || !largest.is_finite() {
        return 1.0;
    }
    // The power of ten at or below the largest value, within the range any measurement has.
    let mut exponent = 0_i32;
    while exponent > -12 && times_ten_to(1.0, exponent) > largest {
        exponent -= 1;
    }
    while exponent < 15 && times_ten_to(1.0, exponent + 1) <= largest {
        exponent += 1;
    }
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|step| times_ten_to(step, exponent))
        .find(|step| *step >= largest)
        .unwrap_or(largest)
}

/// `value` × 10^`exponent`, dividing for a negative exponent so that 5 × 10⁻² is the double
/// nearest 0.05 and its scale label reads "0.05". Powers of ten up to 10²² are exact.
fn times_ten_to(value: f64, exponent: i32) -> f64 {
    if exponent >= 0 {
        value * 10_f64.powi(exponent)
    } else {
        value / 10_f64.powi(-exponent)
    }
}

/// How far back a chart of `points` values reaches at its left edge, a report apart.
#[must_use]
pub fn span(points: usize) -> Duration {
    let gaps = points.clamp(MIN_SPAN, HISTORY_POINTS) - 1;
    REPORT_INTERVAL * u32::try_from(gaps).unwrap_or(u32::MAX)
}

/// A chart's line and the area beneath it, from zero at the bottom to `top` at the top.
pub fn sparkline(
    series: &Series,
    top: f64,
    line: Hsla,
    fill: Hsla,
    height: f32,
) -> impl IntoElement {
    let values: Vec<f64> = series.values().iter().copied().collect();
    div().w_full().h(scaled(height)).child(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let Some(points) = layout(&values, top, bounds) else {
                    return;
                };
                let bottom = bounds.bottom();
                let mut area = PathBuilder::fill();
                if let (Some(first), Some(last)) = (points.first(), points.last()) {
                    area.move_to(point(first.x, bottom));
                    for at in &points {
                        area.line_to(*at);
                    }
                    area.line_to(point(last.x, bottom));
                    area.close();
                }
                if let Ok(path) = area.build() {
                    window.paint_path(path, fill);
                }
                let mut stroke = PathBuilder::stroke(px(1.5));
                stroke.add_polygon(&points, false);
                if let Ok(path) = stroke.build() {
                    window.paint_path(path, line);
                }
            },
        )
        .size_full(),
    )
}

/// Where each value goes in `bounds`, scaled so `top` is the top and zero the bottom. None
/// with fewer than two values, which make no line.
fn layout(values: &[f64], top: f64, bounds: Bounds<Pixels>) -> Option<Vec<Point<Pixels>>> {
    if values.len() < 2 {
        return None;
    }
    let scale = if top > 0.0 { 1.0 / top } else { 0.0 };
    let span = values.len().clamp(MIN_SPAN, HISTORY_POINTS);
    let step = f32::from(bounds.size.width) / f32::from(index(span - 1));
    let height = f32::from(bounds.size.height);
    // The newest value is at the right edge; older ones step left from it.
    let first = span.saturating_sub(values.len());
    Some(
        values
            .iter()
            .enumerate()
            .map(|(offset, value)| {
                let x = f32::from(index(first + offset)) * step;
                let y = height * (1.0 - narrow(value * scale));
                bounds.origin + point(px(x), px(y))
            })
            .collect(),
    )
}

/// A point's place along the chart; there are never more than `HISTORY_POINTS`.
#[must_use]
fn index(at: usize) -> u16 {
    u16::try_from(at).unwrap_or(u16::MAX)
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "a fraction of a chart's height needs no more than f32 precision"
)]
fn narrow(fraction: f64) -> f32 {
    fraction as f32
}

#[cfg(test)]
mod tests {
    use super::{MIN_SPAN, layout, span, top};
    use crate::dashboard::HISTORY_POINTS;
    use gpui::{Bounds, Pixels, Point, point, px, size};
    use std::time::Duration;

    #[test]
    fn the_top_is_a_round_number_holding_the_largest_value() {
        // As a scale shows them, so the values must also print as round numbers.
        let top_of = |values: &[f64]| top(values.iter().copied()).to_string();
        assert_eq!(top_of(&[]), "1");
        assert_eq!(top_of(&[0.0, 0.0]), "1");
        assert_eq!(top_of(&[3.0, 1.0]), "5");
        assert_eq!(top_of(&[1.0]), "1");
        assert_eq!(top_of(&[2.0]), "2");
        assert_eq!(top_of(&[11.0]), "20");
        assert_eq!(top_of(&[0.03]), "0.05");
        assert_eq!(top_of(&[0.0012]), "0.002");
        assert_eq!(top_of(&[1800.0]), "2000");
        assert_eq!(top_of(&[0.3]), "0.5");
        assert_eq!(top_of(&[9.5]), "10");
    }

    #[test]
    fn the_span_grows_with_the_history_from_five_minutes_to_thirty() {
        assert_eq!(span(2), Duration::from_secs(290));
        assert_eq!(span(61), Duration::from_secs(600));
        assert_eq!(span(HISTORY_POINTS + 5), Duration::from_secs(1790));
    }

    fn near(at: Option<&Point<Pixels>>, x: f32, y: f32) -> bool {
        at.is_some_and(|at| {
            (f32::from(at.x) - x).abs() < 0.01 && (f32::from(at.y) - y).abs() < 0.01
        })
    }

    #[test]
    fn a_short_history_fills_part_of_a_five_minute_span_from_the_right() {
        // 29 pixels across 30 points: one pixel a point.
        let bounds = Bounds::new(point(px(10.), px(20.)), size(px(29.), px(100.)));
        assert!(layout(&[1.0], 5.0, bounds).is_none());
        let points = layout(&[0.0, 2.0, 4.0], 5.0, bounds).unwrap_or_default();
        assert_eq!(points.len(), 3);
        // The newest is at the right edge, at four fifths of the way to the top of 5.
        let first_x = 10. + f32::from(u16::try_from(MIN_SPAN - 3).unwrap_or(0));
        assert!(near(points.first(), first_x, 120.), "{points:?}");
        assert!(near(points.last(), 39., 40.), "{points:?}");
        // All zeros lie flat on the bottom.
        let flat = layout(&[0.0, 0.0], 1.0, bounds).unwrap_or_default();
        assert!(flat.iter().all(|p| (f32::from(p.y) - 120.).abs() < 0.01));
    }

    #[test]
    fn a_long_history_spans_the_whole_width() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(179.), px(10.)));
        let values = vec![1.0; HISTORY_POINTS];
        let points = layout(&values, 1.0, bounds).unwrap_or_default();
        assert!(near(points.first(), 0., 0.), "{:?}", points.first());
        assert!(near(points.last(), 179., 0.), "{:?}", points.last());
    }
}
