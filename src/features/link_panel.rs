//! The link feature: editing link configurations and starting/stopping links.
//!
//! Owns the editor draft ([`LinkConfigurator`]) and the set of configured
//! links. Connection lifecycle is handled internally (the router is passed
//! into [`LinkPanel::update`]); persisting the default link configuration is
//! requested from the root through [`Event`].

use std::collections::BTreeMap;

use iced::{
    widget::{button, checkbox, container, pick_list, text::Ellipsis, text_input, Button, Column},
    Alignment, Element, Length, Task, Widget,
};

use crate::{
    foundation::LinkId,
    link::{self, LinkBuild, LinkBuilder, LinkConfig, LinkConfigurator, LinkVariant, BAUDRATES},
    mav::{LinkGuard, MavlinkRouter},
    style,
};

struct Link {
    id: LinkId,
    config: LinkConfig,
    handle: Option<LinkGuard>,
}

pub struct LinkPanel {
    pub(crate) configurator: LinkConfigurator,
    links: BTreeMap<String, Link>,
}

#[derive(Debug, Clone)]
pub enum Message {
    ChangeName(String),
    SetHighLatency(bool),
    SetAutoconnect(bool),
    ChangeVariant(LinkVariant),
    UpdateBuilder(LinkBuilder),
    DetectSerialPorts,

    Save(LinkId, LinkConfig),
    Connect(LinkId),
    Disconnect(LinkId),
    Event(link::Event),
}

/// A request the root must satisfy, since it owns the configuration file.
#[derive(Debug)]
pub enum Event {
    /// Set the in-memory default link configuration, without persisting it.
    SetConfig(LinkBuild),
    /// Set and persist the default link configuration.
    SaveConfig(LinkBuild),
}

impl LinkPanel {
    pub fn new(builder: LinkBuilder, name: String) -> Self {
        Self {
            configurator: LinkConfigurator::new(builder, name),
            links: BTreeMap::new(),
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        router: &MavlinkRouter,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::ChangeName(name) => self.configurator.name = name,
            Message::SetHighLatency(high_latency) => {
                self.configurator.high_latency = high_latency;
            }
            Message::SetAutoconnect(auto_connect) => {
                self.configurator.auto_connect = auto_connect;
            }
            Message::ChangeVariant(variant) => {
                if self.configurator.builder.to_variant() == variant {
                    return (Task::none(), None);
                }

                self.configurator.builder = variant.to_default_builder();
                self.configurator.build = self.configurator.builder.try_build();
                if let Some(build) = self.configurator.build.clone() {
                    return (Task::none(), Some(Event::SaveConfig(build)));
                }
            }
            Message::UpdateBuilder(builder) => {
                self.configurator.builder = builder;
                self.configurator.build = self.configurator.builder.try_build();
                if let Some(build) = self.configurator.build.clone() {
                    return (Task::none(), Some(Event::SaveConfig(build)));
                }
            }
            Message::DetectSerialPorts => {
                if let LinkBuilder::Serial {
                    available_ports, ..
                } = &mut self.configurator.builder
                {
                    match serialport::available_ports() {
                        Ok(ports) => {
                            *available_ports = ports
                                .into_iter()
                                .filter(|p| {
                                    matches!(p.port_type, serialport::SerialPortType::UsbPort(_))
                                })
                                .map(|p| p.port_name)
                                .collect();

                            available_ports.sort();
                        }
                        Err(error) => {
                            log::error!("Unable to fetch serial ports: {error}");
                        }
                    }

                    self.configurator.build = self.configurator.builder.try_build();
                }
            }
            Message::Save(id, config) => {
                self.links.insert(
                    config.name.clone(),
                    Link {
                        id,
                        config: config.clone(),
                        handle: None,
                    },
                );
                return (Task::none(), Some(Event::SetConfig(config.build)));
            }
            Message::Connect(id) => {
                if let Some(link) = self.links.get(id.str()) {
                    return (
                        link.config
                            .build
                            .connect(id.clone(), router.clone())
                            .map(Message::Event),
                        None,
                    );
                }
            }
            Message::Disconnect(id) => {
                if let Some(link) = self.links.values_mut().find(|link| link.id == id) {
                    drop(link.handle.take())
                }
            }
            Message::Event(link::Event::Connected(id, handle)) => {
                if let Some(link) = self.links.values_mut().find(|link| link.id == id) {
                    link.handle = Some(handle)
                }
            }
            Message::Event(link::Event::Failed(error)) => {
                log::error!("Connect failed: {error:?}");
            }
        }

        (Task::none(), None)
    }

    /// The link editor panel (name, flags, builder fields, save).
    pub fn view_editor(&self) -> Element<'_, Message> {
        let link = &self.configurator;

        let name_editor = text_input("Link name", &link.name).on_input(Message::ChangeName);

        let high_latency = checkbox(link.high_latency)
            .on_toggle(Message::SetHighLatency)
            .label("High latency");

        let auto_connect = checkbox(link.auto_connect)
            .on_toggle(Message::SetAutoconnect)
            .label("Auto connect");

