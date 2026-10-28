//! Layout: panels, cards, forms, lists and the other arrangements views
//! share.
//!
//! A pane reads top to bottom in the same order everywhere (see
//! [`crate::ui`]): a [`header`] saying what the panel is about, then its
//! actions, any [`activity`] in progress, and its body.

use iced::{
    Alignment, Element, Length, Padding, Widget,
    widget::{
        Column, Container, ProgressBar, Row, Rule, Text, column, container, progress_bar, row,
        row::Wrapping, rule, text::IntoFragment, tooltip::Position,
    },
};

use super::{
    super::widget::adaptive::{Adaptive, form_row, grid},
    button::button,
    icon::{self, Icon},
    style::{self, ButtonKind, Tone, size, space},
    text::{TextExt, caption, text},
};

/// The body of a pane: its content, padded, filling the pane.
pub fn panel<'a, W>(content: W) -> Container<'a, W> {
    container(content)
        .padding(space::M)
        .width(Length::Fill)
        .height(Length::Fill)
}

/// A pane's body under a [`header`], divided from it by a rule. The body
/// takes the rest of the height, and scrolls itself if it must.
pub fn panel_with<'a, M: 'a>(
    header: impl Widget<M> + 'a,
    body: impl Widget<M> + 'a,
) -> Container<'a, Column<Element<'a, M>>> {
    panel(column![header, divider(), body].spacing(space::M))
}

/// A panel's actions, in groups of related buttons: a wider gap sets the
/// groups apart, and a group wraps onto the next line whole.
pub fn actions<'a, M: 'a>(
    groups: impl IntoIterator<Item = Vec<Element<'a, M>>>,
) -> Wrapping<Row<Element<'a, M>>> {
    row(groups
        .into_iter()
        .filter(|group| !group.is_empty())
        .map(|group| row(group).spacing(space::S)))
    .spacing(space::L)
    .wrap()
    .vertical_spacing(space::S)
}

/// Something under way: what it is, how far along (`done` of `total`, if
/// known), and a button to stop it.
pub fn activity<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    progress: Option<(f32, f32)>,
    cancel: Option<M>,
) -> Row<Element<'a, M>> {
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
}

/// A status, as a small pill tinted with its tone.
pub fn badge<'a>(label: impl IntoFragment<'a>, tone: Tone) -> Container<'a, Text<'a>> {
    container(caption(label).single_line())
        .padding([space::XXS, space::S])
        .style(style::badge(tone))
}

/// A message that needs the operator's attention, across the panel, in
/// the tone of its urgency.
pub fn banner<'a>(message: impl IntoFragment<'a>, tone: Tone) -> Container<'a, Text<'a>> {
    container(text(message))
        .padding([space::S, space::M])
        .width(Length::Fill)
        .style(style::banner(tone))
}

/// A panel with nothing to show but why.
pub fn empty<'a>(reason: impl IntoFragment<'a>) -> Container<'a, Text<'a>> {
    panel(text(reason).tone(Tone::Muted))
}

/// A bordered card grouping related content inside a panel.
pub fn card<'a, W>(content: W) -> Container<'a, W> {
    container(content)
        .padding(space::M)
        .width(Length::Fill)
        .style(style::card)
}

/// A card of rows, such as saved items, each padded like a card's content
/// and separated by a rule that spans the card from edge to edge.
pub fn card_list<'a, M: 'a>(
    rows: impl IntoIterator<Item = impl Widget<M> + 'a>,
) -> Container<'a, Column<Element<'a, M>>> {
    let mut list = column![];
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            list = list.push(divider().style(style::card_divider).boxed());
        }
        list = list.push(container(row).padding(space::M).width(Length::Fill).boxed());
    }

    container(list).width(Length::Fill).style(style::card)
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
) -> Column<Element<'a, M>> {
    column(std::iter::once(overline(heading).boxed()).chain(children)).spacing(space::S)
}

/// A section's heading: small, quiet capitals.
pub fn overline<'a>(heading: impl IntoFragment<'a>) -> Text<'a> {
    caption(heading.into_fragment().to_uppercase()).tone(Tone::Muted)
}

