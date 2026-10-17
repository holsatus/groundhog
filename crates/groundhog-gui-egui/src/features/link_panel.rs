//! The link feature: editing link configurations and starting/stopping links.
//!
//! Ported from the iced frontend. The editor draft ([`LinkConfigurator`]) and
//! the connect/disconnect buttons are view state; the links themselves and
//! their connection lifecycle are owned by the headless [`LinkManager`] in
//! the services layer. The panel translates widget interactions into manager
//! intents and renders manager state. Persisting the link set is requested
//! from the application through [`Effect::SaveLinks`].

use std::net::{Ipv4Addr, SocketAddrV4};
use std::str::FromStr;

use egui::RichText;
use groundhog_logic::{
    link::{LinkBuild, LinkConfig},
    services::{LinkManager, LinkStatus, detect_serial_ports},
};
use tokio::sync::mpsc;

use super::{Context, Effect, failure_notice};
use crate::style;

pub const BAUDRATES: &[u32] = &[
    2_400, 4_800, 9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 500_000, 576_000,
    921_600, 1_000_000,
];

/// The editable draft of a link, held as raw text by the link editor.
#[derive(Debug, Clone)]
pub enum LinkBuilder {
    Tcp {
        addr: String,
        port: String,
    },
    Udp {
        addr: String,
        port: String,
        peer: String,
    },
    Serial {
        port: Option<String>,
        baud: u32,
        available_ports: Vec<String>,
    },
}

impl LinkBuilder {
    pub fn default_tcp() -> LinkBuilder {
        LinkBuilder::Tcp {
            addr: "0.0.0.0".into(),
            port: "5760".into(),
        }
    }

    pub fn default_udp() -> LinkBuilder {
        LinkBuilder::Udp {
            addr: "0.0.0.0".into(),
            port: "14550".into(),
            peer: String::new(),
        }
    }

    pub fn default_serial() -> LinkBuilder {
        LinkBuilder::Serial {
            port: None,
            baud: 115_200,
            available_ports: Vec::new(),
        }
    }

    pub fn to_variant(&self) -> LinkVariant {
        match self {
            LinkBuilder::Tcp { .. } => LinkVariant::Tcp,
            LinkBuilder::Udp { .. } => LinkVariant::Udp,
            LinkBuilder::Serial { .. } => LinkVariant::Serial,
        }
    }

    pub fn try_build(&self) -> Option<LinkBuild> {
        let config = match self {
            LinkBuilder::Tcp { addr, port } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                LinkBuild::Tcp {
                    sock_addr: SocketAddrV4::new(addr, port),
                }
            }
            LinkBuilder::Udp { addr, port, peer } => {
                let addr = Ipv4Addr::from_str(addr).ok()?;
                let port = u16::from_str(port).ok()?;
                let sock = SocketAddrV4::new(addr, port);

                // An empty peer means a listening link; anything else must
                // parse as an `IPv4:port` pair.
                let peer = match peer.trim() {
                    "" => None,
                    peer => Some(SocketAddrV4::from_str(peer).ok()?),
                };

                LinkBuild::Udp {
                    sock_addr: sock,
                    peer,
                }
            }
            LinkBuilder::Serial { port, baud, .. } => {
                let port = port.to_owned()?;
                LinkBuild::Serial { port, baud: *baud }
            }
        };

        Some(config)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinkVariant {
    Tcp,
    Udp,
    Serial,
}

impl LinkVariant {
    pub fn to_default_builder(self) -> LinkBuilder {
        match self {
            LinkVariant::Tcp => LinkBuilder::default_tcp(),
            LinkVariant::Udp => LinkBuilder::default_udp(),
            LinkVariant::Serial => LinkBuilder::default_serial(),
        }
    }

    pub fn list() -> &'static [LinkVariant] {
        &[LinkVariant::Tcp, LinkVariant::Udp, LinkVariant::Serial]
    }
}

impl std::fmt::Display for LinkVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LinkVariant::Tcp => "TCP",
            LinkVariant::Udp => "UDP",
            LinkVariant::Serial => "Serial",
        })
    }
}

/// The link editor's working state: a draft builder plus the last build that
/// parsed successfully (used to enable the save button and persist changes).
pub struct LinkConfigurator {
    pub name: String,
    pub auto_connect: bool,
    pub high_latency: bool,
    pub auto_reconnect: bool,
    pub builder: LinkBuilder,
    pub build: Option<LinkBuild>,
}

impl LinkConfigurator {
    pub fn new(builder: LinkBuilder, name: String) -> LinkConfigurator {
        LinkConfigurator {
            name,
            auto_connect: false,
            high_latency: false,
            auto_reconnect: true,
            builder: builder.clone(),
            build: builder.try_build(),
        }
    }

