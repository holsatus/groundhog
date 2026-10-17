//! A QGroundControl-style integrated compass and attitude indicator.
//!
//! The instrument is a fixed compass card (N up) with a nose arrow that rotates
//! to the heading, a ±30° roll arc above the dial, and a ±30° pitch arc to its
//! right. It mirrors QGC's `IntegratedCompassAttitude`: the dial is an opaque
//! disc, the two arcs are stroked just outside it, and the arrow is drawn on
//! top.
//!
//! It is a pure drawing routine over a [`Frame`] — it owns no state and emits no
//! messages, so it can be painted into any canvas. [`draw`] is the
//! placement-free core; [`overlay`] is the one caller today, anchoring it to the
//! corner of the map.
//!
//! Angles are radians, matching [`Attitude`](groundhog_logic::domain::vehicle::attitude::Attitude)
//! and the rest of the domain; they are converted to degrees only for the fixed
//! ±30° scale. Colours come from the active [`Palette`], so the instrument
//! follows the application theme.

use iced::{
    Font, Pixels, Point, Radians,
    alignment::Vertical,
    theme::Palette,
    widget::canvas::{Fill, Frame, LineCap, Path, Stroke, Style, Text, path::Arc},
};

/// The full-scale deflection of both attitude arcs, in degrees.
const MAX_ATTITUDE_DEG: f32 = 40.0;

/// The arc band's thickness, as a fraction of the dial radius.
const ARC_WIDTH_RATIO: f32 = 0.2;
/// The gap between the dial edge and the arc band, as a fraction of the radius.
const ARC_GAP_RATIO: f32 = 0.1;
/// The instrument's outermost reach from the dial centre, in dial radii:
/// the dial, then the gap, then the full arc thickness.
const ARC_REACH: f32 = 1.0 + ARC_GAP_RATIO + ARC_WIDTH_RATIO;

/// Inset of the overlay from the map pane's top and right edges, in pixels.
const OVERLAY_MARGIN: f32 = 16.0;
/// Largest compass dial radius, in pixels.
const MAX_DIAL_RADIUS: f32 = 64.0;
/// Smallest compass dial radius; below this the cardinals stop being legible.
const MIN_DIAL_RADIUS: f32 = 24.0;

/// Draws the instrument anchored to the top-right corner of the map pane.
///
/// The dial is scaled to fit the pane, clamped to
/// [`MIN_DIAL_RADIUS`]..[`MAX_DIAL_RADIUS`], and inset by [`OVERLAY_MARGIN`].
pub fn overlay(frame: &mut Frame, roll: f32, pitch: f32, yaw: f32, palette: &Palette) {
    let (center, radius) = placement(frame.size());

    draw(frame, center, radius, roll, pitch, yaw, palette);
}

/// The dial centre and radius for an instrument anchored to the top-right of a
/// pane of the given `size`.
///
/// The dial shrinks so the instrument's full reach — the arc band included —
/// stays inset from the pane edges, and is clamped so it remains legible on a
/// small pane and does not dominate a large one.
pub(crate) fn placement(size: iced::Size) -> (Point, f32) {
    let fit = (size.width.min(size.height) / 2.0 - OVERLAY_MARGIN) / ARC_REACH;
    let radius = fit.clamp(MIN_DIAL_RADIUS, MAX_DIAL_RADIUS);

    let reach = radius * ARC_REACH;
    let center = Point::new(size.width - OVERLAY_MARGIN - reach, OVERLAY_MARGIN + reach);

    (center, radius)
}

/// Draws the instrument with its compass dial centred at `center` and of the
/// given `radius`, for an attitude in radians.
///
/// Roll fills the arc above the dial (positive = clockwise, right wing down),
/// pitch fills the arc to its right (positive = upward, nose up), and yaw
/// rotates the nose arrow clockwise from north across the fixed card.
pub fn draw(
    frame: &mut Frame,
    center: Point,
    radius: f32,
    roll: f32,
    pitch: f32,
    yaw: f32,
    palette: &Palette,
) {
    // Arcs sit outside the dial, so they are drawn first and cannot be covered.
    // Positive roll sweeps clockwise from the top; the pitch indicator is the
    // same gauge rotated a quarter turn to the right, and its value is negated
    // so that a nose-up attitude draws upwards (see QGC's comment on the same
    // negation).
    draw_attitude_arc(frame, center, radius, 0.0, roll.to_degrees(), palette);
    draw_attitude_arc(frame, center, radius, 90.0, -pitch.to_degrees(), palette);

    let arrow_width = radius * 2.0 / 3.0;

    draw_dial(frame, center, radius, palette);
    draw_arrow_head(frame, center, arrow_width, yaw, palette, 0.0);
}

