//! Layout: panels, cards, forms, lists and the other arrangements views
//! share.

use iced::{
    Alignment, Element, Length, Widget,
    widget::{
        ProgressBar, Rule, Text, column, container, progress_bar, row, rule, text::IntoFragment,
        tooltip::Position,
    },
};

use super::{
    style::{self, Chip, Tone, size, space},
    text::{TextExt, caption, text, title},
};

/// The body of a pane: its content, padded, filling the pane.
pub fn panel<'a, M: 'a>(content: impl Widget<M> + 'a) -> Element<'a, M> {
    container(content)
        .padding(space::M)
        .width(Length::Fill)
        .height(Length::Fill)
        .boxed()
}

/// A panel with nothing to show but why.
pub fn empty<'a, M: 'a>(reason: impl IntoFragment<'a>) -> Element<'a, M> {
    panel(text(reason).tone(Tone::Muted))
}

/// A bordered card grouping related content inside a panel.
pub fn card<'a, M: 'a>(content: impl Widget<M> + 'a) -> Element<'a, M> {
    container(content)
        .padding(space::M)
        .width(Length::Fill)
        .style(style::card)
        .boxed()
}

/// A card of rows, such as saved items, each padded like a card's content
/// and separated by a rule that spans the card from edge to edge.
pub fn card_list<'a, M: 'a>(rows: impl IntoIterator<Item = Element<'a, M>>) -> Element<'a, M> {
    let mut list = column![];
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            list = list.push(divider().style(style::card_divider).boxed());
        }
        list = list.push(container(row).padding(space::M).width(Length::Fill).boxed());
    }

    container(list)
        .width(Length::Fill)
        .style(style::card)
        .boxed()
}

/// A thin line between groups of content.
pub fn divider<'a>() -> Rule<'a> {
    rule::horizontal(1.0)
}

/// A titled group of content.
pub fn section<'a, M: 'a>(
    heading: impl IntoFragment<'a>,
    children: impl IntoIterator<Item = Element<'a, M>>,
) -> Element<'a, M> {
    column(std::iter::once(title(heading).boxed()).chain(children))
        .spacing(space::S)
        .boxed()
}

/// One row of a form: a label, and the control it names. The labels of a
/// form share one width, so the controls line up.
pub fn field<'a, M: 'a>(
    label: impl IntoFragment<'a>,
    control: impl Widget<M> + 'a,
) -> Element<'a, M> {
    row![text(label).width(size::FIELD_LABEL), control]
        .spacing(space::S)
        .align_y(Alignment::Center)
        .boxed()
}

/// A value and the caption naming it, sharing a row equally with its
/// siblings.
pub fn stat<'a, M: 'a>(label: impl IntoFragment<'a>, value: Text<'a>) -> Element<'a, M> {
    container(column![caption(label), value.single_line()].spacing(space::XXS))
        .width(Length::FillPortion(1))
        .boxed()
}

/// Buttons in a row, wrapping onto more rows when they do not fit.
pub fn toolbar<'a, M: 'a>(buttons: impl IntoIterator<Item = Element<'a, M>>) -> Element<'a, M> {
    row(buttons).spacing(space::S).wrap().boxed()
}

/// Row `index` of a list, striped so long rows stay legible.
pub fn list_row<'a, M: 'a>(index: usize, content: impl Widget<M> + 'a) -> Element<'a, M> {
    container(content)
        .padding(space::XXS)
        .width(Length::Fill)
        .style(style::list_row(index))
        .boxed()
}

/// `content`, explained by `hint` on `side` when hovered.
pub fn tooltip<'a, M: 'a>(
    content: impl Widget<M> + 'a,
    hint: impl IntoFragment<'a>,
    side: Position,
) -> Element<'a, M> {
    iced::widget::tooltip(
        content,
        container(text(hint))
            .padding(space::M)
            .style(style::tooltip),
        side,
    )
    .delay(std::time::Duration::from_millis(250))
    .boxed()
}

/// A small label for one step of a sequence.
pub fn chip<'a, M: 'a>(label: impl IntoFragment<'a>, state: Chip) -> Element<'a, M> {
    container(caption(label))
        .padding([space::XXS, space::M])
        .style(style::chip(state))
        .boxed()
}

/// How far along something is, as `done` of `total`.
pub fn progress<'a>(done: f32, total: f32) -> ProgressBar<'a> {
    progress_bar(0.0..=total.max(f32::EPSILON), done).girth(size::PROGRESS)
}
