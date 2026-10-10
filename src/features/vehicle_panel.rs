//! The vehicles panel: a header with global controls, then the primary
//! vehicle's attitude, battery, heartbeat, and status texts. A stateless view
//! over vehicle data, so it lives as a plain function rather than a component.

use std::collections::BTreeMap;

use iced::{Alignment, Element, Length, Theme, Widget, font, never, widget::rich_text};
use mavio::{
    DefaultDialect,
    default_dialect::{enums::MavSeverity, messages::Heartbeat},
};

use crate::{
    domain::vehicle::{Liveness, MavMessage, Vehicle},
    foundation::MavlinkId,
    style::{list_brighter, list_default},
};

/// A short, legible marker for a vehicle's liveness.
fn liveness_label(liveness: Liveness) -> &'static str {
    match liveness {
        Liveness::Active => "active",
        Liveness::Stale => "stale",
        Liveness::Lost => "lost",
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Make a vehicle the primary one.
    SetPrimary(MavlinkId),
    /// Toggle whether the map follows the primary vehicle.
    Follow(bool),
}

pub fn view<'a>(
    vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
    primary: Option<MavlinkId>,
    follow: bool,
    theme: &Theme,
) -> Element<'a, Message> {
    let mut row_contents = Vec::<Element<'_, Message>>::with_capacity(17);

    // Global controls that used to live in the removed top bar.
    let mut controls = iced::widget::row![
        iced::widget::checkbox(follow)
            .label("Follow primary")
            .on_toggle(Message::Follow)
    ]
    .spacing(10.0)
    .align_y(Alignment::Center);

    if vehicles.len() > 1 {
        // TODO: use vendor and model name here instead of the MAVLink id.
        let mut vehicle_ids = vehicles.keys().copied().collect::<Vec<_>>();
        vehicle_ids.sort();
        let picker = iced::widget::pick_list(primary, vehicle_ids, |id| {
            vehicles.get(id).map_or_else(
                || "Invalid vehicle".to_string(),
                |v| format!("{} ({})", v.pretty_name(), liveness_label(v.liveness)),
            )
        })
        .on_select(Message::SetPrimary);

        controls = controls.push(picker.boxed());
    }

    row_contents.push(controls.boxed());
    row_contents.push(iced::widget::rule::horizontal(1.0).boxed());

    let mut roll = 0.0;
    let mut pitch = 0.0;
    let mut yaw = 0.0;

    let mut voltage = 0.0;
    let mut current = 0.0;
    let mut charge = 0.0;

    let mut flight_mode = "Unknown".to_string();
    let mut mav_state = "Unknown".to_string();
    let mut armed = "Unknown".to_string();

    // Absent any primary vehicle there is nothing to report, so stay `None`
    // rather than claiming a healthy link.
    let liveness = primary
        .and_then(|id| vehicles.get(&id))
        .map(|vehicle| vehicle.liveness);

    if let Some(vehicle) = primary.and_then(|id| vehicles.get(&id)) {
        if let Some(attitude) = vehicle.attitude.current.as_ref() {
            (roll, pitch, yaw) = attitude.attitude.euler_angles();
        }

        if let Some(battery) = vehicle.battery.state.as_ref() {
            voltage = battery.voltage;
            current = battery.current;
            charge = battery.charge;
        }

        flight_mode = vehicle.flight_mode().to_string();
        mav_state = vehicle
            .identity
            .mav_state
            .map_or_else(|| "Unknown".to_string(), |state| format!("{state:?}"));
        armed = vehicle
            .armed()
            .map_or_else(|| "Unknown".to_string(), |armed| armed.to_string());
    }

    let palette = theme.palette();
    let (liveness_text, liveness_color) = match liveness {
        Some(Liveness::Active) => ("active", palette.success.base.color),
        Some(Liveness::Stale) => ("stale", palette.warning.base.color),
        Some(Liveness::Lost) => ("lost", palette.danger.base.color),
        None => ("unknown", palette.secondary.base.color),
    };

    row_contents.push(
        iced::widget::row![
            iced::widget::container(iced::widget::text(format!("armed: {armed}")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("state: {mav_state}")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("mode: {flight_mode}")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(
                iced::widget::text(format!("link: {liveness_text}")).color(liveness_color),
            )
            .width(Length::FillPortion(1)),
        ]
        .width(Length::Fill)
        .spacing(10.0)
        .boxed(),
    );

    row_contents.push(iced::widget::rule::horizontal(1.0).boxed());

    row_contents.push(
        iced::widget::row![
            iced::widget::container(iced::widget::text(format!(
                "roll: {:.2} deg",
                roll.to_degrees()
            )),)
            .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!(
                "pitch: {:.2} deg",
                pitch.to_degrees()
            )),)
            .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!(
                "yaw: {:.2} deg",
                yaw.to_degrees()
            )),)
            .width(Length::FillPortion(1)),
        ]
        .width(Length::Fill)
        .spacing(10.0)
        .boxed(),
    );

    row_contents.push(
        iced::widget::row![
            iced::widget::container(iced::widget::text(format!("volt: {voltage:.2} V")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("curr: {current:.2} A")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("chrg: {charge:.2} %")),)
                .width(Length::FillPortion(1)),
        ]
        .width(Length::Fill)
        .spacing(10.0)
        .boxed(),
    );

    if let Some(vehicle) = primary.and_then(|id| vehicles.get(&id)) {
        if let Some(MavMessage {
            message: DefaultDialect::Heartbeat(heartbeat),
            ..
        }) = vehicle.latest_message.get(&Heartbeat::ID)
        {
            row_contents.push(iced::widget::rule::horizontal(1.0).boxed());
            row_contents.push(
                iced::widget::container(iced::widget::text(format!("{:?}", heartbeat))).boxed(),
            );
        }

        row_contents.push(iced::widget::rule::horizontal(1.0).boxed());
        row_contents.push(
            iced::widget::scrollable(iced::widget::row![
                iced::widget::Column::from_iter(vehicle.status.texts.iter().enumerate().map(
                    |(idx, status)| {
                        let palette = theme.palette();
                        let color = match status.severity {
                            MavSeverity::Emergency => palette.danger.base.color,
                            MavSeverity::Alert => palette.danger.base.color,
                            MavSeverity::Critical => palette.danger.base.color,
                            MavSeverity::Error => palette.danger.base.color,
                            MavSeverity::Warning => palette.warning.base.color,
                            MavSeverity::Notice => palette.warning.base.color,
                            MavSeverity::Info => palette.primary.base.color,
                            MavSeverity::Debug => palette.secondary.base.color,
                        };
                        iced::widget::container(iced::widget::row![
                            rich_text![
                                iced::widget::span(format!("[{:?}]", status.severity))
                                    .color(color)
                                    .font(iced::Font {
                                        weight: font::Weight::Bold,
                                        ..iced::Font::default()
                                    }),
                                iced::widget::span(" "),
                                iced::widget::span(&status.text),
                            ]
                            .on_link_click(never),
                            iced::widget::Space::new().width(Length::Fill)
                        ])
                        .style(if idx % 2 == 0 {
                            list_default
                        } else {
                            list_brighter
                        })
                        .padding(5.0)
                    }
                ))
                .spacing(10.0),
            ])
            .anchor_bottom()
            .boxed(),
        );
    }

    iced::widget::container(
        iced::widget::Column::from_vec(row_contents)
            .width(Length::Fill)
            .align_x(Alignment::Start)
            .spacing(10.0),
    )
    .width(Length::Fill)
    .padding(8.0)
    .boxed()
}
