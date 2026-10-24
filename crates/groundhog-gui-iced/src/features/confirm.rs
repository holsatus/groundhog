//! Confirmation of consequential actions.
//!
//! Commands that change what a vehicle does (arming, starting a mission,
//! jumping to an item, clearing its mission, changing its mode in flight)
//! are never sent on a single click, nor are destructive changes to what is
//! stored (deleting a saved link). The feature asks for a [`Confirmation`];
//! the application shows it as a modal dialog and dispatches the
//! confirmation's message only if the operator accepts.

use iced::{
    Alignment, Element, Length, Widget,
    widget::{center, column, container, mouse_area, opaque, row, space, stack},
};

use crate::ui::{self, style};

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

    let accept = ui::button(confirmation.accept_label)
        .kind(if confirmation.danger {
            style::ButtonKind::Danger
        } else {
            style::ButtonKind::Primary
        })
        .on_press(answer(Answer::Accept));

    let dialog = container(
        column![
            ui::title(&confirmation.title),
            ui::text(&confirmation.body),
            row![
                space::horizontal(),
                ui::button("Cancel")
                    .on_press(answer(Answer::Dismiss))
                    .boxed(),
                accept.boxed(),
            ]
            .spacing(ui::space::M)
            .align_y(Alignment::Center),
        ]
        .spacing(ui::space::L),
    )
    .padding(ui::space::XL)
    .width(Length::Fixed(420.0))
    .style(style::dialog);

    let backdrop = mouse_area(
        container(space::horizontal())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::backdrop),
    )
    .on_press(answer(Answer::Dismiss));

    stack![base, opaque(backdrop), center(opaque(dialog))].boxed()
}
