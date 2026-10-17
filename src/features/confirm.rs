//! Confirmation of consequential actions.
//!
//! Commands that change what a vehicle does (arming, starting a mission,
//! jumping to an item, clearing its mission) are never sent on a single
//! click. The feature asks for a [`Confirmation`]; the application shows it as
//! a modal dialog and dispatches the confirmation's message only if the
//! operator accepts.

use iced::{
    Alignment, Element, Length, Theme, Widget,
    widget::{button, center, column, container, mouse_area, opaque, row, space, stack, text},
};

use crate::style;

/// A question to put to the operator, and the message to send on "yes".
#[derive(Debug, Clone)]
pub struct Confirmation<M> {
    pub title: String,
    pub body: String,
    pub accept_label: &'static str,
    /// Styles the accept button as dangerous.
    pub danger: bool,
    pub accept: M,
}

impl<M> Confirmation<M> {
    pub fn new(title: impl Into<String>, body: impl Into<String>, accept: M) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            accept_label: "Confirm",
            danger: false,
            accept,
        }
    }

    pub fn accept_label(mut self, label: &'static str) -> Self {
        self.accept_label = label;
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn map<N>(self, map: impl FnOnce(M) -> N) -> Confirmation<N> {
        Confirmation {
            title: self.title,
            body: self.body,
            accept_label: self.accept_label,
            danger: self.danger,
            accept: map(self.accept),
        }
    }
}

/// The operator's answer.
#[derive(Debug, Clone, Copy)]
pub enum Answer {
    Accept,
    Dismiss,
}

/// `base` with the confirmation dialog on top, if one is pending. Clicking
/// the backdrop dismisses it.
pub fn view<'a, M: Clone + 'a>(
    base: Element<'a, M>,
    confirmation: Option<&'a Confirmation<impl Sized>>,
    answer: impl Fn(Answer) -> M + 'a,
) -> Element<'a, M> {
    let Some(confirmation) = confirmation else {
        return base;
    };

    let accept_style = if confirmation.danger {
        button::danger
    } else {
        button::primary
    };

    let dialog = container(
        column![
            text(&confirmation.title).size(18),
            text(&confirmation.body),
            row![
                space::horizontal(),
                button(text("Cancel"))
                    .style(button::secondary)
                    .on_press(answer(Answer::Dismiss)),
                button(text(confirmation.accept_label))
                    .style(accept_style)
                    .on_press(answer(Answer::Accept)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(12),
    )
    .padding(16)
    .width(Length::Fixed(420.0))
    .style(style::dialog);

    let backdrop = mouse_area(
        container(space::horizontal())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme: &Theme| style::backdrop(theme)),
    )
    .on_press(answer(Answer::Dismiss));

    stack![base, opaque(backdrop), center(opaque(dialog))].boxed()
}
