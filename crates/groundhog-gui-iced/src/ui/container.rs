//! Layout: panels, cards, forms, lists and the other arrangements views
//! share.
//!
//! A pane reads top to bottom in the same order everywhere (see
//! [`crate::ui`]): a [`header`] saying what the panel is about, then its
//! actions, any [`activity`] in progress, and its body.

use iced::{
    Alignment, Element, Length, Widget,
    widget::{
        ProgressBar, Rule, Text, column, container, progress_bar, row, rule, text::IntoFragment,
        tooltip::Position,
    },
};

use super::{
    super::widget::adaptive::{form_row, grid},
    button::button,
    style::{self, Tone, size, space},
    text::{TextExt, caption, text},
};

/// The body of a pane: its content, padded, filling the pane.
pub fn panel<'a, M: 'a>(content: impl Widget<M> + 'a) -> Element<'a, M> {
    container(content)
        .padding(space::M)
        .width(Length::Fill)
        .height(Length::Fill)
        .boxed()
}

/// A pane's body under a [`header`], divided from it by a rule. The body
/// takes the rest of the height, and scrolls itself if it must.
pub fn panel_with<'a, M: 'a>(header: Element<'a, M>, body: impl Widget<M> + 'a) -> Element<'a, M> {
    panel(column![header, divider(), body].spacing(space::M))
}

/// A panel's actions, in groups of related buttons: a wider gap sets the
/// groups apart, and a group wraps onto the next line whole.
pub fn actions<'a, M: 'a>(groups: impl IntoIterator<Item = Vec<Element<'a, M>>>) -> Element<'a, M> {
    row(groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .map(|group| row(group).spacing(space::S).boxed()))
    .spacing(space::L)
    .wrap()
    .vertical_spacing(space::S)
    .boxed()
}

/// Something under way: what it is, how far along (`done` of `total`, if
/// known), and a button to stop it.
pub fn activity<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    progress: Option<(f32, f32)>,
    cancel: Option<M>,
) -> Element<'a, M> {
    let (done, total) = progress.unwrap_or((0.0, 1.0));
    row![
        column![
            caption(label).tone(Tone::Accent),
            self::progress(done, total)
        ]
        .spacing(space::XS)
        .width(Length::Fill),
        cancel.map(|cancel| button("Cancel").on_press(cancel).boxed()),
    ]
    .spacing(space::M)
    .align_y(Alignment::Center)
    .boxed()
}

/// A status, as a small pill tinted with its tone.
pub fn badge<'a, M: 'a>(label: impl IntoFragment<'a>, tone: Tone) -> Element<'a, M> {
    container(caption(label).single_line())
        .padding([space::XXS, space::S])
        .style(style::badge(tone))
        .boxed()
}

/// A message that needs the operator's attention, across the panel, in
/// the tone of its urgency.
pub fn banner<'a, M: 'a>(message: impl IntoFragment<'a>, tone: Tone) -> Element<'a, M> {
    container(text(message))
        .padding([space::S, space::M])
        .width(Length::Fill)
        .style(style::banner(tone))
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

/// A titled group of content. The title is an [`overline`]: sections are
/// told apart by a quiet heading and the space between them, not by size.
pub fn section<'a, M: 'a>(
    heading: impl IntoFragment<'a>,
    children: impl IntoIterator<Item = Element<'a, M>>,
) -> Element<'a, M> {
    column(std::iter::once(overline(heading)).chain(children))
        .spacing(space::S)
        .boxed()
}

/// A section's heading: small, quiet capitals.
pub fn overline<'a, M: 'a>(heading: impl IntoFragment<'a>) -> Element<'a, M> {
    caption(heading.into_fragment().to_uppercase())
        .tone(Tone::Muted)
        .boxed()
}

/// One row of a form: a label, and the control it names. The labels of a
/// form share one width, so the controls line up.
pub fn field<'a, M: 'a>(
    label: impl IntoFragment<'a>,
    control: impl Widget<M> + 'a,
) -> Element<'a, M> {
    form_row(
        text(label).tone(Tone::Muted),
        control,
        size::FIELD_LABEL,
        size::field_control(),
        space::S,
    )
    .boxed()
}

/// A value and the caption naming it, sharing a row equally with its
/// siblings.
pub fn stat<'a, M: 'a>(label: impl IntoFragment<'a>, value: Text<'a>) -> Element<'a, M> {
    container(column![caption(label).tone(Tone::Muted), value.single_line()].spacing(space::XXS))
        .width(Length::FillPortion(1))
        .boxed()
}

/// Read-outs (each usually a [`stat`]) in columns that line up: as many as
/// fit, up to `max_columns`, so a narrow panel shows fewer to a row instead
/// of squeezing them.
pub fn stats<'a, M: 'a>(
    cells: impl IntoIterator<Item = Element<'a, M>>,
    max_columns: usize,
) -> Element<'a, M> {
    /// The narrowest a read-out gets: a caption, and a value with its unit.
    const MIN_CELL: f32 = 96.0;

    grid(cells, MIN_CELL, max_columns, space::M).boxed()
}

/// Row `index` of a list, striped so long rows stay legible.
pub fn list_row<'a, M: 'a>(index: usize, content: impl Widget<M> + 'a) -> Element<'a, M> {
    container(content)
        .padding(space::XXS)
        .width(Length::Fill)
        .style(style::list_row(index))
        .boxed()
}

/// `content`, explained by `hint` on `side` when hovered; on a touch
/// screen, which cannot hover, only `content`.
pub fn tooltip<'a, M: 'a>(
    content: impl Widget<M> + 'a,
    hint: impl IntoFragment<'a>,
    side: Position,
) -> Element<'a, M> {
    // Without hover, a tooltip opens on a tap and lingers until something
    // else is tapped.
    if crate::platform::TOUCH {
        return content.boxed();
    }

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

/// How far along something is, as `done` of `total`.
pub fn progress<'a>(done: f32, total: f32) -> ProgressBar<'a> {
    progress_bar(0.0..=total.max(f32::EPSILON), done).girth(size::PROGRESS)
}

/// A level, as `value` of `full`, filled in `tone`: a battery's charge.
pub fn meter<'a>(value: f32, full: f32, tone: Tone) -> ProgressBar<'a> {
    progress(value, full).style(style::meter(tone))
}
