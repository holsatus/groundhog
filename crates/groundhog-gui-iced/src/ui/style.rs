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
use std::sync::atomic::{AtomicU8, Ordering};

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

/// How large the interactive parts are drawn: for a pointer, or for a
/// finger. Text keeps its size either way; that is the scale factor's job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Density {
    #[default]
    Pointer,
    Touch,
}

/// The density views are built at. iced gives a view no context to read it
/// from, so the application sets it before each view, which it builds on
/// one thread.
static DENSITY: AtomicU8 = AtomicU8::new(Density::Pointer as u8);

/// Build the views that follow at `density`.
pub fn set_density(density: Density) {
    DENSITY.store(density as u8, Ordering::Relaxed);
}

/// The density views are being built at.
pub fn density() -> Density {
    if DENSITY.load(Ordering::Relaxed) == Density::Touch as u8 {
        Density::Touch
    } else {
        Density::Pointer
    }
}

/// `pointer` or `touch`, by the density views are being built at.
fn by_density(pointer: f32, touch: f32) -> f32 {
    match density() {
        Density::Pointer => pointer,
        Density::Touch => touch,
    }
}

/// The size of body text and the line height it is drawn at: iced's defaults,
/// set explicitly so the controls' padding (`ui::input`) is derived from the
/// same numbers the renderer uses.
pub const TEXT_SIZE: f32 = 16.0;
pub const LINE_HEIGHT: f32 = 1.375;

/// Text and icon sizes, in logical pixels.
pub mod size {
    use super::by_density;

    /// Secondary text: hints, units, details, labels of values. Running
    /// text keeps iced's default size, 16.
    pub const CAPTION: f32 = 12.0;
    /// The title of a card or a dialog.
    pub const TITLE: f32 = 18.0;
    /// An icon in a button.
    pub const ICON: f32 = 18.0;
    /// The height of a control (a button, a field, a picker) and the side
    /// of an icon button: 32 for a pointer, 44 for a finger. 44 is Apple's
    /// smallest target; Material's 48 counts the gap to the next target,
    /// which `space::S` adds.
    pub fn control() -> f32 {
        by_density(32.0, 44.0)
    }
    /// An icon on the rail.
    pub const RAIL_ICON: f32 = 27.0;
    /// The side of a button on the rail.
    pub const RAIL_BUTTON: f32 = 48.0;
    /// The width of the labels in a form, so its controls line up.
    pub const FIELD_LABEL: f32 = 140.0;
    /// The narrowest a form's control gets beside its label; any narrower,
    /// and the label goes above it.
    pub fn field_control() -> f32 {
        by_density(160.0, 200.0)
    }
    /// The height of a list row of one line of body text.
    pub fn list_row() -> f32 {
        by_density(26.0, 44.0)
    }
    /// The height of a slider: its handle's reach.
    pub fn slider() -> f32 {
        by_density(22.0, 44.0)
    }
    /// The side of a checkbox's box.
    pub fn checkbox() -> f32 {
        by_density(16.0, 22.0)
    }
    /// The height of a progress bar.
    pub const PROGRESS: f32 = 6.0;
}

/// The corner radius of a pane, shared with the map's frame.
pub const PANE_RADIUS: f32 = 12.0;
/// The corner radius of controls and cards.
pub const WIDGET_RADIUS: f32 = 6.0;
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

/// A pane floating over the map (a sheet, the navigation bar): a regular
/// pane, lifted off the tiles by a shadow.
pub fn floating(theme: &Theme) -> container::Style {
    container::Style {
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.3),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 10.0,
        },
        ..pane(theme)
    }
}

/// A wash of the window background under the platform's status bar, where
/// the map shows through it, so the bar's icons stay legible over any tile.
pub fn scrim(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(
            theme
                .palette()
                .background
                .base
                .color
                .scale_alpha(0.7)
                .into(),
        ),
        ..container::Style::default()
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
    // The panes float on the workspace's backdrop, so this veil fades a
    // dragged pane into it.
    style.picked_pane = theme.palette().background.base.color.scale_alpha(0.6);
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

/// A container drawn as a button of `kind` at rest, for what looks like a
/// button but handles its own press (a menu's trigger).
pub fn button_at_rest(kind: ButtonKind) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let style = button(kind)(theme, widget::button::Status::Active);
        container::Style {
            background: style.background,
            border: style.border,
            text_color: Some(style.text_color),
            ..container::Style::default()
        }
    }
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
    /// The action should fade into the background.
    Transparent,
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
            ButtonKind::Transparent => transparent_button(theme, status),
            ButtonKind::Primary => widget::button::primary(theme, status),
            ButtonKind::Warning => widget::button::warning(theme, status),
            ButtonKind::Danger => widget::button::danger(theme, status),
        };
        style.border.radius = WIDGET_RADIUS.into();
        style
    }
}

