//! Shared container styles, all derived from the active [`Theme`](iced::Theme).
//!
//! The look is deliberately flat: panes and their title bars sit directly on
//! the window, and separation comes from thin gaps and subtle background
//! shifts rather than borders, shadows, or nested cards.

use iced::{Border, Color, Shadow, Theme, Vector, widget::container};

/// A pane's body surface, a hair lighter than the window background.
pub fn pane(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
        ..container::Style::default()
    }
}

/// The title bar of an unfocused pane.
pub fn title_bar(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weak.color.into()),
        text_color: Some(palette.background.weak.text),
        ..container::Style::default()
    }
}

/// The title bar of the focused pane.
pub fn title_bar_focused(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.strong.color.into()),
        text_color: Some(palette.background.strong.text),
        ..container::Style::default()
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

/// A floating tooltip: the only rounded, bordered surface in the app.
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