    pub fn parse(&self) -> Option<LinkConfig> {
        self.build.as_ref().map(|build| LinkConfig {
            name: self.name.clone(),
            auto_connect: self.auto_connect,
            high_latency: self.high_latency,
            auto_reconnect: self.auto_reconnect,
            build: build.clone(),
        })
    }
}

/// What the panel's async work reports back.
enum LinkEvent {
    SerialPorts(std::result::Result<Vec<String>, String>),
}

pub struct LinkPanel {
    pub(crate) configurator: LinkConfigurator,
    serial_ports: mpsc::UnboundedReceiver<LinkEvent>,
    serial_requests: mpsc::UnboundedSender<()>,
}

impl LinkPanel {
    pub fn new() -> Self {
        let (serial_tab, serial_ports) = mpsc::unbounded_channel();
        let (serial_requests, mut serial_jobs) = mpsc::unbounded_channel::<()>();
        // Enumeration runs for as long as requests arrive.
        tokio::spawn(async move {
            while serial_jobs.recv().await.is_some() {
                let result = detect_serial_ports()
                    .await
                    .map_err(|error| error.to_string());
                let _ = serial_tab.send(LinkEvent::SerialPorts(result));
            }
        });

        Self {
            configurator: LinkConfigurator::new(LinkBuilder::default_udp(), "New link".to_owned()),
            serial_ports,
            serial_requests,
        }
    }
}

impl LinkPanel {
    /// Drain the list of serial ports as it arrives.
    pub fn collect_serial_ports(&mut self, cx: &mut Context) {
        while let Ok(event) = self.serial_ports.try_recv() {
            if let LinkBuilder::Serial {
                available_ports, ..
            } = &mut self.configurator.builder
            {
                match event {
                    LinkEvent::SerialPorts(Ok(ports)) => *available_ports = ports,
                    LinkEvent::SerialPorts(Err(error)) => {
                        cx.notices.push(failure_notice(
                            "Could not list serial ports".to_owned(),
                            error,
                        ));
                    }
                }
                self.configurator.build = self.configurator.builder.try_build();
            }
        }
    }

    /// The link editor: name, flags, builder fields, save.
    pub fn view_editor(&mut self, ui: &mut egui::Ui, manager: &mut LinkManager, cx: &mut Context) {
        let link = &mut self.configurator;

        ui.label(egui::RichText::new("Add a link").strong());
        ui.add(
            egui::TextEdit::singleline(&mut link.name)
                .hint_text("Link name")
                .desired_width(ui.available_width()),
        );

        ui.checkbox(&mut link.high_latency, "High latency");
        ui.checkbox(&mut link.auto_connect, "Connect on startup");
        ui.checkbox(&mut link.auto_reconnect, "Reconnect when lost");

        egui::ComboBox::from_id_salt("links.variant")
            .selected_text(link.builder.to_variant().to_string())
            .width(ui.available_width())
            .show_ui(ui, |combo| {
                for variant in LinkVariant::list() {
                    let active = link.builder.to_variant() == *variant;
                    if combo
                        .selectable_label(active, variant.to_string())
                        .clicked()
                        && !active
                    {
                        link.builder = variant.to_default_builder();
                        link.build = link.builder.try_build();
                    }
                }
            });

        self.builder_fields(ui);

        let saveable = self.configurator.parse();
        if ui
            .add_enabled(saveable.is_some(), egui::Button::new("Save link"))
            .clicked()
            && let Some(config) = saveable
        {
            // Saving a link with an existing name updates it in place,
            // keeping its id and dropping any live connection; the set
            // always changed, so it is always persisted.
            manager.save(config);
            cx.request(Effect::SaveLinks(manager.configs()));
        }
    }

