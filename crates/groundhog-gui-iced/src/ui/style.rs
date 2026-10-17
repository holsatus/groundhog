//! The look: design tokens and the style functions of every widget, all
//! derived from the active [`Theme`].
//!
//! Views do not style widgets themselves; the constructors in [`crate::ui`]
//! apply these. A style function lives here when a constructor needs it, or
//! when a widget that has no constructor (the pane grid, the map) does.
//!
//! The workspace is a set of floating panes separated by gaps of the window
//! background. A regular pane is a single rounded, bordered surface: its header
//! and body are transparent, so they share one colour, and a `rule` divides
//! them. The map cannot work this way — it paints opaque tiles across its whole
//! pane — so it draws its own frame instead (see `map::Map::view`).

use iced::{
    Border, Color, Shadow, Theme, Vector,
    border::Radius,
    theme::Palette,
    widget::{self, container, rule, svg, text},
};
use mavio::default_dialect::enums::MavSeverity;

/// Gaps between and around elements, in logical pixels. Spacing inside a
/// group is smaller than between groups.
pub mod space {
    /// Between lines of one label.
    pub const XXS: f32 = 2.0;
    /// Between tightly related elements (badges, a label and its value).
    pub const XS: f32 = 4.0;
    /// Between the items of a row or column of controls.
    pub const S: f32 = 6.0;
    /// Around a panel's or a card's content, and between groups.
    pub const M: f32 = 8.0;
    /// Between sections.
    pub const L: f32 = 12.0;
    /// Around dialogs and toasts.
    pub const XL: f32 = 16.0;
}

/// Text and icon sizes, in logical pixels.
pub mod size {
    /// Secondary text: hints, units, details, labels of values. Running
    /// text keeps iced's default size, 16.
    pub const CAPTION: f32 = 12.0;
    /// The title of a card or a dialog.
    pub const TITLE: f32 = 18.0;
    /// An icon in a button.
    pub const ICON: f32 = 18.0;
    /// The side of a square icon button.
    pub const ICON_BUTTON: f32 = 32.0;
    /// An icon on the rail.
    pub const RAIL_ICON: f32 = 27.0;
    /// The side of a button on the rail.
    pub const RAIL_BUTTON: f32 = 48.0;
    /// The width of the labels in a form, so its controls line up.
    pub const FIELD_LABEL: f32 = 140.0;
    /// The height of a list row of one line of body text.
    pub const LIST_ROW: f32 = 26.0;
    /// The height of a progress bar.
    pub const PROGRESS: f32 = 6.0;
}

/// The corner radius of a pane, shared with the map's frame.
pub const PANE_RADIUS: f32 = 8.0;
/// The corner radius of controls and cards.
pub const WIDGET_RADIUS: f32 = 4.0;
/// The width of a pane's border, and the height of the header divider.
pub const PANE_BORDER_WIDTH: f32 = 1.0;

/// What a piece of text or an accent means, which decides its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Secondary information, and things that are off or unknown.
    Muted,
    /// Information worth noticing, and things in progress.
    Accent,
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub fn color(self, palette: &Palette) -> Color {
        match self {
            Tone::Muted => palette.secondary.base.color,
            Tone::Accent => palette.primary.base.color,
            Tone::Success => palette.success.base.color,
            Tone::Warning => palette.warning.base.color,
            Tone::Danger => palette.danger.base.color,
        }
    }

    /// The tone of a MAVLink severity.
    pub fn of_severity(severity: MavSeverity) -> Tone {
        match severity {
            MavSeverity::Emergency
            | MavSeverity::Alert
            | MavSeverity::Critical
            | MavSeverity::Error => Tone::Danger,
            MavSeverity::Warning | MavSeverity::Notice => Tone::Warning,
            MavSeverity::Info => Tone::Accent,
            MavSeverity::Debug => Tone::Muted,
        }
    }
}

/// Text in `tone`.
pub fn toned(tone: Tone) -> impl Fn(&Theme) -> text::Style {
    move |theme| text::Style {
        color: Some(tone.color(theme.palette())),
        ..text::Style::default()
    }
}

/// A plain container with a background, at `pair`.
fn surface(pair: iced::theme::palette::Pair) -> container::Style {
    container::Style {
        background: Some(pair.color.into()),
        text_color: Some(pair.text),
        ..container::Style::default()
    }
}

/// A card inside a panel, grouping related content: an outline on the
/// pane's own surface, so the controls in it look as they do outside.
pub fn card(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        border: Border {
            width: 1.0,
            radius: WIDGET_RADIUS.into(),
            color: palette.background.neutral.color,
        },
        ..container::Style::default()
    }
}

/// The rule between the rows of a card, in the card's border colour so it
/// meets the border without a seam.
pub fn card_divider(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme.palette().background.neutral.color,
        ..rule::default(theme)
    }
}