/// The opaque compass card: a filled disc, its border, the tick ring, and the
/// four cardinal labels. The card is fixed; only the needle moves.
fn draw_dial(frame: &mut Frame, center: Point, radius: f32, palette: &Palette) {
    let surface = palette.background.weakest;
    let text_color = surface.text;

    frame.fill(&Path::circle(center, radius), Fill::from(surface.color));
    frame.stroke(
        &Path::circle(center, radius),
        Stroke {
            style: Style::Solid(palette.background.strongest.color),
            width: 1.0,
            ..Stroke::default()
        },
    );

    // Ticks every 22.5°, skipping the four cardinals that carry a label. The
    // diagonals are longer, so the ring reads in 45° steps.
    let ring = radius * 0.94;

    for step in 0..8 {
        let bearing = 45.0 * step as f32;

        let length = if step % 2 == 0 {
            radius * 0.05
        } else {
            radius * 0.15
        };

        frame.stroke(
            &Path::line(
                bearing_point(center, ring - length, bearing),
                bearing_point(center, ring, bearing),
            ),
            Stroke {
                style: Style::Solid(text_color),
                width: 2.0,
                ..Stroke::default()
            },
        );
    }

    // Cardinals in two colour pairs, matching the reference: north and east are
    // cool, south and west warm.
    let cool = palette.primary.base.color;
    let warm = palette.warning.base.color;

    for (bearing, label, color) in [
        (0.0, "N", cool),
        (90.0, "E", cool),
        (180.0, "S", warm),
        (270.0, "W", warm),
    ] {
        let mut cardinal = Text::from(label);
        cardinal.position = bearing_point(center, radius * 0.72, bearing);
        cardinal.color = color;
        cardinal.size = Pixels(radius * 0.26);
        cardinal.font = Font::MONOSPACE;
        cardinal.align_x = iced::advanced::text::Alignment::Center;
        cardinal.align_y = Vertical::Center;
        frame.fill_text(cardinal);
    }
}

/// One roll/pitch gauge: a background band spanning ±[`MAX_ATTITUDE_DEG`] about
/// `zero_bearing`, an accent fill from zero to `sweep`, and a tick marking zero.
///
/// `zero_bearing` is degrees clockwise from up, so roll uses 0° (top) and pitch
/// uses 90° (right).
fn draw_attitude_arc(
    frame: &mut Frame,
    center: Point,
    dial_radius: f32,
    zero_bearing: f32,
    sweep: f32,
    palette: &Palette,
) {
    let width = dial_radius * ARC_WIDTH_RATIO;
    let radius = dial_radius * (1.0 + ARC_GAP_RATIO) + width / 2.0;

    frame.stroke(
        &arc(
            center,
            radius,
            zero_bearing - MAX_ATTITUDE_DEG,
            zero_bearing + MAX_ATTITUDE_DEG,
        ),
        Stroke {
            style: Style::Solid(palette.background.base.color),
            width,
            line_cap: LineCap::Butt,
            ..Stroke::default()
        },
    );

    // Below a hair's breadth the fill is indistinguishable from the zero tick,
    // so it is skipped rather than drawn as a stray sliver.
    let sweep = sweep.clamp(-MAX_ATTITUDE_DEG, MAX_ATTITUDE_DEG);
    if sweep.abs() > 0.5 {
        let (from, to) = if sweep >= 0.0 {
            (zero_bearing, zero_bearing + sweep)
        } else {
            (zero_bearing + sweep, zero_bearing)
        };

        frame.stroke(
            &arc(center, radius, from, to),
            Stroke {
                style: Style::Solid(palette.primary.base.color),
                width,
                line_cap: LineCap::Butt,
                ..Stroke::default()
            },
        );
    }

    // Zero tick, spanning the band so it reads against either colour.
    frame.stroke(
        &Path::line(
            bearing_point(center, radius - width / 2.0, zero_bearing),
            bearing_point(center, radius + width / 2.0, zero_bearing),
        ),
        Stroke {
            style: Style::Solid(palette.background.weakest.text),
            width: 2.0,
            ..Stroke::default()
        },
    );
}

