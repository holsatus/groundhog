//! The link feature: editing link configurations and starting/stopping links.
//!
//! Owns the editor draft ([`LinkConfigurator`]) and the set of configured
//! links. Connection lifecycle is handled internally (the router is passed
//! into [`LinkPanel::update`]); persisting the default link configuration is
//! requested from the root through [`Event`].

mod editor;

use std::collections::BTreeMap;

use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{Button, Column, button, checkbox, container, pick_list, text::Ellipsis, text_input},
};

pub use editor::{BAUDRATES, LinkBuilder, LinkConfigurator, LinkVariant};

use crate::{
    foundation::LinkId,
    link::{self, LinkConfig},
    mavlink::{LinkGuard, MavlinkRouter},
    style,
};

struct Link {
    config: LinkConfig,
    handle: Option<LinkGuard>,
}

pub struct LinkPanel {
    pub(crate) configurator: LinkConfigurator,
    /// Configured links, keyed by a stable opaque id independent of the name.
    links: BTreeMap<LinkId, Link>,
    /// Source of fresh link ids; never derived from the display name.
    next_id: u64,
}

#[derive(Debug, Clone)]
pub enum Message {
    ChangeName(String),
    SetHighLatency(bool),
    SetAutoconnect(bool),
    ChangeVariant(LinkVariant),
    UpdateBuilder(LinkBuilder),
    DetectSerialPorts,
    /// The result of an asynchronous serial-port enumeration.
    SerialPortsDetected(Result<Vec<String>, String>),

    Save(LinkConfig),
    Connect(LinkId),
    Disconnect(LinkId),
    /// The router reported that a link died on its own.
    Closed(LinkId),
    /// Remove a configured link, dropping any live connection with it.
    Delete(LinkId),
    Event(link::Event),
}

/// A request the root must satisfy, since it owns the configuration file.
#[derive(Debug)]
pub enum Event {
    /// The configured links changed and should be persisted.
    LinksChanged(Vec<LinkConfig>),
}

impl LinkPanel {
    /// Build a panel from the persisted links, returning it along with the ids
    /// of the links marked for auto-connect.
    pub fn from_config(configs: Vec<LinkConfig>) -> (Self, Vec<LinkId>) {
        let mut panel = Self {
            configurator: LinkConfigurator::new(LinkBuilder::default_udp(), "New link".to_owned()),
            links: BTreeMap::new(),
            next_id: 0,
        };

        let mut auto_connect = Vec::new();
        for config in configs {
            let id = panel.fresh_id();
            if config.auto_connect {
                auto_connect.push(id.clone());
            }
            panel.links.insert(
                id,
                Link {
                    config,
                    handle: None,
                },
            );
        }

        (panel, auto_connect)
    }

    /// Mint an opaque link id that does not depend on the display name.
    fn fresh_id(&mut self) -> LinkId {
        self.next_id += 1;
        LinkId::new(&format!("link-{:04}", self.next_id))
    }