    /// The interactive fields for the current [`LinkBuilder`] variant. Every
    /// change re-parses the build.
    fn builder_fields(&mut self, ui: &mut egui::Ui) {
        match &mut self.configurator.builder {
            LinkBuilder::Tcp { addr, port } => {
                egui::Grid::new("links.tcp").num_columns(2).show(ui, |row| {
                    row.label("Address");
                    row.add(egui::TextEdit::singleline(addr).hint_text("0.0.0.0"));
                    row.end_row();
                    row.label("Port");
                    row.add(egui::TextEdit::singleline(port).hint_text("5760"));
                    row.end_row();
                });
            }
            LinkBuilder::Udp { addr, port, peer } => {
                egui::Grid::new("links.udp").num_columns(2).show(ui, |row| {
                    row.label("Address");
                    row.add(
                        egui::TextEdit::singleline(addr)
                            .hint_text("0.0.0.0"),
                    );
                    row.end_row();
                    row.label("Port");
                    row.add(egui::TextEdit::singleline(port).hint_text("14550"));
                    row.end_row();
                    row.label(RichText::new("Peer").strong());
                    row.add(
                        egui::TextEdit::singleline(peer)
                            .hint_text("optional, e.g. 192.168.1.2:14550"),
                    )
                    .on_hover_text(
                        "Empty listens for the first datagram; an address sends unsolicited frames there.",
                    );
                    row.end_row();
                });
            }
            LinkBuilder::Serial {
                port,
                available_ports,
                baud,
            } => {
                egui::Grid::new("links.serial")
                    .num_columns(2)
                    .show(ui, |row| {
                        row.label("Port");
                        egui::ComboBox::from_id_salt("links.serial.port")
                            .selected_text(
                                port.as_ref().map(String::as_str).unwrap_or("Select a port"),
                            )
                            .width(row.available_width())
                            .show_ui(row, |combo| {
                                for available in available_ports.clone() {
                                    let active = port.as_ref().is_some_and(|p| *p == available);
                                    if combo.selectable_label(active, available.clone()).clicked() {
                                        *port = Some(available);
                                    }
                                }
                            });
                        row.end_row();
                        row.label("Baud");
                        egui::ComboBox::from_id_salt("links.serial.baud")
                            .selected_text(baud.to_string())
                            .show_ui(row, |combo| {
                                for rate in BAUDRATES {
                                    if combo
                                        .selectable_label(*baud == *rate, rate.to_string())
                                        .clicked()
                                    {
                                        *baud = *rate;
                                    }
                                }
                            });
                        row.end_row();
                        if row.button("Refresh ports").clicked() {
                            let _ = self.serial_requests.send(());
                        }
                    });
            }
        }

        // The saved draft's build follows its fields.
        self.configurator.build = self.configurator.builder.try_build();
    }

    /// The configured links with their supervisor's status and controls.
    pub fn view_status(&mut self, ui: &mut egui::Ui, manager: &mut LinkManager, cx: &mut Context) {
        let now = std::time::Instant::now();

        let entries: Vec<(groundhog_logic::link::LinkId, LinkConfig, LinkStatus)> = manager
            .entries()
            .map(|(id, config, status)| (id.clone(), config.clone(), status))
            .collect();

        for (index, (id, config, status)) in entries.into_iter().enumerate() {
            let (label, fill) = match &status {
                LinkStatus::Connected { .. } => {
                    ("Disconnect", style::severity::CRITICAL.gamma_multiply(0.8))
                }
                LinkStatus::Failed(_) => {
                    ("Connect", style::map::MARKER_DEFAULT.gamma_multiply(0.6))
                }
                status if status.is_active() => {
                    ("Disconnect", style::severity::WARNING.gamma_multiply(0.7))
                }
                _ => ("Connect", style::map::SELECTED.gamma_multiply(0.6)),
            };

            let detail = match &status {
                LinkStatus::Disconnected => String::new(),
                LinkStatus::Connecting { attempt: 1 } => "Connecting…".to_owned(),
                LinkStatus::Connecting { attempt } => format!("Connecting (attempt {attempt})…"),
                LinkStatus::Connected { .. } => match manager.stats(&id) {
                    Some(stats) => {
                        let heard = stats.last_rx.map_or_else(
                            || "nothing received yet".to_owned(),
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
                    None => "Connected".to_owned(),
                },
                LinkStatus::Reconnecting {
                    error, retry_at, ..
                } => format!(
                    "{error}; retrying in {:.0} s",
                    retry_at.saturating_duration_since(now).as_secs_f32().ceil()
                ),
                LinkStatus::Failed(error) => format!("Failed: {error}"),
            };

            let row_start = ui.available_rect_before_wrap();
            ui.horizontal(|row| {
                row.vertical(|rows| {
                    rows.label(RichText::new(&*config.name).strong());
                    if !detail.is_empty() {
                        rows.label(RichText::new(detail).small().weak());
                    }
                });
                row.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |buttons| {
                        if buttons.button("×").clicked() && manager.remove(&id) {
                            cx.request(Effect::SaveLinks(manager.configs()));
                        }
                        if buttons.add(egui::Button::new(label).fill(fill)).clicked() {
                            if status.is_active() {
                                manager.disconnect(&id);
                            } else {
                                manager.connect(&id);
                            }
                        }
                    },
                );
            });

            // Stripes, same as the other lists.
            let fill = if index % 2 == 1 {
                egui::Color32::from_rgb(0x24, 0x29, 0x30)
            } else {
                egui::Color32::TRANSPARENT
            };
            if fill != egui::Color32::TRANSPARENT {
                ui.painter().rect_filled(row_start, 2.0, fill);
            }
            ui.add_space(2.0);
        }
    }
}