/// A quiet button: a surface a shade off the pane's, outlined on hover.
///
/// The border is always there, in the fill's colour when it is not shown:
/// iced paints a border in place of the fill beneath it, so a transparent
/// one would cut a ring out of the button, leaving it smaller than its
/// neighbours and its corners less round. A disabled button keeps its
/// shape too, so a row of buttons stays even.
fn subtle_button(theme: &Theme, status: widget::button::Status) -> widget::button::Style {
    use widget::button::Status;

    let palette = theme.palette();
    let fill = match status {
        Status::Hovered => palette.background.weak.color,
        Status::Active | Status::Pressed | Status::Disabled => palette.background.weaker.color,
    };
    let border = match status {
        Status::Hovered => palette.background.neutral.color,
        _ => fill,
    };
    let text_color = match status {
        Status::Disabled => palette.background.weaker.text.scale_alpha(0.5),
        _ => palette.background.weaker.text,
    };

    widget::button::Style {
        background: Some(fill.into()),
        text_color,
        border: Border {
            color: border,
            width: 1.0,
            radius: WIDGET_RADIUS.into(),
        },
        ..widget::button::Style::default()
    }
}

fn transparent_button(theme: &Theme, status: widget::button::Status) -> widget::button::Style {
    use widget::button::Status;

    let palette = theme.palette();
    let fill = match status {
        Status::Hovered => palette.background.base.text.scale_alpha(0.2),
        Status::Pressed => palette.background.base.text.scale_alpha(0.1),
        Status::Active | Status::Disabled => Color::TRANSPARENT,
    };
    let border = match status {
        Status::Hovered => palette.background.neutral.color,
        _ => fill,
    };
    let text_color = match status {
        Status::Disabled => palette.background.weaker.text.scale_alpha(0.5),
        _ => palette.background.weaker.text,
    };

    widget::button::Style {
        background: Some(fill.into()),
        text_color,
        border: Border {
            color: border,
            width: 1.0,
            radius: WIDGET_RADIUS.into(),
        },
        ..widget::button::Style::default()
    }
}

/// Behind a picture (a video), whose letterbox bands show it: black, as a
/// screen is, in either theme.
pub fn screen(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::BLACK.into()),
        text_color: Some(Color::WHITE),
        border: Border {
            radius: WIDGET_RADIUS.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A pane's body that is a picture: [`screen`], its lower corners rounded
/// as the pane's are inside its border.
pub fn picture(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            radius: Radius::default().bottom(PANE_RADIUS - PANE_BORDER_WIDTH),
            ..Border::default()
        },
        ..screen(theme)
    }
}

/// A page of the floating shell: a pane's surface, filling the window.
pub fn page(theme: &Theme) -> container::Style {
    surface(theme.palette().background.weakest)
}

/// The floating shell's navigation bar or rail, docked at the window's
/// edge: a shade apart from the page and the map beside it.
pub fn navigation(theme: &Theme) -> container::Style {
    surface(theme.palette().background.weaker)
}

/// An item of the navigation bar: no background of its own; its icon's
/// indicator shows which is chosen.
pub fn navigation_item(theme: &Theme, status: widget::button::Status) -> widget::button::Style {
    let palette = theme.palette();
    widget::button::Style {
        background: matches!(status, widget::button::Status::Pressed)
            .then(|| palette.background.weak.color.into()),
        text_color: palette.background.base.text,
        border: Border {
            radius: PANE_RADIUS.into(),
            ..Border::default()
        },
        ..widget::button::Style::default()
    }
}

/// A button with no look of its own, laid over what it acts on (a
/// picture-in-picture, tapped to swap it).
pub fn invisible_button(_theme: &Theme, _status: widget::button::Status) -> widget::button::Style {
    widget::button::Style {
        background: None,
        ..widget::button::Style::default()
    }
}

/// The pill behind a navigation item's icon, filled while it is chosen.
pub fn navigation_indicator(chosen: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.palette();
        container::Style {
            background: chosen.then(|| palette.background.neutral.color.into()),
            border: Border {
                radius: 16.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        }
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
            (true, ButtonKind::Transparent) => palette.background.base.text,
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