    /// The configured links, in display order.
    fn configs(&self) -> Vec<LinkConfig> {
        self.links
            .values()
            .map(|link| link.config.clone())
            .collect()
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
                if self.configurator.builder.to_variant() != variant {
                    self.configurator.builder = variant.to_default_builder();
                    self.configurator.build = self.configurator.builder.try_build();
                }
            }
            Message::UpdateBuilder(builder) => {
                self.configurator.builder = builder;
                self.configurator.build = self.configurator.builder.try_build();
            }
            Message::DetectSerialPorts => {
                if !matches!(self.configurator.builder, LinkBuilder::Serial { .. }) {
                    return (Task::none(), None);
                }

                return (
                    iced_runtime::task::blocking(move |mut sender| {
                        let result = serialport::available_ports()
                            .map(|ports| {
                                let mut names = ports
                                    .into_iter()
                                    .filter(|p| {
                                        matches!(
                                            p.port_type,
                                            serialport::SerialPortType::UsbPort(_)
                                        )
                                    })
                                    .map(|p| p.port_name)
                                    .collect::<Vec<_>>();
                                names.sort();
                                names
                            })
                            .map_err(|error| error.to_string());
                        let _ = sender.try_send(Message::SerialPortsDetected(result));
                    }),
                    None,
                );
            }
            Message::SerialPortsDetected(result) => {
                if let LinkBuilder::Serial {
                    available_ports, ..
                } = &mut self.configurator.builder
                {
                    match result {
                        Ok(ports) => *available_ports = ports,
                        Err(error) => log::error!("Unable to fetch serial ports: {error}"),
                    }

                    self.configurator.build = self.configurator.builder.try_build();
                }
            }
            Message::Save(config) => {
                // Saving a link with an existing name updates it in place,
                // keeping its id and dropping any live connection.
                let existing = self
                    .links
                    .iter()
                    .find(|(_, link)| link.config.name == config.name)
                    .map(|(id, _)| id.clone());
                let id = existing.unwrap_or_else(|| self.fresh_id());

                self.links.insert(
                    id,
                    Link {
                        config: config.clone(),
                        handle: None,
                    },
                );
                return (Task::none(), Some(Event::LinksChanged(self.configs())));
            }
            Message::Connect(id) => {
                if let Some(link) = self.links.get(&id) {
                    let build = link.config.build.clone();
                    let high_latency = link.config.high_latency;
                    let router = router.clone();

                    return (
                        Task::future(async move {
                            let link_id = id.clone();
                            match link::open(build).await {
                                Ok((reader, writer)) => {
                                    let handle = router.attach_reader_writer(
                                        link_id.clone(),
                                        reader,
                                        writer,
                                        high_latency,
                                    );
                                    link::Event::Connected(link_id, handle)
                                }
                                Err(error) => link::Event::Failed(error),
                            }
                        })
                        .map(Message::Event),
                        None,
                    );
                }
            }
            Message::Disconnect(id) => {
                if let Some(link) = self.links.get_mut(&id) {
                    drop(link.handle.take())
                }
            }
            Message::Closed(id) => {
                if let Some(link) = self.links.get_mut(&id) {
                    link.handle = None;
                }
            }
            Message::Delete(id) => {
                self.links.remove(&id);
                return (Task::none(), Some(Event::LinksChanged(self.configs())));
            }
            Message::Event(link::Event::Connected(id, handle)) => {
                if let Some(link) = self.links.get_mut(&id) {
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
        .on_press_maybe(self.configurator.parse().map(Message::Save))
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

        container(contents).padding(8.0).width(Length::Fill).boxed()
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

        for (index, (id, link)) in self.links.iter().enumerate() {
            let button = if link.handle.is_some() {
                Button::new(
                    container("Disconnect")
                        .align_x(Alignment::Center)
                        .align_y(Alignment::Center),
                )
                .style(button::danger)
                .on_press_with({
                    let id = id.clone();
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
                    let id = id.clone();
                    move || Message::Connect(id.clone())
                })
                .width(100.0)
            };

            let delete = Button::new(
                container("x")
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
            .style(button::danger)
            .on_press_with({
                let id = id.clone();
                move || Message::Delete(id.clone())
            })
            .width(30.0);

            let content = container(iced::widget::row![
                iced::widget::text(&link.config.name),
                iced::widget::text(if link.handle.is_some() {
                    ": connected"
                } else {
                    ": disconnected"
                }),
                button,
                delete
            ])
            .style(if index % 2 == 0 {
                style::list_default
            } else {
                style::list_brighter
            })
            .padding(4.0);
            contents.push(content.boxed());
        }

        Column::from_vec(contents).spacing(2.0).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundation::DEFAULT_GCS_ID;
    use crate::link::config::LinkBuild;
    use std::net::SocketAddrV4;

    fn config(name: &str) -> LinkConfig {
        LinkConfig {
            name: name.to_string(),
            auto_connect: false,
            high_latency: false,
            build: LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 14550),
            },
        }
    }

    fn panel() -> (LinkPanel, MavlinkRouter) {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (panel, _auto_connect) = LinkPanel::from_config(Vec::new());
        (panel, router)
    }

    #[test]
    fn from_config_loads_links_and_reports_auto_connect() {
        let auto = LinkConfig {
            auto_connect: true,
            ..config("auto")
        };
        let manual = config("manual");

        let (panel, auto_connect) = LinkPanel::from_config(vec![auto, manual]);

        assert_eq!(panel.links.len(), 2);
        assert_eq!(auto_connect.len(), 1);

        let auto_id = auto_connect[0].clone();
        let auto_link = panel.links.get(&auto_id).expect("auto link loaded");
        assert_eq!(auto_link.config.name, "auto");
        assert!(auto_link.config.auto_connect);

        let manual = panel
            .links
            .values()
            .find(|link| link.config.name == "manual")
            .expect("manual link loaded");
        assert!(!manual.config.auto_connect);
    }

    #[test]
    fn save_emits_links_changed() {
        let (mut panel, router) = panel();

        let (_, event) = panel.update(Message::Save(config("A")), &router);
        let Some(Event::LinksChanged(links)) = event else {
            panic!("Save should emit LinksChanged");
        };
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "A");

        let (_, event) = panel.update(Message::ChangeName("B".to_string()), &router);
        assert!(event.is_none(), "draft edits must not persist");
    }

    #[test]
    fn saving_same_name_reuses_link_id() {
        let (mut panel, router) = panel();

        let _ = panel.update(Message::Save(config("A")), &router);
        assert_eq!(panel.links.len(), 1);
        let first_id = panel.links.keys().next().unwrap().clone();

        let _ = panel.update(Message::Save(config("A")), &router);
        assert_eq!(panel.links.len(), 1);
        let second_id = panel.links.keys().next().unwrap().clone();

        assert_eq!(first_id, second_id);
    }

    #[test]
    fn saving_different_name_adds_link_with_new_id() {
        let (mut panel, router) = panel();

        let _ = panel.update(Message::Save(config("A")), &router);
        let _ = panel.update(Message::Save(config("B")), &router);

        assert_eq!(panel.links.len(), 2);
        let ids = panel.links.keys().cloned().collect::<Vec<_>>();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn connect_and_disconnect_unknown_ids_are_noops() {
        let (mut panel, router) = panel();
        let unknown = LinkId::new("does-not-exist");

        let (_, event) = panel.update(Message::Disconnect(unknown.clone()), &router);
        assert!(event.is_none());
        assert!(panel.links.is_empty());

        let (_, event) = panel.update(Message::Connect(unknown), &router);
        assert!(event.is_none());
        assert!(panel.links.is_empty());
    }

    #[test]
    fn delete_removes_link_and_unknown_is_noop() {
        let (mut panel, router) = panel();

        let _ = panel.update(Message::Save(config("A")), &router);
        let id = panel.links.keys().next().unwrap().clone();

        let _ = panel.update(Message::Delete(LinkId::new("does-not-exist")), &router);
        assert_eq!(panel.links.len(), 1);

        let _ = panel.update(Message::Delete(id), &router);
        assert!(panel.links.is_empty());
    }
}
