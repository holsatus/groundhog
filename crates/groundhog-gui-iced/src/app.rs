//! The application root: composition, message routing, and policy.
//!
//! The root wires the layers together at boot, routes messages to features,
//! applies the [`Effect`]s features request, and decides which fleet and link
//! events become operator notices. It holds the latest fleet snapshot, which
//! the fleet runtime publishes from off the UI thread.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use groundhog_logic::{
    Backend, BackendOptions, SystemId,
    domain::{
        calibration::Phase,
        fleet::{FleetEvent, FleetSnapshot},
        vehicle::{Liveness, status},
    },
    services::{FleetUpdate, LinkEvent, LinkStatus, TlogRecorder},
};
use iced::{
    Alignment, Element, Length, Padding, Subscription, Task, Theme, Widget,
    widget::{Space, button, column, container, row, rule, stack, tooltip},
};
use mavio::default_dialect::enums::MavSeverity;

use crate::{
    config::{self, ConfigWriter, Configuration},
    features::{
        Context, Effect, MapContext,
        calibration::{self, CalibrationPanel},
        confirm::{self, Answer, Confirmation},
        link_panel::{self, LinkPanel},
        log_panel::{self, LogPanel},
        mission::{self, MissionEditor},
        notifications::{self, Level, Notice, Notifications},
        parameter_panel::{self, ParameterPanel},
        settings_panel,
        vehicles::{self, VehiclePanel},
    },
    layout::{self, Layout},
    map::{self, LayerSet, Map},
    ui::{
        self, Icon,
        style::{self, ButtonKind},
    },
    widget::pane_grid::{self, PaneGrid},
};

/// The features that take a [`Context`], grouped so their state can be
/// borrowed apart from the context's.
#[derive(Default)]
struct Features {
    vehicles: VehiclePanel,
    mission: MissionEditor,
    parameters: ParameterPanel,
    calibration: CalibrationPanel,
}

pub struct Application {
    layout: Layout,
    map: Map,
    configuration: Configuration,
    config_writer: ConfigWriter,
    backend: Backend,
    /// The fleet as last published by the runtime.
    fleet: Arc<FleetSnapshot>,
    /// The vehicle the UI shows.
    selected: Option<SystemId>,
    recorder: Option<TlogRecorder>,
    notices: Notifications,
    confirmation: Option<Confirmation<Message>>,
    features: Features,
    link_panel: LinkPanel,
    log_panel: LogPanel,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The fleet runtime published.
    Fleet(FleetUpdate),
    /// A link's supervisor changed status.
    Link(LinkEvent),
    /// A configuration write finished.
    ConfigurationSaved(Result<(), String>),

    Layout(layout::Message),
    Map(map::Message),
    Vehicles(vehicles::Message),
    Mission(mission::Message),
    Parameters(parameter_panel::Message),
    Calibration(calibration::Message),
    Links(link_panel::Message),
    Logs(log_panel::Message),
    Settings(settings_panel::Message),
    Notifications(notifications::Message),
    Confirm(Answer),

    /// Debounce tick that flushes a pending layout save.
    LayoutSaveTick,
    /// Refreshes time-dependent views (toast expiry, link countdowns).
    Tick(Instant),
}

/// Idle time after the last layout change before the layout is persisted.
const LAYOUT_SAVE_DEBOUNCE: Duration = Duration::from_millis(800);

impl Application {
    pub fn boot() -> (Self, Task<Message>) {
        let mut notices = Notifications::default();

        let configuration = Configuration::initialize().unwrap_or_else(|error| {
            notices.push(Notice::error("Could not read the configuration").detail(error));
            Configuration::default()
        });

        // The backend spawns its tasks here: boot runs inside the
        // application's async runtime.
        let (backend, events) = Backend::start(BackendOptions {
            links: configuration.links.clone(),
            ..BackendOptions::default()
        });

        let (config_writer, saved) = config::spawn_writer();

        let layout = configuration
            .layout
            .clone()
            .map_or_else(Layout::new, Layout::from_configuration);

        let mut application = Application {
            layout,
            map: Map::new(),
            configuration,
            config_writer,
            backend,
            fleet: Arc::default(),
            selected: None,
            recorder: None,
            notices,
            confirmation: None,
            features: Features::default(),
            link_panel: LinkPanel::new(),
            log_panel: LogPanel::new(),
        };

        if application.configuration.settings.record_telemetry {
            application.start_recording();
        }

        let saved = futures::stream::unfold(saved, |mut saved| async move {
            saved.recv().await.map(|outcome| (outcome, saved))
        });

        let tasks = Task::batch([
            Task::stream(events.fleet.into_stream()).map(Message::Fleet),
            Task::stream(events.links.into_stream()).map(Message::Link),
            Task::stream(saved).map(Message::ConfigurationSaved),
        ]);

        (application, tasks)
    }

