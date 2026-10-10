//! Shared container styles, all derived from the active [`Theme`](iced::Theme).

use iced::{widget::container, Border, Color, Shadow, Theme, Vector};

pub fn shaded_bordered_box(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
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

pub fn list_brighter(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weaker.color.into()),
        text_color: Some(palette.background.weakest.text),
        ..container::Style::default()
    }
}

pub fn list_default(theme: &Theme) -> container::Style {
    let palette = theme.palette();

    container::Style {
        background: Some(palette.background.weakest.color.into()),
        text_color: Some(palette.background.weakest.text),
        ..container::Style::default()
    }
}

#[allow(dead_code)]
pub fn darken(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::BLACK.scale_alpha(0.5).into()),
        text_color: None,
        ..container::Style::default()
    }
}
