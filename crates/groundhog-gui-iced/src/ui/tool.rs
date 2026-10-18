//! Tools: actions on what a panel holds as a whole (a file to open, an
//! item to add), and the header that shows them.
//!
//! A tool is an icon and a label. Where the header has room, it is a quiet
//! icon button, its label the hint; where it has not, the tools fold into a
//! menu that lists them labelled, which is also how a touch screen, without
//! hover to show a hint, learns what each one does.

use iced::{
    Alignment, Element, Length, Widget,
    widget::{button, column, container, row, text::IntoFragment},
};

use super::{
    button::icon_button,
    icon::{self, Icon},
    style::{self, ButtonKind, size, space},
    text::{TextExt, text},
};
use crate::widget::{adaptive, menu::menu_button};

/// The narrowest a header's subject gets before its tools fold into a menu.
const MIN_SUBJECT: f32 = 140.0;

/// The width of a menu of tools.
const MENU_WIDTH: f32 = 240.0;

/// An action on what a panel holds as a whole; see the module docs.
pub struct Tool<'a, M> {
    icon: Icon,
    label: iced::widget::text::Fragment<'a>,
    on_press: Option<M>,
    chosen: bool,
}

/// A tool showing `icon`, called `label`.
pub fn tool<'a, M>(icon: Icon, label: impl IntoFragment<'a>) -> Tool<'a, M> {
    Tool {
        icon,
        label: label.into_fragment(),
        on_press: None,
        chosen: false,
    }
}

impl<'a, M: Clone + 'a> Tool<'a, M> {
    pub fn on_press(mut self, message: M) -> Self {
        self.on_press = Some(message);
        self
    }

    pub fn on_press_maybe(mut self, message: Option<M>) -> Self {
        self.on_press = message;
        self
    }

    /// Highlight the tool while a mode it switches is on.
    pub fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    fn kind(&self) -> ButtonKind {
        if self.chosen {
            ButtonKind::Primary
        } else {
            ButtonKind::Subtle
        }
    }

    /// The tool as an icon button, its label the hint.
    fn button(&self) -> Element<'a, M> {
        icon_button(self.icon)
            .kind(self.kind())
            .hint(self.label.clone())
            .on_press_maybe(self.on_press.clone())
            .boxed()
    }

    /// The tool as an entry of a menu: its icon and its label.
    fn entry(&self) -> Element<'a, M> {
        let enabled = self.on_press.is_some();
        let kind = self.kind();
        let content = row![
            icon::icon(self.icon, size::ICON, kind, enabled),
            text(self.label.clone()).single_line()
        ]
        .spacing(space::M)
        .align_y(Alignment::Center);

        button(container(content).center_y(Length::Fill))
            .padding([0.0, space::M])
            .width(Length::Fill)
            .height(size::control())
            .style(style::button(kind))
            .on_press_maybe(self.on_press.clone())
            .boxed()
    }
}

/// What a panel is about, on one line: `subject` (a picker, a name) fills
/// it, `status` (badges, a count) sits at its end, and then `tools`, in
/// groups. Where the tools would leave the subject too little room, they
/// fold into a menu.
pub fn header<'a, M: Clone + 'a>(
    subject: impl Widget<M> + 'a,
    status: impl IntoIterator<Item = Element<'a, M>>,
    tools: impl IntoIterator<Item = Vec<Tool<'a, M>>>,
) -> Element<'a, M> {
    let groups: Vec<Vec<Tool<'a, M>>> = tools
        .into_iter()
        .filter(|group| !group.is_empty())
        .collect();

    let status = row(status).spacing(space::S).align_y(Alignment::Center);
    let buttons = row(groups.iter().map(|group| {
        row(group.iter().map(Tool::button))
            .spacing(space::XS)
            .boxed()
    }))
    .spacing(space::L)
    .align_y(Alignment::Center);

    let menu: Element<'a, M> = if groups.is_empty() {
        iced::widget::Space::new().boxed()
    } else {
        let entries = groups.iter().flatten().map(Tool::entry);
        let trigger = container(icon::icon(Icon::More, size::ICON, ButtonKind::Subtle, true))
            .center(size::control())
            .style(style::button_at_rest(ButtonKind::Subtle));
        let list = container(column(entries).spacing(space::XXS))
            .padding(space::XS)
            .width(MENU_WIDTH)
            .style(style::floating);
        menu_button(trigger, list).boxed()
    };

    adaptive::header(
        container(subject).width(Length::Fill),
        status,
        buttons,
        menu,
        MIN_SUBJECT,
        size::control(),
        space::M,
    )
    .boxed()
}
