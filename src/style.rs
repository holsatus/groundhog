//! Shared container styles, all derived from the active [`Theme`](iced::Theme).
//!
//! The workspace is a set of floating panes separated by gaps of the window
//! background. A regular pane is a single rounded, bordered surface: its header
//! and body are transparent, so they share one colour, and a `rule` divides
//! them. The map cannot work this way — it paints opaque tiles across its whole
//! pane — so it draws its own frame instead (see `features::map`).

use iced::{
    Border, Color, Shadow, Theme, Vector, border::Radius, theme::Palette, widget::container,
};
use mavio::default_dialect::enums::MavSeverity;

/// The corner radius of a pane, shared with the map's frame.
pub(crate) const PANE_RADIUS: f32 = 8.0;
/// The width of a pane's border, and the height of the header divider.
pub(crate) const PANE_BORDER_WIDTH: f32 = 1.0;

/// A regular pane: one solid surface with a rounded border. The body is
/// transparent against this backdrop and a `rule` divides it from the header.
pub fn pane(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
        border: Border {
            width: PANE_BORDER_WIDTH,
            radius: Radius::new(PANE_RADIUS),
            color: palette.background.strongest.color,
        },
        ..container::Style::default()
    }
}

/// The header of an unfocused pane: a shade brighter than [`pane`]'s backdrop,
/// with rounded top corners so it sits flush inside the pane border.
pub fn title_bar(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weaker.color.into()),
        text_color: Some(palette.background.weaker.text),
        border: Border {
            radius: Radius::default().top(PANE_RADIUS),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// The header of the focused pane, a shade brighter still.
pub fn title_bar_focused(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weak.color.into()),
        text_color: Some(palette.background.weak.text),
        border: Border {
            radius: Radius::default().top(PANE_RADIUS),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// The `rule` separating a pane's header from its body. It shares the border
/// colour, so the divider meets the pane outline without a seam.
pub fn divider(theme: &Theme) -> iced::widget::rule::Style {
    iced::widget::rule::Style {
        color: theme.palette().background.strongest.color,
        ..iced::widget::rule::default(theme)
    }
}

/// The vertical rail of panel buttons.
pub fn rail(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
        ..container::Style::default()
    }
}

/// An even list row.
pub fn list_default(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
        ..container::Style::default()
    }
}

/// An odd list row, subtly brighter than [`list_default`].
pub fn list_brighter(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weaker.color.into()),
        text_color: Some(palette.background.weaker.text),
        ..container::Style::default()
    }
}

/// A floating tooltip.
pub fn tooltip(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weak.color.into()),
        text_color: Some(palette.background.weak.text),
        border: Border {
            width: 1.0,
            radius: 6.0.into(),
            color: palette.background.strongest.color,
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.5),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 6.0,
        },
        ..container::Style::default()
    }
}

/// The palette color a severity maps to.
pub fn severity_color(palette: Palette, severity: MavSeverity) -> Color {
    match severity {
        MavSeverity::Emergency
        | MavSeverity::Alert
        | MavSeverity::Critical
        | MavSeverity::Error => palette.danger.base.color,
        MavSeverity::Warning | MavSeverity::Notice => palette.warning.base.color,
        MavSeverity::Info => palette.primary.base.color,
        MavSeverity::Debug => palette.secondary.base.color,
    }
}

/// The semantic color for a parameter's type label.
///
/// This is presentation, so it names the meaning rather than reaching into an
/// arbitrary palette slot at the call site.
pub fn parameter_type_color(theme: &Theme) -> Color {
    theme.palette().secondary.base.color
}

// Map overlay colors.
//
// The overlay is drawn on top of the opaque OpenStreetMap tiles, not on the
// themed pane surface, so it is deliberately theme-independent: a single fixed
// palette keeps waypoints and the route legible over any tile. These are the
// one control point for the overlay's look.

/// The translucent wash that darkens the map tiles behind the overlay.
pub fn map_backdrop() -> Color {
    Color::BLACK.scale_alpha(0.4)
}

/// The stroke connecting consecutive positioned mission items.
pub fn map_route() -> Color {
    Color::WHITE.scale_alpha(1.0)
}

/// A mission marker that is neither current nor being dragged.
pub fn marker_default() -> Color {
    Color::WHITE
}

/// The mission marker at the vehicle's current execution item.
pub fn marker_current() -> Color {
    Color::from_rgb(0.3, 0.9, 0.4)
}

/// The mission marker currently being dragged.
pub fn marker_dragged() -> Color {
    Color::from_rgb(1.0, 0.6, 0.1)
}

/// The number label drawn on a mission marker.
pub fn marker_label() -> Color {
    Color::BLACK
}