    pub fn theme(&self) -> Theme {
        self.configuration.settings.theme.theme()
    }

    pub fn title(&self) -> String {
        "Holsatus Groundhog".to_string()
    }

    pub fn scale_factor(&self) -> f32 {
        self.configuration.settings.scale_factor
    }

    fn save_configuration(&self) {
        self.config_writer.save(self.configuration.clone());
    }

    fn start_recording(&mut self) {
        let Some(directory) = config::data_dir().map(|directory| directory.join("tlogs")) else {
            self.notices
                .push(Notice::error("No data directory to record telemetry into"));
            return;
        };

        match self.backend.record(&directory) {
            Ok(recorder) => self.recorder = Some(recorder),
            Err(error) => self
                .notices
                .push(Notice::error("Could not start telemetry recording").detail(error)),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Fleet(update) => {
                self.on_fleet(update);
                Task::none()
            }
            Message::Link(event) => {
                self.on_link(event);
                Task::none()
            }
            Message::ConfigurationSaved(result) => {
                if let Err(error) = result {
                    self.notices
                        .push(Notice::error("Could not save the configuration").detail(error));
                }
                Task::none()
            }
            Message::Layout(message) => {
                if self.layout.update(message) {
                    self.configuration.layout = Some(self.layout.configuration());
                }
                Task::none()
            }
            Message::Map(message) => self.map.update(message).map(Message::Map),
            Message::Vehicles(message) => self.dispatch(Message::Vehicles, |features, cx| {
                features.vehicles.update(message, cx)
            }),
            Message::Mission(message) => self.dispatch(Message::Mission, |features, cx| {
                features.mission.update(message, cx)
            }),
            Message::Parameters(message) => self.dispatch(Message::Parameters, |features, cx| {
                features.parameters.update(message, cx)
            }),
            Message::Calibration(message) => self.dispatch(Message::Calibration, |features, cx| {
                features.calibration.update(message, cx)
            }),
            Message::Links(message) => {
                let (task, event) =
                    self.link_panel
                        .update(message, self.backend.links_mut(), &mut self.notices);

                if let Some(link_panel::Event::LinksChanged(links)) = event {
                    self.configuration.links = links;
                    self.save_configuration();
                }
                task.map(Message::Links)
            }
            Message::Logs(message) => {
                self.log_panel.update(message);
                Task::none()
            }
            Message::Settings(message) => {
                self.on_settings(message);
                Task::none()
            }
            Message::Notifications(message) => {
                self.notices.update(message);
                Task::none()
            }
            Message::Confirm(answer) => match (answer, self.confirmation.take()) {
                (Answer::Accept, Some(confirmation)) => Task::done(confirmation.accept),
                _ => Task::none(),
            },
            Message::LayoutSaveTick => {
                if self.layout.is_dirty() {
                    self.layout.clear_dirty();
                    self.save_configuration();
                }
                Task::none()
            }
            Message::Tick(now) => {
                self.notices.update(notifications::Message::Tick(now));
                Task::none()
            }
        }
    }

    /// Run a feature update with a fresh [`Context`], then apply the effects
    /// it requested.
    fn dispatch<M>(
        &mut self,
        wrap: fn(M) -> Message,
        update: impl FnOnce(&mut Features, &mut Context<M>) -> Task<M>,
    ) -> Task<Message>
    where
        M: Send + 'static,
    {
        let fleet = self.fleet.clone();
        let mut context = Context::new(
            &fleet,
            self.backend.fleet(),
            &mut self.notices,
            self.configuration.file_picker_path.as_deref(),
            &self.configuration.settings,
        );

        let task = update(&mut self.features, &mut context).map(wrap);
        let effects = context.into_effects();

        for effect in effects {
            self.apply(effect.map(wrap));
        }

        task
    }

