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
//!
//! Labelled and icon buttons share one height, so a row of them lines up.
//! A label is one line: a button says a verb, not a sentence.

use iced::{
    Alignment, Element, Length, Widget,
    widget::{Button, Row, container, row, text::IntoFragment, tooltip::Position},
};

use super::{
    container::tooltip,
    icon::{self, Icon},
    style::{self, ButtonKind, size, space},
    text::{TextExt, text},
};

/// What a button shows.
enum Content<'a, M> {
    /// A label, after an icon if it has one.
    Label(iced::widget::text::Fragment<'a>, Option<Icon>),
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
    Action::new(Content::Label(label.into_fragment(), None))
}

/// A square button showing `icon`. Give it a [`hint`](Action::hint): an icon
/// alone rarely says enough.
pub fn icon_button<'a, M>(icon: Icon) -> Action<'a, M> {
    Action::new(Content::Icon(icon))
}

/// A square button showing `icon`. Give it a [`hint`](Action::hint): an icon
/// alone rarely says enough.
pub fn transparent_icon_button<'a, M>(icon: Icon) -> Action<'a, M> {
    let mut action = Action::new(Content::Icon(icon));
    action.kind = ButtonKind::Transparent;
    action
}

/// A row that opens what it names: `icon`, `title` and a line about it,
/// and a chevron. Two lines high, so a finger finds it at any density.
pub fn entry<'a, M: Clone + 'a>(
    glyph: Icon,
    title: impl IntoFragment<'a>,
    detail: impl IntoFragment<'a>,
    on_press: M,
) -> Button<'a, M, Row<Element<'a, M>>> {
    use super::text::caption;

    let content = row![
        icon::icon(glyph, size::ICON, ButtonKind::Subtle, true),
        iced::widget::column![
            text(title).single_line(),
            caption(detail).tone(super::Tone::Muted),
        ]
        .spacing(space::XXS)
        .width(Length::Fill),
        icon::icon(Icon::Forward, size::ICON, ButtonKind::Subtle, true),
    ]
    .spacing(space::L)
    .align_y(Alignment::Center);

    iced::widget::button(content)
        .padding([space::M, space::M])
        .width(Length::Fill)
        .style(style::button(ButtonKind::Subtle))
        .on_press(on_press)
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

    /// Show `icon` before the label. An icon helps a verb that recurs
    /// across panels (download, upload, add) to be found at a glance.
    pub fn icon(mut self, glyph: Icon) -> Self {
        if let Content::Label(label, _) = self.content {
            self.content = Content::Label(label, Some(glyph));
        }
        self
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
            Content::Label(label, glyph) => {
                let label = row![
                    glyph.map(|glyph| icon::icon(glyph, size::ICON, kind, enabled)),
                    text(busy.unwrap_or(label)).single_line()
                ]
                .spacing(space::S)
                .align_y(Alignment::Center);
                let content = if width == Length::Shrink {
                    container(label).center_y(Length::Fill)
                } else {
                    container(label).center(Length::Fill)
                };
                iced::widget::button(content.boxed())
                    .width(width)
                    .height(size::control())
                    .padding([0.0, space::M])
            }
            Content::Icon(glyph) => iced::widget::button(
                container(icon::icon(glyph, size::ICON, kind, enabled))
                    .center(Length::Fill)
                    .boxed(),
            )
            .padding(0)
            .width(size::control())
            .height(size::control()),
            Content::Element(content) => iced::widget::button(content).width(width),
        };

        let button = button.style(style::button(kind)).on_press_maybe(on_press);

        match hint {
            Some(hint) => tooltip(button, hint, Position::Top),
            None => button.boxed(),
        }
    }
}
