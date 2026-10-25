//! The link feature: editing link configurations and starting/stopping links.
//!
//! This is a thin adapter. The editor draft ([`LinkConfigurator`]) and the
//! connect/disconnect buttons are view state; the links themselves and their
//! connection lifecycle are owned by the headless [`LinkManager`] in the
//! services layer. The panel translates widget messages into manager intents,
//! maps manager futures into [`Task`]s, and renders manager state. Persisting
//! the link set is requested from the root through [`Event`].

mod editor;

use std::time::{Duration, Instant};

use iced::{
    Alignment, Element, Length, Task, Widget,
    widget::{Column, Space, column, container, row, scrollable, text::Ellipsis, tooltip},
};

pub use editor::{BAUDRATES, LinkBuilder, LinkConfigurator, LinkField, LinkVariant};
use groundhog_logic::{
    link::{LinkConfig, LinkId},
    services::{self, LinkManager, LinkStats, LinkStatus, Saved},
};

use super::{
    confirm::Confirmation,
    notifications::{Notice, Notifications},
};
use crate::ui::{self, Icon, TextExt, Tone};

pub struct LinkPanel {
    configurator: LinkConfigurator,
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
    /// Ask the operator, then delete a configured link.
    RequestDelete(LinkId),
    /// Remove a configured link, dropping any live connection with it.
    Delete(LinkId),
}

/// A request the root must satisfy, since it owns the configuration file
/// and the confirmation dialog.
#[derive(Debug)]
pub enum Event {
    /// The configured links changed and should be persisted.
    LinksChanged(Vec<LinkConfig>),
    /// Ask the operator before sending the message.
    Confirm(Confirmation<Message>),
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
                // keeping its id, and its connection unless the transport
                // changed.
                let name = config.name.trim().to_string();
                match manager.save(config) {
                    Ok(saved) => {
                        if let Saved::Reconnected(_) = saved {
                            notices.push(Notice::info(format!(
                                "Reconnecting {name} with the new settings"
                            )));
                        }
                        return (Task::none(), Some(Event::LinksChanged(manager.configs())));
                    }
                    Err(error) => {
                        notices.push(Notice::warning("Could not save the link").detail(error))
                    }
                }
            }
            Message::Connect(id) => {
                // The manager refuses a second supervisor on an active link.
                manager.connect(&id);
            }
            Message::Disconnect(id) => manager.disconnect(&id),
            Message::RequestDelete(id) => {
                let Some((_, config, status)) = manager.entries().find(|(link, ..)| **link == id)
                else {
                    return (Task::none(), None);
                };
                let body = if status.is_active() {
                    "It is removed from the saved links, and its connection is closed."
                } else {
                    "It is removed from the saved links."
                };
                let confirmation = Confirmation::new(
                    format!("Delete {}?", config.name),
                    body,
                    Message::Delete(id),
                )
                .accept_label("Delete");
                return (Task::none(), Some(Event::Confirm(confirmation)));
            }
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
            .any(|(_, config, _)| config.name == self.configurator.name.trim());

        ui::panel(scrollable(
            column![
                ui::section("Saved links", [links]),
                ui::section(
                    if overwrites { "Edit link" } else { "New link" },
                    [self.view_editor(overwrites)],
                ),
            ]
            .spacing(ui::space::L),
        ))
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
                ui::field(
                    "",
                    column![
                        ui::checkbox("Connect on startup", link.auto_connect)
                            .on_toggle(Message::SetAutoconnect),
                        ui::checkbox("Reconnect when lost", link.auto_reconnect)
                            .on_toggle(Message::SetAutoReconnect),
                        ui::checkbox("High latency", link.high_latency)
                            .on_toggle(Message::SetHighLatency),
                    ]
                    .spacing(ui::space::S),
                ),
                row![
                    Space::new().width(Length::Fill),
                    save_button(
                        link.name.trim(),
                        overwrites,
                        link.parse()
                            .filter(|config| !config.name.trim().is_empty())
                            .map(Message::Save),
                    ),
                ],
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
    button.on_press_maybe(save).boxed()
}