    fn apply(&mut self, effect: Effect<Message>) {
        match effect {
            Effect::Confirm(confirmation) => self.confirmation = Some(confirmation),
            Effect::SelectVehicle(system) => {
                if self.fleet.contains(system) {
                    self.selected = Some(system);
                    self.follow_selected();
                }
            }
            Effect::CenterMap(position) => {
                self.map.set_following(false);
                self.map.center_on(position);
            }
            Effect::FollowVehicle(follow) => {
                self.map.set_following(follow);
                self.follow_selected();
            }
            Effect::RememberDirectory(path) => self.remember_directory(&path),
        }
    }

    fn remember_directory(&mut self, path: &Path) {
        let Some(directory) = path.is_dir().then_some(path).or(path.parent()) else {
            return;
        };

        if self.configuration.file_picker_path.as_deref() != Some(directory) {
            self.configuration.file_picker_path = Some(directory.to_path_buf());
            self.save_configuration();
        }
    }

    /// Keep the map on the selected vehicle, if following.
    fn follow_selected(&mut self) {
        if let Some(position) = self
            .selected
            .and_then(|system| self.fleet.vehicle(system))
            .and_then(|vehicle| vehicle.position.current.as_ref())
        {
            self.map.follow_to(position.pos);
        }
    }

    fn on_fleet(&mut self, update: FleetUpdate) {
        self.fleet = update.snapshot;
        self.features.mission.sync(&self.fleet);

        for event in update.events {
            self.on_fleet_event(event);
        }

        self.follow_selected();
    }

    /// Fleet events become selection changes and operator notices.
    fn on_fleet_event(&mut self, event: FleetEvent) {
        let name = |system| self.fleet.vehicle_name(system);

        let notice = match event {
            FleetEvent::Registered(system) => {
                if self.selected.is_none() {
                    self.selected = Some(system);
                }
                Notice::info(format!("{} connected", name(system))).vehicle(system)
            }
            // A late vehicle shows in the vehicle panel; only losing and
            // regaining contact are announced, so a flaky link does not
            // flood the operator.
            FleetEvent::LivenessChanged { system, from, to } => match (from, to) {
                (_, Liveness::Lost) => {
                    Notice::error(format!("Lost contact with {}", name(system))).vehicle(system)
                }
                (Liveness::Lost, Liveness::Active) => {
                    Notice::success(format!("Contact with {} restored", name(system)))
                        .vehicle(system)
                }
                _ => return,
            },
            FleetEvent::ArmedChanged(system, true) => {
                Notice::warning(format!("{} armed", name(system))).vehicle(system)
            }
            FleetEvent::ArmedChanged(system, false) => {
                Notice::info(format!("{} disarmed", name(system))).vehicle(system)
            }
            FleetEvent::ModeChanged(system, mode) => Notice::info(format!(
                "{}: {} mode",
                name(system),
                mode.map_or("unknown", |mode| mode.name)
            ))
            .vehicle(system),
            FleetEvent::StatusText(system, text) => {
                // Status texts are in the vehicle's own log; only the serious
                // ones are toasted, and not duplicated into the history.
                let level = match status::rank(text.severity) {
                    rank if rank <= status::rank(MavSeverity::Error) => Level::Error,
                    rank if rank <= status::rank(MavSeverity::Warning) => Level::Warning,
                    _ => return,
                };
                self.notices.toast(
                    Notice::new(level, format!("{}: {}", name(system), text.text)).vehicle(system),
                );
                return;
            }
            FleetEvent::OnboardMissionStale(system) => Notice::warning(format!(
                "The mission on {} was changed elsewhere",
                name(system)
            ))
            .detail("Download it to see the vehicle's current mission.")
            .vehicle(system),
            FleetEvent::CalibrationFinished(system, kind, phase) => {
                let subject = format!("{kind} calibration of {}", name(system));
                match phase {
                    Phase::Succeeded => Notice::success(format!("{subject} succeeded")),
                    Phase::Failed(reason) => {
                        Notice::error(format!("{subject} failed")).detail(reason)
                    }
                    Phase::Cancelled => Notice::info(format!("{subject} cancelled")),
                    Phase::Starting | Phase::Running => return,
                }
                .vehicle(system)
            }
        };

        self.notices.push(notice);
    }