/// One row of a form: a label, and the control it names. The labels of a
/// form share one width, so the controls line up.
pub fn field<'a, M: 'a>(
    label: impl IntoFragment<'a>,
    control: impl Widget<M> + 'a,
) -> Adaptive<'a, M> {
    form_row(
        text(label).tone(Tone::Muted),
        control,
        size::FIELD_LABEL,
        size::field_control(),
        space::S,
    )
}

/// A value and the caption naming it, sharing a row equally with its
/// siblings.
pub fn stat<'a, M: 'a>(
    label: impl IntoFragment<'a>,
    value: Text<'a>,
) -> Container<'a, Column<Element<'a, M>>> {
    container(column![caption(label).tone(Tone::Muted), value.single_line()].spacing(space::XXS))
        .width(Length::FillPortion(1))
}

/// A dark surface for a picture (a video), so the bands around it read as
/// the edges of a screen rather than of the panel.
pub fn screen<'a, W>(content: W) -> Container<'a, W> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(style::screen)
}

/// A card floating over a picture or the map: a small pane lifted by a
/// shadow. Pad and size it as its content needs.
pub fn floating<'a, M: 'a, W: Widget<M> + 'a>(content: W) -> Container<'a, W> {
    container(content).style(style::floating)
}

/// A pane's body that is a picture (a video), with whatever floats over it:
/// edge to edge inside the pane's border, black around the picture.
pub fn picture<'a, W>(content: W) -> Container<'a, Container<'a, W>> {
    container(
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(style::picture),
    )
    .padding(Padding {
        top: 0.0,
        right: style::PANE_BORDER_WIDTH,
        bottom: style::PANE_BORDER_WIDTH,
        left: style::PANE_BORDER_WIDTH,
    })
}

/// What lies over a picture: `content` centred, over a dark wash while
/// `dim` (see [`style::veil`]). The layer stays while not dimmed, so the
/// tree keeps its shape when the picture stops and starts again.
pub fn veil<'a, M: 'a, W: Widget<M> + 'a>(content: W, dim: bool) -> Container<'a, W> {
    container(content).center(Length::Fill).style(move |theme| {
        if dim {
            style::veil(theme)
        } else {
            container::Style::default()
        }
    })
}

/// Read-outs (each usually a [`stat`]) in columns that line up: as many as
/// fit, up to `max_columns`, so a narrow panel shows fewer to a row instead
/// of squeezing them.
pub fn stats<'a, M: 'a>(
    cells: impl IntoIterator<Item = impl Widget<M> + 'a>,
    max_columns: usize,
) -> Adaptive<'a, M> {
    /// The narrowest a read-out gets: a caption, and a value with its unit.
    const MIN_CELL: f32 = 96.0;

    grid(
        cells.into_iter().map(|cell| cell.boxed()),
        MIN_CELL,
        max_columns,
        space::M,
    )
}

/// Row `index` of a list, striped so long rows stay legible.
pub fn list_row<'a, W>(index: usize, content: W) -> Container<'a, W> {
    container(content)
        .padding(space::XXS)
        .width(Length::Fill)
        .style(style::list_row(index))
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

/// A label for something being dragged, its `icon` and `name`, styled as a
/// tooltip: what follows the pointer when a panel's rail button or its pane
/// is dragged.
pub fn drag_label<'a, M: 'a>(
    icon: Icon,
    name: impl IntoFragment<'a>,
) -> Container<'a, Row<Element<'a, M>>> {
    container(
        row![
            icon::icon(icon, size::ICON, ButtonKind::Subtle, true),
            text(name)
        ]
        .spacing(space::S)
        .align_y(Alignment::Center),
    )
    .padding([space::M, space::L])
    .style(style::tooltip)
}

/// How far along something is, as `done` of `total`.
pub fn progress<'a>(done: f32, total: f32) -> ProgressBar<'a> {
    progress_bar(0.0..=total.max(f32::EPSILON), done).girth(size::PROGRESS)
}

/// A level, as `value` of `full`, filled in `tone`: a battery's charge.
pub fn meter<'a>(value: f32, full: f32, tone: Tone) -> ProgressBar<'a> {
    progress(value, full).style(style::meter(tone))
}