/// One configured link: its name and status, and its controls.
fn view_link<'a>(
    manager: &LinkManager,
    id: &LinkId,
    config: &'a LinkConfig,
    status: LinkStatus,
) -> Element<'a, Message> {
    // The state is the line under the name; the button only acts.
    let toggle = if status.is_active() {
        ui::button("Disconnect").on_press(Message::Disconnect(id.clone()))
    } else {
        ui::button("Connect").on_press(Message::Connect(id.clone()))
    };

    let edit = ui::icon_button(Icon::Edit)
        .hint("Edit in the editor below")
        .on_press(Message::EditLink(id.clone()));

    let delete = ui::icon_button(Icon::Delete)
        .hint("Delete the link")
        .on_press(Message::RequestDelete(id.clone()));

    let summary = summarize(
        &status,
        manager.stats(id),
        manager.last_connected(id),
        Instant::now(),
    );
    let label = column![
        ui::text(&config.name).single_line(),
        ui::caption(summary.glance).tone(summary.tone),
    ]
    .spacing(ui::space::XXS);

    // The details are a hover away, so the row stays readable at a glance.
    let label = if summary.details.is_empty() {
        label.boxed()
    } else {
        ui::tooltip(label, summary.details.join("\n"), tooltip::Position::Bottom)
    };

    row![
        container(label).width(Length::Fill),
        toggle.width(110.0).boxed(),
        edit.boxed(),
        delete.boxed()
    ]
    .spacing(ui::space::S)
    .align_y(Alignment::Center)
    .boxed()
}

/// A link connected for this long without receiving a frame is called
/// silent: whatever is at the other end is not talking.
const SILENT_AFTER: Duration = Duration::from_secs(3);

/// What the operator sees of a link: a line to read at a glance, its tone,
/// and the details behind it.
#[derive(Debug, PartialEq)]
struct Summary {
    glance: String,
    tone: Tone,
    details: Vec<String>,
}

fn summarize(
    status: &LinkStatus,
    stats: Option<LinkStats>,
    last_connected: Option<Instant>,
    now: Instant,
) -> Summary {
    let summary = |glance: String, tone, details| Summary {
        glance,
        tone,
        details,
    };

    match status {
        LinkStatus::Disconnected => match last_connected {
            Some(at) => summary(
                format!(
                    "Last connected {} ago",
                    span(now.saturating_duration_since(at))
                ),
                Tone::Muted,
                Vec::new(),
            ),
            None => summary("Not connected".to_string(), Tone::Muted, Vec::new()),
        },
        LinkStatus::Connecting { attempt: 1 } => {
            summary("Connecting…".to_string(), Tone::Accent, Vec::new())
        }
        LinkStatus::Connecting { attempt } => summary(
            format!("Connecting (attempt {attempt})…"),
            Tone::Accent,
            Vec::new(),
        ),
        LinkStatus::Connected { since } => {
            let connected = now.saturating_duration_since(*since);
            let heard = stats
                .and_then(|stats| stats.last_rx)
                .map(|at| now.saturating_duration_since(at));
            let silence = heard.unwrap_or(connected);

            let mut details = vec![format!("Connected for {}", span(connected))];
            details.extend(stats.map(traffic).into_iter().flatten());

            if silence > SILENT_AFTER {
                summary(
                    format!("Connected, but silent for {}", span(silence)),
                    Tone::Warning,
                    details,
                )
            } else {
                summary(
                    format!("Connected for {}", span(connected)),
                    Tone::Success,
                    details,
                )
            }
        }
        LinkStatus::Reconnecting {
            error,
            attempt,
            retry_at,
        } => summary(
            format!(
                "Connection lost, retrying in {}",
                span(retry_at.saturating_duration_since(now))
            ),
            Tone::Warning,
            vec![error.clone(), format!("Attempt {attempt}")],
        ),
        LinkStatus::Failed(error) => {
            summary("Failed".to_string(), Tone::Danger, vec![error.clone()])
        }
    }
}