/// The nose arrow: a two-tone dart pointing at the heading, rotated clockwise
/// from north. Its size is QGC's: a square of two thirds the dial diameter.
pub(crate) fn draw_arrow_head(
    frame: &mut Frame,
    center: Point,
    width: f32,
    yaw: f32,
    palette: &Palette,
    ouline_width: f32,
) {
    // Half-width and half-height, in local coordinates centred on the dial.
    let half = width / 2.0;
    let tip = Point::new(0.0, -half);
    let notch = Point::new(0.0, half / 2.0);

    frame.with_save(|frame| {
        frame.translate(center - Point::ORIGIN);
        frame.rotate(Radians(yaw));

        let right_half = Path::new(|builder| {
            builder.move_to(tip);
            builder.line_to(Point::new(half, half));
            builder.line_to(notch);
            builder.close();
        });

        let left_half = Path::new(|builder| {
            builder.move_to(tip);
            builder.line_to(Point::new(-half, half));
            builder.line_to(notch);
            builder.close();
        });

        if ouline_width > 0.0 {
            let outline = Path::new(|builder| {
                builder.move_to(tip);
                builder.line_to(Point::new(half, half));
                builder.line_to(notch);
                builder.line_to(Point::new(-half, half));
                builder.close();
            });

            let stroke = Stroke::default()
                .with_color(palette.background.strongest.text)
                .with_width(ouline_width);

            frame.stroke(&outline, stroke);
        }

        frame.fill(&right_half, Fill::from(palette.danger.base.color));
        frame.fill(&left_half, Fill::from(palette.danger.strong.color));
    });
}

/// A point on a circle at `bearing` degrees clockwise from up.
fn bearing_point(center: Point, radius: f32, bearing: f32) -> Point {
    let radians = bearing.to_radians();

    Point::new(
        center.x + radius * radians.sin(),
        center.y - radius * radians.cos(),
    )
}

/// An open circular arc from `from` to `to` bearing degrees.
///
/// [`Arc`] angles are clockwise from the
/// positive x-axis, while bearings are clockwise from up, hence the quarter-turn.
fn arc(center: Point, radius: f32, from: f32, to: f32) -> Path {
    Path::new(|builder| {
        builder.arc(Arc {
            center,
            radius,
            start_angle: Radians((from - 90.0).to_radians()),
            end_angle: Radians((to - 90.0).to_radians()),
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Size;

    #[test]
    fn bearing_point_starts_up_and_turns_clockwise() {
        let center = Point::new(10.0, 20.0);

        assert_eq!(bearing_point(center, 5.0, 0.0), Point::new(10.0, 15.0));
        assert_eq!(bearing_point(center, 5.0, 90.0), Point::new(15.0, 20.0));
        assert_eq!(bearing_point(center, 5.0, 180.0), Point::new(10.0, 25.0));
        assert_eq!(bearing_point(center, 5.0, 270.0), Point::new(5.0, 20.0));
    }

    #[test]
    fn placement_caps_the_radius_on_a_large_pane() {
        let (center, radius) = placement(Size::new(1000.0, 1000.0));

        assert_eq!(radius, MAX_DIAL_RADIUS);

        // The instrument's reach is exactly one margin from each anchored edge.
        let reach = radius * ARC_REACH;
        assert_eq!(
            center,
            Point::new(1000.0 - OVERLAY_MARGIN - reach, OVERLAY_MARGIN + reach)
        );
    }

    #[test]
    fn placement_shrinks_to_fit_a_small_pane() {
        // Height is the binding dimension: the reach must fit inside it.
        let (center, radius) = placement(Size::new(500.0, 140.0));
        let reach = radius * ARC_REACH;

        assert!(radius < MAX_DIAL_RADIUS);
        assert!((reach - (140.0 / 2.0 - OVERLAY_MARGIN)).abs() < 1e-3);
        assert!((center.y - reach - OVERLAY_MARGIN).abs() < 1e-3);
    }

    #[test]
    fn placement_does_not_go_below_the_minimum() {
        let (_, radius) = placement(Size::new(10.0, 10.0));

        assert_eq!(radius, MIN_DIAL_RADIUS);
    }
}