    fn on_link(&mut self, event: LinkEvent) {
        let was_connected = event.previous.is_connected();
        let notice = match &event.status {
            LinkStatus::Connected { .. } if !was_connected => {
                Some(Notice::success(format!("Link {} connected", event.name)))
            }
            LinkStatus::Reconnecting { error, .. } if was_connected => Some(
                Notice::warning(format!("Link {} lost; reconnecting", event.name)).detail(error),
            ),
            LinkStatus::Failed(error) => {
                Some(Notice::error(format!("Link {} failed", event.name)).detail(error))
            }
            _ => None,
        };

        if let Some(notice) = notice {
            self.notices.push(notice);
        }
    }

    fn on_settings(&mut self, message: settings_panel::Message) {
        let settings = &mut self.configuration.settings;
        let before = *settings;

        match message {
            settings_panel::Message::Theme(mode) => settings.theme = mode,
            settings_panel::Message::ScaleFactor(scale) => settings.scale_factor = scale,
            settings_panel::Message::DefaultAltitude(altitude) => {
                settings.default_waypoint_altitude_m = altitude;
            }
            settings_panel::Message::RecordTelemetry(record) => {
                settings.record_telemetry = record;
                if record && self.recorder.is_none() {
                    self.start_recording();
                } else if !record && let Some(recorder) = self.recorder.take() {
                    recorder.stop();
                }
            }
        }

        if self.configuration.settings != before {
            self.save_configuration();
        }
    }

    pub fn view(&self) -> impl Widget<Message> + '_ {
        // The rail and the panes float on the window's background, one gap
        // apart from each other and from the window's edges.
        let base = container(row![self.view_rail(), self.view_panes()].spacing(ui::space::S))
            .padding(ui::space::S)
            .style(style::workspace);

        let toasts = container(self.notices.view().map(Message::Notifications))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_right(Length::Fill)
            .align_bottom(Length::Fill)
            .padding(ui::space::XL);

