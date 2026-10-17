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
    widget::{Column, column, row, text::Ellipsis},
};

pub use editor::{BAUDRATES, LinkBuilder, LinkConfigurator, LinkField, LinkVariant};
use groundhog_logic::{
    link::{LinkConfig, LinkId},
    services::{self, LinkManager, LinkStatus},
};

use super::notifications::{Notice, Notifications};
use crate::ui::{self, Icon, TextExt, Tone};

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
    SetField(LinkField, String),
    SelectSerialPort(String),
    SelectBaud(u32),
    DetectSerialPorts,
    /// The result of an asynchronous serial-port enumeration.
    SerialPortsDetected(Result<Vec<String>, String>),

    /// Load a saved link into the editor.
    EditLink(LinkId),
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
            Message::SetField(field, value) => {
                self.configurator
                    .edit_builder(|builder| builder.set_field(field, value));
            }
            Message::SelectSerialPort(port) => {
                self.configurator
                    .edit_builder(|builder| builder.select_serial_port(port));
            }
            Message::SelectBaud(baud) => {
                self.configurator
                    .edit_builder(|builder| builder.select_baud(baud));
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
            Message::EditLink(id) => {
                if let Some((_, config, _)) = manager.entries().find(|(link, ..)| **link == id) {
                    self.configurator = LinkConfigurator::from_config(config);
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

    /// The configured links with their controls, and the editor.
    pub fn view<'a>(&'a self, manager: &'a LinkManager) -> Element<'a, Message> {
        let links = if manager.entries().next().is_none() {
            ui::text("No links yet. Add one below.")
                .tone(Tone::Muted)
                .boxed()
        } else {
            ui::card_list(
                manager
                    .entries()
                    .map(|(id, config, status)| view_link(manager, id, config, status)),
            )
        };

        let overwrites = manager
            .entries()
            .any(|(_, config, _)| config.name == self.configurator.name);

        ui::panel(
            column![
                ui::section("Links", [links]),
                ui::section("Add or update a link", [self.view_editor(overwrites)]),
            ]
            .spacing(ui::space::L),
        )
    }

    /// The link editor: name, kind, the kind's fields, flags, and save.
    /// Saving under the name of a saved link replaces it, which the save
    /// button says, in the warning colour, when it `overwrites`.
    fn view_editor(&self, overwrites: bool) -> Element<'_, Message> {
        let link = &self.configurator;

        let variant = ui::pick_list(
            Some(link.builder.to_variant()),
            LinkVariant::list(),
            LinkVariant::to_string,
        )
        .on_select(Message::ChangeVariant);

        ui::card(
            column![
                ui::field(
                    "Name",
                    ui::text_input("Link name", &link.name).on_input(Message::ChangeName)
                ),
                ui::field("Kind", variant),
                Column::with_children(self.builder_fields()).spacing(ui::space::S),
                ui::checkbox("High latency", link.high_latency).on_toggle(Message::SetHighLatency),
                ui::checkbox("Connect on startup", link.auto_connect)
                    .on_toggle(Message::SetAutoconnect),
                ui::checkbox("Reconnect when lost", link.auto_reconnect)
                    .on_toggle(Message::SetAutoReconnect),
                save_button(&link.name, overwrites, link.parse().map(Message::Save)),
            ]
            .spacing(ui::space::S),
        )
    }

    /// The fields of the draft's kind of link.
    fn builder_fields(&self) -> Vec<Element<'_, Message>> {
        fn field<'a>(
            label: &'static str,
            placeholder: &'static str,
            value: &'a str,
            which: LinkField,
        ) -> Element<'a, Message> {
            ui::field(
                label,
                ui::text_input(placeholder, value)
                    .on_input(move |value| Message::SetField(which, value)),
            )
        }

        match &self.configurator.builder {
            LinkBuilder::Tcp { addr, port } => vec![
                field("Address", "0.0.0.0", addr, LinkField::Address),
                field("Port", "5760", port, LinkField::Port),
            ],
            LinkBuilder::Udp { addr, port, peer } => vec![
                field("Address", "0.0.0.0", addr, LinkField::Address),
                field("Port", "14550", port, LinkField::Port),
                // Empty means the link listens and locks onto its first
                // inbound datagram.
                field(
                    "Peer",
                    "optional, e.g. 192.168.1.2:14550",
                    peer,
                    LinkField::Peer,
                ),
            ],
            LinkBuilder::Serial {
                port,
                available_ports,
                baud,
            } => vec![
                ui::field(
                    "Port",
                    ui::pick_list(port.as_ref(), available_ports.as_slice(), String::clone)
                        .placeholder(if available_ports.is_empty() {
                            "No ports found"
                        } else {
                            "Select a port"
                        })
                        .ellipsis(Ellipsis::Start)
                        .on_open(Message::DetectSerialPorts)
                        .on_select(Message::SelectSerialPort),
                ),
                ui::field(
                    "Baud rate",
                    ui::pick_list(Some(baud), BAUDRATES, u32::to_string)
                        .on_select(Message::SelectBaud),
                ),
            ],
        }
    }
}

/// The editor's save button, saying what saving does to the name in the
/// editor; disabled while the draft does not parse.
fn save_button<'a>(name: &str, overwrites: bool, save: Option<Message>) -> Element<'a, Message> {
    let button = if overwrites {
        ui::button(format!("Overwrite '{name}'")).warning()
    } else {
        ui::button(format!("Save as '{name}'")).primary()
    };
    button.width(Length::Fill).on_press_maybe(save).boxed()
}

/// One configured link: its name and status, and its controls.
fn view_link<'a>(
    manager: &LinkManager,
    id: &LinkId,
    config: &'a LinkConfig,
    status: LinkStatus,
) -> Element<'a, Message> {
    let toggle = if status.is_active() {
        ui::button("Disconnect")
            .danger()
            .on_press(Message::Disconnect(id.clone()))
    } else {
        ui::button("Connect")
            .primary()
            .on_press(Message::Connect(id.clone()))
    };

    let edit = ui::icon_button(Icon::Edit)
        .hint("Edit in the editor below")
        .on_press(Message::EditLink(id.clone()));

    let delete = ui::icon_button(Icon::Delete)
        .hint("Delete the link")
        .on_press(Message::Delete(id.clone()));

    let detail = status_detail(manager, id, &status);
    let label = column![
        ui::text(&config.name).single_line(),
        (!detail.is_empty()).then(|| ui::caption(detail).tone(Tone::Muted)),
    ]
    .spacing(ui::space::XXS)
    .width(Length::Fill);

    row![
        label,
        toggle.width(110.0).boxed(),
        edit.boxed(),
        delete.boxed()
    ]
    .spacing(ui::space::S)
    .align_y(Alignment::Center)
    .boxed()
}

/// A line on how the link is doing; empty while it is off.
fn status_detail(manager: &LinkManager, id: &LinkId, status: &LinkStatus) -> String {
    let now = Instant::now();
    match status {
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

    #[tokio::test]
    async fn editing_a_saved_link_loads_it_into_the_editor() {
        let (mut panel, mut backend, mut notices) = panel();
        let _ = panel.update(
            Message::Save(config("A")),
            backend.links_mut(),
            &mut notices,
        );
        let id = backend.links().entries().next().unwrap().0.clone();

        let (_, event) = panel.update(Message::EditLink(id), backend.links_mut(), &mut notices);
        assert!(event.is_none(), "loading a link does not change the set");
        assert_eq!(panel.configurator.name, "A");
        assert!(matches!(
            panel.configurator.builder,
            LinkBuilder::Udp { ref port, .. } if port == "14550"
        ));
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
