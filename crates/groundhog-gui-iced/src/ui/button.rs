//! Buttons: a label or an icon, a kind, and what pressing does.
//!
//! [`button`] and [`icon_button`] start an [`Action`], which the call site
//! configures and finishes with [`Action::boxed`]:
//!
//! ```ignore
//! ui::button("Arm")
//!     .primary()
//!     .busy(arming, "Arming…")
//!     .on_press(Message::Arm)
//!     .boxed()
//! ```

use iced::{
    Element, Length, Widget,
    widget::{container, text::IntoFragment, tooltip::Position},
};

use super::{
    container::tooltip,
    icon::{self, Icon},
    style::{self, ButtonKind, size},
    text::text,
};

/// What a button shows.
enum Content<'a, M> {
    Label(iced::widget::text::Fragment<'a>),
    Icon(Icon),
    Element(Element<'a, M>),
}

/// A button being built; [`Action::boxed`] finishes it.
pub struct Action<'a, M> {
    content: Content<'a, M>,
    kind: ButtonKind,
    on_press: Option<M>,
    width: Length,
    busy: Option<iced::widget::text::Fragment<'a>>,
    hint: Option<iced::widget::text::Fragment<'a>>,
}

/// A button labelled `label`.
pub fn button<'a, M>(label: impl IntoFragment<'a>) -> Action<'a, M> {
    Action::new(Content::Label(label.into_fragment()))
}

/// A square button showing `icon`. Give it a [`hint`](Action::hint): an icon
/// alone rarely says enough.
pub fn icon_button<'a, M>(icon: Icon) -> Action<'a, M> {
    Action::new(Content::Icon(icon))
}

/// A button around arbitrary `content`, such as a list row.
pub fn button_with<'a, M>(content: impl Widget<M> + 'a) -> Action<'a, M> {
    Action::new(Content::Element(content.boxed()))
}

impl<'a, M> Action<'a, M> {
    fn new(content: Content<'a, M>) -> Self {
        Self {
            content,
            kind: ButtonKind::Subtle,
            on_press: None,
            width: Length::Shrink,
            busy: None,
            hint: None,
        }
    }

    /// The action a view is for.
    pub fn primary(self) -> Self {
        self.kind(ButtonKind::Primary)
    }

    /// An action that replaces something stored.
    pub fn warning(self) -> Self {
        self.kind(ButtonKind::Warning)
    }

    /// An action that stops or removes something.
    pub fn danger(self) -> Self {
        self.kind(ButtonKind::Danger)
    }

    /// Highlight the button while `chosen`: the selected row, a mode that
    /// is on.
    pub fn chosen(self, chosen: bool) -> Self {
        if chosen {
            self.primary()
        } else {
            self.kind(ButtonKind::Subtle)
        }
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn on_press(mut self, message: M) -> Self {
        self.on_press = Some(message);
        self
    }

    /// Press sends `message`; `None` disables the button.
    pub fn on_press_maybe(mut self, message: Option<M>) -> Self {
        self.on_press = message;
        self
    }

    /// While `busy`, show `label` instead and refuse presses, so an
    /// operation in flight is neither hidden nor started twice.
    pub fn busy(mut self, busy: bool, label: impl IntoFragment<'a>) -> Self {
        self.busy = busy.then(|| label.into_fragment());
        self
    }

    /// A tooltip explaining the button, above it.
    pub fn hint(mut self, hint: impl IntoFragment<'a>) -> Self {
        self.hint = Some(hint.into_fragment());
        self
    }

    /// The button's width; a label is centred in a fixed or filling width.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn boxed(self) -> Element<'a, M>
    where
        M: Clone + 'a,
    {
        let Action {
            content,
            kind,
            on_press,
            width,
            busy,
            hint,
        } = self;

        let on_press = on_press.filter(|_| busy.is_none());
        let enabled = on_press.is_some();

        let button = match content {
            Content::Label(label) => {
                let label = text(busy.unwrap_or(label));
                let content = if width == Length::Shrink {
                    label.boxed()
                } else {
                    container(label).center_x(Length::Fill).boxed()
                };
                iced::widget::button(content).width(width)
            }
            Content::Icon(glyph) => iced::widget::button(
                container(icon::icon(glyph, size::ICON, kind, enabled))
                    .center(Length::Fill)
                    .boxed(),
            )
            .padding(0)
            .width(size::ICON_BUTTON)
            .height(size::ICON_BUTTON),
            Content::Element(content) => iced::widget::button(content).width(width),
        };

        let button = button.style(style::button(kind)).on_press_maybe(on_press);

        match hint {
            Some(hint) => tooltip(button, hint, Position::Top),
            None => button.boxed(),
        }
    }
}