/// The window behind the panes; its colour shows in the gaps between them.
pub fn workspace(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.palette().background.base.color.into()),
        ..container::Style::default()
    }
}

/// A regular pane: one solid surface with a rounded border. The body is
/// transparent against this backdrop and a `rule` divides it from the header.
pub fn pane(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        border: Border {
            width: PANE_BORDER_WIDTH,
            radius: Radius::new(PANE_RADIUS),
            color: palette.background.strongest.color,
        },
        ..surface(palette.background.weakest)
    }
}

/// The header of a pane, with rounded top corners so it sits flush inside
/// the pane border. The focused pane's header is a shade brighter.
pub fn title_bar(focused: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.palette();
        let pair = if focused {
            palette.background.weak
        } else {
            palette.background.weaker
        };

        container::Style {
            border: Border {
                radius: Radius::default().top(PANE_RADIUS),
                ..Border::default()
            },
            ..surface(pair)
        }
    }
}

/// The pane grid: its defaults, with drop highlights rounded like the
/// panes they stand for.
pub fn pane_grid(theme: &Theme) -> crate::widget::pane_grid::Style {
    let mut style = crate::widget::pane_grid::default(theme);
    style.hovered_region.border.radius = PANE_RADIUS.into();
    style
}

/// The `rule` separating a pane's header from its body. It shares the border
/// colour, so the divider meets the pane outline without a seam.
pub fn pane_divider(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme.palette().background.strongest.color,
        ..rule::default(theme)
    }
}

/// A list row; odd rows are a shade brighter, so long rows stay legible.
pub fn list_row(index: usize) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.palette();
        surface(if index.is_multiple_of(2) {
            palette.background.weakest
        } else {
            palette.background.weaker
        })
    }
}

/// A meter (a progress bar showing a level), filled in `tone`.
pub fn meter(tone: Tone) -> impl Fn(&Theme) -> widget::progress_bar::Style {
    move |theme| {
        let palette = theme.palette();
        widget::progress_bar::Style {
            background: palette.background.weak.color.into(),
            bar: tone.color(palette).into(),
            border: Border {
                radius: WIDGET_RADIUS.into(),
                ..Border::default()
            },
        }
    }
}

/// A floating tooltip.
pub fn tooltip(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        border: Border {
            width: 1.0,
            radius: 6.0.into(),
            color: palette.background.strongest.color,
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.2),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 6.0,
        },
        ..surface(palette.background.weak)
    }
}

/// How much of its tone a tinted surface (a toast, a banner) takes.
const TINT: f32 = 0.12;
/// How much of its tone a badge takes: more than a surface, as it is small.
const BADGE_TINT: f32 = 0.22;

/// A status badge: a small pill tinted with its tone, its text in it.
pub fn badge(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.palette();
        let color = tone.color(palette);

        container::Style {
            background: Some(mix(palette.background.weakest.color, color, BADGE_TINT).into()),
            text_color: Some(color),
            border: Border {
                radius: WIDGET_RADIUS.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
    }
}

/// A banner: a message across a panel, on a surface tinted with its tone
/// and bordered in it.
pub fn banner(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.palette();
        let color = tone.color(palette);
        let surface_pair = palette.background.weakest;

        container::Style {
            background: Some(mix(surface_pair.color, color, TINT).into()),
            text_color: Some(surface_pair.text),
            border: Border {
                width: 1.0,
                radius: WIDGET_RADIUS.into(),
                color,
            },
            ..container::Style::default()
        }
    }
}

/// A notification toast, accented by its tone: bordered in it, on a
/// background tinted with it. The background stays opaque, since toasts
/// float over the map.
pub fn toast(tone: Tone) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.palette();
        let surface_pair = palette.background.weak;

        container::Style {
            background: Some(mix(surface_pair.color, tone.color(palette), TINT).into()),
            border: Border {
                width: 2.0,
                radius: PANE_RADIUS.into(),
                color: tone.color(palette),
            },
            shadow: Shadow {
                color: Color::BLACK.scale_alpha(0.3),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 8.0,
            },
            ..surface(surface_pair)
        }
    }
}

/// `base` with `amount` (0 to 1) of `tint` mixed in, opaque.
fn mix(base: Color, tint: Color, amount: f32) -> Color {
    let channel = |base: f32, tint: f32| base + (tint - base) * amount;
    Color::from_rgb(
        channel(base.r, tint.r),
        channel(base.g, tint.g),
        channel(base.b, tint.b),
    )
}

/// A modal dialog.
pub fn dialog(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        border: Border {
            width: 1.0,
            radius: PANE_RADIUS.into(),
            color: palette.background.strongest.color,
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.4),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 16.0,
        },
        ..surface(palette.background.weakest)
    }
}

/// The dimmed backdrop behind a modal dialog.
pub fn backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::BLACK.scale_alpha(0.45).into()),
        ..container::Style::default()
    }
}