        let link_kind = pick_list(
            Some(link.builder.to_variant()),
            LinkVariant::list(),
            LinkVariant::to_string,
        )
        .width(Length::Fill)
        .on_select(Message::ChangeVariant);

        let fields =
            Column::from_vec(self.builder_fields(Length::Fill, Length::Fill)).spacing(10.0);

        let button = Button::new(
            container("Save link")
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press_maybe(
            self.configurator
                .parse()
                .map(|config| Message::Save(LinkId::new(&self.configurator.name), config)),
        )
        .width(Length::Fill);

        let contents = Column::with_children(vec![
            name_editor.boxed(),
            high_latency.boxed(),
            auto_connect.boxed(),
            link_kind.boxed(),
            fields.boxed(),
            button.boxed(),
        ])
        .spacing(10.0);

        container(contents)
            .style(style::shaded_bordered_box)
            .padding(10.0)
            .width(170.0)
            .boxed()
    }

    /// The interactive fields for the current [`LinkBuilder`] variant, with
    /// per-call widths so the editor and the top bar can share them.
    pub fn builder_fields(&self, addr: Length, port: Length) -> Vec<Element<'_, Message>> {
        let mut children = Vec::with_capacity(2);

        match &self.configurator.builder {
            LinkBuilder::Tcp { addr: a, port: p } => {
                children.push(
                    text_input("address", a)
                        .on_input({
                            let port = p.clone();
                            move |addr| {
                                Message::UpdateBuilder(LinkBuilder::Tcp {
                                    addr,
                                    port: port.clone(),
                                })
                            }
                        })
                        .width(addr)
                        .boxed(),
                );

                children.push(
                    text_input("port", p)
                        .on_input({
                            let addr = a.clone();
                            move |port| {
                                Message::UpdateBuilder(LinkBuilder::Tcp {
                                    addr: addr.clone(),
                                    port,
                                })
                            }
                        })
                        .width(port)
                        .boxed(),
                );
            }
            LinkBuilder::Udp { addr: a, port: p } => {
                children.push(
                    text_input("address", a)
                        .on_input({
                            let port = p.clone();
                            move |addr| {
                                Message::UpdateBuilder(LinkBuilder::Udp {
                                    addr,
                                    port: port.clone(),
                                })
                            }
                        })
                        .width(addr)
                        .boxed(),
                );

                children.push(
                    text_input("port", p)
                        .on_input({
                            let addr = a.clone();
                            move |port| {
                                Message::UpdateBuilder(LinkBuilder::Udp {
                                    addr: addr.clone(),
                                    port,
                                })
                            }
                        })
                        .width(port)
                        .boxed(),
                );
            }
            LinkBuilder::Serial {
                port: p,
                available_ports,
                baud,
            } => {
                children.push(
                    pick_list(p.as_ref(), available_ports.as_slice(), |x| x.clone())
                        .placeholder(if available_ports.is_empty() {
                            "No connection"
                        } else {
                            "Select a port"
                        })
                        .ellipsis(Ellipsis::Start)
                        .on_open(Message::DetectSerialPorts)
                        .on_select({
                            let baud = *baud;
                            let available_ports = available_ports.clone();
                            move |selected_port| {
                                Message::UpdateBuilder(LinkBuilder::Serial {
                                    port: Some(selected_port),
                                    baud,
                                    available_ports: available_ports.clone(),
                                })
                            }
                        })
                        .width(addr)
                        .boxed(),
                );

                children.push(
                    pick_list(Some(baud), BAUDRATES, u32::to_string)
                        .on_select({
                            let port = p.clone();
                            let available_ports = available_ports.clone();
                            move |selected_baud| {
                                Message::UpdateBuilder(LinkBuilder::Serial {
                                    port: port.clone(),
                                    baud: selected_baud,
                                    available_ports: available_ports.clone(),
                                })
                            }
                        })
                        .width(port)
                        .boxed(),
                );
            }
        }

        children
    }

    /// The list of configured links with connect/disconnect buttons.
    pub fn view_status(&self) -> Element<'_, Message> {
        let mut contents = Vec::new();

        for link in self.links.values() {
            let button = if link.handle.is_some() {
                Button::new(
                    container("Disconnect")
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center),
                )
                .style(button::danger)
                .on_press_with({
                    let id = link.id.clone();
                    move || Message::Disconnect(id.clone())
                })
                .width(100.0)
            } else {
                Button::new(
                    container("Connect")
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center),
                )
                .on_press_with({
                    let id = link.id.clone();
                    move || Message::Connect(id.clone())
                })
                .width(100.0)
            };

            let content = container(iced::widget::row![
                iced::widget::text(&link.config.name),
                iced::widget::text(if link.handle.is_some() {
                    ": connected"
                } else {
                    ": disconnected"
                }),
                button
            ])
            .style(style::shaded_bordered_box)
            .padding(10.0);
            contents.push(content.boxed());
        }

        Column::from_vec(contents).spacing(10.0).boxed()
    }
}
