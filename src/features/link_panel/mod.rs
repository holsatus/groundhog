//! The link feature: editing link configurations and starting/stopping links.
//!
//! This is a thin adapter. The editor draft ([`LinkConfigurator`]) and the
//! connect/disconnect buttons are view state; the links themselves and their
//! connection lifecycle are owned by the headless [`LinkManager`] in the
//! services layer. The panel translates widget messages into manager intents,
//! maps manager futures into [`Task`]s, and renders manager state. Persisting
//! the link set is requested from the root through [`Event`].

mod editor;

use iced::{
    widget::{button, checkbox, container, pick_list, text::Ellipsis, text_input, Button, Column},
    Alignment, Element, Length, Task, Widget,
};

pub use editor::{LinkBuilder, LinkConfigurator, LinkVariant, BAUDRATES};

use crate::{
    link::LinkConfig,
    services::{self, LinkId, LinkManager, LinkStatus, LinkUpdate},
    style,
};

pub struct LinkPanel {
    pub(crate) configurator: LinkConfigurator,
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
    /// Remove a configured link, dropping any live connection with it.
    Delete(LinkId),
    /// The result of an attempted link attach, produced by the manager.
    Update(LinkUpdate),
}

/// A request the root must satisfy, since it owns the configuration file.
#[derive(Debug)]
pub enum Event {
    /// The configured links changed and should be persisted.
    LinksChanged(Vec<LinkConfig>),
}

impl LinkPanel {
    pub fn new() -> Self {
        Self {
            configurator: LinkConfigurator::new(LinkBuilder::default_udp(), "New link".to_owned()),
        }
    }

    pub fn update(
        &mut self,
        message: Message,
        manager: &mut LinkManager,
    ) -> (Task<Message>, Option<Event>) {
        // `LinkClosed` rides the lossy telemetry ring, so a stored handle can
        // outlive the attachment it names; reconcile on every message, exactly
        // as the buttons do, so the view cannot show a connection whose
        // attachment has gone.
        manager.reconcile();

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
                    Task::future(services::link::detect_serial_ports())
                        .map(Message::SerialPortsDetected),
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
                // keeping its id and dropping any live connection; the set
                // always changed, so it is always persisted.
                manager.save(config);
                return (Task::none(), Some(Event::LinksChanged(manager.configs())));
            }
            Message::Connect(id) => {
                // The manager refuses a second attach on a live link; a refused
                // connect spawns no task at all.
                if let Some(update) = manager.connect(&id) {
                    return (Task::future(update).map(Message::Update), None);
                }
            }
            Message::Disconnect(id) => manager.disconnect(&id),
            Message::Delete(id) => {
                if manager.remove(&id) {
                    return (Task::none(), Some(Event::LinksChanged(manager.configs())));
                }
            }
            Message::Update(update) => manager.apply(update),
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
            Column::from_vec(self.builder_fields(Length::Fill, Length::Fill, Length::Fill))
                .spacing(10.0);

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
    /// per-call widths so the editor and the top bar can share them. For UDP
    /// the third width sizes the optional peer field.
    pub fn builder_fields(
        &self,
        addr: Length,
        port: Length,
        peer: Length,
    ) -> Vec<Element<'_, Message>> {
        let mut children = Vec::with_capacity(3);

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
            LinkBuilder::Udp {
                addr: a,
                port: p,
                peer: d,
            } => {
                children.push(
                    text_input("address", a)
                        .on_input({
                            let port = p.clone();
                            let peer = d.clone();
                            move |addr| {
                                Message::UpdateBuilder(LinkBuilder::Udp {
                                    addr,
                                    port: port.clone(),
                                    peer: peer.clone(),
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
                            let peer = d.clone();
                            move |port| {
                                Message::UpdateBuilder(LinkBuilder::Udp {
                                    addr: addr.clone(),
                                    port,
                                    peer: peer.clone(),
                                })
                            }
                        })
                        .width(port)
                        .boxed(),
                );

                // The peer is optional: empty means the link listens and locks
                // onto its first inbound datagram. (This iced has no separate
                // placeholder setter, so the example lives in the label.)
                children.push(
                    text_input("Peer (optional), e.g. 192.168.1.2:14550", d)
                        .on_input({
                            let addr = a.clone();
                            let port = p.clone();
                            move |peer| {
                                Message::UpdateBuilder(LinkBuilder::Udp {
                                    addr: addr.clone(),
                                    port: port.clone(),
                                    peer,
                                })
                            }
                        })
                        .width(peer)
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
    ///
    /// "Connected" here means a live guard on an attachment the router still
    /// holds, checked against the manager (which checks the router) on every
    /// frame: the update-time reconciliation repairs the stored state, and
    /// between messages the view cannot lie about a connection whose
    /// attachment has gone.
    pub fn view_status<'a>(&self, manager: &'a LinkManager) -> Element<'a, Message> {
        let mut contents = Vec::new();

        for (index, (id, config, status)) in manager.entries().enumerate() {
            let connected = manager.is_connected(id);
            let connecting = matches!(status, LinkStatus::Connecting);

            let button = if connected {
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
                .on_press_maybe((!connecting).then(|| Message::Connect(id.clone())))
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

            let mut children: Vec<Element<'_, Message>> = vec![iced::widget::text(&config.name)
                .ellipsis(Ellipsis::End)
                .boxed()];

            match &status {
                LinkStatus::Connecting => {
                    children.push(iced::widget::text("Connecting…").size(11).boxed());
                }
                LinkStatus::Failed(error) => {
                    children.push(iced::widget::text(error.clone()).size(11).boxed());
                }
                LinkStatus::Connected | LinkStatus::Disconnected => {}
            }

            children.push(iced::widget::space().width(Length::Fill).boxed());
            children.push(button.boxed());
            children.push(delete.boxed());

            let row = iced::widget::Row::with_children(children)
                .spacing(5.0)
                .align_y(Alignment::Center);

            let content = container(row)
                .style(if index % 2 == 0 {
                    style::list_default
                } else {
                    style::list_brighter
                })
                .padding(10.0);

            contents.push(iced::widget::rule::horizontal(1).boxed());
            contents.push(content.boxed());
        }

        Column::from_vec(contents).spacing(2.0).boxed()
    }
}

impl Default for LinkPanel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::config::LinkBuild;
    use crate::mavlink::{MavlinkRouter, DEFAULT_GCS_ID};
    use std::net::SocketAddrV4;

    fn config(name: &str) -> LinkConfig {
        LinkConfig {
            name: name.to_string(),
            auto_connect: false,
            high_latency: false,
            build: LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 14550),
                peer: None,
            },
        }
    }

