//! Toast cards and the layer that places them.

use iced::{
    Alignment, Length, Padding, Widget,
    widget::{Container, column, container, row},
};

use crate::{
    ui::{self, Icon, Tone, style},
    widget::surface::surface,
};

/// How wide a toast grows. It is narrow enough for a phone in either
/// orientation, and does not stretch across a desktop window.
const WIDTH: f32 = 340.0;

/// One notice: its title, an optional detail, and the message that dismisses
/// it.
pub fn toast<'a, M: Clone + 'a>(
    title: &'a str,
    detail: Option<&'a str>,
    tone: Tone,
    dismiss: M,
) -> impl Widget<M> + 'a {
    let body = column![ui::text(title)]
        .push(detail.map(ui::caption).boxed())
        .spacing(ui::space::XXS)
        .width(Length::Fill);

    let card = container(
        row![
            body,
            ui::button::transparent_icon_button(Icon::Close)
                .on_press(dismiss)
                .boxed()
        ]
        .spacing(ui::space::M)
        .align_y(Alignment::Center),
    )
    .padding(Padding::new(ui::space::S).left(ui::space::L))
    .width(Length::Fixed(WIDTH))
    .style(style::toast(tone));

    // A toast is opaque to the pointer and to fingers: what lies beneath (the
    // map) is not hovered, zoomed or dragged through it.
    surface(card)
}

/// `content` in the bottom-right corner of the window, `insets` clear of the
/// platform's chrome.
pub fn layer<'a, W>(content: W, insets: Padding) -> Container<'a, W> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_right(Length::Fill)
        .align_bottom(Length::Fill)
        .padding(insets)
}
