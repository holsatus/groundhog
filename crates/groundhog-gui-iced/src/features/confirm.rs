//! Confirmation of consequential actions.
//!
//! Commands that change what a vehicle does (arming, starting a mission,
//! jumping to an item, clearing its mission, changing its mode in flight)
//! are never sent on a single click, nor are destructive changes to what is
//! stored (deleting a saved link). The feature asks for a [`Confirmation`];
//! the application shows it as a modal dialog and dispatches the
//! confirmation's message only if the operator accepts.

use iced::{Element, Padding};

use crate::ui;

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

/// `base` with the confirmation dialog on top, if one is pending. Pressing
/// the backdrop or Cancel dismisses it.
pub fn view<'a, M: Clone + 'a>(
    base: Element<'a, M>,
    confirmation: Option<&'a Confirmation<impl Sized>>,
    answer: impl Fn(Answer) -> M + 'a,
    insets: Padding,
) -> Element<'a, M> {
    let Some(confirmation) = confirmation else {
        return base;
    };

    ui::dialog::view(
        base,
        ui::dialog::Dialog {
            title: &confirmation.title,
            body: &confirmation.body,
            accept_label: confirmation.accept_label,
            danger: confirmation.danger,
            accept: answer(Answer::Accept),
            dismiss: answer(Answer::Dismiss),
        },
        insets,
    )
}