    fn panel() -> (LinkPanel, LinkManager, MavlinkRouter) {
        let (router, _events) = MavlinkRouter::new(DEFAULT_GCS_ID);
        let (manager, _auto_connect) = LinkManager::from_configs(&router, Vec::new());
        (LinkPanel::new(), manager, router)
    }

    #[test]
    fn save_emits_links_changed() {
        let (mut panel, mut manager, _router) = panel();

        let (_, event) = panel.update(Message::Save(config("A")), &mut manager);
        let Some(Event::LinksChanged(links)) = event else {
            panic!("Save should emit LinksChanged");
        };
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "A");

        let (_, event) = panel.update(Message::ChangeName("B".to_string()), &mut manager);
        assert!(event.is_none(), "draft edits must not persist");
    }

    #[test]
    fn delete_emits_links_changed_and_unknown_is_a_noop() {
        let (mut panel, mut manager, _router) = panel();

        let _ = panel.update(Message::Save(config("A")), &mut manager);
        let id = manager.entries().next().unwrap().0.clone();

        let (_, event) = panel.update(Message::Delete(LinkId::new("does-not-exist")), &mut manager);
        assert!(event.is_none());
        assert_eq!(manager.configs().len(), 1);

        let (_, event) = panel.update(Message::Delete(id), &mut manager);
        let Some(Event::LinksChanged(links)) = event else {
            panic!("Delete should emit LinksChanged");
        };
        assert!(links.is_empty());
    }

    #[test]
    fn connect_unknown_id_spawns_no_task() {
        let (mut panel, mut manager, _router) = panel();

        let (task, event) = panel.update(
            Message::Connect(LinkId::new("does-not-exist")),
            &mut manager,
        );
        assert_eq!(task.units(), 0);
        assert!(event.is_none());
    }

    #[test]
    fn detect_serial_ports_outside_the_serial_editor_is_a_noop() {
        let (mut panel, mut manager, _router) = panel();

        // The default draft is UDP, so the enumeration must not start.
        let (task, event) = panel.update(Message::DetectSerialPorts, &mut manager);
        assert_eq!(task.units(), 0);
        assert!(event.is_none());
    }
}
