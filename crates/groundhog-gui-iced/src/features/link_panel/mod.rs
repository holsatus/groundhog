//! The link feature: editing link configurations and starting/stopping links.
//!
//! This is a thin adapter. The editor draft ([`LinkConfigurator`]) and the
//! connect/disconnect buttons are view state; the links themselves and their
//! connection lifecycle are owned by the headless [`LinkManager`] in the
//! services layer. The panel translates widget messages into manager intents,
//! maps manager futures into [`Task`]s, and renders manager state. Persisting
//! the link set is requested from the root through [`Event`].

mod editor;

use std::time::Instant;

use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{Button, Column, button, checkbox, container, pick_list, text::Ellipsis, text_input},
};

pub use editor::{BAUDRATES, LinkBuilder, LinkConfigurator, LinkVariant};
use groundhog_logic::{
    link::{LinkConfig, LinkId},
    services::{self, LinkManager, LinkStatus},
};

use super::notifications::{Notice, Notifications};
use crate::style;

pub struct LinkPanel {
    pub(crate) configurator: LinkConfigurator,
}

#[derive(Debug, Clone)]
pub enum Message {
    ChangeName(String),
    SetHighLatency(bool),
    SetAutoconnect(bool),
    SetAutoReconnect(bool),
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
        notices: &mut Notifications,
    ) -> (Task<Message>, Option<Event>) {
        match message {
            Message::ChangeName(name) => self.configurator.name = name,
            Message::SetHighLatency(high_latency) => {
                self.configurator.high_latency = high_latency;
            }
            Message::SetAutoconnect(auto_connect) => {
                self.configurator.auto_connect = auto_connect;
            }
            Message::SetAutoReconnect(auto_reconnect) => {
                self.configurator.auto_reconnect = auto_reconnect;
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
                    Task::future(services::detect_serial_ports()).map(Message::SerialPortsDetected),
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
                        Err(error) => {
                            notices.push(Notice::error("Could not list serial ports").detail(error))
                        }
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
                // The manager refuses a second supervisor on an active link.
                manager.connect(&id);
            }
            Message::Disconnect(id) => manager.disconnect(&id),
            Message::Delete(id) => {
                if manager.remove(&id) {
                    return (Task::none(), Some(Event::LinksChanged(manager.configs())));
                }
            }
        }

        (Task::none(), None)
    }

    /// The link editor panel (name, flags, builder fields, save).
    pub fn view_editor(&self) -> Element<'_, Message> {
        let link = &self.configurator;

        let name_editor = text_input("Link name", &link.name)
            .on_input(Message::ChangeName)
            .style(crate::style::text_input::default);

        let high_latency = checkbox(link.high_latency)
            .on_toggle(Message::SetHighLatency)
            .label("High latency");

        let auto_connect = checkbox(link.auto_connect)
            .on_toggle(Message::SetAutoconnect)
            .label("Connect on startup");

        let auto_reconnect = checkbox(link.auto_reconnect)
            .on_toggle(Message::SetAutoReconnect)
            .label("Reconnect when lost");

        let link_kind = pick_list(
            Some(link.builder.to_variant()),
            LinkVariant::list(),
            LinkVariant::to_string,
        )
        .on_select(Message::ChangeVariant)
        .style(crate::style::pick_list::default)
        .width(Length::Fill);

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
            auto_reconnect.boxed(),
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
                        .style(crate::style::text_input::default)
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
                        .style(crate::style::text_input::default)
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
                        .style(crate::style::text_input::default)
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
                        .style(crate::style::text_input::default)
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
                        .style(crate::style::text_input::default)
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
                        .style(crate::style::pick_list::default)
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
                        .style(crate::style::pick_list::default)
                        .width(port)
                        .boxed(),
                );
            }
        }

        children
    }

    /// The configured links with their supervisor's status and controls.
    pub fn view_status<'a>(&self, manager: &'a LinkManager) -> Element<'a, Message> {
        let now = Instant::now();
        let mut contents: Vec<Element<'a, Message>> = Vec::new();

        for (index, (id, config, status)) in manager.entries().enumerate() {
            let button = if status.is_active() {
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
            .style(crate::style::button::transparent)
            .on_press_with({
                let id = id.clone();
                move || Message::Delete(id.clone())
            })
            .width(30.0);

            let detail = match &status {
                LinkStatus::Disconnected => String::new(),
                LinkStatus::Connecting { attempt: 1 } => "Connecting…".to_string(),
                LinkStatus::Connecting { attempt } => format!("Connecting (attempt {attempt})…"),
                LinkStatus::Connected { .. } => match manager.stats(id) {
                    Some(stats) => {
                        let heard = stats.last_rx.map_or_else(
                            || "nothing received yet".to_string(),
                            |at| {
                                format!(
                                    "last frame {:.1} s ago",
                                    now.duration_since(at).as_secs_f32()
                                )
                            },
                        );
                        format!(
                            "Connected · {} in, {} out · {heard}",
                            stats.rx_frames, stats.tx_frames
                        )
                    }
                    None => "Connected".to_string(),
                },
                LinkStatus::Reconnecting {
                    error, retry_at, ..
                } => format!(
                    "{error}; retrying in {:.0} s",
                    retry_at.saturating_duration_since(now).as_secs_f32().ceil()
                ),
                LinkStatus::Failed(error) => format!("Failed: {error}"),
            };

            let label = Column::new()
                .push(
                    iced::widget::text(&config.name)
                        .ellipsis(Ellipsis::End)
                        .boxed(),
                )
                .push(
                    (!detail.is_empty())
                        .then(|| iced::widget::text(detail).size(11))
                        .boxed(),
                )
                .width(Length::Fill);

            let row = iced::widget::row![label, button, delete]
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
    use groundhog_logic::{Backend, BackendOptions, link::LinkBuild};

    use std::net::SocketAddrV4;

    use super::*;

    fn config(name: &str) -> LinkConfig {
        LinkConfig {
            name: name.to_string(),
            auto_connect: false,
            high_latency: false,
            auto_reconnect: true,
            build: LinkBuild::Udp {
                sock_addr: SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 14550),
                peer: None,
            },
        }
    }

    fn panel() -> (LinkPanel, Backend, Notifications) {
        let (backend, _events) = Backend::start(BackendOptions::default());
        (LinkPanel::new(), backend, Notifications::default())
    }

    #[tokio::test]
    async fn save_emits_links_changed() {
        let (mut panel, mut backend, mut notices) = panel();

        let (_, event) = panel.update(
            Message::Save(config("A")),
            backend.links_mut(),
            &mut notices,
        );
        let Some(Event::LinksChanged(links)) = event else {
            panic!("Save should emit LinksChanged");
        };
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].name, "A");

        let (_, event) = panel.update(
            Message::ChangeName("B".to_string()),
            backend.links_mut(),
            &mut notices,
        );
        assert!(event.is_none(), "draft edits must not persist");
    }

    #[tokio::test]
    async fn delete_emits_links_changed_and_unknown_is_a_noop() {
        let (mut panel, mut backend, mut notices) = panel();

        let _ = panel.update(
            Message::Save(config("A")),
            backend.links_mut(),
            &mut notices,
        );
        let id = backend.links().entries().next().unwrap().0.clone();

        let (_, event) = panel.update(
            Message::Delete(LinkId::new("does-not-exist")),
            backend.links_mut(),
            &mut notices,
        );
        assert!(event.is_none());
        assert_eq!(backend.links().configs().len(), 1);

        let (_, event) = panel.update(Message::Delete(id), backend.links_mut(), &mut notices);
        let Some(Event::LinksChanged(links)) = event else {
            panic!("Delete should emit LinksChanged");
        };
        assert!(links.is_empty());
    }

    #[test]
    fn the_editor_builds_reconnecting_links_by_default() {
        let panel = LinkPanel::new();
        assert!(panel.configurator.parse().unwrap().auto_reconnect);
    }

    #[tokio::test]
    async fn detect_serial_ports_outside_the_serial_editor_is_a_noop() {
        let (mut panel, mut backend, mut notices) = panel();

        // The default draft is UDP, so the enumeration must not start.
        let (task, event) = panel.update(
            Message::DetectSerialPorts,
            backend.links_mut(),
            &mut notices,
        );
        assert_eq!(task.units(), 0);
        assert!(event.is_none());
    }
}
