//! The top bar: link draft controls, the follow toggle, and the primary
//! vehicle picker. A stateless view; its messages are translated by the root.

use std::collections::BTreeMap;

use iced::{Alignment, Element, Length, Widget};

use crate::{
    domain::vehicle::Vehicle,
    features::link_panel::{self, LinkPanel},
    foundation::MavlinkId,
    link::LinkVariant,
    style::shaded_bordered_box,
};

#[derive(Debug, Clone)]
pub enum Message {
    Link(link_panel::Message),
    Follow(bool),
    SetPrimary(MavlinkId),
}

pub fn view<'a>(
    link_panel: &'a LinkPanel,
    follow: bool,
    vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
    primary: Option<MavlinkId>,
) -> Element<'a, Message> {
    let mut row_contents = Vec::<Element<'_, Message>>::with_capacity(16);

    let selector = iced::widget::pick_list(
        Some(link_panel.configurator.builder.to_variant()),
        LinkVariant::list(),
        ToString::to_string,
    )
    .placeholder("Pick one")
    .on_select(|v| Message::Link(link_panel::Message::ChangeVariant(v)))
    .boxed();

    row_contents.push(selector);
    row_contents.push(iced::widget::rule::vertical(1.0).boxed());

    row_contents.extend(
        link_panel
            .builder_fields(Length::Fixed(160.0), Length::Fixed(100.0))
            .into_iter()
            .map(|element| element.map(Message::Link).boxed()),
    );

    row_contents.push(iced::widget::rule::vertical(1.0).boxed());
    row_contents.push(iced::widget::text("Follow primary vehicle").boxed());
    row_contents.push(
        iced::widget::checkbox(follow)
            .on_toggle(Message::Follow)
            .boxed(),
    );

    row_contents.push(
        iced::widget::space::Space::new()
            .width(Length::Fill)
            .boxed(),
    );

    if vehicles.len() > 1 {
        // TODO: We should use vendor and model name here instead
        let mut vehicle_ids = vehicles.keys().cloned().collect::<Vec<_>>();
        vehicle_ids.sort();
        let vehicle_picker = iced::widget::pick_list(primary, vehicle_ids, |id| {
            vehicles
                .get(id)
                .map_or_else(|| "Invalid vehicle".to_string(), |v| v.pretty_name())
        })
        .on_select(Message::SetPrimary);
        row_contents.push(vehicle_picker.boxed());
    }

    iced::widget::container(
        iced::widget::row::Row::from_vec(row_contents)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .spacing(10.0)
            .height(32.0),
    )
    .style(shaded_bordered_box)
    .width(Length::Fill)
    .padding(10.0)
    .boxed()
}