/// The traffic counters, as lines of a tooltip.
fn traffic(stats: LinkStats) -> Vec<String> {
    let errors = stats.read_errors + stats.parse_errors + stats.write_errors;
    let total = stats.rx_frames + stats.tx_frames + errors;

    let mut lines = vec![
        format!("Received {} frames", stats.rx_frames),
        format!("Sent {} frames", stats.tx_frames),
    ];
    lines.push(if errors == 0 {
        "No errors".to_string()
    } else {
        format!(
            "Errors: {} read, {} parse, {} write ({:.1} % of traffic)",
            stats.read_errors,
            stats.parse_errors,
            stats.write_errors,
            errors as f64 / total as f64 * 100.0
        )
    });
    lines
}

/// A duration in its largest sensible unit: "4 s", "12 min", "3 h 5 min".
fn span(duration: Duration) -> String {
    let seconds = duration.as_secs_f32().ceil() as u64;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
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

        drop(panel.update(
            Message::Save(config("A")),
            backend.links_mut(),
            &mut notices,
        ));
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
    async fn deleting_a_link_asks_first() {
        let (mut panel, mut backend, mut notices) = panel();
        drop(panel.update(
            Message::Save(config("A")),
            backend.links_mut(),
            &mut notices,
        ));
        let id = backend.links().entries().next().unwrap().0.clone();

        let (_, event) = panel.update(
            Message::RequestDelete(id.clone()),
            backend.links_mut(),
            &mut notices,
        );

        let Some(Event::Confirm(confirmation)) = event else {
            panic!("expected a confirmation")
        };
        assert!(matches!(&confirmation.accept, Message::Delete(link) if *link == id));
        assert_eq!(backend.links().configs().len(), 1, "kept until accepted");
    }

    #[tokio::test]
    async fn editing_a_saved_link_loads_it_into_the_editor() {
        let (mut panel, mut backend, mut notices) = panel();
        drop(panel.update(
            Message::Save(config("A")),
            backend.links_mut(),
            &mut notices,
        ));
        let id = backend.links().entries().next().unwrap().0.clone();

        let (_, event) = panel.update(Message::EditLink(id), backend.links_mut(), &mut notices);
        assert!(event.is_none(), "loading a link does not change the set");
        assert_eq!(panel.configurator.name, "A");
        assert!(matches!(
            panel.configurator.builder,
            LinkBuilder::Udp { ref port, .. } if port == "14550"
        ));
    }

    fn stats(rx_frames: u64, last_rx: Option<Instant>) -> LinkStats {
        LinkStats {
            rx_frames,
            tx_frames: 10,
            read_errors: 0,
            parse_errors: 0,
            write_errors: 0,
            last_rx,
        }
    }

    #[test]
    fn a_healthy_link_reads_as_connected_with_traffic_behind_a_hover() {
        let now = Instant::now();
        let status = LinkStatus::Connected {
            since: now - Duration::from_secs(150),
        };

        let summary = summarize(&status, Some(stats(90, Some(now))), None, now);
        assert_eq!(summary.glance, "Connected for 2 min");
        assert_eq!(summary.tone, Tone::Success);
        assert_eq!(
            summary.details,
            [
                "Connected for 2 min",
                "Received 90 frames",
                "Sent 10 frames",
                "No errors"
            ]
        );
    }

    #[test]
    fn a_silent_link_warns_and_errors_show_their_rate() {
        let now = Instant::now();
        let status = LinkStatus::Connected {
            since: now - Duration::from_secs(60),
        };
        let quiet = LinkStats {
            parse_errors: 5,
            ..stats(85, Some(now - Duration::from_secs(12)))
        };

        let summary = summarize(&status, Some(quiet), None, now);
        assert_eq!(summary.glance, "Connected, but silent for 12 s");
        assert_eq!(summary.tone, Tone::Warning);
        assert_eq!(
            summary.details.last().unwrap(),
            "Errors: 0 read, 5 parse, 0 write (5.0 % of traffic)"
        );
    }

    #[test]
    fn a_disconnected_link_says_when_it_was_last_connected() {
        let now = Instant::now();
        let never = summarize(&LinkStatus::Disconnected, None, None, now);
        assert_eq!(never.glance, "Not connected");

        let earlier = summarize(
            &LinkStatus::Disconnected,
            None,
            Some(now - Duration::from_secs(3 * 3600 + 300)),
            now,
        );
        assert_eq!(earlier.glance, "Last connected 3 h 5 min ago");
        assert!(earlier.details.is_empty());
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
