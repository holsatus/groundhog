//! The right-hand panel: primary-vehicle attitude, battery, heartbeat, and
//! status texts. This is a stateless view over vehicle data, so it lives as a
//! plain function rather than a component.

use std::collections::BTreeMap;

use iced::{font, never, widget::rich_text, Alignment, Element, Length, Theme, Widget};
use mavio::{
    default_dialect::{
        enums::{MavModeFlag, MavSeverity},
        messages::Heartbeat,
    },
    DefaultDialect,
};
use num_enum::FromPrimitive;

use crate::{
    app::Message,
    domain::vehicle::{MavMessage, Vehicle},
    foundation::MavlinkId,
    style::{list_brighter, list_default, shaded_bordered_box},
};

pub fn view<'a>(
    vehicles: &'a BTreeMap<MavlinkId, Vehicle>,
    primary: Option<MavlinkId>,
    theme: &Theme,
) -> Element<'a, Message> {
    let mut row_contents = Vec::<Element<'_, Message>>::with_capacity(16);

    let mut roll = 0.0;
    let mut pitch = 0.0;
    let mut yaw = 0.0;

    let mut voltage = 0.0;
    let mut current = 0.0;
    let mut charge = 0.0;

    let mut flight_mode = "Unknown".to_string();
    let mut mav_state = "Unknown".to_string();
    let mut armed = "Unknown".to_string();

    if let Some(vehicle) = primary.and_then(|id| vehicles.get(&id)) {
        if let Some(attitude) = vehicle.attitude.as_ref() {
            (roll, pitch, yaw) = attitude.attitude.euler_angles();
        }

        if let Some(battery) = vehicle.battery_state.as_ref() {
            voltage = battery.voltage;
            current = battery.current;
            charge = battery.charge;
        }

        if let Some(MavMessage {
            message: DefaultDialect::Heartbeat(hb),
            ..
        }) = vehicle.latest_message.get(&Heartbeat::ID)
        {
            use mavio::dialects::minimal::enums::{MavAutopilot, MavType};

            if hb.autopilot == MavAutopilot::Ardupilotmega {
                use crate::domain::firmware::ardupilot::flight_mode;

                match hb.type_ {
                    MavType::FixedWing | MavType::VtolTiltrotor => {
                        let mode = flight_mode::Plane::from_primitive(hb.custom_mode as u8);
                        flight_mode = format!("{mode:?}");
                    }
                    MavType::Quadrotor | MavType::Helicopter => {
                        let mode = flight_mode::Copter::from_primitive(hb.custom_mode as u8);
                        flight_mode = format!("{mode:?}");
                    }
                    _ => (),
                }
            }

            mav_state = format!("{:?}", hb.system_status);
            armed = format!("{}", hb.base_mode.intersects(MavModeFlag::SAFETY_ARMED));
        }
    }

    row_contents.push(
        iced::widget::row![
            iced::widget::container(iced::widget::text(format!("armed: {armed}")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("state: {mav_state}")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("mode: {flight_mode}")),)
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
                "roll: {:.5} deg",
                roll.to_degrees()
            )),)
            .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!(
                "pitch: {:.5} deg",
                pitch.to_degrees()
            )),)
            .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!(
                "yaw: {:.5} deg",
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
            iced::widget::container(iced::widget::text(format!("volt: {voltage:.5} V")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("curr: {current:.5} A")),)
                .width(Length::FillPortion(1)),
            iced::widget::container(iced::widget::text(format!("chrg: {charge:.5} %")),)
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
            iced::widget::scrollable(iced::widget::row![iced::widget::Column::from_iter(
                vehicle
                    .status_texts
                    .iter()
                    .enumerate()
                    .map(|(idx, status)| {
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
                    })
            )
            .spacing(10.0),])
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
    .style(shaded_bordered_box)
    .width(500.0)
    .padding(10.0)
    .boxed()
}