        container(confirm::view(
            stack![base, toasts].boxed(),
            self.confirmation.as_ref(),
            Message::Confirm,
        ))
    }

    /// The rail: a button per panel, highlighted while it is open. Pressing
    /// one opens its panel, or closes it if open.
    fn view_rail(&self) -> Element<'_, Message> {
        let buttons = layout::Panel::DOCKABLE.into_iter().map(|panel| {
            let open = self.layout.is_open(panel);
            let icon = ui::icon::icon(panel.icon(), ui::size::RAIL_ICON, ButtonKind::Subtle, true);
            let button = button(container(icon).center(Length::Fill))
                .padding(0)
                .width(ui::size::RAIL_BUTTON)
                .height(ui::size::RAIL_BUTTON)
                .style(style::rail_button(open))
                .on_press(Message::Layout(layout::Message::Toggle(panel)));

            let hint = if open {
                format!("Close {}", panel.title())
            } else {
                format!("Open {}", panel.title())
            };
            ui::tooltip(button, hint, tooltip::Position::Right)
        });

        column(buttons).spacing(ui::space::S).boxed()
    }

    fn view_panes(&self) -> impl Widget<Message> + '_ {
        PaneGrid::new(self.layout.panes(), |pane, panel, is_maximized| {
            // The map paints opaque tiles across its whole pane, which a rounded
            // container cannot clip; it draws its own frame instead (see
            // `map::Map::view`), and has no title bar to grab, so it cannot be
            // moved or closed.
            if panel.is_fixed() {
                let empty = Space::new().height(Length::Fixed(0.0)).boxed();
                return pane_grid::Content::new(self.pane_body(*panel))
                    .title_bar(pane_grid::TitleBar::new(empty).padding(0));
            }

            // Every other pane is a rounded card, its header split off by a
            // rule.
            let body = column![
                rule::horizontal(style::PANE_BORDER_WIDTH).style(style::pane_divider),
                self.pane_body(*panel),
            ]
            .height(Length::Fill);

            let layout = |message| Message::Layout(message);
            let size = if is_maximized {
                ui::icon_button(Icon::Minimize)
                    .hint("Restore")
                    .on_press(layout(layout::Message::Restore))
            } else {
                ui::icon_button(Icon::Maximize)
                    .hint("Maximize")
                    .on_press(layout(layout::Message::Maximize(pane)))
            };
            let close = ui::icon_button(Icon::Close)
                .hint("Close")
                .on_press(layout(layout::Message::Close(pane)));

            let title = row![
                ui::icon::icon(panel.icon(), ui::size::ICON, ButtonKind::Subtle, true),
                ui::text(panel.title())
            ]
            .spacing(ui::space::S)
            .align_y(Alignment::Center);

            let title_bar = pane_grid::TitleBar::new(title.boxed())
                .controls(pane_grid::Controls::new(
                    row![size.boxed(), close.boxed()].spacing(ui::space::XS),
                ))
                // The controls are as far from the bar's right edge as from
                // its top and bottom; the title gets more room on the left.
                // The pane's border is drawn over the bar's top and right
                // edges, while the divider lies below it, so those sides add
                // the border's width.
                .padding(Padding {
                    top: ui::space::XS + style::PANE_BORDER_WIDTH,
                    right: ui::space::XS + style::PANE_BORDER_WIDTH,
                    bottom: ui::space::XS,
                    left: ui::space::M,
                })
                .style(style::title_bar(self.layout.is_focused(pane)));

            pane_grid::Content::new(body.boxed())
                .style(style::pane)
                .title_bar(title_bar)
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .spacing(ui::space::S)
        .on_click(|pane| Message::Layout(layout::Message::Clicked(pane)))
        .on_drag(|event| Message::Layout(layout::Message::Dragged(event)))
        .on_resize(7, |event| Message::Layout(layout::Message::Resized(event)))
    }

    fn pane_body(&self, panel: layout::Panel) -> Element<'_, Message> {
        let fleet: &FleetSnapshot = &self.fleet;

        match panel {
            layout::Panel::Map => {
                let theme = self.theme();
                let context = MapContext {
                    fleet,
                    selected: self.selected,
                    palette: *theme.palette(),
                };

                let mut layers = LayerSet::new();
                layers.plugin(&self.features.vehicles, context, Message::Vehicles);
                layers.plugin(&self.features.mission, context, Message::Mission);

                self.map.view(layers, Message::Map, &theme)
            }
            layout::Panel::Vehicles => self
                .features
                .vehicles
                .view(fleet, self.selected, self.map.is_following())
                .map(Message::Vehicles)
                .boxed(),
            layout::Panel::Parameters => self
                .features
                .parameters
                .view(fleet, self.selected)
                .map(Message::Parameters)
                .boxed(),
            layout::Panel::Calibration => self
                .features
                .calibration
                .view(fleet, self.selected)
                .map(Message::Calibration)
                .boxed(),
            layout::Panel::Mission => self
                .features
                .mission
                .view(fleet, self.selected)
                .map(Message::Mission)
                .boxed(),
            layout::Panel::Links => self
                .link_panel
                .view(self.backend.links())
                .map(Message::Links)
                .boxed(),
            layout::Panel::Logs => self
                .log_panel
                .view(fleet, self.selected, &self.notices)
                .map(Message::Logs)
                .boxed(),
            layout::Panel::Settings => settings_panel::view(
                &self.configuration.settings,
                self.recorder.as_ref().map(TlogRecorder::path),
            )
            .map(Message::Settings)
            .boxed(),
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // Trailing debounce for layout persistence: the timer is restarted on
        // every layout change (the revision is part of the subscription
        // identity), so the write happens `LAYOUT_SAVE_DEBOUNCE` after the last
        // change rather than repeatedly throughout a drag/resize.
        let layout_tick = if self.layout.is_dirty() {
            iced::time::every(LAYOUT_SAVE_DEBOUNCE)
                .with(self.layout.revision())
                .map(|_| Message::LayoutSaveTick)
        } else {
            Subscription::none()
        };

        // Time-dependent views only need refreshing while they show something.
        let active_links = self
            .backend
            .links()
            .entries()
            .any(|(_, _, status)| status.is_active());
        let refresh = if self.notices.has_toasts() || active_links {
            iced::time::every(Duration::from_secs(1)).map(Message::Tick)
        } else {
            Subscription::none()
        };

        Subscription::batch([layout_tick, refresh])
    }
}

#[cfg(test)]
mod gallery;