/// A pick list with the controls' corner radius.
pub fn pick_list(theme: &Theme, status: widget::pick_list::Status) -> widget::pick_list::Style {
    let mut style = widget::pick_list::default(theme, status);
    style.border.radius = WIDGET_RADIUS.into();
    style
}

/// A text input with the controls' corner radius.
pub fn text_input(theme: &Theme, status: widget::text_input::Status) -> widget::text_input::Style {
    let mut style = widget::text_input::default(theme, status);
    style.border.radius = WIDGET_RADIUS.into();
    style
}

/// A text input marked in `tone`: a coloured border over a tinted field.
pub fn marked_text_input(
    tone: Tone,
) -> impl Fn(&Theme, widget::text_input::Status) -> widget::text_input::Style {
    move |theme, status| {
        let color = tone.color(theme.palette());
        let mut style = text_input(theme, status);
        style.border.color = color;
        style.border.width = 2.0;
        style.background = color.scale_alpha(0.1).into();
        style
    }
}

/// The kinds of button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// Most actions: a quiet surface that lights up on hover.
    #[default]
    Subtle,
    /// The action a view is for, or a chosen option.
    Primary,
    /// An action that replaces something stored.
    Warning,
    /// An action that stops or removes something.
    Danger,
}

/// A button of `kind`, with the controls' corner radius.
pub fn button(
    kind: ButtonKind,
) -> impl Fn(&Theme, widget::button::Status) -> widget::button::Style {
    move |theme, status| {
        let mut style = match kind {
            ButtonKind::Subtle => subtle_button(theme, status),
            ButtonKind::Primary => widget::button::primary(theme, status),
            ButtonKind::Warning => widget::button::warning(theme, status),
            ButtonKind::Danger => widget::button::danger(theme, status),
        };
        style.border.radius = WIDGET_RADIUS.into();
        style
    }
}

fn subtle_button(theme: &Theme, status: widget::button::Status) -> widget::button::Style {
    use widget::button::Status;

    let palette = theme.palette();
    let (background, border) = match status {
        Status::Active | Status::Pressed => (
            Some(palette.background.weaker.color.into()),
            Color::TRANSPARENT,
        ),
        Status::Hovered => (
            Some(palette.background.weak.color.into()),
            palette.background.neutral.color,
        ),
        Status::Disabled => (None, Color::TRANSPARENT),
    };
    let text_color = match status {
        Status::Disabled => palette.background.weaker.text.scale_alpha(0.5),
        _ => palette.background.weaker.text,
    };

    widget::button::Style {
        background,
        text_color,
        border: Border {
            color: border,
            width: 1.0,
            radius: WIDGET_RADIUS.into(),
        },
        ..widget::button::Style::default()
    }
}

/// A button on the rail: no background while its panel is closed, a
/// stronger one while it is open, and the panes' rounder corners, since it
/// floats on the window's background like they do.
pub fn rail_button(open: bool) -> impl Fn(&Theme, widget::button::Status) -> widget::button::Style {
    use widget::button::Status;

    move |theme, status| {
        let palette = theme.palette();
        let (rest, hover) = if open {
            (Some(palette.background.neutral), palette.background.strong)
        } else {
            (None, palette.background.weak)
        };
        let background = match status {
            Status::Hovered => Some(hover),
            _ => rest,
        };

        widget::button::Style {
            background: background.map(|pair| pair.color.into()),
            text_color: palette.background.base.text,
            border: Border {
                radius: PANE_RADIUS.into(),
                ..Border::default()
            },
            ..widget::button::Style::default()
        }
    }
}

/// An icon, in the text colour of `kind`'s button, or faded while `enabled`
/// is false.
pub fn icon(kind: ButtonKind, enabled: bool) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |theme, _| {
        let palette = theme.palette();
        let color = match (enabled, kind) {
            (false, _) => palette.background.strongest.color,
            (true, ButtonKind::Subtle) => palette.background.base.text,
            (true, ButtonKind::Primary) => palette.primary.base.text,
            (true, ButtonKind::Warning) => palette.warning.base.text,
            (true, ButtonKind::Danger) => palette.danger.base.text,
        };
        svg::Style { color: Some(color) }
    }
}

// Map overlay colors.
//
// The overlay is drawn on top of the opaque OpenStreetMap tiles, not on the
// themed pane surface, so it is deliberately theme-independent: a single fixed
// palette keeps waypoints and the route legible over any tile. These are the
// one control point for the overlay's look.

/// The stroke connecting consecutive positioned mission items.
pub fn map_route() -> Color {
    Color::WHITE
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

/// The selected or hovered mission marker.
pub fn marker_selected() -> Color {
    Color::from_rgb(0.45, 0.75, 1.0)
}

/// A vehicle's home marker.
pub fn map_home() -> Color {
    Color::from_rgb(1.0, 0.85, 0.2)
}
