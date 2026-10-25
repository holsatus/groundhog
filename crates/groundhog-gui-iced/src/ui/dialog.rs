//! A modal dialog over a dimmed backdrop.
//!
//! The dialog fits the window it is drawn in: it fills the width up to
//! [`MAX_WIDTH`], keeps clear of the platform's chrome (`insets`), and
//! scrolls its body when the window is too short for it, so its buttons stay
//! on screen. On a narrow window the buttons stack.

use iced::{
    Alignment, Element, Length, Padding, Widget,
    widget::{column, container, mouse_area, opaque, responsive, scrollable, space, stack},
};

use crate::ui::{self, style};

/// The widest the dialog grows: a readable line length, not the window.
const MAX_WIDTH: f32 = 420.0;

/// Below this height the dialog takes the window's height and scrolls its
/// body; above it, the dialog is as tall as its content.
const SHORT_WINDOW: f32 = 480.0;

/// A modal question: what it says, and what its two answers send.
pub struct Dialog<'a, M> {
    pub title: &'a str,
    pub body: &'a str,
    pub accept_label: &'a str,
    /// Styles the accept button as dangerous.
    pub danger: bool,
    pub accept: M,
    pub dismiss: M,
}

/// `base` with `dialog` on top of a backdrop. Pressing the backdrop or Cancel
/// dismisses it.
pub fn view<'a, M: Clone + 'a>(
    base: Element<'a, M>,
    dialog: Dialog<'a, M>,
    insets: Padding,
) -> Element<'a, M> {
    let backdrop = mouse_area(
        container(space::horizontal())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::backdrop),
    )
    .on_press(dialog.dismiss.clone());

    let stage = responsive(move |size| {
        let width =
            (size.width - insets.left - insets.right - ui::space::L * 2.0).clamp(0.0, MAX_WIDTH);
        let short = size.height < SHORT_WINDOW;

        let cancel = ui::button("Cancel")
            .on_press(dialog.dismiss.clone())
            .boxed();
        let accept = ui::button(dialog.accept_label)
            .kind(if dialog.danger {
                style::ButtonKind::Danger
            } else {
                style::ButtonKind::Primary
            })
            .on_press(dialog.accept.clone())
            .boxed();
        // Separate groups wrap whole, so the buttons stack side by side
        // where they fit and on top of each other where they do not.
        let buttons = container(ui::actions([vec![cancel], vec![accept]]))
            .width(Length::Fill)
            .align_x(Alignment::End);

        let body = scrollable(ui::text(dialog.body));
        let body = if short {
            body.height(Length::Fill)
        } else {
            body.height(Length::Shrink)
        };

        let panel =
            container(column![ui::title(dialog.title), body, buttons].spacing(ui::space::L))
                .padding(ui::space::L)
                .width(Length::Fixed(width))
                .style(style::dialog);
        let panel = if short {
            panel.height(Length::Fill)
        } else {
            panel.height(Length::Shrink)
        };

        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(Padding {
                top: insets.top + ui::space::L,
                right: insets.right + ui::space::L,
                bottom: insets.bottom + ui::space::L,
                left: insets.left + ui::space::L,
            })
            .boxed()
    });

    stack![base, opaque(backdrop), opaque(stage)].boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Size;

    #[derive(Debug, Clone, PartialEq)]
    enum Message {
        Accept,
        Dismiss,
    }

    fn element() -> Element<'static, Message> {
        // The application's base is its shell, which fills the window; a
        // `stack` sizes its layers by its base layer.
        let base = container(ui::text("the panel behind"))
            .width(Length::Fill)
            .height(Length::Fill)
            .boxed();

        view(
            base,
            Dialog {
                title: "Clear the mission?",
                body: "This deletes the mission stored on the vehicle. It cannot be undone \
                       once the vehicle has accepted the clear.",
                accept_label: "Clear",
                danger: true,
                accept: Message::Accept,
                dismiss: Message::Dismiss,
            },
            Padding::ZERO,
        )
    }

    /// The accept button is wholly inside `size`, and pressing it answers.
    fn assert_fits(size: Size) {
        let mut simulator = iced_test::Simulator::with_size(Default::default(), size, element());

        let accept = simulator
            .find("Clear")
            .expect("the accept button is visible");
        let bounds = accept.visible_bounds().expect("the button is visible");
        assert!(
            bounds.x >= 0.0 && bounds.y >= 0.0,
            "the button starts inside the window: {bounds:?}"
        );
        assert!(
            bounds.x + bounds.width <= size.width,
            "the button ends inside the window: {bounds:?} in {size:?}"
        );
        assert!(
            bounds.y + bounds.height <= size.height,
            "the button ends inside the window: {bounds:?} in {size:?}"
        );

        simulator.click("Clear").expect("the button is clickable");
        let messages: Vec<Message> = simulator.into_messages().collect();
        assert_eq!(messages, vec![Message::Accept]);
    }

    #[test]
    fn the_dialog_fits_a_phone() {
        assert_fits(Size::new(400.0, 700.0));
    }

    #[test]
    fn the_dialog_fits_a_short_landscape_window() {
        assert_fits(Size::new(700.0, 320.0));
    }
}
